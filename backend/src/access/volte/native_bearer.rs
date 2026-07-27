//! Native-QMI IMS bearer: the seam between a raw WDS session and the rest of
//! the VoLTE stack.
//!
//! # Why this exists
//!
//! Everything downstream of the bearer in this module — `configure_bearer_network`,
//! `route_pcscf`, the per-family SIP loop — is written against
//! [`BearerConnection`], which was shaped by ModemManager's `mmcli -b` output. A
//! WDS session established directly through `qmicli` yields the same
//! information (addresses, gateway, DNS, MTU, P-CSCF) but none of the packaging:
//! no D-Bus object path and, critically, **no interface name**.
//!
//! That missing interface is the whole difficulty. On the reference platform the
//! IMS data path lands on one of eight `bam-dmux` netdevs (`wwan0`..`wwan7`) and
//! the firmware does not report which; `--wds-bind-mux-data-port` — the QMI
//! command that would pin it — is unsupported by the 2015 firmware and issuing
//! one restarts the baseband. So the netdev has to be *observed* after the
//! session is up (see [`crate::cellular::qmi_netdev`]).
//!
//! This module therefore does three things and nothing else:
//!   1. start the IMS session on the endpoint that can actually hold a CID
//!      through a multi-step flow (the primary port, via `qmi-proxy`),
//!   2. resolve which netdev carries it,
//!   3. present the result as a `BearerConnection` so no downstream code has to
//!      know which of the two paths produced it.
//!
//! # Relationship to the ModemManager path
//!
//! This is an alternative to `bearer::ensure_ims_bearer`, not a replacement.
//! ModemManager's path is better when it works: it packages the netdev for us
//! and handles vendor differences. It is unusable on this firmware only because
//! activating the IMS PDP context through it wedges the baseband. Both paths
//! produce the same `BearerConnection`, so the caller picks one and the rest of
//! the flow is identical.

use std::net::IpAddr;

use crate::cellular::{
    qmi_netdev::{self, NetdevConfig, ResolvedNetdev},
    qmi_wds::{self, CurrentSettings, ImsSession, WdsEndpoint, WdsError},
};

use super::{
    bearer::{BearerConnection, BearerRequest},
    errors::{code, VolteError},
    pcscf::ImsIpSettings,
    plan::{FailureClass, ImsConnectionPlan, IpFamily, IpType},
};

/// Synthetic `path` for a natively established bearer.
///
/// `BearerConnection::path` is a ModemManager object path everywhere else, and
/// two things key off it: teardown (`mmcli -b <path> --disconnect`) and the
/// `bearer_path` shown in the UI. A native session has no such object, so it
/// gets a clearly non-ModemManager marker instead — `is_native` below is what
/// teardown actually branches on, and the prefix keeps the UI honest rather than
/// displaying a path that does not exist.
pub const NATIVE_BEARER_PATH_PREFIX: &str = "qmi-wds:";

/// Is this bearer one we established directly over QMI?
///
/// Teardown must not send a native bearer to `mmcli`: there is no bearer object,
/// so the call fails and, worse, the real WDS session would be left running.
pub fn is_native_bearer(path: &str) -> bool {
    path.starts_with(NATIVE_BEARER_PATH_PREFIX)
}

/// Build the synthetic path for a session on `device_path` with `handle`.
pub fn native_bearer_path(device_path: &str, handle: &str) -> String {
    format!("{NATIVE_BEARER_PATH_PREFIX}{device_path}#{handle}")
}

/// A live native IMS bearer: the `BearerConnection` the rest of the stack uses,
/// plus the session handle needed to tear it down.
pub struct NativeImsBearer {
    pub connection: BearerConnection,
    /// beta2 represents a native dual-stack bearer as one WDS session per
    /// family. Single-stack fallback contains exactly one entry.
    pub sessions: Vec<ImsSession>,
    /// How the interface was determined. Carried so the UI/logs can distinguish
    /// an observed netdev from an assumed one.
    pub netdev: ResolvedNetdev,
    /// Family-specific addresses and policy routes installed for the retained
    /// WDS sessions. They must be removed without flushing the shared netdev.
    configured_netdevs: Vec<(String, NetdevConfig)>,
}

