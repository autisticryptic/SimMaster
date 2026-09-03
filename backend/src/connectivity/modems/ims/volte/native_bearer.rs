//! Native IMS bearer *strategy*: which family to attempt, in what order, and how
//! to turn a device-agnostic result into the stack's `BearerConnection`.
//!
//! # Why this exists, and where the IMS session actually runs
//!
//! SimAdmin never runs IMS through the primary ModemManager host bearer. The
//! device driver starts a native IMS bearer and its netdev is
//! moved into the line's UE namespace before SIP or media sockets are created.
//! The provider retains any device-native session state needed for settings
//! and teardown.
//! The IMS path also reads its authoritative IP configuration and P-CSCF from
//! **`AT+CGCONTRDP`** on the active IMS context
//! (`Native VoLTE P-CSCF candidates discovered from active IMS bearer`,
//! `volte.rs:3671`).
//!
//! The native session mechanism is the *device driver's* job, hidden behind
//! the [`ImsBearerTransport`] trait. This module only orchestrates: it walks the
//! plan's attempts (single-family / dual-stack, in the configured preference
//! order), re-drives the family the network forces, classifies failures, and
//! projects the device-agnostic [`ImsBearerInfo`] onto the [`BearerConnection`]
//! contract so no downstream code has to know which path produced it.
//!
//! [`ImsBearerTransport`]: crate::hardware::devices::transport::ImsBearerTransport
//! [`ImsBearerInfo`]: crate::hardware::devices::transport::ImsBearerInfo

use std::net::IpAddr;

use crate::hardware::devices::transport::{
    BearerInterfaceOwnership, ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint,
    ImsBearerHandle, ImsBearerInfo, ImsBearerTransport,
};
use crate::{
    platform::netns,
    services::ue_worker::{UeWorkerBinding, UeWorkerHandle},
};

use super::{
    bearer::{teardown_bearer_network_in_worker, BearerConnection, BearerRequest},
    errors::{code, VolteError},
    pcscf::{self, ImsIpSettings},
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
pub const NATIVE_BEARER_PATH_PREFIX: &str = "native-bearer:";

/// Is this bearer one we established through a device-native provider?
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
/// plus the opaque handle that tears the device session down again. The strategy
/// layer owns the handle for the life of the call/session; teardown goes through
/// [`release_native_ims_bearer`].
pub struct NativeImsBearer {
    pub connection: BearerConnection,
    /// Opaque endpoint identifier reported by the device provider.
    pub provider_endpoint: String,
    /// Opaque retained-session identifier reported by the device provider.
    pub provider_session: String,
    /// Interface that carries the session (for the attempt log).
    pub interface: String,
    /// How the interface was decided (`sole_candidate` / `probe_answered` /
    /// `assumed`), carried so the UI/logs can distinguish an observed netdev
    /// from an assumed one.
    pub netdev_method: &'static str,
    /// Ownership declared by the bearer provider. Only an application-owned
    /// interface (or an interface already created in the worker) may cross the
    /// namespace boundary.
    pub interface_ownership: BearerInterfaceOwnership,
    /// Device-owned teardown handle. This module never inspects it.
    handle: Box<dyn ImsBearerHandle + Send>,
    worker: Option<UeWorkerHandle>,
    /// Generation that received the interface, routes and sockets. The
    /// cloneable worker handle survives a respawn, so cleanup must not use it
    /// after this binding becomes stale.
    worker_binding: Option<UeWorkerBinding>,
    moved_to_worker: bool,
}

impl NativeImsBearer {
    /// Move the dedicated native netdev into this line's UE namespace. The
    /// primary ModemManager interface is intentionally rejected; only a
    /// provider-declared application-owned bearer may cross the
    /// namespace boundary.
    pub async fn move_into_worker(&mut self, worker: UeWorkerHandle) -> Result<(), VolteError> {
        let worker_binding = worker.bind();
        if !worker_binding.is_current() {
            return Err(VolteError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED));
        }
        if self.moved_to_worker {
            if self.worker_binding_is_current() {
                return Ok(());
            }
            return Err(VolteError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED));
        }
        match self.interface_ownership {
            BearerInterfaceOwnership::HostManagedPrimary => {
                return Err(VolteError::with_detail(
                    code::COMMAND_FAILED,
                    format!(
                        "native bearer refuses to move host-managed interface {}",
                        self.interface
                    ),
                ));
            }
            BearerInterfaceOwnership::Unknown => {
                return Err(VolteError::with_detail(
                    code::COMMAND_FAILED,
                    format!(
                        "native bearer ownership is unknown; refusing to move {}",
                        self.interface
                    ),
                ));
            }
            BearerInterfaceOwnership::WorkerNative => {
                // The provider already established this interface in the UE
                // worker. Record the worker for route/socket teardown, but do
                // not attempt a second namespace move.
                self.worker = Some(worker);
                self.worker_binding = Some(worker_binding);
                return Ok(());
            }
            BearerInterfaceOwnership::ApplicationOwnedNative => {}
        }
        netns::move_iface_in(worker_binding.namespace(), &self.interface)
            .await
            .map_err(|error| {
                VolteError::with_detail(
                    code::COMMAND_FAILED,
                    format!(
                        "move native bearer {} into {}: {error}",
                        self.interface,
                        worker_binding.namespace()
                    ),
                )
            })?;
        let status = worker_binding.worker().refresh_net_status().await;
        if status.as_ref().ok().is_none_or(|snapshot| {
            !snapshot
                .interfaces
                .iter()
                .any(|name| name == &self.interface)
        }) {
            let _ = netns::move_iface_out(worker_binding.namespace(), &self.interface).await;
            return Err(VolteError::with_detail(
                code::COMMAND_FAILED,
                format!(
                    "worker cannot observe moved native interface {}",
                    self.interface
                ),
            ));
        }
        if !worker_binding.is_current() {
            // The worker respawned while the interface was being moved. Do not
            // use the long-lived handle to clean the namespace: that would
            // target the replacement generation. Its teardown owns the stale
            // namespace state, while the provider handle is still released by
            // the caller.
            self.worker = Some(worker);
            self.worker_binding = Some(worker_binding);
            self.moved_to_worker = true;
            return Err(VolteError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED));
        }
        self.worker = Some(worker);
        self.worker_binding = Some(worker_binding);
        self.moved_to_worker = true;
        Ok(())
    }

    pub fn worker(&self) -> Option<&UeWorkerHandle> {
        self.worker.as_ref()
    }

    pub fn worker_binding(&self) -> Option<&UeWorkerBinding> {
        self.worker_binding.as_ref()
    }

    pub async fn restore_from_worker(&mut self) {
        if !self.moved_to_worker {
            return;
        }
        if let Some(binding) = self.worker_binding.as_ref() {
            if binding.is_current() {
                let _ = netns::move_iface_out(binding.namespace(), &self.interface).await;
                let _ = binding.worker().refresh_net_status().await;
            } else {
                tracing::warn!(
                    interface = %self.interface,
                    "Skipping native VoLTE interface restore bound to a stale UE worker generation"
                );
            }
        }
        self.moved_to_worker = false;
        self.worker = None;
        self.worker_binding = None;
    }

    pub fn worker_binding_is_current(&self) -> bool {
        self.worker_binding
            .as_ref()
            .is_none_or(UeWorkerBinding::is_current)
    }
}

