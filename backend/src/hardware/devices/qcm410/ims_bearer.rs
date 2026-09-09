//! Qualcomm 410 native IMS bearer driver.
//!
//! QCA410 has one supported IMS access-leg layout. IMS WDS always uses the
//! primary `/dev/wwan0qmi0` control endpoint through `qmi-proxy`; DATA6 is
//! reserved for the ordinary cellular-data transport. This is a device
//! contract, not a runtime policy and must not be exposed as a user-selectable
//! mode. Other device drivers have their own implementations and do not use
//! this module.
//!
//! The QMI control endpoint remains owned by qmi-proxy/ModemManager. The
//! primary WDS data interface is still resolved and moved into the line's UE
//! namespace by the native bearer strategy, so SIP and IPsec remain isolated
//! per line.

use std::{
    future::Future,
    net::IpAddr,
    pin::Pin,
    process::Stdio,
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
};

use crate::hardware::cellular::cgcontrdp::{self, CgcontrdpSettings};
use crate::hardware::devices::qcm410::{
    netdev::{self as qmi_netdev, NetdevConfig},
    secondary_qmi,
};
use crate::hardware::devices::transport::{
    BearerInterfaceOwnership, ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint,
    ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, TransportFuture,
};

const PRIMARY_QMI_DEVICE: &str = "/dev/wwan0qmi0";
// A single qmicli process owns the primary WDS bearer for its entire lifetime.
// On QCA410, moving a retained CID between short-lived qmicli processes makes
// qmi-proxy hang up the endpoint; this proxy-only access leg therefore must not
// be changed back to an allocate/start/query/stop sequence. `--device-open-qmi`
// remains required by the project-created DATA6 secondary endpoint and must not
// be copied into this primary IMS leg.
const PRIMARY_QMI_OPEN_FLAGS: [&str; 2] = ["--device-open-proxy", secondary_qmi::QMI_OPEN_NET_ARG];
const CURRENT_SETTINGS_RETRIES: usize = 12;
const WDS_FOLLOW_START_TIMEOUT: Duration = Duration::from_secs(65);

fn is_primary_qmi_device(device: &str) -> bool {
    device.trim().starts_with("/dev/") && qmi_netdev::primary_netdev_for_qmi(device).is_some()
}

/// The QCA410 IMS access leg is deliberately singular. Do not add a second
/// path, environment switch, or configuration knob here: the verified device
/// contract is primary QMI + qmi-proxy for IMS, with DATA6 reserved for data.
fn primary_netdev_for_qmi(device: &str) -> Option<String> {
    qmi_netdev::primary_netdev_for_qmi(device)
}

/// Primary-QMI WDS session owned by one long-lived qmicli process.
///
/// Do not replace this with a numeric CID plus short-lived qmicli calls. The
/// QCA410 qmi-proxy/device combination invalidates that ownership boundary and
/// takes down ModemManager's primary endpoint.
struct PrimaryQmiSession {
    client_id: String,
    packet_data_handle: String,
    process: Child,
}

impl PrimaryQmiSession {
    fn check_liveness(&mut self) -> Result<(), String> {
        match self
            .process
            .try_wait()
            .map_err(|error| format!("qca410_primary_qmi_follow_status_failed:{error}"))?
        {
            None => Ok(()),
            Some(status) => Err(format!(
                "qca410_primary_qmi_session_disconnected:pid_exit={status}"
            )),
        }
    }
}

/// Everything needed to tear down one primary-QMI IMS bearer.
pub struct Qcm410ImsBearerHandle {
    session: PrimaryQmiSession,
    configured_netdev: Option<(String, NetdevConfig)>,
}

impl ImsBearerHandle for Qcm410ImsBearerHandle {
    fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
        self.session
            .check_liveness()
            .map_err(|detail| ImsBearerError {
                kind: ImsBearerErrorKind::SessionLost,
                hint: ImsBearerFailureHint::None,
                detail,
            })
    }

    fn release(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            let Qcm410ImsBearerHandle {
                mut session,
                configured_netdev,
            } = *self;
            if let Some((interface, config)) = configured_netdev {
                qmi_netdev::teardown(&interface, &config).await;
            }
            stop_primary_session(&mut session).await;
        })
    }
}

/// QCA410's only IMS bearer implementation: primary QMI + qmi-proxy.
pub struct Qcm410ImsBearer;

