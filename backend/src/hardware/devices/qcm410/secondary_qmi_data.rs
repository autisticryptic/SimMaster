//! Normal mobile data on a spare Qualcomm DATA channel.
//!
//! The MSM8916 firmware cannot keep IMS and Internet bearers alive through the
//! same ModemManager data slot: starting one regularly deactivates the other.
//! When IMS uses the primary QMI port, this runtime keeps a
//! retained WDS CID alive through qmi-proxy on DATA6 for user data. If an
//! ordinary-data bearer already exists on qmi0, the allocator leaves it there
//! and moves IMS to DATA6 instead, so this runtime is not started.

use std::{net::IpAddr, time::Duration};

use tokio::{process::Command, sync::Mutex};
use tracing::{info, warn};

use crate::{
    hardware::cellular::{
        qmi_netdev::{self, NetdevConfig, ResolvedNetdev},
        qmi_wds,
    },
    platform::config::ApnConfig,
};

use super::secondary_qmi::{self, SecondaryQmiEndpoint};

const START_TIMEOUT: Duration = Duration::from_secs(65);
const CONTEXT_RETRIES: usize = 12;

struct SecondaryDataSession {
    client_id: String,
    packet_data_handle: String,
    endpoint: SecondaryQmiEndpoint,
    netdev: ResolvedNetdev,
    netdev_config: NetdevConfig,
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
        if let Some(session) = guard.as_ref() {
            if retained_session_is_active(session).await {
                return Ok(session.netdev.interface.clone());
            }
        }
        if let Some(session) = guard.take() {
            stop_session(session).await;
        }

        let endpoint = secondary_qmi::runtime_endpoint(primary_qmi)
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
    let retained = start_retained_session(endpoint, apn, apn_name, family).await?;

    let settings = match wait_for_current_settings(endpoint, &retained.client_id, family).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_retained_session(endpoint, &retained).await;
            return Err(error);
        }
    };
    let baseband = secondary_qmi::baseband_key_for_device(&endpoint.device_path)
        .or_else(|_| secondary_qmi::baseband_key_for_device(&endpoint.port_name))
        .unwrap_or_else(|_| endpoint.remoteproc.clone());
    let netdev = match qmi_netdev::resolve(&baseband, &settings).await {
        Ok(netdev) => netdev,
        Err(error) => {
            stop_retained_session(endpoint, &retained).await;
            return Err(format!("cellular_data_netdev_unresolved:{error}"));
        }
    };
    if let Err(error) = qmi_netdev::install_default_route(&netdev.interface, &settings).await {
        qmi_netdev::teardown(&netdev.interface, &settings).await;
        stop_retained_session(endpoint, &retained).await;
        return Err(format!("cellular_data_policy_route_failed:{error}"));
    }

    Ok(SecondaryDataSession {
        client_id: retained.client_id,
        packet_data_handle: retained.packet_data_handle,
        endpoint: endpoint.clone(),
        netdev,
        netdev_config: settings,
    })
}

struct RetainedSession {
    client_id: String,
    packet_data_handle: String,
}

async fn start_retained_session(
    endpoint: &SecondaryQmiEndpoint,
    apn: &ApnConfig,
    apn_name: &str,
    family: u8,
) -> Result<RetainedSession, String> {
    let action = start_action(apn, apn_name, family)?;
    let family_action = format!("--wds-set-ip-family={family}");
    let allocation = run_qmicli(&retained_allocate_args(endpoint.device_path.as_str())).await?;
    let allocation_text = output_text(&allocation);
    if !allocation.status.success() {
        return Err(format!(
            "secondary_qmi_data_cid_allocate_failed:{}",
            compact(&allocation_text)
        ));
    }
    let client_id = secondary_qmi::parse_wds_client_id(&allocation_text).ok_or_else(|| {
        format!(
            "secondary_qmi_data_cid_missing:{}",
            compact(&allocation_text)
        )
    })?;

    if let Err(error) = run_retained_action(
        endpoint,
        &client_id,
        &family_action,
        Duration::from_secs(20),
    )
    .await
    {
        release_retained_client(endpoint, &client_id).await;
        return Err(error);
    }
    let output = match run_retained_action(endpoint, &client_id, &action, START_TIMEOUT).await {
        Ok(output) => output,
        Err(error) => {
            release_retained_client(endpoint, &client_id).await;
            return Err(error);
        }
    };
    let text = output_text(&output);
    let packet_data_handle = qmi_wds::parse_packet_data_handle(&text)
        .ok_or_else(|| format!("secondary_qmi_data_handle_missing:{}", compact(&text)));
    let packet_data_handle = match packet_data_handle {
        Ok(handle) => handle,
        Err(error) => {
            release_retained_client(endpoint, &client_id).await;
            return Err(error);
        }
    };
    Ok(RetainedSession {
        client_id,
        packet_data_handle,
    })
}

fn retained_allocate_args(device: &str) -> Vec<&str> {
    vec![
        "--verbose",
        "-d",
        device,
        "--device-open-qmi",
        "--device-open-proxy",
        secondary_qmi::QMI_OPEN_NET_ARG,
        "--client-no-release-cid",
        "--wds-noop",
    ]
}

fn retained_action_args<'a>(device: &'a str, cid: &'a str, action: &'a str) -> Vec<&'a str> {
    vec![
        "-d",
        device,
        "--device-open-qmi",
        "--device-open-proxy",
        secondary_qmi::QMI_OPEN_NET_ARG,
        cid,
        "--client-no-release-cid",
        action,
    ]
}

