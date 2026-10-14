//! WiFi REGISTER waits outlive profile/worker/channel caches. Owners are
//! captured UICC selections, never mutable profile labels or modem numbers.
use super::*;
use crate::connectivity::core::register_retry::{EndpointRetryState, RetryAdmission};
use crate::connectivity::core::register::{RegisterFailureKind, RetryAfter};

pub(super) const DEFERRED: &str = "vowifi_register_retry_deferred";
pub(super) const STOPPED: &str = "vowifi_register_retry_stopped";
const MAX_LINE_SCOPES: usize = 256;
static SCOPES: OnceLock<Mutex<HashMap<String, LineScope>>> = OnceLock::new();
struct LineScope { owner: Arc<LiveImsSelection>, state: Arc<Mutex<EndpointRetryState>> }
fn scopes() -> &'static Mutex<HashMap<String, LineScope>> { SCOPES.get_or_init(|| Mutex::new(HashMap::new())) }

#[derive(Clone)]
pub(super) struct Scope {
    line: String,
    owner: Arc<LiveImsSelection>,
    state: Arc<Mutex<EndpointRetryState>>,
}
impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("WifiRegisterRetryScope") }
}
impl Scope {
    pub(super) async fn for_owner(line: &str, owner: &Arc<LiveImsSelection>) -> Result<Self, ImsError> {
        // A late task may retain old data but cannot claim a fresh subscription
        // or erase its wait. No registry/state mutex is held during card I/O.
        let previous = scopes().lock().await.get(line).map(|entry| entry.state.clone());
        owner.verify(line).await.map_err(ImsError::new)?;
        let mut map = scopes().lock().await;
        if map.get(line).is_some_and(|entry| !entry.owner.same_subscription(owner)
            && previous.as_ref().is_none_or(|previous| !Arc::ptr_eq(previous, &entry.state))) {
            return Err(ImsError::new("ims_uicc_binding_changed"));
        }
        let changed_subscription = map.get(line).is_some_and(|entry| !entry.owner.same_subscription(owner));
        let state = if let Some(existing) = map.get_mut(line).filter(|entry| entry.owner.same_subscription(owner)) {
            existing.owner = owner.clone();
            existing.state.clone()
        } else {
            if !map.contains_key(line) && map.len() >= MAX_LINE_SCOPES {
                return Err(ImsError::new(STOPPED));
            }
            let state = Arc::new(Mutex::new(EndpointRetryState::default()));
            map.insert(line.to_owned(), LineScope { owner: owner.clone(), state: state.clone() });
            state
        };
        drop(map);
        if changed_subscription { clear_live_ims_refresh_failure_for_line(line).await; }
        Ok(Self { line: line.to_owned(), owner: owner.clone(), state })
    }
    pub(super) async fn check(&self, endpoint: SocketAddr) -> RetryAdmission {
        self.state.lock().await.check(endpoint, Instant::now())
    }
    pub(super) async fn observe(&self, endpoint: SocketAddr, metadata: &RegisterFailureMetadata) -> RetryAdmission {
        self.state.lock().await.observe_failure(endpoint, metadata, Instant::now())
    }
    pub(super) async fn pause_restart(&self) -> RetryAdmission {
        self.state.lock().await.pause_restart(Instant::now())
    }
    pub(super) async fn accepted(&self, endpoint: SocketAddr) -> Result<(), ImsError> {
        self.owner.verify(&self.line).await.map_err(ImsError::new)?;
        if !scopes().lock().await.get(&self.line).is_some_and(|entry| Arc::ptr_eq(&entry.state, &self.state)) {
            return Err(ImsError::new("ims_uicc_binding_changed"));
        }
        let mut state = self.state.lock().await;
        let admission = state.check(endpoint, Instant::now());
        match admission {
            RetryAdmission::Allowed => {},
            RetryAdmission::Deferred { .. } => return Err(ImsError::new(DEFERRED)),
            RetryAdmission::Stopped(_) => return Err(ImsError::new(STOPPED)),
        }
        state.registered();
        Ok(())
    }
}