impl ImsBearerTransport for Qcm410ImsBearer {
    fn endpoint_available(&self, primary_device: &str) -> bool {
        let device = primary_device.trim();
        is_primary_qmi_device(device) && std::path::Path::new(device).exists()
    }

    fn establish_ims_bearer<'a>(
        &'a self,
        primary_device: &'a str,
        modem_id: &'a str,
        apn: &'a str,
        profile_id: Option<u32>,
        cid: u8,
        families: &'a [u8],
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>>
    {
        Box::pin(async move {
            let device = primary_device.trim();
            if !is_primary_qmi_device(device) {
                return Err(ImsBearerError {
                    kind: ImsBearerErrorKind::EndpointUnavailable,
                    hint: ImsBearerFailureHint::None,
                    detail: format!(
                        "qca410_ims_requires_primary_qmi_proxy:{PRIMARY_QMI_DEVICE}:got={device}"
                    ),
                });
            }
            let baseband =
                secondary_qmi::baseband_key_for_device(device).map_err(|error| ImsBearerError {
                    kind: ImsBearerErrorKind::BasebandUnresolved,
                    hint: ImsBearerFailureHint::None,
                    detail: format!("native_ims_baseband_unresolved:{error}"),
                })?;
            let netdev = primary_netdev_for_qmi(device).ok_or_else(|| ImsBearerError {
                kind: ImsBearerErrorKind::EndpointUnavailable,
                hint: ImsBearerFailureHint::None,
                detail: format!("qca410_primary_qmi_netdev_unresolved:{device}"),
            })?;
            establish_bearer(
                device, &netdev, &baseband, modem_id, apn, profile_id, cid, families,
            )
            .await
            .map(|established| {
                (
                    established.info,
                    Box::new(established.handle) as Box<dyn ImsBearerHandle + Send>,
                )
            })
        })
    }
}

struct Established {
    info: ImsBearerInfo,
    handle: Qcm410ImsBearerHandle,
}

async fn establish_bearer(
    device: &str,
    primary_netdev: &str,
    baseband: &str,
    modem_id: &str,
    apn: &str,
    profile_id: Option<u32>,
    context_cid: u8,
    families: &[u8],
) -> Result<Established, ImsBearerError> {
    let Some(first_family) = families.first().copied() else {
        return Err(session_start_error("native_ims_no_address_family"));
    };
    let mut session = match start_primary_session(device, apn, first_family, profile_id).await {
        Ok(session) => session,
        Err(error) => {
            return Err(ImsBearerError {
                kind: ImsBearerErrorKind::SessionStartFailed,
                hint: classify_session_failure(&error),
                detail: error,
            })
        }
    };

    // The long-lived qmicli process is the sole WDS owner. Read the active
    // context through the AT path instead of reopening qmi-proxy with the
    // retained CID from another process.
    let settings = match wait_for_current_settings(modem_id, context_cid, apn, first_family).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(error);
        }
    };
    let settings = match settings_for_started_family(settings, first_family) {
        Ok(settings) => settings,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(error);
        }
    };
    let Some(config) = netdev_config_for(&settings, first_family) else {
        stop_primary_session(&mut session).await;
        return Err(settings_missing(
            "qca410_primary_ims_session_has_no_address".to_string(),
        ));
    };

    // The control port is primary qmi0, but the WDS data interface is moved into
    // the line worker. DATA6 remains exclusively owned by secondary_qmi_data.
    let resolution = match qmi_netdev::resolve_exact(baseband, &config, primary_netdev).await {
        Ok(resolution) => resolution,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(ImsBearerError {
                kind: ImsBearerErrorKind::NetdevUnresolved,
                hint: if matches!(error, qmi_netdev::NetdevError::LinkUnavailable(_)) {
                    ImsBearerFailureHint::BasebandWedged
                } else {
                    ImsBearerFailureHint::None
                },
                detail: format!("qca410_primary_ims_netdev_unresolved:{error}"),
            });
        }
    };

    let info = ImsBearerInfo {
        interface: resolution.interface.clone(),
        netdev_method: resolution.method.as_str(),
        ip_type: ip_type_for(first_family).to_string(),
        path_device: device.to_string(),
        path_handle: format!("{}:{}", session.client_id, session.packet_data_handle),
        ipv4_address: settings.ipv4_address,
        ipv4_gateway: settings.ipv4_gateway,
        ipv4_dns: settings.ipv4_dns,
        ipv4_prefix: settings.ipv4_prefix,
        ipv6_address: settings.ipv6_address,
        ipv6_gateway: settings.ipv6_gateway,
        ipv6_dns: settings.ipv6_dns,
        ipv6_prefix: settings.ipv6_prefix,
        pcscf: settings.pcscf,
        // The primary control node is proxy-owned, while the data netdev is
        // intentionally application-owned for namespace migration.
        interface_ownership: BearerInterfaceOwnership::ApplicationOwnedNative,
        ..Default::default()
    };
    Ok(Established {
        info,
        handle: Qcm410ImsBearerHandle {
            session,
            configured_netdev: Some((resolution.interface, config)),
        },
    })
}