/// Families to attempt, in the plan's order, as QMI `ip-type` values.
///
/// The plan speaks in `IpType` (including dual-stack); QMI's `--wds-start-network`
/// takes a single family here. Dual-stack is deliberately not attempted: on the
/// reference SIM the network answers `[3gpp] ipv4-only-allowed`, and the
/// single-family attempts are what actually succeed.
pub fn qmi_families_for(plan: &ImsConnectionPlan) -> Vec<u8> {
    let mut families = Vec::with_capacity(2);
    for family in plan.pcscf_order() {
        let value = match family {
            IpFamily::Ipv4 => 4,
            IpFamily::Ipv6 => 6,
        };
        if !families.contains(&value) {
            families.push(value);
        }
    }
    families
}

/// Establish the IMS bearer natively on `primary_device` and resolve its netdev.
///
/// `primary_device` is the line's primary QMI control port — the one ModemManager
/// uses for normal data. That is intentional and is the one endpoint where a CID
/// survives across `qmicli` invocations, which the start → settings → P-CSCF
/// sequence requires. Sharing is safe because access goes through `qmi-proxy`,
/// which is how ModemManager itself reaches the port.
pub async fn establish_native_ims_bearer(
    primary_device: &str,
    request: &BearerRequest,
    plan: &ImsConnectionPlan,
) -> Result<NativeImsBearer, VolteError> {
    let endpoint = WdsEndpoint::primary_via_proxy(primary_device);
    if !qmi_wds::proxy_is_ready(primary_device).await {
        return Err(VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            format!("qmi_proxy_unavailable:{primary_device}"),
        ));
    }

    // The netdev must belong to the same baseband as the control port, or a
    // multi-modem host would configure another line's interface.
    let baseband = match crate::cellular::secondary_qmi::baseband_key_for_device(primary_device) {
        Ok(baseband) => baseband,
        Err(error) => {
            return Err(VolteError::with_detail(
                code::IP_SETTINGS_MISSING,
                format!("native_ims_baseband_unresolved:{error}"),
            ));
        }
    };

    let families = qmi_families_for(plan);
    let mut fallback_families = families.clone();
    if plan.initial_bearer_attempt() == IpType::Ipv4v6 && families.len() >= 2 {
        match establish_dual_stack(
            &endpoint,
            &baseband,
            primary_device,
            request,
            &families[..2],
        )
        .await
        {
            Ok(bearer) => return Ok(bearer),
            Err(error) if failure_class(&error).is_unsafe_to_retry() => return Err(error),
            Err(error) => {
                if let Some(forced) = forced_qmi_family(failure_class(&error)) {
                    fallback_families = vec![forced];
                }
                tracing::warn!(
                    error = %error,
                    "Native VoLTE dual-stack WDS activation failed; falling back to single-stack attempts"
                );
            }
        }
    }

    let mut last_error = None;
    for family in fallback_families {
        match establish_one(&endpoint, &baseband, primary_device, request, family).await {
            Ok(bearer) => return Ok(bearer),
            Err(error) if failure_class(&error).is_unsafe_to_retry() => return Err(error),
            Err(error) => {
                tracing::warn!(family, error = %error, "Native VoLTE single-stack WDS attempt failed");
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "native_ims_no_family_attempted".to_string(),
        )
    }))
}

async fn establish_dual_stack(
    endpoint: &WdsEndpoint,
    baseband: &str,
    primary_device: &str,
    request: &BearerRequest,
    families: &[u8],
) -> Result<NativeImsBearer, VolteError> {
    let mut sessions = Vec::with_capacity(2);
    for family in families.iter().copied() {
        match start_session(endpoint, request, family).await {
            Ok(session) => sessions.push(session),
            Err(error) => {
                release_sessions(sessions).await;
                return Err(error);
            }
        }
    }
    let mut resolutions = Vec::with_capacity(2);
    for session in &sessions {
        match resolve_session_netdev(baseband, session).await {
            Ok(resolution) => resolutions.push(resolution),
            Err(error) => {
                teardown_resolutions(&sessions, &resolutions).await;
                release_sessions(sessions).await;
                return Err(error);
            }
        }
    }
    let Some(first_resolution) = resolutions.first().cloned() else {
        release_sessions(sessions).await;
        return Err(VolteError::new(code::IP_SETTINGS_MISSING));
    };
    if resolutions
        .iter()
        .any(|resolution| resolution.interface != first_resolution.interface)
    {
        teardown_resolutions(&sessions, &resolutions).await;
        release_sessions(sessions).await;
        return Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            "native_ims_dual_stack_netdev_mismatch".to_string(),
        ));
    }
    let settings = merge_session_settings(&sessions);
    let handles = sessions
        .iter()
        .map(|session| session.packet_data_handle.as_str())
        .collect::<Vec<_>>()
        .join("+");
    let mut connection = match to_bearer_connection(
        primary_device,
        &handles,
        &first_resolution.interface,
        0,
        &settings,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            teardown_resolutions(&sessions, &resolutions).await;
            release_sessions(sessions).await;
            return Err(error);
        }
    };
    connection.ip_type = "ipv4v6".to_string();
    let configured_netdevs = configured_netdevs(&sessions, &resolutions);
    Ok(NativeImsBearer {
        connection,
        sessions,
        netdev: first_resolution,
        configured_netdevs,
    })
}

