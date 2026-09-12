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

use std::{future::Future, net::IpAddr, pin::Pin, time::Duration};

use crate::hardware::cellular::cgcontrdp::{self, CgcontrdpSettings};
use crate::hardware::devices::qcm410::{
    netdev::{self as qmi_netdev, NetdevConfig},
    primary_ims_session::{PrimaryImsRequest, PrimaryImsSession},
    secondary_qmi,
};
use crate::hardware::devices::transport::{
    BearerInterfaceOwnership, ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint,
    ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, TransportFuture,
};

const PRIMARY_QMI_DEVICE: &str = "/dev/wwan0qmi0";
// The primary control endpoint and WDS client stay with ModemManager/proxy.
// Do not replace its complete BAM-DMUX session with an independent qmicli
// start/follow command: that obtained PCO addresses but no SIP replies on SIM-03.
// Only our newly created, exclusive IMS bearer supplies the data interface that
// moves into the UE worker. Existing Internet bearers are never borrowed.
const CURRENT_SETTINGS_RETRIES: usize = 12;

fn is_primary_qmi_device(device: &str) -> bool {
    device.trim().starts_with("/dev/") && qmi_netdev::primary_netdev_for_qmi(device).is_some()
}

/// The QCA410 IMS access leg is deliberately singular. Do not add a second
/// path, environment switch, or configuration knob here: the verified device
/// contract is primary QMI + qmi-proxy for IMS, with DATA6 reserved for data.
fn primary_netdev_for_qmi(device: &str) -> Option<String> {
    qmi_netdev::primary_netdev_for_qmi(device)
}

/// Everything needed to tear down one primary-QMI IMS bearer.
pub struct Qcm410ImsBearerHandle {
    session: PrimaryImsSession,
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
            let Qcm410ImsBearerHandle { mut session } = *self;
            stop_primary_session(&mut session).await;
        })
    }

    fn prepare_namespace_move(&mut self, namespace: &str) -> Result<Box<dyn Send>, ImsBearerError> {
        self.session
            .namespace_will_change(namespace)
            .map(|guard| Box::new(guard) as Box<dyn Send>)
            .map_err(settings_missing)
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
        allow_roaming: bool,
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
                device,
                &netdev,
                &baseband,
                modem_id,
                apn,
                profile_id,
                cid,
                families,
                allow_roaming,
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
    allow_roaming: bool,
) -> Result<Established, ImsBearerError> {
    let Some(first_family) = families.first().copied() else {
        return Err(session_start_error("native_ims_no_address_family"));
    };
    let mut session = match PrimaryImsSession::start(PrimaryImsRequest {
        device,
        modem: modem_id,
        interface: primary_netdev,
        apn,
        profile_id,
        family: first_family,
        allow_roaming,
    })
    .await
    {
        Ok(session) => session,
        Err(error) => {
            return Err(ImsBearerError {
                kind: ImsBearerErrorKind::SessionStartFailed,
                hint: classify_session_failure(&error),
                detail: error,
            })
        }
    };

    // ModemManager holds this exact IMS WDS client for its whole lifetime.
    // AT reads the corresponding active context without taking over the CID.
    let settings = match wait_for_current_settings(modem_id, context_cid, apn, first_family).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(error);
        }
    };
    if let Err(detail) = session.check_liveness() {
        stop_primary_session(&mut session).await;
        return Err(ImsBearerError {
            kind: ImsBearerErrorKind::SessionLost,
            hint: ImsBearerFailureHint::None,
            detail,
        });
    }
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
    let network_guard = match session.network_will_be_configured(&config) {
        Ok(guard) => guard,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(settings_missing(error));
        }
    };

    // The control port is primary qmi0, but the WDS data interface is moved into
    // the line worker. DATA6 remains exclusively owned by secondary_qmi_data.
    let resolution = match qmi_netdev::resolve_exact(baseband, &config, primary_netdev).await {
        Ok(resolution) => resolution,
        Err(error) => {
            drop(network_guard);
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
    drop(network_guard);
    if let Err(detail) = session.check_liveness() {
        stop_primary_session(&mut session).await;
        return Err(ImsBearerError {
            kind: ImsBearerErrorKind::SessionLost,
            hint: ImsBearerFailureHint::None,
            detail,
        });
    }

    let info = ImsBearerInfo {
        interface: resolution.interface.clone(),
        netdev_method: resolution.method.as_str(),
        ip_type: ip_type_for(first_family).to_string(),
        path_device: device.to_string(),
        path_handle: format!("mm:{}", session.path()),
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
        handle: Qcm410ImsBearerHandle { session },
    })
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

async fn stop_primary_session(session: &mut PrimaryImsSession) {
    session.stop().await;
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
