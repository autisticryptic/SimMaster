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

use std::{future::Future, net::IpAddr, pin::Pin, process::Command as StdCommand, time::Duration};

use tokio::process::Command;

use crate::hardware::cellular::{cgcontrdp::CgcontrdpSettings, qmi_wds};
use crate::hardware::devices::qcm410::{
    netdev::{self as qmi_netdev, NetdevConfig},
    secondary_qmi,
};
use crate::hardware::devices::transport::{
    BearerInterfaceOwnership, ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint,
    ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, TransportFuture,
};

const PRIMARY_QMI_DEVICE: &str = "/dev/wwan0qmi0";
const PRIMARY_QMI_OPEN_FLAGS: [&str; 3] = [
    "--device-open-qmi",
    "--device-open-proxy",
    secondary_qmi::QMI_OPEN_NET_ARG,
];
const CURRENT_SETTINGS_RETRIES: usize = 12;
const QMI_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const WDS_START_TIMEOUT: Duration = Duration::from_secs(65);

fn is_primary_qmi_device(device: &str) -> bool {
    device.trim().starts_with("/dev/") && qmi_netdev::primary_netdev_for_qmi(device).is_some()
}

/// The QCA410 IMS access leg is deliberately singular. Do not add a second
/// path, environment switch, or configuration knob here: the verified device
/// contract is primary QMI + qmi-proxy for IMS, with DATA6 reserved for data.
fn primary_netdev_for_qmi(device: &str) -> Option<String> {
    qmi_netdev::primary_netdev_for_qmi(device)
}

/// Retained primary-QMI WDS session. The WDS CID is kept by qmi-proxy so
/// subsequent qmicli processes can fetch settings/status and stop the call.
struct PrimaryQmiSession {
    device_path: String,
    client_id: String,
    packet_data_handle: String,
}

impl PrimaryQmiSession {
    fn check_liveness(&mut self) -> Result<(), String> {
        let cid = format!("--client-cid={}", self.client_id);
        let args = action_args(
            &self.device_path,
            &cid,
            "--wds-get-packet-service-status",
            true,
        );
        let output = StdCommand::new("qmicli")
            .args(args)
            .output()
            .map_err(|error| format!("qca410_primary_qmi_status_spawn_failed:{error}"))?;
        let text = output_text(&output);
        if output.status.success()
            && text.lines().any(|line| {
                line.to_ascii_lowercase()
                    .contains("connection status: 'connected'")
            })
        {
            Ok(())
        } else {
            Err(format!(
                "qca410_primary_qmi_session_disconnected:{}",
                compact(&text)
            ))
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
            release_primary_client(&session).await;
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
    _modem_id: &str,
    apn: &str,
    profile_id: Option<u32>,
    _context_cid: u8,
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

    let settings = match wait_for_current_settings(&session, first_family).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_primary_session(&mut session).await;
            release_primary_client(&session).await;
            return Err(error);
        }
    };
    let settings = match settings_for_started_family(settings, first_family) {
        Ok(settings) => settings,
        Err(error) => {
            stop_primary_session(&mut session).await;
            release_primary_client(&session).await;
            return Err(error);
        }
    };
    let Some(config) = netdev_config_for(&settings, first_family) else {
        stop_primary_session(&mut session).await;
        release_primary_client(&session).await;
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
            release_primary_client(&session).await;
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

fn action_args(device: &str, cid: &str, action: &str, no_release: bool) -> Vec<String> {
    let mut args = primary_open_args(device);
    args.push(cid.to_string());
    if no_release {
        args.push("--client-no-release-cid".to_string());
    }
    args.push(action.to_string());
    args
}

async fn run_primary(args: Vec<String>, timeout: Duration) -> Result<std::process::Output, String> {
    tokio::time::timeout(timeout, Command::new("qmicli").args(args).output())
        .await
        .map_err(|_| "qca410_primary_qmi_command_timeout".to_string())?
        .map_err(|error| format!("qca410_primary_qmi_command_spawn_failed:{error}"))
}

async fn start_primary_session(
    device: &str,
    apn: &str,
    family: u8,
    profile_id: Option<u32>,
) -> Result<PrimaryQmiSession, String> {
    let allocation = run_primary(
        {
            let mut args = primary_open_args(device);
            args.extend([
                "--client-no-release-cid".to_string(),
                "--wds-noop".to_string(),
            ]);
            args
        },
        QMI_COMMAND_TIMEOUT,
    )
    .await?;
    let allocation_text = output_text(&allocation);
    if !allocation.status.success() {
        return Err(format!(
            "qca410_primary_qmi_cid_allocate_failed:{}",
            compact(&allocation_text)
        ));
    }
    let client_id = secondary_qmi::parse_wds_client_id(&allocation_text).ok_or_else(|| {
        format!(
            "qca410_primary_qmi_cid_missing:{}",
            compact(&allocation_text)
        )
    })?;
    let cid = format!("--client-cid={client_id}");

    let family_action = format!("--wds-set-ip-family={family}");
    let family_output = run_primary(
        action_args(device, &cid, &family_action, true),
        QMI_COMMAND_TIMEOUT,
    )
    .await;
    if let Err(error) = family_output.and_then(|output| {
        if output.status.success() {
            Ok(output)
        } else {
            Err(format!(
                "qca410_primary_qmi_family_failed:{}",
                compact(&output_text(&output))
            ))
        }
    }) {
        release_primary_client_parts(device, &client_id).await;
        return Err(error);
    }

    let mut start = format!("--wds-start-network=apn={apn}");
    if let Some(profile_id) = profile_id {
        start.push_str(&format!(",3gpp-profile={profile_id}"));
    }
    start.push_str(&format!(",ip-type={family}"));
    let output = match run_primary(action_args(device, &cid, &start, true), WDS_START_TIMEOUT).await
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            release_primary_client_parts(device, &client_id).await;
            return Err(format!(
                "qca410_primary_qmi_start_failed:{}",
                compact(&output_text(&output))
            ));
        }
        Err(error) => {
            release_primary_client_parts(device, &client_id).await;
            return Err(error);
        }
    };
    let packet_data_handle = match secondary_qmi::parse_packet_data_handle(&output_text(&output)) {
        Some(handle) => handle,
        None => {
            release_primary_client_parts(device, &client_id).await;
            return Err("qca410_primary_qmi_packet_data_handle_missing".to_string());
        }
    };
    Ok(PrimaryQmiSession {
        device_path: device.to_string(),
        client_id,
        packet_data_handle,
    })
}

