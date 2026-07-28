//! Normal mobile data on a spare Qualcomm DATA channel.
//!
//! The MSM8916 firmware cannot keep IMS and Internet bearers alive through the
//! same ModemManager data slot: starting one regularly deactivates the other.
//! IMS therefore stays on the primary QMI port and this runtime keeps a
//! `qmicli --wds-follow-network` process alive on DATA6 for user data.

use std::{net::IpAddr, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::{mpsc, Mutex},
    task::JoinHandle,
};
use tracing::{debug, info, warn};

use crate::platform::config::ApnConfig;

use super::{
    qmi_netdev::{self, NetdevConfig, ResolvedNetdev},
    qmi_wds,
    secondary_qmi::{self, SecondaryQmiEndpoint, QMI_OPEN_NET_ARG},
};

const START_TIMEOUT: Duration = Duration::from_secs(65);
const CONTEXT_RETRIES: usize = 12;

struct SecondaryDataSession {
    holder: FollowProcess,
    endpoint: SecondaryQmiEndpoint,
    netdev: ResolvedNetdev,
    netdev_config: NetdevConfig,
}

struct FollowProcess {
    child: Child,
    stdout_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    client_id: String,
    packet_data_handle: String,
}

#[derive(Default)]
pub struct SecondaryDataRuntime {
    session: Mutex<Option<SecondaryDataSession>>,
}

impl SecondaryDataRuntime {
    pub async fn interface(&self) -> Option<String> {
        self.session
            .lock()
            .await
            .as_ref()
            .map(|session| session.netdev.interface.clone())
    }

    pub async fn start(
        &self,
        _modem_id: &str,
        primary_qmi: &str,
        apn: &ApnConfig,
    ) -> Result<String, String> {
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.as_mut() {
            if session.holder.child.try_wait().ok().flatten().is_none() {
                return Ok(session.netdev.interface.clone());
            }
        }
        if let Some(session) = guard.take() {
            stop_session(session).await;
        }

        let endpoint = secondary_qmi::ensure_endpoint(primary_qmi)
            .await
            .map_err(|error| format!("cellular_secondary_qmi_unavailable:{error}"))?;
        let apn_name = normalized_data_apn(&apn.apn)?;
        let families = data_family_attempts(&apn.protocol);
        let mut errors = Vec::new();

        for family in families {
            match start_family(&endpoint, apn, &apn_name, family).await {
                Ok(session) => {
                    let interface = session.netdev.interface.clone();
                    info!(
                        device = %endpoint.device_path,
                        interface = %interface,
                        family,
                        "Secondary DATA QMI bearer activated"
                    );
                    *guard = Some(session);
                    return Ok(interface);
                }
                Err(error) => {
                    warn!(family, error = %error, "Secondary DATA family failed");
                    errors.push(error);
                }
            }
        }

        Err(format!(
            "cellular_secondary_data_start_failed:{}",
            errors.join(" | ")
        ))
    }

    pub async fn stop(&self) {
        if let Some(session) = self.session.lock().await.take() {
            stop_session(session).await;
        }
    }
}

async fn start_family(
    endpoint: &SecondaryQmiEndpoint,
    apn: &ApnConfig,
    apn_name: &str,
    family: u8,
) -> Result<SecondaryDataSession, String> {
    let mut holder = spawn_follow_process(endpoint, apn, apn_name, family).await?;

    let settings = match wait_for_current_settings(endpoint, &holder.client_id, family).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_holder(&mut holder, endpoint).await;
            return Err(error);
        }
    };
    let baseband = secondary_qmi::baseband_key_for_device(&endpoint.device_path)
        .or_else(|_| secondary_qmi::baseband_key_for_device(&endpoint.port_name))
        .unwrap_or_else(|_| endpoint.remoteproc.clone());
    let netdev = match qmi_netdev::resolve(&baseband, &settings).await {
        Ok(netdev) => netdev,
        Err(error) => {
            stop_holder(&mut holder, endpoint).await;
            return Err(format!("cellular_data_netdev_unresolved:{error}"));
        }
    };
    if let Err(error) = qmi_netdev::install_default_route(&netdev.interface, &settings).await {
        qmi_netdev::teardown(&netdev.interface, &settings).await;
        stop_holder(&mut holder, endpoint).await;
        return Err(format!("cellular_data_policy_route_failed:{error}"));
    }

    Ok(SecondaryDataSession {
        holder,
        endpoint: endpoint.clone(),
        netdev,
        netdev_config: settings,
    })
}