/// Families to request from the provider, in the plan's configured order.
///
/// beta2's pre-baked WDS strings try `ip-type=6` before `ip-type=4`, but the
/// order here follows the configured preference so a v4-first line stays v4-first.
/// On the reference SIM the network answers `[3gpp] ipv4-only-allowed`, and the
/// single-family attempts are what actually succeed.
pub fn requested_families_for(plan: &ImsConnectionPlan) -> Vec<u8> {
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

/// Establish the IMS bearer through the line's selected device transport and
/// resolve its network interface.
///
/// `primary_device` identifies the line's modem to the transport, allowing a
/// multi-line provider to select resources belonging to the same baseband.
/// `modem_id` selects the line when reading `+CGCONTRDP` settings.
pub async fn establish_native_ims_bearer(
    transport: &dyn ImsBearerTransport,
    primary_device: &str,
    modem_id: &str,
    request: &BearerRequest,
    plan: &ImsConnectionPlan,
) -> Result<NativeImsBearer, VolteError> {
    let cid = ims_context_cid(request);
    let families = requested_families_for(plan);
    // Walk the plan's attempts in order. Dual-stack is an ordinary entry, so a
    // per-line list may place it after a single family or omit it entirely — the
    // configured order is what runs, not "dual-stack first" hardcoded here.
    let mut last_error = None;
    let mut forced_single: Option<u8> = None;
    for attempt in plan.bearer_attempts() {
        let attempt_families: &[u8] = match attempt {
            IpType::Ipv4v6 => {
                if families.len() < 2 {
                    // Dual-stack needs both families admitted by this plan.
                    continue;
                }
                &families[..2]
            }
            IpType::Ipv4 => &[4],
            IpType::Ipv6 => &[6],
        };
        let result = transport
            .establish_ims_bearer(
                primary_device,
                modem_id,
                &request.apn,
                request.profile_id,
                cid,
                attempt_families,
            )
            .await;
        match result {
            Ok((info, handle)) => return adopt_bearer(info, handle).await,
            Err(error) => {
                let hint = error.hint;
                let error = volte_error_from_ims_bearer(error);
                if hint == ImsBearerFailureHint::BasebandWedged {
                    return Err(error);
                }
                tracing::warn!(
                    attempt = attempt.as_mm_str(),
                    error = %error,
                    "Native VoLTE WDS activation failed; trying the next planned attempt"
                );
                // The network told us only one family is allowed. Nothing later in
                // the plan can succeed, so stop and try exactly that family.
                let forced = forced_native_family(hint);
                last_error = Some(error);
                if let Some(forced) = forced {
                    forced_single = Some(forced);
                    break;
                }
            }
        }
    }

    if let Some(forced) = forced_single {
        match transport
            .establish_ims_bearer(
                primary_device,
                modem_id,
                &request.apn,
                request.profile_id,
                cid,
                &[forced],
            )
            .await
        {
            Ok((info, handle)) => return adopt_bearer(info, handle).await,
            Err(error) => {
                let error = volte_error_from_ims_bearer(error);
                tracing::warn!(family = forced, error = %error, "Native VoLTE network-forced family WDS attempt failed");
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

/// Tear down a native bearer's WDS session(s) and release its endpoint.
pub async fn release_native_ims_bearer(mut bearer: NativeImsBearer) {
    if bearer.worker_binding_is_current() {
        if let Some(worker) = bearer.worker.as_ref() {
            teardown_bearer_network_in_worker(&bearer.connection, worker).await;
        }
    } else {
        tracing::warn!(
            interface = %bearer.interface,
            "Skipping native VoLTE bearer network cleanup bound to a stale UE worker generation"
        );
    }
    bearer.restore_from_worker().await;
    bearer.handle.release().await;
}

/// Project a successful transport result onto the `NativeImsBearer` the rest of
/// the stack consumes. If the projection rejects the bearer (e.g. no address),
/// the device handle is still released so nothing leaks.
async fn adopt_bearer(
    info: ImsBearerInfo,
    handle: Box<dyn ImsBearerHandle + Send>,
) -> Result<NativeImsBearer, VolteError> {
    match to_bearer_connection(&info) {
        Ok(connection) => Ok(NativeImsBearer {
            connection,
            provider_endpoint: info.path_device,
            provider_session: info.path_handle,
            interface: info.interface,
            netdev_method: info.netdev_method,
            interface_ownership: info.interface_ownership,
            handle,
            worker: None,
            worker_binding: None,
            moved_to_worker: false,
        }),
        Err(error) => {
            handle.release().await;
            Err(error)
        }
    }
}

fn forced_native_family(hint: ImsBearerFailureHint) -> Option<u8> {
    match hint {
        ImsBearerFailureHint::NetworkForcedIpv4 => Some(4),
        ImsBearerFailureHint::NetworkForcedIpv6 => Some(6),
        _ => None,
    }
}

/// Fold a device-agnostic [`ImsBearerError`] into the stack's [`VolteError`],
/// preserving the exact codes and detail strings used by runtime diagnostics.
fn volte_error_from_ims_bearer(error: ImsBearerError) -> VolteError {
    let error_code = match error.kind {
        ImsBearerErrorKind::BasebandUnresolved => code::IP_SETTINGS_MISSING,
        ImsBearerErrorKind::EndpointUnavailable => code::RUNTIME_IMS_ENDPOINT_UNAVAILABLE,
        ImsBearerErrorKind::SessionStartFailed | ImsBearerErrorKind::NetdevUnresolved => {
            if error.hint == ImsBearerFailureHint::BasebandWedged {
                code::RUNTIME_MM_BEARER_CONNECT_FAILED
            } else {
                code::RUNTIME_IMS_BEARER_START_FAILED
            }
        }
        ImsBearerErrorKind::SettingsMissing => code::IP_SETTINGS_MISSING,
    };
    VolteError::with_detail(error_code, error.detail)
}

/// Project the device-agnostic bearer result onto the `BearerConnection`
/// contract the rest of the VoLTE stack consumes.
///
/// Kept separate from the IO above so the mapping is testable without a modem.
pub fn to_bearer_connection(info: &ImsBearerInfo) -> Result<BearerConnection, VolteError> {
    let ims = ImsIpSettings {
        ipv4_address: info.ipv4_address,
        ipv4_gateway: info.ipv4_gateway,
        ipv4_dns: info.ipv4_dns.clone(),
        ipv6_address: info.ipv6_address,
        ipv6_gateway: info.ipv6_gateway,
        ipv6_dns: info.ipv6_dns.clone(),
        pcscf: info.pcscf.clone(),
    };
    if ims.local_addr().is_none() {
        return Err(VolteError::with_detail(
            code::IP_SETTINGS_MISSING,
            "native_ims_session_has_no_address".to_string(),
        ));
    }
    Ok(BearerConnection {
        path: native_bearer_path(&info.path_device, &info.path_handle),
        interface: info.interface.clone(),
        ip_type: info.ip_type.clone(),
        settings: ims,
        ipv4_prefix: info.ipv4_prefix,
        ipv6_prefix: info.ipv6_prefix,
        mtu: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::config::VolteIpFamilyPreference;

    /// The reference IMS context as `+CGCONTRDP` reports it: address+mask,
    /// gateway, DNS and a P-CSCF, all on the same line, projected onto the
    /// device-agnostic result the transport would produce.
    fn reference_info() -> ImsBearerInfo {
        ImsBearerInfo {
            interface: "wwan0".to_string(),
            netdev_method: "probe_answered",
            ip_type: "ipv4".to_string(),
            path_device: "/dev/wwan0qmi1".to_string(),
            path_handle: "3263198272".to_string(),
            ipv4_address: Some("10.129.39.207".parse().unwrap()),
            ipv4_gateway: Some("10.129.39.208".parse().unwrap()),
            ipv4_dns: vec![
                "172.17.163.218".parse().unwrap(),
                "172.17.167.218".parse().unwrap(),
            ],
            ipv4_prefix: Some(27),
            pcscf: vec!["10.11.12.13".parse().unwrap()],
            ..Default::default()
        }
    }

    #[test]
    fn reference_session_maps_onto_the_bearer_contract() {
        let bearer = to_bearer_connection(&reference_info()).unwrap();
        assert_eq!(bearer.interface, "wwan0");
        assert_eq!(bearer.ip_type, "ipv4");
        assert_eq!(bearer.ipv4_prefix, Some(27));
        assert_eq!(
            bearer.local_addr().unwrap(),
            "10.129.39.207".parse::<IpAddr>().unwrap()
        );
        assert_eq!(bearer.settings.ipv4_dns.len(), 2);
        assert_eq!(
            bearer.settings.pcscf,
            vec!["10.11.12.13".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn native_path_is_recognisable_and_never_sent_to_modemmanager() {
        let bearer = to_bearer_connection(&reference_info()).unwrap();
        assert!(is_native_bearer(&bearer.path), "{}", bearer.path);
        assert!(!bearer.path.starts_with("/org/freedesktop/"));
        assert!(!super::super::bearer::is_valid_bearer_path(&bearer.path));
        assert!(!is_native_bearer("/org/freedesktop/ModemManager1/Bearer/4"));
    }

    #[test]
    fn a_session_without_any_address_is_rejected() {
        let empty = ImsBearerInfo {
            path_device: "/dev/wwan0qmi1".to_string(),
            path_handle: "1".to_string(),
            ..Default::default()
        };
        let error = to_bearer_connection(&empty).unwrap_err();
        assert_eq!(error.code(), code::IP_SETTINGS_MISSING);
    }

    #[test]
    fn families_follow_the_plan_order() {
        let v4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4First);
        assert_eq!(requested_families_for(&v4), vec![4, 6]);
        let v6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6First);
        assert_eq!(requested_families_for(&v6), vec![6, 4]);
        let only4 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv4Only);
        assert_eq!(requested_families_for(&only4), vec![4]);
        let only6 = ImsConnectionPlan::from_preference(VolteIpFamilyPreference::Ipv6Only);
        assert_eq!(requested_families_for(&only6), vec![6]);
        assert_eq!(
            forced_native_family(ImsBearerFailureHint::NetworkForcedIpv4),
            Some(4)
        );
        assert_eq!(
            forced_native_family(ImsBearerFailureHint::NetworkForcedIpv6),
            Some(6)
        );
        assert_eq!(forced_native_family(ImsBearerFailureHint::None), None);
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
        // A retained-client start that returns a wedge signature must abort the family
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
    fn a_forced_family_error_keeps_the_wedge_code() {
        // The wedge signature on a start failure must surface as
        // RUNTIME_MM_BEARER_CONNECT_FAILED, not as a generic start failure, so the
        // runtime does not hand a dead baseband to ModemManager.
        let error = volte_error_from_ims_bearer(ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            hint: ImsBearerFailureHint::BasebandWedged,
            detail: "secondary_qmi_start_failed:endpoint hangup".to_string(),
        });
        assert_eq!(error.code(), code::RUNTIME_MM_BEARER_CONNECT_FAILED);
        let ordinary = volte_error_from_ims_bearer(ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            hint: ImsBearerFailureHint::None,
            detail: "secondary_qmi_start_failed:verbose call end reason (2,201): [internal] error"
                .to_string(),
        });
        assert_eq!(ordinary.code(), code::RUNTIME_IMS_BEARER_START_FAILED);
    }
}
