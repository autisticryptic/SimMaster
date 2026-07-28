//! Native-QMI IMS bearer: the seam between a raw WDS session and the rest of
//! the VoLTE stack.
//!
//! # Why this exists, and where the IMS session actually runs (beta2 alignment)
//!
//! beta2 does not run the IMS bearer on the primary QMI control port. The binary
//! logs `Native VoLTE secondary QMI IMS WDS bearer started` (`volte.rs:1976`) and
//! keeps `--wds-get-current-settings` strictly on the *data* path
//! (`secondary_qmi_data.rs`). The IMS path instead reads its IP configuration and
//! P-CSCF from **`AT+CGCONTRDP`** on the active IMS context
//! (`Native VoLTE P-CSCF candidates discovered from active IMS bearer`,
//! `volte.rs:3671`).
//!
//! That matters because it removes the one reason the earlier implementation had
//! to run IMS on the primary port: it believed it needed a WDS client id that
//! survives across `qmicli` invocations
//! (`--wds-start-network → --wds-get-current-settings`) to read the P-CSCF from
//! PCO. It does not. With P-CSCF coming from AT, the IMS session is a *single*
//! `--wds-start-network`, which can — and on this firmware must — run on a
//! dedicated secondary endpoint (DATA6), leaving the primary port to
//! ModemManager. Running a second data session on the primary port is exactly
//! what produced `verbose call end reason (2,201): [internal] error` on both
//! families in the field logs.
//!
//! This module therefore does three things and nothing else:
//!   1. bring up the IMS session on the line's secondary QMI endpoint with a
//!      single `--wds-start-network` (no CID reuse, no bind commands),
//!   2. read the session's IP configuration and P-CSCF from `AT+CGCONTRDP`,
//!   3. resolve which bam-dmux netdev carries it and present the result as a
//!      [`BearerConnection`] so no downstream code has to know which path
//!      produced it.

use std::net::IpAddr;

use crate::hardware::cellular::{
    qmi_netdev::{self, NetdevConfig, ResolvedNetdev},
    secondary_qmi::{self, ImsSession, SecondaryQmiEndpoint, SecondaryQmiError},
};

use super::{
    bearer::{BearerConnection, BearerRequest},
    errors::{code, VolteError},
    pcscf::{self, CgcontrdpSettings},
    plan::{FailureClass, ImsConnectionPlan, IpFamily, IpType},
};

/// Synthetic `path` for a natively established bearer.
///
/// `BearerConnection::path` is a ModemManager object path everywhere else, and
/// two things key off it: teardown (`mmcli -b <path> --disconnect`) and the
/// `bearer_path` shown in the UI. A native session has no such object, so it
/// gets a clearly non-ModemManager marker instead — `is_native_bearer` below is
/// what teardown actually branches on, and the prefix keeps the UI honest rather
/// than displaying a path that does not exist.
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
/// plus everything needed to tear the WDS session down again.
pub struct NativeImsBearer {
    pub connection: BearerConnection,
    /// Secondary QMI endpoint the session runs on. Held so teardown can stop the
    /// session and release the endpoint if this module bound it.
    endpoint: SecondaryQmiEndpoint,
    /// WDS packet data handles to stop on teardown. One per family for a
    /// dual-stack bearer; exactly one for a single-family bearer.
    handles: Vec<String>,
    /// How the interface was determined. Carried so the UI/logs can distinguish
    /// an observed netdev from an assumed one.
    pub netdev: ResolvedNetdev,
    /// Family-specific addresses and policy routes installed for the retained
    /// WDS session(s). They must be removed without flushing the shared netdev.
    configured_netdevs: Vec<(String, NetdevConfig)>,
}

/// Families to attempt, in the plan's order, as QMI `ip-type` values.
///
/// beta2's pre-baked WDS strings try `ip-type=6` before `ip-type=4`, but the
/// order here follows the configured preference so a v4-first line stays v4-first.
/// On the reference SIM the network answers `[3gpp] ipv4-only-allowed`, and the
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

/// The AT PDP context id to read `+CGCONTRDP` on. Qualcomm's WDS `3gpp-profile`
/// and the AT PDP context id share the same index on this firmware, so the
/// profile the session started on is the context whose settings describe it.
fn ims_context_cid(request: &BearerRequest) -> u8 {
    request
        .profile_id
        .and_then(|profile| u8::try_from(profile).ok())
        .filter(|cid| (1..=16).contains(cid))
        .unwrap_or_else(pcscf::configured_ims_cid)
}