async fn spawn_follow_process(
    endpoint: &SecondaryQmiEndpoint,
    apn: &ApnConfig,
    apn_name: &str,
    family: u8,
) -> Result<FollowProcess, String> {
    let action = start_action(apn, apn_name, family)?;
    let mut child = Command::new("qmicli")
        .args([
            "--verbose",
            "-d",
            endpoint.device_path.as_str(),
            // The holder and settings reader are separate processes. They must
            // share QMUX through qmi-proxy or the retained WDS CID is not
            // addressable by the settings reader.
            "--device-open-proxy",
            QMI_OPEN_NET_ARG,
            "--client-no-release-cid",
            action.as_str(),
            "--wds-follow-network",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("secondary_qmi_data_spawn_failed:{error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "secondary_qmi_data_stdout_missing".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "secondary_qmi_data_stderr_missing".to_string())?;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let stdout_task = spawn_line_reader(stdout, tx.clone());
    let stderr_task = spawn_line_reader(stderr, tx);

    let started = tokio::time::timeout(START_TIMEOUT, async {
        let mut transcript = Vec::new();
        let mut client_id = None;
        let mut packet_data_handle = None;
        while let Some(line) = rx.recv().await {
            let line = line.trim().to_string();
            if !line.is_empty() {
                transcript.push(line.clone());
            }
            client_id = client_id.or_else(|| parse_verbose_wds_client_id(&line));
            packet_data_handle =
                packet_data_handle.or_else(|| qmi_wds::parse_packet_data_handle(&line));
            if line.contains("Network started") {
                let client_id = client_id.ok_or_else(|| {
                    format!("secondary_qmi_data_cid_missing:{}", transcript.join(" "))
                })?;
                // qmicli normally prints the handle on the next line.
                while packet_data_handle.is_none() {
                    let Some(next) = rx.recv().await else {
                        break;
                    };
                    let next = next.trim().to_string();
                    if !next.is_empty() {
                        transcript.push(next.clone());
                    }
                    packet_data_handle = qmi_wds::parse_packet_data_handle(&next);
                }
                let packet_data_handle = packet_data_handle.ok_or_else(|| {
                    format!("secondary_qmi_data_handle_missing:{}", transcript.join(" "))
                })?;
                return Ok((client_id, packet_data_handle));
            }
            if line.starts_with("error:") {
                return Err(transcript.join(" "));
            }
        }
        Err(transcript.join(" "))
    })
    .await;

    match started {
        Ok(Ok((client_id, packet_data_handle))) => Ok(FollowProcess {
            child,
            stdout_task,
            stderr_task,
            client_id,
            packet_data_handle,
        }),
        Ok(Err(error)) => {
            stop_process(&mut child).await;
            stdout_task.abort();
            stderr_task.abort();
            Err(format!("secondary_qmi_data_start_failed:{error}"))
        }
        Err(_) => {
            stop_process(&mut child).await;
            stdout_task.abort();
            stderr_task.abort();
            Err("secondary_qmi_data_start_timeout".to_string())
        }
    }
}

fn spawn_line_reader<R>(reader: R, tx: mpsc::UnboundedSender<String>) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(line);
        }
    })
}

fn parse_verbose_wds_client_id(line: &str) -> Option<String> {
    let marker = "client with ID '";
    if line.contains("registered 'wds'") {
        let value = line.split_once(marker)?.1.split_once('\'')?.0;
        if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
            return Some(value.to_string());
        }
    }
    if line.contains("service = 'wds'") {
        let value = line.split_once("cid = '")?.1.split_once('\'')?.0;
        if !value.is_empty() && value.chars().all(|character| character.is_ascii_digit()) {
            return Some(value.to_string());
        }
    }
    None
}