async fn run_retained_action(
    endpoint: &SecondaryQmiEndpoint,
    client_id: &str,
    action: &str,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let cid = format!("--client-cid={client_id}");
    let output = run_qmicli_with_timeout(
        &retained_action_args(endpoint.device_path.as_str(), cid.as_str(), action),
        timeout,
    )
    .await?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "secondary_qmi_data_action_failed:{}",
            compact(&output_text(&output))
        ))
    }
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
        "--device-open-qmi",
        "--device-open-proxy",
        secondary_qmi::QMI_OPEN_NET_ARG,
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
    let retained = RetainedSession {
        client_id: std::mem::take(&mut session.client_id),
        packet_data_handle: std::mem::take(&mut session.packet_data_handle),
    };
    stop_retained_session(&session.endpoint, &retained).await;
    qmi_netdev::teardown(&session.netdev.interface, &session.netdev_config).await;
    info!(interface = %session.netdev.interface, "Secondary DATA QMI bearer deactivated");
}

async fn stop_retained_session(endpoint: &SecondaryQmiEndpoint, session: &RetainedSession) {
    let cid = format!("--client-cid={}", session.client_id);
    let stop = format!("--wds-stop-network={}", session.packet_data_handle);
    let _ = run_qmicli(&[
        "-d",
        endpoint.device_path.as_str(),
        "--device-open-qmi",
        "--device-open-proxy",
        secondary_qmi::QMI_OPEN_NET_ARG,
        cid.as_str(),
        "--client-no-release-cid",
        stop.as_str(),
    ])
    .await;
    release_retained_client(endpoint, &session.client_id).await;
}

async fn release_retained_client(endpoint: &SecondaryQmiEndpoint, client_id: &str) {
    let cid = format!("--client-cid={client_id}");
    // Omitting --client-no-release-cid makes qmicli return the retained WDS CID.
    let _ = run_qmicli(&[
        "-d",
        endpoint.device_path.as_str(),
        "--device-open-qmi",
        "--device-open-proxy",
        secondary_qmi::QMI_OPEN_NET_ARG,
        cid.as_str(),
        "--wds-noop",
    ])
    .await;
}

async fn retained_session_is_active(session: &SecondaryDataSession) -> bool {
    let cid = format!("--client-cid={}", session.client_id);
    let Ok(output) = run_qmicli(&[
        "-d",
        session.endpoint.device_path.as_str(),
        "--device-open-qmi",
        "--device-open-proxy",
        secondary_qmi::QMI_OPEN_NET_ARG,
        cid.as_str(),
        "--client-no-release-cid",
        "--wds-get-packet-service-status",
    ])
    .await
    else {
        return false;
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.status.success()
        && text.lines().any(|line| {
            line.to_ascii_lowercase()
                .contains("connection status: 'connected'")
        })
}

async fn run_qmicli(args: &[&str]) -> Result<std::process::Output, String> {
    run_qmicli_with_timeout(args, Duration::from_secs(20)).await
}

async fn run_qmicli_with_timeout(
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    tokio::time::timeout(timeout, Command::new("qmicli").args(args).output())
        .await
        .map_err(|_| "secondary_qmi_data_command_timeout".to_string())?
        .map_err(|error| format!("secondary_qmi_data_command_spawn_failed:{error}"))
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reference_wds_client_id() {
        let line = "[Debug] [/dev/wwan0qmi1] registered 'wds' (version unknown) client with ID '5'";
        assert_eq!(
            secondary_qmi::parse_wds_client_id(line).as_deref(),
            Some("5")
        );
        let allocation = "translated = [ service = 'wds' cid = '17' ]";
        assert_eq!(
            secondary_qmi::parse_wds_client_id(allocation).as_deref(),
            Some("17")
        );
        assert_eq!(
            secondary_qmi::parse_wds_client_id("CID: '23'").as_deref(),
            Some("23")
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
    fn retained_start_matches_beta8_open_and_cid_contract() {
        let apn = ApnConfig {
            apn: "internet".to_string(),
            protocol: "dual".to_string(),
            username: "u".to_string(),
            password: "p".to_string(),
            auth_method: "chap".to_string(),
        };
        let action = start_action(&apn, "internet", 4).unwrap();
        let family = "--wds-set-ip-family=4";
        let allocation = retained_allocate_args("/dev/wwan0at2");
        let cid = "--client-cid=17";
        let family_args = retained_action_args("/dev/wwan0at2", cid, family);
        let start_args = retained_action_args("/dev/wwan0at2", cid, &action);
        assert!(action.contains("--wds-start-network=apn=internet,ip-type=4"));
        assert!(action.contains(",auth=CHAP"));
        assert!(allocation.contains(&"--wds-noop"));
        assert!(allocation.contains(&"--client-no-release-cid"));
        assert!(allocation.contains(&secondary_qmi::QMI_OPEN_NET_ARG));
        assert!(family_args.contains(&secondary_qmi::QMI_OPEN_NET_ARG));
        assert!(start_args.contains(&secondary_qmi::QMI_OPEN_NET_ARG));
        assert!(family_args.contains(&family));
        assert!(family_args.contains(&cid));
        assert!(!family_args.contains(&action.as_str()));
        assert!(start_args.contains(&action.as_str()));
        assert!(start_args.contains(&cid));
        assert!(!start_args.contains(&family));
        assert!(!start_args.contains(&"--wds-follow-network"));
    }
}