async fn establish_one(
    endpoint: &WdsEndpoint,
    baseband: &str,
    primary_device: &str,
    request: &BearerRequest,
    family: u8,
) -> Result<NativeImsBearer, VolteError> {
    let (session, resolution) = establish_session(endpoint, baseband, request, family).await?;
    let connection = match to_bearer_connection(
        primary_device,
        &session.packet_data_handle,
        &resolution.interface,
        session.ip_family,
        &session.settings,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            if let Some(config) = netdev_config_for(&session.settings, session.ip_family) {
                qmi_netdev::teardown(&resolution.interface, &config).await;
            }
            qmi_wds::stop_ims_session(session).await;
            return Err(error);
        }
    };
    let configured_netdevs = netdev_config_for(&session.settings, session.ip_family)
        .map(|config| vec![(resolution.interface.clone(), config)])
        .unwrap_or_default();
    Ok(NativeImsBearer {
        connection,
        sessions: vec![session],
        netdev: resolution,
        configured_netdevs,
    })
}

async fn establish_session(
    endpoint: &WdsEndpoint,
    baseband: &str,
    request: &BearerRequest,
    family: u8,
) -> Result<(ImsSession, ResolvedNetdev), VolteError> {
    let session = start_session(endpoint, request, family).await?;
    match resolve_session_netdev(baseband, &session).await {
        Ok(resolution) => Ok((session, resolution)),
        Err(error) => {
            qmi_wds::stop_ims_session(session).await;
            Err(error)
        }
    }
}

async fn start_session(
    endpoint: &WdsEndpoint,
    request: &BearerRequest,
    family: u8,
) -> Result<ImsSession, VolteError> {
    qmi_wds::start_ims_session(endpoint, &request.apn, &[family], request.profile_id)
        .await
        .map_err(wds_error_to_volte)
}

async fn resolve_session_netdev(
    baseband: &str,
    session: &ImsSession,
) -> Result<ResolvedNetdev, VolteError> {
    let resolution = match netdev_config_for(&session.settings, session.ip_family) {
        Some(config) => qmi_netdev::resolve(baseband, &config)
            .await
            .map_err(|error| {
                VolteError::with_detail(
                    code::IP_SETTINGS_MISSING,
                    format!("native_ims_netdev_unresolved:{error}"),
                )
            }),
        None => Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            "native_ims_session_has_no_address".to_string(),
        )),
    };
    resolution
}

fn merge_session_settings(sessions: &[ImsSession]) -> CurrentSettings {
    merge_current_settings(sessions.iter().map(|session| &session.settings))
}

fn merge_current_settings<'a>(
    settings_set: impl IntoIterator<Item = &'a CurrentSettings>,
) -> CurrentSettings {
    let mut merged = CurrentSettings::default();
    for settings in settings_set {
        if settings.ipv4_address.is_some() {
            merged.ipv4_address.clone_from(&settings.ipv4_address);
            merged.ipv4_gateway.clone_from(&settings.ipv4_gateway);
            merged.ipv4_prefix = settings.ipv4_prefix;
        }
        if settings.ipv6_address.is_some() {
            merged.ipv6_address.clone_from(&settings.ipv6_address);
            merged.ipv6_gateway.clone_from(&settings.ipv6_gateway);
            merged.ipv6_prefix = settings.ipv6_prefix;
        }
        merge_strings(&mut merged.ipv4_dns, &settings.ipv4_dns);
        merge_strings(&mut merged.ipv6_dns, &settings.ipv6_dns);
        merge_strings(&mut merged.pcscf, &settings.pcscf);
        merged.mtu = match (merged.mtu, settings.mtu) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (None, value) | (value, None) => value,
        };
    }
    merged.ip_family = Some("ipv4v6".to_string());
    merged
}