/// Real endpoint-pass controller. Production supplies its tunnel exchange;
/// loopback tests use the same controller without creating an ePDG or TUN.
pub(super) async fn run_candidates<T, F, Fut>(scope: &Scope, endpoints: &[SocketAddr], mut attempt: F)
    -> Result<T, LiveStageError>
where F: FnMut(SocketAddr) -> Fut, Fut: Future<Output = Result<T, LiveStageError>> {
    let mut last_error = None;
    for endpoint in endpoints.iter().copied() {
        let admission = scope.check(endpoint).await;
        if let Some(error) = admission_error(admission) {
            publish_admission(&scope.line, admission).await;
            last_error = Some(error);
            continue;
        }
        match attempt(endpoint).await {
            Ok(result) => return Ok(result),
            Err(error) => {
                if let Some(metadata) = error.register_failure {
                    if metadata.is_temporary() || response_requires_wait(metadata) {
                        let admission = scope.observe(endpoint, &metadata).await;
                        publish_admission(&scope.line, admission).await;
                    }
                }
                let terminal = live_register_error_is_terminal(&error)
                    || error.register_failure.is_some_and(|metadata| response_requires_wait(metadata)
                        && metadata.kind != RegisterFailureKind::TemporaryEndpoint);
                last_error = Some(error);
                if terminal { break; }
            }
        }
    }
    let error = last_error.unwrap_or_else(|| live_stage_error("ims_pcscf_candidate_missing"));
    let admission = scope.pause_restart().await;
    publish_admission(&scope.line, admission).await;
    Err(with_admission(error, admission))
}

/// A complete temporary response or any received Retry-After stops internal
/// header/ESP exploration immediately. A saved challenge on a later transport
/// failure is not a received wait for that failed send.
pub(super) fn response_requires_wait(metadata: RegisterFailureMetadata) -> bool {
    metadata.kind == RegisterFailureKind::TemporaryEndpoint
        || (metadata.kind != RegisterFailureKind::Transport && metadata.retry_after != RetryAfter::Absent)
}

/// Adapter-owned sends retain their true transport provenance rather than
/// inheriting a saved 401/Retry-After from the outer shared challenge driver.
pub(super) fn owned_transport_error(error: ImsError, send: bool) -> LiveStageError {
    use crate::connectivity::core::register::RegisterFailureStage;
    let flow = error.code().starts_with("ims_outbound_");
    let mut result = live_stage_error(if flow { error.code() } else if send { "ims_register_write_failed" } else { error.code() });
    let kind = if flow { RegisterFailureKind::FlowFailed } else if send || matches!(error.code(),
        "ims_channel_read_timeout" | "ims_channel_read_failed" | "ims_register_read_timeout") {
        RegisterFailureKind::Transport
    } else { RegisterFailureKind::LocalFailure };
    result.register_failure = Some(RegisterFailureMetadata { sip_status: None,
        stage: RegisterFailureStage::Authenticated, kind, retry_after: RetryAfter::Absent, min_expires: None });
    result
}

pub(super) fn with_admission(mut error: LiveStageError, admission: RetryAdmission) -> LiveStageError {
    if let Some(wait) = admission_error(admission) {
        error.retry_admission = Some(admission);
        error.reason = format!("{}:{}", wait.reason, error.reason);
    }
    error
}

pub(super) fn admission_error(admission: RetryAdmission) -> Option<LiveStageError> {
    let code = match admission { RetryAdmission::Allowed => return None,
        RetryAdmission::Deferred { .. } => DEFERRED, RetryAdmission::Stopped(_) => STOPPED };
    let mut error = live_stage_error(code);
    error.retry_admission = Some(admission);
    Some(error)
}