/// Establish the IMS bearer natively on the line's secondary QMI endpoint and
/// resolve its netdev.
///
/// `primary_device` is the line's primary QMI control port; it is used only to
/// find the *baseband*, so the secondary endpoint and the netdev are paired to
/// the same modem (multi-line correctness). The IMS session itself never touches
/// the primary port — that stays with ModemManager. `modem_id` is the mmcli
/// selector used to read `+CGCONTRDP` for the P-CSCF and IP configuration.
pub async fn establish_native_ims_bearer(
    primary_device: &str,
    modem_id: &str,
    request: &BearerRequest,
    plan: &ImsConnectionPlan,
) -> Result<NativeImsBearer, VolteError> {
    let baseband = match secondary_qmi::baseband_key_for_device(primary_device) {
        Ok(baseband) => baseband,
        Err(error) => {
            return Err(VolteError::with_detail(
                code::IP_SETTINGS_MISSING,
                format!("native_ims_baseband_unresolved:{error}"),
            ));
        }
    };

    let endpoint = match secondary_qmi::ensure_endpoint(primary_device).await {
        Ok(endpoint) => endpoint,
        Err(error) => return Err(endpoint_error_to_volte(error)),
    };

    let cid = ims_context_cid(request);
    let families = qmi_families_for(plan);
    let mut fallback_families = families.clone();
    if plan.initial_bearer_attempt() == IpType::Ipv4v6 && families.len() >= 2 {
        match establish_dual_stack(&endpoint, &baseband, modem_id, request, cid, &families[..2])
            .await
        {
            Ok(bearer) => return Ok(bearer),
            Err(error) if failure_class(&error).is_unsafe_to_retry() => {
                secondary_qmi::release_endpoint(&endpoint).await;
                return Err(error);
            }
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
        match establish_one(&endpoint, &baseband, modem_id, request, cid, family).await {
            Ok(bearer) => return Ok(bearer),
            Err(error) if failure_class(&error).is_unsafe_to_retry() => {
                secondary_qmi::release_endpoint(&endpoint).await;
                return Err(error);
            }
            Err(error) => {
                tracing::warn!(family, error = %error, "Native VoLTE single-stack WDS attempt failed");
                last_error = Some(error);
            }
        }
    }
    secondary_qmi::release_endpoint(&endpoint).await;
    Err(last_error.unwrap_or_else(|| {
        VolteError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "native_ims_no_family_attempted".to_string(),
        )
    }))
}

async fn establish_dual_stack(
    endpoint: &SecondaryQmiEndpoint,
    baseband: &str,
    modem_id: &str,
    request: &BearerRequest,
    cid: u8,
    families: &[u8],
) -> Result<NativeImsBearer, VolteError> {
    let mut handles = Vec::with_capacity(2);
    for family in families.iter().copied() {
        match start_session(endpoint, request, family).await {
            Ok(session) => handles.push(session.packet_data_handle),
            Err(error) => {
                stop_handles(endpoint, &handles).await;
                return Err(error);
            }
        }
    }

    // Both single-shot sessions are up; the modem now describes the merged
    // context. Read it once from AT — that is beta2's IMS source of truth.
    let settings = match read_ims_settings(modem_id, cid).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_handles(endpoint, &handles).await;
            return Err(error);
        }
    };

    let netdev_family = if settings.ipv6_address.is_some() { 6 } else { 4 };
    let Some(config) = netdev_config_for(&settings, netdev_family) else {
        stop_handles(endpoint, &handles).await;
        return Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            "native_ims_session_has_no_address".to_string(),
        ));
    };
    let resolution = match resolve_netdev(baseband, &config).await {
        Ok(resolution) => resolution,
        Err(error) => {
            stop_handles(endpoint, &handles).await;
            return Err(error);
        }
    };

    let mut connection = match to_bearer_connection(
        &endpoint.device_path,
        &handles.join("+"),
        &resolution.interface,
        0,
        &settings,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            qmi_netdev::teardown(&resolution.interface, &config).await;
            stop_handles(endpoint, &handles).await;
            return Err(error);
        }
    };
    connection.ip_type = "ipv4v6".to_string();
    Ok(NativeImsBearer {
        connection,
        endpoint: endpoint.clone(),
        handles,
        netdev: resolution.clone(),
        configured_netdevs: vec![(resolution.interface, config)],
    })
}