fn merge_strings(target: &mut Vec<String>, source: &[String]) {
    for item in source {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}

fn failure_class(error: &VolteError) -> FailureClass {
    FailureClass::from_details(error.detail().unwrap_or(""))
}

fn forced_qmi_family(class: FailureClass) -> Option<u8> {
    match class.forced_family()? {
        IpFamily::Ipv4 => Some(4),
        IpFamily::Ipv6 => Some(6),
    }
}

async fn release_sessions(sessions: Vec<ImsSession>) {
    for session in sessions {
        qmi_wds::stop_ims_session(session).await;
    }
}

fn configured_netdevs(
    sessions: &[ImsSession],
    resolutions: &[ResolvedNetdev],
) -> Vec<(String, NetdevConfig)> {
    sessions
        .iter()
        .zip(resolutions)
        .filter_map(|(session, resolution)| {
            netdev_config_for(&session.settings, session.ip_family)
                .map(|config| (resolution.interface.clone(), config))
        })
        .collect()
}

async fn teardown_resolutions(sessions: &[ImsSession], resolutions: &[ResolvedNetdev]) {
    for (interface, config) in configured_netdevs(sessions, resolutions) {
        qmi_netdev::teardown(&interface, &config).await;
    }
}

/// Build the netdev probe configuration for the family the session came up on.
///
/// Returns `None` when the session reported no address in that family, which
/// means there is nothing to configure an interface with.
fn netdev_config_for(settings: &CurrentSettings, family: u8) -> Option<NetdevConfig> {
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
    let dns: Vec<IpAddr> = dns.iter().filter_map(|item| item.parse().ok()).collect();
    let gateway = gateway.and_then(|item| item.parse().ok());
    Some(NetdevConfig::from_session(
        address,
        prefix,
        settings.mtu,
        &dns,
        gateway,
    ))
}

/// Tear down a native bearer's WDS session.
pub async fn release_native_ims_bearer(bearer: NativeImsBearer) {
    for (interface, config) in bearer.configured_netdevs {
        qmi_netdev::teardown(&interface, &config).await;
    }
    release_sessions(bearer.sessions).await;
}

/// Project a WDS session's settings onto the `BearerConnection` contract.
///
/// Kept separate from the IO above so the mapping — including the details that
/// bit on real hardware, like the subnet mask becoming a prefix length — is
/// testable without a modem.
pub fn to_bearer_connection(
    device_path: &str,
    handle: &str,
    interface: &str,
    ip_family: u8,
    settings: &CurrentSettings,
) -> Result<BearerConnection, VolteError> {
    let ims = ImsIpSettings {
        ipv4_address: parse_addr(settings.ipv4_address.as_deref()),
        ipv4_gateway: parse_addr(settings.ipv4_gateway.as_deref()),
        ipv4_dns: parse_addrs(&settings.ipv4_dns),
        ipv6_address: parse_addr(settings.ipv6_address.as_deref()),
        ipv6_gateway: parse_addr(settings.ipv6_gateway.as_deref()),
        ipv6_dns: parse_addrs(&settings.ipv6_dns),
        pcscf: parse_addrs(&settings.pcscf),
    };
    if ims.local_addr().is_none() {
        return Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            "native_ims_session_has_no_address".to_string(),
        ));
    }
    Ok(BearerConnection {
        path: native_bearer_path(device_path, handle),
        interface: interface.to_string(),
        ip_type: match ip_family {
            6 => "ipv6".to_string(),
            _ => "ipv4".to_string(),
        },
        settings: ims,
        ipv4_prefix: settings.ipv4_prefix,
        ipv6_prefix: settings.ipv6_prefix,
        mtu: settings.mtu,
    })
}