fn primary_open_args(device: &str) -> Vec<String> {
    let mut args = vec!["-d".to_string(), device.to_string()];
    args.extend(
        PRIMARY_QMI_OPEN_FLAGS
            .iter()
            .map(|flag| (*flag).to_string()),
    );
    args
}

fn primary_follow_args(
    device: &str,
    apn: &str,
    family: u8,
    profile_id: Option<u32>,
) -> Vec<String> {
    let mut args = primary_open_args(device);
    let mut start = format!("--wds-start-network=apn={apn}");
    if let Some(profile_id) = profile_id {
        start.push_str(&format!(",3gpp-profile={profile_id}"));
    }
    start.push_str(&format!(",ip-type={family}"));
    args.push(start);
    args.push("--wds-follow-network".to_string());
    args
}

async fn start_primary_session(
    device: &str,
    apn: &str,
    family: u8,
    profile_id: Option<u32>,
) -> Result<PrimaryQmiSession, String> {
    let mut process = Command::new("qmicli")
        .args(primary_follow_args(device, apn, family, profile_id))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("qca410_primary_qmi_follow_spawn_failed:{error}"))?;
    let stdout = match process.stdout.take() {
        Some(stdout) => stdout,
        None => {
            stop_child(&mut process).await;
            return Err("qca410_primary_qmi_follow_stdout_missing".to_string());
        }
    };
    let stderr = match process.stderr.take() {
        Some(stderr) => stderr,
        None => {
            stop_child(&mut process).await;
            return Err("qca410_primary_qmi_follow_stderr_missing".to_string());
        }
    };
    let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
    tokio::spawn(drain_qmicli_stream(BufReader::new(stdout), sender.clone()));
    tokio::spawn(drain_qmicli_stream(BufReader::new(stderr), sender));

    let deadline = Instant::now() + WDS_FOLLOW_START_TIMEOUT;
    let mut startup = String::new();
    let mut client_id = None;
    let mut packet_data_handle = None;
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), receiver.recv()).await {
            Ok(Some(line)) => {
                if startup.len() < 16 * 1024 {
                    startup.push_str(&line);
                    startup.push('\n');
                }
                client_id = client_id.or_else(|| secondary_qmi::parse_wds_client_id(&line));
                packet_data_handle =
                    packet_data_handle.or_else(|| secondary_qmi::parse_packet_data_handle(&line));
                if packet_data_handle.is_some() {
                    break;
                }
            }
            Ok(None) | Err(_) => {}
        }
        if let Some(status) = process
            .try_wait()
            .map_err(|error| format!("qca410_primary_qmi_follow_status_failed:{error}"))?
        {
            return Err(format!(
                "qca410_primary_qmi_start_failed:pid_exit={status}:{}",
                compact(&startup)
            ));
        }
    }
    let Some(packet_data_handle) = packet_data_handle else {
        stop_child(&mut process).await;
        return Err(format!(
            "qca410_primary_qmi_packet_data_handle_missing:{}",
            compact(&startup)
        ));
    };
    Ok(PrimaryQmiSession {
        client_id: client_id.unwrap_or_else(|| "follow".to_string()),
        packet_data_handle,
        process,
    })
}

async fn drain_qmicli_stream<R>(mut reader: BufReader<R>, sender: mpsc::UnboundedSender<String>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    while let Ok(Some(line)) = reader.lines().next_line().await {
        if sender.send(line).is_err() {
            break;
        }
    }
}

