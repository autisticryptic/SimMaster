//! Qualcomm IPv6 IMS data-path preflight.
//!
//! The 410 reference runtime primes the QMI WDS/MUX path before asking
//! ModemManager to create the final IMS bearer. This is not the shared mobile
//! data connection: it is a short-lived IPv6 `ims` WDS session used to make
//! the modem allocate the PCO/prefix state that the later bearer consumes.

use std::{net::IpAddr, process::Output, time::Duration};

use tokio::{process::Command, time::timeout};

use super::errors::{code, VolteError};

const QMI_TIMEOUT: Duration = Duration::from_secs(30);
const IMS_APN: &str = "ims";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// WDS returned an IPv6 address and gateway; SIP probing is intentionally
    /// left to the real REGISTER path.
    Ready,
    /// The reference also tolerates a SIP probe timeout after WDS succeeds.
    NoSipResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbePath<'a> {
    interface: &'a str,
    data_port: &'a str,
}

const PROBE_PATHS: &[ProbePath<'_>] = &[
    ProbePath {
        interface: "wwan1",
        data_port: "a2-mux-rmnet1",
    },
    ProbePath {
        interface: "wwan0",
        data_port: "a2-mux-rmnet0",
    },
];

/// Run the bounded WDS preflight. A failed path is cleaned up before trying
/// the next MUX endpoint; callers may continue to the ModemManager bearer if
/// every path is unavailable, matching the reference's best-effort probe.
pub async fn probe_ims_ipv6(qmi_device: &str, cid_hint: u8) -> Result<ProbeResult, VolteError> {
    let mut last_error = None;
    let mut initialized = false;
    for path in PROBE_PATHS {
        match probe_path(qmi_device, cid_hint, path).await {
            // The reference continues to the shared wwan0 MUX after the
            // wwan1 SIP capability probe times out, so prime both paths.
            Ok(()) => initialized = true,
            Err(error) => last_error = Some(error),
        }
    }
    if initialized {
        return Ok(ProbeResult::NoSipResponse);
    }
    Err(last_error.unwrap_or_else(|| VolteError::new(code::COMMAND_FAILED)))
}

async fn probe_path(
    qmi_device: &str,
    cid_hint: u8,
    path: &ProbePath<'_>,
) -> Result<(), VolteError> {
    let cid = allocate_wds_client(qmi_device).await?;
    let mut pdh = None;
    let result = async {
        qmi_action(
            qmi_device,
            cid,
            &format!("--wds-bind-data-port={}", path.data_port),
        )
        .await?;
        qmi_action(qmi_device, cid, "--wds-set-ip-family=6").await?;
        let start = format!("--wds-start-network=apn={IMS_APN},3gpp-profile={cid_hint},ip-type=6");
        let started = qmi_action(qmi_device, cid, &start).await?;
        pdh = parse_packet_handle(&started);
        if pdh.is_none() {
            return Err(VolteError::with_detail(
                code::COMMAND_FAILED,
                "qmicli:wds-start-network:pdh-missing",
            ));
        }
        let settings = qmi_action(qmi_device, cid, "--wds-get-current-settings").await?;
        if !has_ipv6_settings(&settings) {
            return Err(VolteError::with_detail(
                code::COMMAND_FAILED,
                format!("qmicli:{}:ipv6-settings-missing", path.interface),
            ));
        }
        Ok(())
    }
    .await;

    if let Some(handle) = pdh {
        let stop = format!("--wds-stop-network={handle}");
        let _ = qmi_action(qmi_device, cid, &stop).await;
    }
    release_wds_client(qmi_device, cid).await;
    result
}

async fn allocate_wds_client(qmi_device: &str) -> Result<u8, VolteError> {
    let output = qmi_command(qmi_device, &["--client-no-release-cid", "--wds-noop"]).await?;
    parse_client_id(&output)
        .ok_or_else(|| VolteError::with_detail(code::COMMAND_FAILED, "qmicli:wds-noop:cid-missing"))
}

