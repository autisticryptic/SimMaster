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
//! Provider-owned IP settings remain authoritative (the QCA410/MM provider
//! reads its retained D-Bus bearer). Supplementary P-CSCF observation can also
//! be delegated to that retained provider; unsupported providers keep the
//! exact-address AT fallback. Never substitute AT addressing for the MM grant.
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
    ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, ImsPcscfDiscovery,
};
use crate::{
    platform::netns,
    services::ue_worker::{UeWorkerBinding, UeWorkerHandle},
};

use super::{
    bearer::{teardown_bearer_network_in_worker, BearerConnection, BearerRequest},
    errors::{code, CellularImsError},
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
    pub fn check_liveness(&mut self) -> Result<(), CellularImsError> {
        self.handle
            .check_liveness()
            .map_err(cellular_ims_error_from_ims_bearer)
    }

    /// Use the retained provider's association if supported. Only an explicit
    /// unsupported result permits the legacy exact-address observation.
    pub async fn discover_pcscf<F, Fut>(
        &mut self,
        exact_at_fallback: F,
    ) -> Result<ImsPcscfDiscovery, CellularImsError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<ImsPcscfDiscovery, CellularImsError>>,
    {
        self.check_liveness()?;
        if !self.worker_binding_is_current() {
            return Err(CellularImsError::new(
                code::RUNTIME_UE_WORKER_GENERATION_CHANGED,
            ));
        }
        let provider = self.handle.discover_pcscf().await;
        self.check_liveness()?;
        if !self.worker_binding_is_current() {
            return Err(CellularImsError::new(
                code::RUNTIME_UE_WORKER_GENERATION_CHANGED,
            ));
        }
        let result = match provider {
            Ok(Some(discovery)) => Ok(discovery),
            Ok(None) => exact_at_fallback().await,
            Err(error) => Err(cellular_ims_error_from_ims_bearer(error)),
        };
        self.check_liveness()?;
        if !self.worker_binding_is_current() {
            return Err(CellularImsError::new(
                code::RUNTIME_UE_WORKER_GENERATION_CHANGED,
            ));
        }
        result
    }

    /// Move the dedicated native netdev into this line's UE namespace. The
    /// host-managed Internet interface is intentionally rejected. A device
    /// provider may use ModemManager as the control-plane owner of its own
    /// exclusive IMS bearer, but must verify data-interface ownership before
    /// declaring it application-owned and crossing the namespace boundary.
    pub async fn move_into_worker(
        &mut self,
        worker: UeWorkerHandle,
    ) -> Result<(), CellularImsError> {
        let worker_binding = worker.bind();
        let _generation = worker_binding
            .lock_current_generation()
            .await
            .map_err(|_| CellularImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED))?;
        if let Some(existing) = self.worker_binding.as_ref() {
            if existing.is_current() && existing.matches(&worker_binding) {
                return Ok(());
            }
            return Err(CellularImsError::new(
                code::RUNTIME_UE_WORKER_GENERATION_CHANGED,
            ));
        }
        if !worker.status().await.ready {
            return Err(CellularImsError::new(code::RUNTIME_UE_WORKER_UNAVAILABLE));
        }
        match self.interface_ownership {
            BearerInterfaceOwnership::HostManagedPrimary => {
                return Err(CellularImsError::with_detail(
                    code::COMMAND_FAILED,
                    format!(
                        "native bearer refuses to move host-managed interface {}",
                        self.interface
                    ),
                ));
            }
            BearerInterfaceOwnership::Unknown => {
                return Err(CellularImsError::with_detail(
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
                self.worker_binding = Some(worker_binding.clone());
                return Ok(());
            }
            BearerInterfaceOwnership::ApplicationOwnedNative => {}
        }
        let _move_guard = self
            .handle
            .prepare_namespace_move(worker_binding.namespace().as_str())
            .map_err(cellular_ims_error_from_ims_bearer)?;
        netns::move_iface_in(worker_binding.namespace(), &self.interface)
            .await
            .map_err(|error| {
                CellularImsError::with_detail(
                    code::COMMAND_FAILED,
                    format!(
                        "move native bearer {} into {}: {error}",
                        self.interface,
                        worker_binding.namespace()
                    ),
                )
            })?;
        // Record ownership as soon as the move succeeds. Any later failure is
        // released through the captured binding, never a same-name lookup in
        // a replacement worker. The lifecycle guard prevents respawn/move-out
        // races until this method returns.
        self.worker = Some(worker);
        self.worker_binding = Some(worker_binding.clone());
        self.moved_to_worker = true;
        let status = worker_binding.worker().refresh_net_status().await;
        if status.as_ref().ok().is_none_or(|snapshot| {
            !snapshot
                .interfaces
                .iter()
                .any(|name| name == &self.interface)
        }) {
            return Err(CellularImsError::with_detail(
                code::COMMAND_FAILED,
                format!(
                    "worker cannot observe moved native interface {}",
                    self.interface
                ),
            ));
        }
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
        let Some(binding) = self.worker_binding.as_ref() else {
            // A missing binding is not evidence that the interface is home.
            return;
        };
        let Ok(_generation) = binding.lock_current_generation().await else {
            tracing::warn!(
                interface = %self.interface,
                "Skipping native VoLTE interface restore bound to a stale UE worker generation"
            );
            return;
        };
        if let Err(error) = netns::move_iface_out(binding.namespace(), &self.interface).await {
            tracing::warn!(
                interface = %self.interface,
                error = %error,
                "Native interface restore failed; retaining namespace ownership"
            );
            return;
        }
        if let Err(error) = self
            .handle
            .confirm_namespace_restore(binding.namespace().as_str())
            .await
        {
            tracing::warn!(
                interface = %self.interface,
                error = %error.detail,
                "Native interface restore unconfirmed; retaining namespace ownership"
            );
            return;
        }
        let _ = binding.worker().refresh_net_status().await;
        drop(_generation);
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
) -> Result<NativeImsBearer, CellularImsError> {
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
                request.allow_roaming,
            )
            .await;
        match result {
            Ok((info, handle)) => return adopt_bearer(info, handle).await,
            Err(error) => {
                let hint = error.hint;
                let error = cellular_ims_error_from_ims_bearer(error);
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
                    if let Some(error) = pinned_profile_forced_family_error(request, forced) {
                        // MM 1.18 resolves a pinned profile's PDP family before
                        // considering the request flag. Retrying the forced label
                        // with the same pin repeats the same PDN attempt (the
                        // IPv4-profile/IPv6 retry seen in SIM-04 T03). Do not
                        // silently drop or overwrite the pin; an exact-family
                        // lease must be established by a separate maintenance
                        // path before another family can be requested.
                        last_error = Some(error);
                    } else {
                        forced_single = Some(forced);
                    }
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
                request.allow_roaming,
            )
            .await
        {
            Ok((info, handle)) => return adopt_bearer(info, handle).await,
            Err(error) => {
                let error = cellular_ims_error_from_ims_bearer(error);
                tracing::warn!(family = forced, error = %error, "Native VoLTE network-forced family WDS attempt failed");
                last_error = Some(error);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        CellularImsError::with_detail(
            code::RUNTIME_MM_BEARER_CONNECT_FAILED,
            "native_ims_no_family_attempted".to_string(),
        )
    }))
}

fn pinned_profile_forced_family_error(
    request: &BearerRequest,
    forced: u8,
) -> Option<CellularImsError> {
    request.profile_id.map(|profile| {
        CellularImsError::with_detail(
            code::RUNTIME_IMS_FAMILY_UNSUPPORTED,
            format!("profile_pin_family_conflict:profile_id={profile}:forced_family={forced}"),
        )
    })
}

/// Release a handle whose device binding became unverified. Do not first
/// touch an interface merely because the worker generation still matches:
/// only the provider can revalidate MM ownership/exclusivity or retain a
/// recovery receipt. In particular, do not move a replacement owner's link.
pub(super) async fn release_unverified_native_ims_bearer(bearer: NativeImsBearer) {
    bearer.handle.release().await;
}

/// Tear down a native bearer's WDS session(s) and release its endpoint.
pub async fn release_native_ims_bearer(mut bearer: NativeImsBearer) {
    if bearer.worker_binding_is_current() {
        if let Some(worker) = bearer.worker_binding.as_ref() {
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
pub(crate) async fn adopt_bearer(
    info: ImsBearerInfo,
    handle: Box<dyn ImsBearerHandle + Send>,
) -> Result<NativeImsBearer, CellularImsError> {
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

/// Fold a device-agnostic [`ImsBearerError`] into the stack's [`CellularImsError`],
/// preserving the exact codes and detail strings used by runtime diagnostics.
pub(crate) fn cellular_ims_error_from_ims_bearer(error: ImsBearerError) -> CellularImsError {
    if error.hint == ImsBearerFailureHint::BasebandWedged {
        return CellularImsError::with_detail(code::RUNTIME_IMS_BASEBAND_WEDGED, error.detail);
    }
    let error_code = match error.kind {
        ImsBearerErrorKind::BasebandUnresolved => code::IP_SETTINGS_MISSING,
        ImsBearerErrorKind::EndpointUnavailable => code::RUNTIME_IMS_ENDPOINT_UNAVAILABLE,
        ImsBearerErrorKind::SessionStartFailed | ImsBearerErrorKind::NetdevUnresolved => {
            code::RUNTIME_IMS_BEARER_START_FAILED
        }
        ImsBearerErrorKind::SessionLost => code::BEARER_SESSION_LOST,
        ImsBearerErrorKind::SettingsMissing => code::IP_SETTINGS_MISSING,
        ImsBearerErrorKind::PcscfUnavailable => code::RUNTIME_ALL_PCSCF_FAILED,
    };
    CellularImsError::with_detail(error_code, error.detail)
}

/// Project the device-agnostic bearer result onto the `BearerConnection`
/// contract the rest of the VoLTE stack consumes.
///
/// Kept separate from the IO above so the mapping is testable without a modem.
pub fn to_bearer_connection(info: &ImsBearerInfo) -> Result<BearerConnection, CellularImsError> {
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
        return Err(CellularImsError::with_detail(
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
    use crate::hardware::devices::transport::TransportFuture;
    use crate::platform::config::CellularImsIpFamilyPreference;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    struct PcscfHandle {
        result: Option<Result<Option<ImsPcscfDiscovery>, ImsBearerError>>,
        alive: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
        lose_during_read: bool,
    }

    fn lost_pcscf_session() -> ImsBearerError {
        ImsBearerError {
            kind: ImsBearerErrorKind::SessionLost,
            hint: ImsBearerFailureHint::None,
            detail: "test_session_lost".to_string(),
        }
    }

    impl ImsBearerHandle for PcscfHandle {
        fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
            self.alive
                .load(Ordering::Acquire)
                .then_some(())
                .ok_or_else(lost_pcscf_session)
        }

        fn discover_pcscf(
            &mut self,
        ) -> TransportFuture<'_, Result<Option<ImsPcscfDiscovery>, ImsBearerError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::AcqRel);
                if self.lose_during_read {
                    self.alive.store(false, Ordering::Release);
                }
                self.result.take().expect("one provider observation")
            })
        }

        fn release(
            self: Box<Self>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
            Box::pin(async {})
        }
    }

    fn discovered(source: &'static str) -> ImsPcscfDiscovery {
        ImsPcscfDiscovery {
            candidates: vec!["192.0.2.20".parse().unwrap()],
            context_id: Some(2),
            source,
        }
    }

    #[tokio::test]
    async fn provider_success_missing_and_lost_results_never_retry_legacy_at() {
        let missing = ImsBearerError {
            kind: ImsBearerErrorKind::PcscfUnavailable,
            hint: ImsBearerFailureHint::None,
            detail: "test_pcscf_missing".to_string(),
        };
        for (reply, expected_code) in [
            (Ok(Some(discovered("provider"))), None),
            (Err(missing), Some(code::RUNTIME_ALL_PCSCF_FAILED)),
            (Err(lost_pcscf_session()), Some(code::BEARER_SESSION_LOST)),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let handle = PcscfHandle {
                result: Some(reply),
                alive: Arc::new(AtomicBool::new(true)),
                calls: Arc::clone(&calls),
                lose_during_read: false,
            };
            let mut bearer = adopt_bearer(reference_info(), Box::new(handle))
                .await
                .unwrap();
            let before = bearer.connection.settings.clone();
            let result = bearer
                .discover_pcscf(|| async {
                    panic!("a supported provider must not retry modem-wide AT")
                })
                .await;
            assert_eq!(calls.load(Ordering::Acquire), 1);
            if let Some(code) = expected_code {
                assert_eq!(result.unwrap_err().code(), code);
            } else {
                assert_eq!(result.unwrap().source, "provider");
            }
            assert_eq!(bearer.connection.settings, before);
            release_native_ims_bearer(bearer).await;
        }
    }

    #[tokio::test]
    async fn unsupported_provider_runs_the_exact_address_fallback_once() {
        struct Unsupported;
        impl ImsBearerHandle for Unsupported {
            fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
                Ok(())
            }
            fn release(
                self: Box<Self>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>
            {
                Box::pin(async {})
            }
        }
        let mut bearer = adopt_bearer(reference_info(), Box::new(Unsupported))
            .await
            .unwrap();
        let calls = AtomicUsize::new(0);
        let result = bearer
            .discover_pcscf(|| async {
                calls.fetch_add(1, Ordering::AcqRel);
                Ok(discovered("legacy_exact"))
            })
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(result.source, "legacy_exact");
        release_native_ims_bearer(bearer).await;
    }

    #[tokio::test]
    async fn provider_or_fallback_liveness_loss_discards_the_observation() {
        for lose_during_read in [true, false] {
            let alive = Arc::new(AtomicBool::new(true));
            let handle = PcscfHandle {
                result: Some(Ok(None)),
                alive: Arc::clone(&alive),
                calls: Arc::new(AtomicUsize::new(0)),
                lose_during_read,
            };
            let mut bearer = adopt_bearer(reference_info(), Box::new(handle))
                .await
                .unwrap();
            let fallbacks = AtomicUsize::new(0);
            let error = bearer
                .discover_pcscf(|| async {
                    fallbacks.fetch_add(1, Ordering::AcqRel);
                    alive.store(false, Ordering::Release);
                    Ok(discovered("stale"))
                })
                .await
                .unwrap_err();
            assert_eq!(error.code(), code::BEARER_SESSION_LOST);
            assert_eq!(
                fallbacks.load(Ordering::Acquire),
                usize::from(!lose_during_read)
            );
            release_native_ims_bearer(bearer).await;
        }
    }

    #[tokio::test]
    async fn unverified_provider_release_never_sends_generic_worker_cleanup() {
        struct ReleaseProbe(Arc<AtomicUsize>);
        impl ImsBearerHandle for ReleaseProbe {
            fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
                Err(lost_pcscf_session())
            }
            fn release(
                self: Box<Self>,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>
            {
                Box::pin(async move {
                    self.0.fetch_add(1, Ordering::AcqRel);
                })
            }
        }
        let line = "pcscf-unverified-cleanup";
        let worker = UeWorkerHandle::for_line(
            line,
            netns::NetnsName::for_line(netns::DEFAULT_NAMESPACE_PREFIX, line),
        );
        worker
            .enable_test_net_config_outcome(Some("unexpected_generic_cleanup".into()))
            .await;
        let releases = Arc::new(AtomicUsize::new(0));
        let mut info = reference_info();
        info.interface = "pcscf-test0".into();
        let mut bearer = adopt_bearer(info, Box::new(ReleaseProbe(Arc::clone(&releases))))
            .await
            .unwrap();
        bearer.worker_binding = Some(worker.bind());
        bearer.worker = Some(worker.clone());
        assert!(bearer.worker_binding_is_current());
        release_unverified_native_ims_bearer(bearer).await;
        assert_eq!(releases.load(Ordering::Acquire), 1);
        assert!(worker.status().await.last_net_config_error.is_none());
        assert!(!worker.status().await.last_net_config_ok);
    }

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
    fn pinned_profile_does_not_repeat_a_forced_family_with_the_same_mm_profile() {
        let mut request = BearerRequest::ims(false);
        request.profile_id = Some(3);
        let error = pinned_profile_forced_family_error(&request, 6).unwrap();
        assert_eq!(error.code(), code::RUNTIME_IMS_FAMILY_UNSUPPORTED);
        assert!(error
            .detail()
            .is_some_and(|detail| detail.contains("profile_id=3")));
        request.profile_id = None;
        assert!(pinned_profile_forced_family_error(&request, 6).is_none());
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
        let v4 = ImsConnectionPlan::from_preference(CellularImsIpFamilyPreference::Ipv4First);
        assert_eq!(requested_families_for(&v4), vec![4, 6]);
        let v6 = ImsConnectionPlan::from_preference(CellularImsIpFamilyPreference::Ipv6First);
        assert_eq!(requested_families_for(&v6), vec![6, 4]);
        let only4 = ImsConnectionPlan::from_preference(CellularImsIpFamilyPreference::Ipv4Only);
        assert_eq!(requested_families_for(&only4), vec![4]);
        let only6 = ImsConnectionPlan::from_preference(CellularImsIpFamilyPreference::Ipv6Only);
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
        // RUNTIME_IMS_BASEBAND_WEDGED, not as a generic start failure, so both
        // the family loop and the outer profile batch stop on the same hint.
        let error = cellular_ims_error_from_ims_bearer(ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            hint: ImsBearerFailureHint::BasebandWedged,
            detail: "secondary_qmi_start_failed:endpoint hangup".to_string(),
        });
        assert_eq!(error.code(), code::RUNTIME_IMS_BASEBAND_WEDGED);
        assert_eq!(
            FailureClass::from_error(&error),
            FailureClass::BasebandWedged
        );
        let ordinary = cellular_ims_error_from_ims_bearer(ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            hint: ImsBearerFailureHint::None,
            detail: "secondary_qmi_start_failed:verbose call end reason (2,201): [internal] error"
                .to_string(),
        });
        assert_eq!(ordinary.code(), code::RUNTIME_IMS_BEARER_START_FAILED);
    }

    #[test]
    fn lost_session_does_not_retry_another_family_on_the_dead_bearer() {
        let error = cellular_ims_error_from_ims_bearer(ImsBearerError {
            kind: ImsBearerErrorKind::SessionLost,
            hint: ImsBearerFailureHint::None,
            detail: "secondary_qmi_session_exited:exit status: 0".to_string(),
        });
        assert_eq!(error.code(), code::BEARER_SESSION_LOST);
        assert!(!FailureClass::from_error(&error).is_retryable_family());
        assert!(!FailureClass::from_error(&error).is_unsafe_to_retry());
    }
}