async fn wait_for_current_settings(
    session: &PrimaryQmiSession,
    family: u8,
) -> Result<CgcontrdpSettings, ImsBearerError> {
    let cid = format!("--client-cid={}", session.client_id);
    let mut last = String::new();
    for _ in 0..CURRENT_SETTINGS_RETRIES {
        match run_primary(
            action_args(
                &session.device_path,
                &cid,
                "--wds-get-current-settings",
                true,
            ),
            QMI_COMMAND_TIMEOUT,
        )
        .await
        {
            Ok(output) => {
                let text = output_text(&output);
                last = text.clone();
                if output.status.success() {
                    let current = qmi_wds::parse_current_settings(&text);
                    if let Some(settings) = current_settings_for_family(&current, family) {
                        return Ok(settings);
                    }
                }
            }
            Err(error) => last = error,
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(settings_missing(format!(
        "qca410_primary_qmi_current_settings_unavailable:{}",
        compact(&last)
    )))
}

fn current_settings_for_family(
    current: &qmi_wds::CurrentSettings,
    family: u8,
) -> Option<CgcontrdpSettings> {
    let parse = |value: Option<&String>| value.and_then(|value| value.parse::<IpAddr>().ok());
    let settings = CgcontrdpSettings {
        ipv4_address: parse(current.ipv4_address.as_ref()),
        ipv4_gateway: parse(current.ipv4_gateway.as_ref()),
        ipv4_dns: current
            .ipv4_dns
            .iter()
            .filter_map(|value| value.parse().ok())
            .collect(),
        ipv4_prefix: current.ipv4_prefix,
        ipv6_address: parse(current.ipv6_address.as_ref()),
        ipv6_gateway: parse(current.ipv6_gateway.as_ref()),
        ipv6_dns: current
            .ipv6_dns
            .iter()
            .filter_map(|value| value.parse().ok())
            .collect(),
        ipv6_prefix: current.ipv6_prefix,
        pcscf: current
            .pcscf
            .iter()
            .filter_map(|value| value.parse().ok())
            .collect(),
    };
    let address = if family == 6 {
        settings.ipv6_address
    } else {
        settings.ipv4_address
    }?;
    if (family == 4 && address.is_ipv4()) || (family == 6 && address.is_ipv6()) {
        Some(settings)
    } else {
        None
    }
}

async fn stop_primary_session(session: &mut PrimaryQmiSession) {
    let cid = format!("--client-cid={}", session.client_id);
    let stop = format!("--wds-stop-network={}", session.packet_data_handle);
    let _ = run_primary(
        action_args(&session.device_path, &cid, &stop, true),
        QMI_COMMAND_TIMEOUT,
    )
    .await;
}

async fn release_primary_client(session: &PrimaryQmiSession) {
    release_primary_client_parts(&session.device_path, &session.client_id).await;
}

async fn release_primary_client_parts(device: &str, client_id: &str) {
    let cid = format!("--client-cid={client_id}");
    let _ = run_primary(
        action_args(device, &cid, "--wds-noop", false),
        QMI_COMMAND_TIMEOUT,
    )
    .await;
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
        let args = action_args(
            PRIMARY_QMI_DEVICE,
            "--client-cid=7",
            "--wds-get-current-settings",
            true,
        );
        assert!(args.iter().any(|arg| arg == "--device-open-qmi"));
        assert!(args.iter().any(|arg| arg == "--device-open-proxy"));
        assert!(args
            .iter()
            .any(|arg| arg == secondary_qmi::QMI_OPEN_NET_ARG));
        assert!(!args.iter().any(|arg| arg.contains("bind-data-port")));
        assert!(!args.iter().any(|arg| arg.contains("bind-mux-data-port")));
    }

    #[test]
    fn current_settings_are_converted_for_the_started_family() {
        let current = qmi_wds::CurrentSettings {
            ipv4_address: Some("10.0.0.2".to_string()),
            ipv4_gateway: Some("10.0.0.1".to_string()),
            ipv4_dns: vec!["1.1.1.1".to_string()],
            ipv4_prefix: Some(30),
            pcscf: vec!["10.0.0.3".to_string()],
            ..Default::default()
        };
        let settings = current_settings_for_family(&current, 4).unwrap();
        assert_eq!(settings.ipv4_address, Some("10.0.0.2".parse().unwrap()));
        assert_eq!(settings.ipv4_prefix, Some(30));
        assert_eq!(settings.pcscf, vec!["10.0.0.3".parse::<IpAddr>().unwrap()]);
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