async fn wait_for_current_settings(
    modem_id: &str,
    cid: u8,
    apn: &str,
    family: u8,
) -> Result<CgcontrdpSettings, ImsBearerError> {
    let mut last = String::new();
    for _ in 0..CURRENT_SETTINGS_RETRIES {
        match cgcontrdp::read_cgcontrdp_settings(modem_id, cid, apn).await {
            Ok(settings) if has_started_family(&settings, family) => return Ok(settings),
            Ok(_) => last = format!("active IMS context has no ipv{family} address"),
            Err(error) => last = error.to_string(),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(settings_missing(format!(
        "qca410_primary_qmi_current_settings_unavailable:{}",
        compact(&last)
    )))
}

fn has_started_family(settings: &CgcontrdpSettings, family: u8) -> bool {
    match family {
        4 => settings
            .ipv4_address
            .is_some_and(|address| address.is_ipv4()),
        6 => settings
            .ipv6_address
            .is_some_and(|address| address.is_ipv6()),
        _ => false,
    }
}

async fn stop_primary_session(session: &mut PrimaryQmiSession) {
    stop_child(&mut session.process).await;
}

async fn stop_child(process: &mut Child) {
    if let Some(pid) = process.id() {
        // qmicli's follow mode owns the WDS CID and performs the graceful
        // stop/release path when interrupted. Do not reopen qmi-proxy with a
        // second process to issue stop-network or release-cid.
        let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            match process.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(_) => break,
            }
        }
        let _ = process.start_kill();
    }
    let _ = process.wait().await;
}

fn ip_type_for(family: u8) -> &'static str {
    if family == 6 {
        "ipv6"
    } else {
        "ipv4"
    }
}

fn settings_for_started_family(
    mut settings: CgcontrdpSettings,
    family: u8,
) -> Result<CgcontrdpSettings, ImsBearerError> {
    let address = match family {
        4 => settings.ipv4_address.filter(|address| address.is_ipv4()),
        6 => settings.ipv6_address.filter(|address| address.is_ipv6()),
        _ => None,
    };
    if address.is_none() {
        return Err(settings_missing(format!(
            "native_ims_started_family_address_missing:ipv{family}"
        )));
    }
    if family == 4 {
        settings.ipv6_address = None;
        settings.ipv6_gateway = None;
        settings.ipv6_dns.clear();
        settings.ipv6_prefix = None;
    } else {
        settings.ipv4_address = None;
        settings.ipv4_gateway = None;
        settings.ipv4_dns.clear();
        settings.ipv4_prefix = None;
    }
    settings
        .pcscf
        .retain(|address| address.is_ipv4() == (family == 4));
    Ok(settings)
}

fn netdev_config_for(settings: &CgcontrdpSettings, family: u8) -> Option<NetdevConfig> {
    let (address, gateway, dns, prefix) = if family == 6 {
        (
            settings.ipv6_address,
            settings.ipv6_gateway,
            &settings.ipv6_dns,
            settings.ipv6_prefix,
        )
    } else {
        (
            settings.ipv4_address,
            settings.ipv4_gateway,
            &settings.ipv4_dns,
            settings.ipv4_prefix,
        )
    };
    Some(NetdevConfig::from_session(
        address?, prefix, None, dns, gateway,
    ))
}

fn classify_session_failure(detail: &str) -> ImsBearerFailureHint {
    let error = detail.to_ascii_lowercase();
    if error.contains("ipv6onlyallowed")
        || error.contains("ipv6-only-allowed")
        || error.contains("only ipv6 allowed")
        || error.contains("pdn-ipv4-call-disallowed")
    {
        ImsBearerFailureHint::NetworkForcedIpv6
    } else if error.contains("ipv4onlyallowed")
        || error.contains("ipv4-only-allowed")
        || error.contains("only ipv4 allowed")
        || error.contains("pdn-ipv6-call-disallowed")
    {
        ImsBearerFailureHint::NetworkForcedIpv4
    } else if error.contains("interface-in-use-config-match")
        || error.contains("endpoint hangup")
        || error.contains("mobileequipment.unknown")
        || error.contains("call failed") && error.contains("internal")
    {
        ImsBearerFailureHint::BasebandWedged
    } else {
        ImsBearerFailureHint::None
    }
}