/// Map a WDS-layer error onto the VoLTE error vocabulary, preserving the
/// distinction the retry logic depends on.
///
/// A wedge must arrive at the caller as something [`FailureClass::from_details`]
/// still recognises as `BasebandWedged`, otherwise the retry batch would keep
/// hammering a modem that is already in trouble — the exact escalation path that
/// took the reference device down.
pub fn wds_error_to_volte(error: WdsError) -> VolteError {
    let detail = error.to_string();
    match error {
        WdsError::BasebandWedged(_) => {
            debug_assert_eq!(
                FailureClass::from_details(&detail),
                FailureClass::BasebandWedged,
                "wedge detail must stay classifiable as a wedge"
            );
            VolteError::with_detail(code::RUNTIME_MM_BEARER_CONNECT_FAILED, detail)
        }
        WdsError::StartFailed { .. } => {
            VolteError::with_detail(code::RUNTIME_MM_BEARER_CONNECT_FAILED, detail)
        }
        WdsError::SettingsUnavailable(_) => {
            VolteError::with_detail(code::IP_SETTINGS_MISSING, detail)
        }
        _ => VolteError::with_detail(code::RUNTIME_MM_BEARER_CONNECT_FAILED, detail),
    }
}

fn parse_addr(value: Option<&str>) -> Option<IpAddr> {
    value?.split('/').next()?.trim().parse().ok()
}

