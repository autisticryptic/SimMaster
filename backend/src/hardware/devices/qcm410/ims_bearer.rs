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

use std::{future::Future, pin::Pin, time::Duration};

use crate::hardware::cellular::cgcontrdp::CgcontrdpSettings;
use crate::hardware::devices::qcm410::{
    netdev::{self as qmi_netdev, NetdevConfig},
    primary_ims_session::{PrimaryImsRequest, PrimaryImsSession},
    primary_ims_settings::MmIpFamily,
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
    _context_cid: u8,
    families: &[u8],
    allow_roaming: bool,
) -> Result<Established, ImsBearerError> {
    // A two-family request is MM's distinct IPV4V6 flag, not two independent
    // owners and not an instruction to silently start only the first family.
    let requested_family =
        MmIpFamily::from_requested(families).map_err(|detail| session_start_error(&detail))?;
    let mut session = match PrimaryImsSession::start(PrimaryImsRequest {
        device,
        modem: modem_id,
        interface: primary_netdev,
        apn,
        profile_id,
        family: requested_family,
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

    // MM reads IP/DNS on its retained WDS client and publishes the result on
    // the exact owned bearer. AT can omit DNS or refer to a different PDP CID;
    // it must not replace this object's addressing. The upper IMS layer may
    // supplement P-CSCF only after matching the observed bearer source address.
    let settings = match wait_for_current_settings(&session).await {
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
    let GrantedSettings {
        settings,
        networks,
        family: granted_family,
    } = match prepare_granted_settings(settings, families) {
        Ok(granted) => granted,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(error);
        }
    };
    tracing::info!(
        bearer = session.path(),
        requested_ip_type = requested_family.as_str(),
        granted_ip_type = granted_family.as_str(),
        settings_source = "modemmanager_bearer_ip_config",
        ipv4_dns_count = settings.ipv4_dns.len(),
        ipv6_dns_count = settings.ipv6_dns.len(),
        has_ipv4_gateway = settings.ipv4_gateway.is_some(),
        has_ipv6_gateway = settings.ipv6_gateway.is_some(),
        "Read primary IMS IP configuration from the owned ModemManager bearer"
    );
    // Persist every granted address before either family can change the kernel.
    // A failed/cancelled second step must not leave the first family untracked.
    let network_guard = match session.network_will_be_configured(&networks) {
        Ok(guard) => guard,
        Err(error) => {
            stop_primary_session(&mut session).await;
            return Err(settings_missing(error));
        }
    };

    // The control port is primary qmi0, but the WDS data interface is moved into
    // the line worker. DATA6 remains exclusively owned by secondary_qmi_data.
    // Both families configure this same verified interface; no candidate probe
    // or independent WDS start is introduced for the second family.
    let baseband = baseband.to_string();
    let interface = primary_netdev.to_string();
    let resolution = match configure_primary_networks(networks, network_guard, move |config| {
        let baseband = baseband.clone();
        let interface = interface.clone();
        async move { qmi_netdev::resolve_exact(&baseband, &config, &interface).await }
    })
    .await
    {
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
        // This is the actual grant, not an echo of the requested MM flag.
        ip_type: granted_family.as_str().to_string(),
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
    session: &PrimaryImsSession,
) -> Result<CgcontrdpSettings, ImsBearerError> {
    tokio::time::timeout(Duration::from_secs(20), async {
        for _ in 0..CURRENT_SETTINGS_RETRIES {
            // A missing publication can settle; malformed data, owner loss or
            // unsupported DHCP/PPP must not be hidden by a CLI/AT fallback.
            if let Some(settings) = session.read_ip_settings().await? {
                return Ok(settings);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Err("qca410_primary_mm_ip_config_not_ready".to_string())
    })
    .await
    .map_err(|_| settings_missing("qca410_primary_mm_ip_config_timeout".to_string()))?
    .map_err(settings_missing)
}

async fn stop_primary_session(session: &mut PrimaryImsSession) {
    session.stop().await;
}

/// Hold the durable lease's activity guard inside a shielded task. Cancelling
/// the caller must not release the guard while an `ip` mutation can still land;
/// session cleanup waits for this entire recorded batch before tearing it down.
async fn configure_primary_networks<F, R, G>(
    networks: Vec<NetdevConfig>,
    guard: G,
    mut configure: F,
) -> Result<qmi_netdev::ResolvedNetdev, qmi_netdev::NetdevError>
where
    F: FnMut(NetdevConfig) -> R + Send + 'static,
    R: Future<Output = Result<qmi_netdev::ResolvedNetdev, qmi_netdev::NetdevError>>
        + Send
        + 'static,
    G: Send + 'static,
{
    tokio::spawn(async move {
        let _guard = guard;
        let mut resolution = None;
        for network in networks {
            resolution = Some(configure(network).await?);
        }
        resolution.ok_or_else(|| {
            qmi_netdev::NetdevError::ConfigureFailed(
                "qca410_primary_ims_network_plan_empty".to_string(),
            )
        })
    })
    .await
    .map_err(|_| {
        qmi_netdev::NetdevError::ConfigureFailed(
            "qca410_primary_ims_network_task_failed".to_string(),
        )
    })?
}

struct GrantedSettings {
    settings: CgcontrdpSettings,
    networks: Vec<NetdevConfig>,
    family: MmIpFamily,
}

/// MM may grant just one family for IPV4V6. Configure and report only granted
/// addresses, in the caller's preference order, without manufacturing a second
/// grant or discarding a valid one. Explicit single-family requests stay single.
fn prepare_granted_settings(
    mut settings: CgcontrdpSettings,
    families: &[u8],
) -> Result<GrantedSettings, ImsBearerError> {
    let requested = MmIpFamily::from_requested(families).map_err(settings_missing)?;
    if requested == MmIpFamily::Ipv6 || settings.ipv4_address.is_none() {
        settings.ipv4_address = None;
        settings.ipv4_gateway = None;
        settings.ipv4_dns.clear();
        settings.ipv4_prefix = None;
    }
    if requested == MmIpFamily::Ipv4 || settings.ipv6_address.is_none() {
        settings.ipv6_address = None;
        settings.ipv6_gateway = None;
        settings.ipv6_dns.clear();
        settings.ipv6_prefix = None;
    }
    let has_ipv4 = settings.ipv4_address.is_some();
    let has_ipv6 = settings.ipv6_address.is_some();
    let granted = match (has_ipv4, has_ipv6) {
        (true, true) => MmIpFamily::Ipv4v6,
        (true, false) => MmIpFamily::Ipv4,
        (false, true) => MmIpFamily::Ipv6,
        (false, false) => {
            return Err(settings_missing(
                "qca410_primary_ims_session_has_no_address".to_string(),
            ));
        }
    };
    let networks: Vec<_> = families
        .iter()
        .filter_map(|family| netdev_config_for(&settings, *family))
        .collect();
    if networks.len() != usize::from(has_ipv4) + usize::from(has_ipv6) {
        return Err(settings_missing(
            "qca410_primary_ims_granted_network_invalid".to_string(),
        ));
    }
    settings.pcscf.retain(|address| {
        if address.is_ipv4() {
            has_ipv4
        } else {
            has_ipv6
        }
    });
    Ok(GrantedSettings {
        settings,
        networks,
        family: granted,
    })
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
    let address = address?;
    let prefix = prefix?;
    if !matches!(family, 4 | 6)
        || address.is_ipv4() != (family == 4)
        || prefix > if family == 4 { 32 } else { 128 }
    {
        return None;
    }
    Some(NetdevConfig::from_session(
        address,
        Some(prefix),
        None,
        dns,
        gateway,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{IpAddr, Ipv4Addr},
        sync::{Arc, Mutex},
    };

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

    fn dual_settings() -> CgcontrdpSettings {
        let mut settings = reference_settings();
        settings.ipv6_address = Some("2001:db8::2".parse().unwrap());
        settings.ipv6_gateway = Some("2001:db8::1".parse().unwrap());
        settings.ipv6_prefix = Some(64);
        settings.ipv6_dns = vec!["2001:db8::53".parse().unwrap()];
        settings.pcscf.push("2001:db8::3".parse().unwrap());
        settings
    }

    fn resolved() -> qmi_netdev::ResolvedNetdev {
        qmi_netdev::ResolvedNetdev {
            interface: "wwan0".to_string(),
            rx_packets: 0,
            method: qmi_netdev::ResolutionMethod::SoleCandidate,
        }
    }

    #[test]
    fn dual_grant_preserves_both_families_in_the_requested_order() {
        for families in [[4, 6], [6, 4]] {
            let grant = prepare_granted_settings(dual_settings(), &families).unwrap();
            assert_eq!(grant.family, MmIpFamily::Ipv4v6);
            assert_eq!(grant.networks.len(), 2);
            assert_eq!(grant.networks[0].address.is_ipv4(), families[0] == 4);
            assert_eq!(grant.networks[1].address.is_ipv4(), families[1] == 4);
            assert_eq!(grant.settings.ipv4_prefix, Some(27));
            assert_eq!(grant.settings.ipv6_prefix, Some(64));
            assert_eq!(grant.settings.ipv4_dns.len(), 1);
            assert_eq!(grant.settings.ipv6_dns.len(), 1);
            assert_eq!(grant.settings.pcscf.len(), 2);
        }
    }

    #[test]
    fn partial_dual_grant_reports_the_granted_family_not_the_first_request() {
        for available in [4, 6] {
            let mut settings = dual_settings();
            if available == 4 {
                settings.ipv6_address = None;
            } else {
                settings.ipv4_address = None;
            }
            let grant = prepare_granted_settings(settings, &[6, 4]).unwrap();
            assert_eq!(
                grant.family.as_str(),
                if available == 4 { "ipv4" } else { "ipv6" }
            );
            assert_eq!(grant.networks.len(), 1);
            assert_eq!(grant.networks[0].address.is_ipv4(), available == 4);
            assert_eq!(grant.settings.ipv4_dns.is_empty(), available != 4);
            assert_eq!(grant.settings.ipv6_dns.is_empty(), available != 6);
            assert_eq!(grant.settings.pcscf.len(), 1);
            assert_eq!(grant.settings.pcscf[0].is_ipv4(), available == 4);
        }
    }

    #[test]
    fn explicit_single_family_does_not_consume_an_unrequested_grant() {
        for family in [4, 6] {
            let grant = prepare_granted_settings(dual_settings(), &[family]).unwrap();
            assert_eq!(grant.networks.len(), 1);
            assert_eq!(grant.networks[0].address.is_ipv4(), family == 4);
            assert_eq!(grant.settings.ipv4_address.is_some(), family == 4);
            assert_eq!(grant.settings.ipv6_address.is_some(), family == 6);
        }
        assert!(prepare_granted_settings(reference_settings(), &[6]).is_err());
    }

    #[test]
    fn incomplete_or_wrong_family_grants_cannot_become_a_network_plan() {
        assert!(prepare_granted_settings(CgcontrdpSettings::default(), &[4, 6]).is_err());
        for families in [&[][..], &[0][..], &[4, 4][..], &[6, 4, 6][..]] {
            assert!(prepare_granted_settings(dual_settings(), families).is_err());
        }
        let mut missing_prefix = dual_settings();
        missing_prefix.ipv6_prefix = None;
        assert!(prepare_granted_settings(missing_prefix, &[4, 6]).is_err());
        let mut invalid_prefix = dual_settings();
        invalid_prefix.ipv4_prefix = Some(33);
        assert!(prepare_granted_settings(invalid_prefix, &[4, 6]).is_err());
        let mut wrong_family = dual_settings();
        wrong_family.ipv6_address = Some("192.0.2.2".parse().unwrap());
        assert!(prepare_granted_settings(wrong_family, &[4, 6]).is_err());
    }

    #[tokio::test]
    async fn network_batch_configures_both_addresses_in_order() {
        let networks = prepare_granted_settings(dual_settings(), &[6, 4])
            .unwrap()
            .networks;
        let expected = networks.clone();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::clone(&observed);
        let result = configure_primary_networks(networks, (), move |network| {
            calls.lock().unwrap().push(network);
            std::future::ready(Ok(resolved()))
        })
        .await
        .unwrap();
        assert_eq!(result.interface, "wwan0");
        assert_eq!(*observed.lock().unwrap(), expected);
    }

    #[tokio::test]
    async fn network_batch_propagates_each_family_failure_without_continuing() {
        for fail_at in [0, 1] {
            let networks = prepare_granted_settings(dual_settings(), &[6, 4])
                .unwrap()
                .networks;
            let calls = Arc::new(Mutex::new(0));
            let count = Arc::clone(&calls);
            let error = configure_primary_networks(networks, (), move |_| {
                let mut count = count.lock().unwrap();
                let fail = *count == fail_at;
                *count += 1;
                std::future::ready(if fail {
                    Err(qmi_netdev::NetdevError::ConfigureFailed(
                        "injected".to_string(),
                    ))
                } else {
                    Ok(resolved())
                })
            })
            .await
            .unwrap_err();
            assert_eq!(
                error,
                qmi_netdev::NetdevError::ConfigureFailed("injected".to_string())
            );
            assert_eq!(*calls.lock().unwrap(), fail_at + 1);
        }
    }

    #[tokio::test]
    async fn cancelling_second_family_keeps_the_lease_guard_until_io_finishes() {
        struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let (dropped, mut observe_drop) = tokio::sync::oneshot::channel();
        let (entered, observe_entry) = tokio::sync::oneshot::channel();
        let (resume, wait_for_resume) = tokio::sync::oneshot::channel();
        let networks = prepare_granted_settings(dual_settings(), &[4, 6])
            .unwrap()
            .networks;
        let mut entered = Some(entered);
        let mut wait_for_resume = Some(wait_for_resume);
        let waiter = tokio::spawn(configure_primary_networks(
            networks,
            DropSignal(Some(dropped)),
            move |network| {
                let blocked = if network.address.is_ipv6() {
                    Some((entered.take().unwrap(), wait_for_resume.take().unwrap()))
                } else {
                    None
                };
                async move {
                    if let Some((entered, wait)) = blocked {
                        let _ = entered.send(());
                        wait.await.unwrap();
                    }
                    Ok(resolved())
                }
            },
        ));
        tokio::time::timeout(Duration::from_secs(2), observe_entry)
            .await
            .unwrap()
            .unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(matches!(
            observe_drop.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        resume.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), observe_drop)
            .await
            .unwrap()
            .unwrap();
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
        let settings = prepare_granted_settings(settings, &[4]).unwrap().settings;
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
        let v4 = prepare_granted_settings(dual, &[4]).unwrap().settings;
        assert!(v4.ipv6_address.is_none());
        assert!(v4.pcscf.iter().all(IpAddr::is_ipv4));
    }
}