async fn establish_one(
    endpoint: &SecondaryQmiEndpoint,
    baseband: &str,
    modem_id: &str,
    request: &BearerRequest,
    cid: u8,
    family: u8,
) -> Result<NativeImsBearer, VolteError> {
    let session = start_session(endpoint, request, family).await?;
    let handle = session.packet_data_handle;

    let settings = match read_ims_settings(modem_id, cid).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_handles(endpoint, std::slice::from_ref(&handle)).await;
            return Err(error);
        }
    };
    let Some(config) = netdev_config_for(&settings, family) else {
        stop_handles(endpoint, std::slice::from_ref(&handle)).await;
        return Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            "native_ims_session_has_no_address".to_string(),
        ));
    };
    let resolution = match resolve_netdev(baseband, &config).await {
        Ok(resolution) => resolution,
        Err(error) => {
            stop_handles(endpoint, std::slice::from_ref(&handle)).await;
            return Err(error);
        }
    };
    let connection = match to_bearer_connection(
        &endpoint.device_path,
        &handle,
        &resolution.interface,
        family,
        &settings,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            qmi_netdev::teardown(&resolution.interface, &config).await;
            stop_handles(endpoint, std::slice::from_ref(&handle)).await;
            return Err(error);
        }
    };
    Ok(NativeImsBearer {
        connection,
        endpoint: endpoint.clone(),
        handles: vec![handle],
        netdev: resolution.clone(),
        configured_netdevs: vec![(resolution.interface, config)],
    })
}

/// Start one single-shot IMS WDS session on the secondary endpoint.
///
/// beta2 issues a single `--wds-start-network` per family; there is no CID reuse
/// and never a bind command. A `(2,201) [internal] error` here is a normal
/// per-family rejection and stays retryable, while a genuine wedge signature is
/// classified as unsafe so the family loop aborts instead of hammering the modem.
async fn start_session(
    endpoint: &SecondaryQmiEndpoint,
    request: &BearerRequest,
    family: u8,
) -> Result<ImsSession, VolteError> {
    secondary_qmi::start_ims_session(endpoint, &request.apn, family, request.profile_id)
        .await
        .map_err(|detail| {
            let code = if FailureClass::from_details(&detail) == FailureClass::BasebandWedged {
                code::RUNTIME_MM_BEARER_CONNECT_FAILED
            } else {
                code::RUNTIME_IMS_BEARER_START_FAILED
            };
            VolteError::with_detail(code, detail)
        })
}

/// Read the IMS context's IP configuration and P-CSCF from `AT+CGCONTRDP`.
///
/// This is beta2's IMS source of truth (`volte.rs:3671`). A context that reports
/// neither an address nor a P-CSCF is treated as missing so the caller does not
/// build an unusable bearer.
async fn read_ims_settings(modem_id: &str, cid: u8) -> Result<CgcontrdpSettings, VolteError> {
    let settings = pcscf::read_cgcontrdp_settings(modem_id, cid).await?;
    if settings.ipv4_address.is_none() && settings.ipv6_address.is_none() {
        return Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            format!("native_ims_cgcontrdp_no_address:cid={cid}"),
        ));
    }
    Ok(settings)
}

async fn resolve_netdev(
    baseband: &str,
    config: &NetdevConfig,
) -> Result<ResolvedNetdev, VolteError> {
    qmi_netdev::resolve(baseband, config).await.map_err(|error| {
        VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            format!("native_ims_netdev_unresolved:{error}"),
        )
    })
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

fn endpoint_error_to_volte(error: SecondaryQmiError) -> VolteError {
    // No fallback to the ModemManager bearer: activating the IMS PDP context
    // through the primary port is what wedges this baseband.
    VolteError::with_detail(code::RUNTIME_IMS_ENDPOINT_UNAVAILABLE, error.to_string())
}

async fn stop_handles(endpoint: &SecondaryQmiEndpoint, handles: &[String]) {
    for handle in handles {
        secondary_qmi::stop_ims_session(endpoint, handle).await;
    }
}

/// Build the netdev probe configuration for the family the session came up on.
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
    Some(NetdevConfig::from_session(
        address,
        prefix,
        None,
        dns,
        gateway,
    ))
}

/// Tear down a native bearer's WDS session(s) and release its endpoint.
pub async fn release_native_ims_bearer(bearer: NativeImsBearer) {
    for (interface, config) in &bearer.configured_netdevs {
        qmi_netdev::teardown(interface, config).await;
    }
    stop_handles(&bearer.endpoint, &bearer.handles).await;
    secondary_qmi::release_endpoint(&bearer.endpoint).await;
}