fn parse_addrs(values: &[String]) -> Vec<IpAddr> {
    values
        .iter()
        .filter_map(|value| parse_addr(Some(value)))
        .fold(Vec::new(), |mut unique, addr| {
            if !unique.contains(&addr) {
                unique.push(addr);
            }
            unique
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::config::VolteIpFamilyPreference;

    /// The exact settings the reference device returned on the successful IPv4
    /// session, so the mapping is pinned to observed output rather than a guess.
    fn reference_settings() -> CurrentSettings {
        CurrentSettings {
            ip_family: Some("ipv4".to_string()),
            ipv4_address: Some("10.129.39.207".to_string()),
            ipv4_gateway: Some("10.129.39.208".to_string()),
            ipv4_dns: vec!["172.17.163.218".to_string(), "172.17.167.218".to_string()],
            ipv4_prefix: Some(27),
            mtu: Some(1500),
            ..Default::default()
        }
    }

    #[test]
    fn reference_session_maps_onto_the_bearer_contract() {
        let bearer = to_bearer_connection(
            "/dev/wwan0qmi0",
            "3263198272",
            "wwan0",
            4,
            &reference_settings(),
        )
        .unwrap();
        assert_eq!(bearer.interface, "wwan0");
        assert_eq!(bearer.ip_type, "ipv4");
        // /27 comes from the observed 255.255.255.224 mask; getting this wrong
        // installs a wrong on-link prefix.
        assert_eq!(bearer.ipv4_prefix, Some(27));
        assert_eq!(bearer.mtu, Some(1500));
        assert_eq!(
            bearer.local_addr().unwrap(),
            "10.129.39.207".parse::<IpAddr>().unwrap()
        );
        assert_eq!(bearer.settings.ipv4_dns.len(), 2);
    }

    #[test]
    fn native_path_is_recognisable_and_never_sent_to_modemmanager() {
        let bearer = to_bearer_connection(
            "/dev/wwan0qmi0",
            "3263198272",
            "wwan0",
            4,
            &reference_settings(),
        )
        .unwrap();
        assert!(is_native_bearer(&bearer.path), "{}", bearer.path);
        // Must not be mistaken for a real bearer object path.
        assert!(!bearer.path.starts_with("/org/freedesktop/"));
        assert!(!super::super::bearer::is_valid_bearer_path(&bearer.path));
        // A genuine ModemManager path must not be treated as native, or teardown
        // would skip the mmcli disconnect and leak a bearer.
        assert!(!is_native_bearer("/org/freedesktop/ModemManager1/Bearer/4"));
    }

    #[test]
    fn a_session_without_any_address_is_rejected() {
        let empty = CurrentSettings::default();
        let error = to_bearer_connection("/dev/wwan0qmi0", "1", "wwan0", 4, &empty).unwrap_err();
        assert_eq!(error.code(), code::IP_SETTINGS_MISSING);
    }

    #[test]
    fn ipv6_session_keeps_its_prefix_and_reports_the_right_type() {
        let settings = CurrentSettings {
            ipv6_address: Some("2001:db8::20".to_string()),
            ipv6_gateway: Some("2001:db8::1".to_string()),
            ipv6_prefix: Some(64),
            ..Default::default()
        };
        let bearer = to_bearer_connection("/dev/wwan0qmi0", "9", "wwan3", 6, &settings).unwrap();
        assert_eq!(bearer.ip_type, "ipv6");
        assert_eq!(bearer.ipv6_prefix, Some(64));
        assert!(bearer.local_addr().unwrap().is_ipv6());
    }

    #[test]
    fn pcscf_from_pco_reaches_the_bearer_settings() {
        // This is the payoff of holding one CID across the flow: without it the
        // settings read fails and there is no P-CSCF at all.
        let settings = CurrentSettings {
            ipv4_address: Some("10.129.39.207".to_string()),
            pcscf: vec!["10.11.12.13".to_string(), "10.11.12.13".to_string()],
            ..Default::default()
        };
        let bearer = to_bearer_connection("/dev/wwan0qmi0", "1", "wwan0", 4, &settings).unwrap();
        // Deduplicated.
        assert_eq!(
            bearer.settings.pcscf,
            vec!["10.11.12.13".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn families_follow_the_plan_order() {
        let v4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        assert_eq!(qmi_families_for(&v4), vec![4, 6]);
        let v6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First);
        assert_eq!(qmi_families_for(&v6), vec![6, 4]);
        // Single-family preferences must never attempt the other one.
        let only4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4Only);
        assert_eq!(qmi_families_for(&only4), vec![4]);
        let only6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6Only);
        assert_eq!(qmi_families_for(&only6), vec![6]);
        assert_eq!(forced_qmi_family(FailureClass::NetworkForcedIpv4), Some(4));
        assert_eq!(forced_qmi_family(FailureClass::NetworkForcedIpv6), Some(6));
        assert_eq!(forced_qmi_family(FailureClass::PrefixUnavailable), None);
    }

    #[test]
    fn dual_stack_settings_merge_both_families_and_deduplicate_pcscf() {
        let ipv4 = CurrentSettings {
            ipv4_address: Some("10.0.0.2".to_string()),
            ipv4_gateway: Some("10.0.0.1".to_string()),
            ipv4_prefix: Some(30),
            ipv4_dns: vec!["10.0.0.53".to_string()],
            pcscf: vec!["10.0.0.10".to_string()],
            mtu: Some(1500),
            ..Default::default()
        };
        let ipv6 = CurrentSettings {
            ipv6_address: Some("2001:db8::2".to_string()),
            ipv6_gateway: Some("2001:db8::1".to_string()),
            ipv6_prefix: Some(64),
            ipv6_dns: vec!["2001:db8::53".to_string()],
            pcscf: vec!["10.0.0.10".to_string(), "2001:db8::10".to_string()],
            mtu: Some(1428),
            ..Default::default()
        };
        let merged = merge_current_settings([&ipv4, &ipv6]);
        assert_eq!(merged.ip_family.as_deref(), Some("ipv4v6"));
        assert_eq!(merged.ipv4_address.as_deref(), Some("10.0.0.2"));
        assert_eq!(merged.ipv6_address.as_deref(), Some("2001:db8::2"));
        assert_eq!(merged.mtu, Some(1428));
        assert_eq!(
            merged.pcscf,
            vec!["10.0.0.10".to_string(), "2001:db8::10".to_string()]
        );
    }

    #[test]
    fn wedge_errors_stay_classifiable_as_wedges_through_the_conversion() {
        // The retry batch aborts on BasebandWedged. If the detail were reworded
        // on the way through, the abort would silently stop working.
        let wedged =
            WdsError::BasebandWedged("error: operation failed: endpoint hangup".to_string());
        let converted = wds_error_to_volte(wedged);
        let class = FailureClass::from_details(converted.detail().unwrap_or(""));
        assert_eq!(class, FailureClass::BasebandWedged);
        assert!(class.is_unsafe_to_retry());
    }

    #[test]
    fn family_rejection_survives_as_a_readable_reason() {
        // The network telling us v4-only is normal and must remain retryable.
        let refused = WdsError::StartFailed {
            reason: "verbose call end reason (6,50): [3gpp] ipv4-only-allowed".to_string(),
        };
        let converted = wds_error_to_volte(refused);
        let detail = converted.detail().unwrap_or_default();
        assert!(detail.contains("ipv4-only-allowed"), "{detail}");
        let class = FailureClass::from_details(detail);
        assert_eq!(class, FailureClass::NetworkForcedIpv4);
        assert!(!class.is_unsafe_to_retry());
    }

    #[test]
    fn addresses_with_inline_prefixes_are_parsed_bare() {
        assert_eq!(
            parse_addr(Some("2001:db8::20/64")),
            Some("2001:db8::20".parse::<IpAddr>().unwrap())
        );
        assert_eq!(parse_addr(Some("bogus")), None);
        assert_eq!(parse_addr(None), None);
    }
}