async fn qmi_action(qmi_device: &str, cid: u8, action: &str) -> Result<String, VolteError> {
    let cid_arg = format!("--client-cid={cid}");
    qmi_command(qmi_device, &["--client-no-release-cid", &cid_arg, action]).await
}

async fn release_wds_client(qmi_device: &str, cid: u8) {
    let cid_arg = format!("--client-cid={cid}");
    let _ = qmi_command(qmi_device, &[&cid_arg, "--wds-noop"]).await;
}

async fn qmi_command(qmi_device: &str, args: &[&str]) -> Result<String, VolteError> {
    let mut command_args = vec!["-d", qmi_device, "--device-open-proxy"];
    command_args.extend_from_slice(args);
    let output = timeout(
        QMI_TIMEOUT,
        Command::new("qmicli").args(&command_args).output(),
    )
    .await
    .map_err(|_| VolteError::with_detail(code::COMMAND_TIMEOUT, "qmicli"))?
    .map_err(|error| {
        VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("qmicli:{error}"))
    })?;
    command_output("qmicli", &command_args, output)
}

fn command_output(program: &str, args: &[&str], output: Output) -> Result<String, VolteError> {
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr)
        .trim()
        .replace('\n', " ");
    Err(VolteError::with_detail(
        code::COMMAND_FAILED,
        format!(
            "{program}:{}:{}:{stderr}",
            output.status.code().unwrap_or(-1),
            args.join(" ")
        ),
    ))
}

fn parse_client_id(output: &str) -> Option<u8> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("CID:")?;
        value.trim().trim_matches('\'').parse::<u8>().ok()
    })
}

fn parse_packet_handle(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        let (_, value) = line.split_once("Packet data handle:")?;
        let value = value.trim().trim_matches('\'');
        value
            .strip_prefix("0x")
            .and_then(|v| u32::from_str_radix(v, 16).ok())
            .or_else(|| value.parse::<u32>().ok())
    })
}

fn has_ipv6_settings(output: &str) -> bool {
    let has_address = output.lines().any(|line| {
        line.to_ascii_lowercase().contains("ipv6 address")
            && line.split_once(':').is_some_and(|(_, value)| {
                value.trim().parse::<IpAddr>().is_ok()
                    || value
                        .trim()
                        .split('/')
                        .next()
                        .unwrap_or("")
                        .parse::<IpAddr>()
                        .is_ok()
            })
    });
    let has_gateway = output.lines().any(|line| {
        line.to_ascii_lowercase().contains("ipv6 gateway")
            && line.split_once(':').is_some_and(|(_, value)| {
                value.trim().parse::<IpAddr>().is_ok()
                    || value
                        .trim()
                        .split('/')
                        .next()
                        .unwrap_or("")
                        .parse::<IpAddr>()
                        .is_ok()
            })
    });
    has_address && has_gateway
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_match_reference_order() {
        assert_eq!(PROBE_PATHS[0].interface, "wwan1");
        assert_eq!(PROBE_PATHS[0].data_port, "a2-mux-rmnet1");
        assert_eq!(PROBE_PATHS[1].interface, "wwan0");
        assert_eq!(PROBE_PATHS[1].data_port, "a2-mux-rmnet0");
    }

    #[test]
    fn parses_qmi_ids_and_handles() {
        assert_eq!(parse_client_id("Service: 'wds'\n    CID: '2'"), Some(2));
        assert_eq!(
            parse_packet_handle("Packet data handle: '0x1234'"),
            Some(0x1234)
        );
        assert_eq!(parse_packet_handle("Packet data handle: 42"), Some(42));
    }

    #[test]
    fn validates_ipv6_settings() {
        let output = "IPv6 address: 240e::1/64\nIPv6 gateway address: fe80::1\n";
        assert!(has_ipv6_settings(output));
        assert!(!has_ipv6_settings("IPv6 address: 240e::1/64\n"));
    }
}