async fn wait_for_current_settings(
    endpoint: &SecondaryQmiEndpoint,
    client_id: &str,
    family: u8,
) -> Result<NetdevConfig, String> {
    let mut last = String::new();
    for _ in 0..CONTEXT_RETRIES {
        match read_current_settings(endpoint, client_id).await {
            Ok((text, settings)) => {
                last = text;
                if let Some(config) = netdev_config_for(&settings, family) {
                    return Ok(config);
                }
            }
            Err(error) => last = error,
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(format!(
        "cellular_data_current_settings_unavailable:{}",
        compact(&last)
    ))
}

async fn read_current_settings(
    endpoint: &SecondaryQmiEndpoint,
    client_id: &str,
) -> Result<(String, qmi_wds::CurrentSettings), String> {
    let cid = format!("--client-cid={client_id}");
    let output = run_qmicli(&[
        "-d",
        endpoint.device_path.as_str(),
        "--device-open-proxy",
        QMI_OPEN_NET_ARG,
        cid.as_str(),
        "--client-no-release-cid",
        "--wds-get-current-settings",
    ])
    .await?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        let settings = qmi_wds::parse_current_settings(&text);
        Ok((text, settings))
    } else {
        Err(format!(
            "cellular_data_current_settings_failed:{}",
            compact(&text)
        ))
    }
}

fn netdev_config_for(settings: &qmi_wds::CurrentSettings, family: u8) -> Option<NetdevConfig> {
    let (address, gateway, dns, prefix) = if family == 6 {
        (
            settings.ipv6_address.as_deref(),
            settings.ipv6_gateway.as_deref(),
            &settings.ipv6_dns,
            settings.ipv6_prefix,
        )
    } else {
        (
            settings.ipv4_address.as_deref(),
            settings.ipv4_gateway.as_deref(),
            &settings.ipv4_dns,
            settings.ipv4_prefix,
        )
    };
    let address: IpAddr = address?.parse().ok()?;
    let dns = dns
        .iter()
        .filter_map(|value| value.parse::<IpAddr>().ok())
        .collect::<Vec<_>>();
    let gateway = gateway.and_then(|value| value.parse::<IpAddr>().ok());
    Some(NetdevConfig::from_session(
        address,
        prefix,
        settings.mtu,
        &dns,
        gateway,
    ))
}

fn normalized_data_apn(apn: &str) -> Result<String, String> {
    let apn = apn.trim();
    if apn.is_empty() || apn.eq_ignore_ascii_case("ims") {
        return Err("cellular_data_apn_missing".to_string());
    }
    if apn.contains(',') || apn.contains('=') {
        return Err("cellular_data_apn_invalid".to_string());
    }
    Ok(apn.to_string())
}

fn data_family_attempts(protocol: &str) -> Vec<u8> {
    match protocol.trim().to_ascii_lowercase().as_str() {
        "ipv4" | "ip" => vec![4],
        "ipv6" => vec![6],
        _ => vec![4, 6],
    }
}

fn start_action(apn: &ApnConfig, apn_name: &str, family: u8) -> Result<String, String> {
    for value in [&apn.username, &apn.password] {
        if value.contains(',') || value.contains('=') {
            return Err("cellular_data_credentials_invalid".to_string());
        }
    }
    let mut action = format!("--wds-start-network=apn={apn_name},ip-type={family}");
    if !apn.username.is_empty() {
        action.push_str(&format!(",username={}", apn.username));
    }
    if !apn.password.is_empty() {
        action.push_str(&format!(",password={}", apn.password));
    }
    let auth = match apn.auth_method.trim().to_ascii_lowercase().as_str() {
        "pap" => "PAP",
        "both" | "pap-or-chap" => "BOTH",
        _ => "CHAP",
    };
    if !apn.username.is_empty() || !apn.password.is_empty() {
        action.push_str(&format!(",auth={auth}"));
    }
    Ok(action)
}

async fn stop_session(mut session: SecondaryDataSession) {
    stop_holder(&mut session.holder, &session.endpoint).await;
    qmi_netdev::teardown(&session.netdev.interface, &session.netdev_config).await;
    info!(interface = %session.netdev.interface, "Secondary DATA QMI bearer deactivated");
}

async fn stop_holder(holder: &mut FollowProcess, endpoint: &SecondaryQmiEndpoint) {
    stop_process(&mut holder.child).await;
    holder.stdout_task.abort();
    holder.stderr_task.abort();

    let cid = format!("--client-cid={}", holder.client_id);
    let stop = format!("--wds-stop-network={}", holder.packet_data_handle);
    let _ = run_qmicli(&[
        "-d",
        endpoint.device_path.as_str(),
        "--device-open-proxy",
        QMI_OPEN_NET_ARG,
        cid.as_str(),
        "--client-no-release-cid",
        stop.as_str(),
    ])
    .await;
    // Omitting --client-no-release-cid makes qmicli return the retained WDS CID.
    let _ = run_qmicli(&[
        "-d",
        endpoint.device_path.as_str(),
        "--device-open-proxy",
        QMI_OPEN_NET_ARG,
        cid.as_str(),
        "--wds-noop",
    ])
    .await;
}

async fn run_qmicli(args: &[&str]) -> Result<std::process::Output, String> {
    tokio::time::timeout(
        Duration::from_secs(20),
        Command::new("qmicli").args(args).output(),
    )
    .await
    .map_err(|_| "secondary_qmi_data_command_timeout".to_string())?
    .map_err(|error| format!("secondary_qmi_data_command_spawn_failed:{error}"))
}

async fn stop_process(child: &mut Child) {
    if let Some(pid) = child.id() {
        let _ = Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()
            .await;
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_ok()
        {
            return;
        }
    }
    if let Err(error) = child.kill().await {
        debug!(error = %error, "Killing secondary DATA holder failed");
    }
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reference_wds_client_id() {
        let line = "[Debug] [/dev/wwan0qmi1] registered 'wds' (version unknown) client with ID '5'";
        assert_eq!(parse_verbose_wds_client_id(line).as_deref(), Some("5"));
        let allocation = "translated = [ service = 'wds' cid = '17' ]";
        assert_eq!(
            parse_verbose_wds_client_id(allocation).as_deref(),
            Some("17")
        );
    }

    #[test]
    fn builds_netdev_config_from_same_cid_settings() {
        let output = "IP Family: IPv4\nIPv4 address: 10.129.24.126\nIPv4 subnet mask: 255.255.255.252\nIPv4 gateway address: 10.129.24.125\nIPv4 primary DNS: 172.17.163.218\nMTU: 1500\n";
        let settings = qmi_wds::parse_current_settings(output);
        let config = netdev_config_for(&settings, 4).unwrap();
        assert_eq!(config.address.to_string(), "10.129.24.126");
        assert_eq!(config.prefix, 30);
        assert_eq!(config.probe_target.unwrap().to_string(), "172.17.163.218");
    }

    #[test]
    fn dual_data_prefers_ipv4_then_ipv6() {
        assert_eq!(data_family_attempts("dual"), vec![4, 6]);
        assert_eq!(data_family_attempts("ipv6"), vec![6]);
    }

    #[test]
    fn command_keeps_the_session_in_one_process() {
        let apn = ApnConfig {
            apn: "internet".to_string(),
            protocol: "dual".to_string(),
            username: "u".to_string(),
            password: "p".to_string(),
            auth_method: "chap".to_string(),
        };
        let action = start_action(&apn, "internet", 4).unwrap();
        assert!(action.contains("--wds-start-network=apn=internet,ip-type=4"));
        assert!(action.contains(",auth=CHAP"));
    }
}