/// Read failures preserve the wait. Only a positively different SIM discards
/// this map entry, and a late captured Scope can still update only its old Arc.
pub(super) async fn restart_admission(line: &str) -> RetryAdmission {
    for _ in 0..3 {
        let (captured, capacity) = {
            let map = scopes().lock().await;
            (map.get(line).map(|entry| (entry.owner.clone(), entry.state.clone())), map.len() >= MAX_LINE_SCOPES)
        };
        let Some((owner, state)) = captured else {
            return if capacity { RetryAdmission::Stopped(crate::connectivity::core::register_retry::RetryStopReason::CapacityExceeded) }
                else { RetryAdmission::Allowed };
        };
        let admission = state.lock().await.restart_admission(Instant::now());
        if admission.is_allowed() { return admission; }
        let changed = tokio::time::timeout(Duration::from_secs(8), owner.subscription_is_current(line))
            .await.is_ok_and(|result| result == Ok(false));
        let mut map = scopes().lock().await;
        if !map.get(line).is_some_and(|entry| Arc::ptr_eq(&entry.state, &state)) { continue; }
        if changed {
            map.remove(line); drop(map);
            clear_live_ims_refresh_failure_for_line(line).await;
            return RetryAdmission::Allowed;
        }
        drop(map);
        return state.lock().await.restart_admission(Instant::now());
    }
    // Churning ownership cannot be interpreted as a free retry window.
    RetryAdmission::Stopped(crate::connectivity::core::register_retry::RetryStopReason::OwnershipUncertain)
}

pub(super) async fn publish_admission(line: &str, admission: RetryAdmission) {
    let scope = super::diagnostics_live::Scope::current_or_begin(line).await;
    scope.admission(admission).await;
}

pub(crate) async fn check_restart_gate(line: &str) -> Result<(), LiveStageError> {
    let admission = restart_admission(line).await;
    if let Some(error) = admission_error(admission) {
        publish_admission(line, admission).await;
        return Err(error);
    }
    Ok(())
}

pub(crate) async fn live_register_retry_remaining_for_line(line: &str) -> Option<Duration> {
    let admission = restart_admission(line).await;
    match admission {
        RetryAdmission::Allowed => None,
        RetryAdmission::Deferred { not_before } => Some(not_before.saturating_duration_since(Instant::now())),
        RetryAdmission::Stopped(_) => Some(Duration::MAX),
    }
}

#[cfg(test)]
pub(crate) async fn seed_register_retry_for_test(line: &str, seconds: u32) {
    let profile = profiles::derive_standard_3gpp_profile("001", "01", profiles::Standard3gppAccess::WifiEpdg).unwrap();
    let identity = LiveImsRegisterIdentity {
        shared: crate::connectivity::core::context::ImsIdentity { private_user: "fixture@ims.example.test".into(),
            public_uri: "sip:fixture@ims.example.test".into(), contact_user: "fixture".into(),
            home_domain: "ims.example.test".into(), contact_user_phone: false }, shape: "retry-http-fixture",
    };
    let owner = live_identity::selection_for_test(line, identity, live_ims_target(line, profile),
        crate::connectivity::modems::ims::uicc_ims::UiccCardBinding { endpoint: format!("/synthetic/{line}"),
            slot: 1, owner: "test-owner".into(), iccid: "synthetic-api-card".into(), imsi: "001010000000001".into() });
    let scope = Scope::for_owner(line, &owner).await.unwrap();
    scope.observe("192.0.2.1:5060".parse().unwrap(), &RegisterFailureMetadata {
        sip_status: Some(503), stage: crate::connectivity::core::register::RegisterFailureStage::Initial,
        kind: RegisterFailureKind::TemporaryEndpoint, retry_after: RetryAfter::DelaySeconds(seconds), min_expires: None,
    }).await;
    scope.pause_restart().await;
}

#[cfg(test)]
#[path = "register_retry_live_tests.rs"]
mod tests;