fn session_start_error(detail: &str) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::SessionStartFailed,
        hint: ImsBearerFailureHint::None,
        detail: detail.to_string(),
    }
}

fn settings_missing(detail: String) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::SettingsMissing,
        hint: ImsBearerFailureHint::None,
        detail,
    }
}

fn compact(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn reference_settings() -> CgcontrdpSettings {
        CgcontrdpSettings {
            ipv4_address: Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 207))),
            ipv4_gateway: Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 208))),
            ipv4_dns: vec![IpAddr::V4(Ipv4Addr::new(172, 17, 163, 218))],
            ipv4_prefix: Some(27),
            pcscf: vec![IpAddr::V4(Ipv4Addr::new(10, 11, 12, 13))],
            ..Default::default()
        }
    }

    #[test]
    fn qca410_ims_has_one_primary_qmi_endpoint() {
        assert_eq!(PRIMARY_QMI_DEVICE, "/dev/wwan0qmi0");
        assert_eq!(
            primary_netdev_for_qmi(PRIMARY_QMI_DEVICE).as_deref(),
            Some("wwan0")
        );
        assert_eq!(primary_netdev_for_qmi("/dev/wwan0at2"), None);
    }

    #[test]
    fn qca410_rejects_a_secondary_ims_endpoint() {
        assert!(primary_netdev_for_qmi("/dev/wwan0qmi1").is_none());
        assert!(primary_netdev_for_qmi("/dev/wwanqmi0").is_none());
        assert!(!is_primary_qmi_device("wwan0qmi0"));
        assert_eq!(
            primary_netdev_for_qmi("/dev/wwan12qmi0").as_deref(),
            Some("wwan12")
        );
    }

    #[test]
    fn qca410_primary_commands_always_use_proxy_and_never_bind() {
        let args = primary_follow_args(PRIMARY_QMI_DEVICE, "ims", 4, Some(2));
        assert!(args.iter().any(|arg| arg == "--device-open-proxy"));
        assert!(args
            .iter()
            .any(|arg| arg == secondary_qmi::QMI_OPEN_NET_ARG));
        assert!(!args.iter().any(|arg| arg == "--device-open-qmi"));
        assert!(!args.iter().any(|arg| arg.contains("bind-data-port")));
        assert!(!args.iter().any(|arg| arg.contains("bind-mux-data-port")));
        assert!(args.iter().any(|arg| arg == "--wds-follow-network"));
        assert!(args
            .iter()
            .any(|arg| arg == "--wds-start-network=apn=ims,3gpp-profile=2,ip-type=4"));
    }

    #[test]
    fn current_settings_are_filtered_for_the_started_family() {
        let mut settings = reference_settings();
        settings.ipv6_address = Some("2001:db8::2".parse().unwrap());
        settings.ipv6_gateway = Some("2001:db8::1".parse().unwrap());
        settings.ipv6_prefix = Some(64);
        settings.ipv6_dns = vec!["2001:4860:4860::8888".parse().unwrap()];
        settings.pcscf.push("2001:db8::3".parse().unwrap());
        let settings = settings_for_started_family(settings, 4).unwrap();
        assert_eq!(
            settings.ipv4_address,
            Some("10.129.39.207".parse().unwrap())
        );
        assert!(settings.ipv6_address.is_none());
        assert!(settings.pcscf.iter().all(IpAddr::is_ipv4));
    }

    #[test]
    fn netdev_config_picks_the_started_family() {
        let config = netdev_config_for(&reference_settings(), 4).unwrap();
        assert_eq!(config.address, "10.129.39.207".parse::<IpAddr>().unwrap());
        assert_eq!(config.prefix, 27);
        assert!(netdev_config_for(&reference_settings(), 6).is_none());
    }

    #[test]
    fn started_family_filter_drops_unstarted_addresses() {
        let mut dual = reference_settings();
        dual.ipv6_address = Some("2001:db8::2".parse().unwrap());
        dual.ipv6_gateway = Some("2001:db8::1".parse().unwrap());
        dual.ipv6_prefix = Some(64);
        dual.pcscf.push("2001:db8::3".parse().unwrap());
        let v4 = settings_for_started_family(dual, 4).unwrap();
        assert!(v4.ipv6_address.is_none());
        assert!(v4.pcscf.iter().all(IpAddr::is_ipv4));
    }
}