/// Project the IMS context's `+CGCONTRDP` settings onto the `BearerConnection`
/// contract the rest of the VoLTE stack consumes.
///
/// Kept separate from the IO above so the mapping is testable without a modem.
pub fn to_bearer_connection(
    device_path: &str,
    handle: &str,
    interface: &str,
    ip_family: u8,
    settings: &CgcontrdpSettings,
) -> Result<BearerConnection, VolteError> {
    let ims = pcscf::ImsIpSettings {
        ipv4_address: settings.ipv4_address,
        ipv4_gateway: settings.ipv4_gateway,
        ipv4_dns: settings.ipv4_dns.clone(),
        ipv6_address: settings.ipv6_address,
        ipv6_gateway: settings.ipv6_gateway,
        ipv6_dns: settings.ipv6_dns.clone(),
        pcscf: settings.pcscf.clone(),
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
        mtu: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::config::VolteIpFamilyPreference;
    use std::net::Ipv4Addr;

    /// The reference IMS context as `+CGCONTRDP` reports it: address+mask,
    /// gateway, DNS and a P-CSCF, all on the same line.
    fn reference_settings() -> CgcontrdpSettings {
        CgcontrdpSettings {
            ipv4_address: Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 207))),
            ipv4_gateway: Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 208))),
            ipv4_dns: vec![
                IpAddr::V4(Ipv4Addr::new(172, 17, 163, 218)),
                IpAddr::V4(Ipv4Addr::new(172, 17, 167, 218)),
            ],
            ipv4_prefix: Some(27),
            pcscf: vec![IpAddr::V4(Ipv4Addr::new(10, 11, 12, 13))],
            ..Default::default()
        }
    }

    #[test]
    fn reference_session_maps_onto_the_bearer_contract() {
        let bearer =
            to_bearer_connection("/dev/wwan0qmi1", "3263198272", "wwan0", 4, &reference_settings())
                .unwrap();
        assert_eq!(bearer.interface, "wwan0");
        assert_eq!(bearer.ip_type, "ipv4");
        assert_eq!(bearer.ipv4_prefix, Some(27));
        assert_eq!(
            bearer.local_addr().unwrap(),
            "10.129.39.207".parse::<IpAddr>().unwrap()
        );
        assert_eq!(bearer.settings.ipv4_dns.len(), 2);
        assert_eq!(bearer.settings.pcscf, vec!["10.11.12.13".parse::<IpAddr>().unwrap()]);
    }

    #[test]
    fn native_path_is_recognisable_and_never_sent_to_modemmanager() {
        let bearer =
            to_bearer_connection("/dev/wwan0qmi1", "3263198272", "wwan0", 4, &reference_settings())
                .unwrap();
        assert!(is_native_bearer(&bearer.path), "{}", bearer.path);
        assert!(!bearer.path.starts_with("/org/freedesktop/"));
        assert!(!super::super::bearer::is_valid_bearer_path(&bearer.path));
        assert!(!is_native_bearer("/org/freedesktop/ModemManager1/Bearer/4"));
    }

    #[test]
    fn a_session_without_any_address_is_rejected() {
        let empty = CgcontrdpSettings::default();
        let error = to_bearer_connection("/dev/wwan0qmi1", "1", "wwan0", 4, &empty).unwrap_err();
        assert_eq!(error.code(), code::IP_SETTINGS_MISSING);
    }

    #[test]
    fn families_follow_the_plan_order() {
        let v4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        assert_eq!(qmi_families_for(&v4), vec![4, 6]);
        let v6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First);
        assert_eq!(qmi_families_for(&v6), vec![6, 4]);
        let only4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4Only);
        assert_eq!(qmi_families_for(&only4), vec![4]);
        let only6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6Only);
        assert_eq!(qmi_families_for(&only6), vec![6]);
        assert_eq!(forced_qmi_family(FailureClass::NetworkForcedIpv4), Some(4));
        assert_eq!(forced_qmi_family(FailureClass::NetworkForcedIpv6), Some(6));
        assert_eq!(forced_qmi_family(FailureClass::PrefixUnavailable), None);
    }

    #[test]
    fn ims_context_cid_prefers_the_started_profile() {
        let mut request = BearerRequest::ims(false);
        request.profile_id = Some(2);
        assert_eq!(ims_context_cid(&request), 2);
        // Out-of-range or missing profiles fall back to the configured default.
        request.profile_id = Some(99);
        assert_eq!(ims_context_cid(&request), pcscf::configured_ims_cid());
        request.profile_id = None;
        assert_eq!(ims_context_cid(&request), pcscf::configured_ims_cid());
    }

    #[test]
    fn a_wedge_signature_from_the_secondary_start_is_classified_unsafe() {
        // A single-shot start that returns a wedge signature must abort the family
        // loop; an ordinary internal call-end reason must not.
        assert_eq!(
            FailureClass::from_details("secondary_qmi_start_failed:endpoint hangup"),
            FailureClass::BasebandWedged
        );
        assert_ne!(
            FailureClass::from_details(
                "secondary_qmi_start_failed:verbose call end reason (2,201): [internal] error"
            ),
            FailureClass::BasebandWedged
        );
    }

    #[test]
    fn netdev_config_picks_the_family_specific_address() {
        let settings = reference_settings();
        let config = netdev_config_for(&settings, 4).unwrap();
        assert_eq!(config.address, "10.129.39.207".parse::<IpAddr>().unwrap());
        assert_eq!(config.prefix, 27);
        // No v6 address in the reference settings, so a v6 request has nothing to
        // configure.
        assert!(netdev_config_for(&settings, 6).is_none());
    }
}
