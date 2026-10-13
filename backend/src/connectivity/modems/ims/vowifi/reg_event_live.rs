//! Captured-owner N5 credential provider and line-scoped recovery state.
use super::*;
use std::sync::Weak;
use super::super::runtime::VowifiRuntime;

const LOSS_REASON: &str = "ims_reg_event_binding_terminated";
static RUNTIMES: OnceLock<StdRwLock<HashMap<String, Weak<VowifiRuntime>>>> = OnceLock::new();
static REVOKED: OnceLock<Mutex<HashMap<String, Revoked>>> = OnceLock::new();

// Not cleared by profile changes, worker replacement or clear_live_runtime.
// The captured UICC Arc is retained only to compare ownership, never logged.
struct Revoked {
    owner: Arc<LiveImsSelection>,
    // None is a fail-closed wait beyond the supported 24h horizon.
    not_before: Option<Instant>,
}
fn revoked() -> &'static Mutex<HashMap<String, Revoked>> {
    REVOKED.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn bind_reg_event_runtime(line_id: &str, runtime: &Arc<VowifiRuntime>) {
    RUNTIMES.get_or_init(|| StdRwLock::new(HashMap::new())).write()
        .unwrap_or_else(|error| error.into_inner()).insert(line_id.to_string(), Arc::downgrade(runtime));
}

pub(super) fn runtime_for_line(line_id: &str) -> Option<Arc<VowifiRuntime>> {
    RUNTIMES.get().and_then(|runtimes| runtimes.read().unwrap_or_else(|error| error.into_inner())
        .get(line_id).and_then(Weak::upgrade))
}

pub(crate) async fn live_reg_event_retry_remaining_for_line(line_id: &str) -> Option<Duration> {
    let owner = revoked().lock().await.get(line_id).map(|state| state.owner.clone());
    let owner = owner?;
    // Read failures do not clear a server wait. Only a positively different
    // subscription may discard it; same-card worker/profile restarts may not.
    if tokio::time::timeout(Duration::from_secs(8), owner.subscription_is_current(line_id))
        .await.is_ok_and(|result| result == Ok(false))
    {
        let mut states = revoked().lock().await;
        if states.get(line_id).is_some_and(|state| Arc::ptr_eq(&state.owner, &owner)) { states.remove(line_id); }
        return None;
    }
    revoked().lock().await.get(line_id).and_then(|state| {
        let remaining = state.not_before.map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::MAX);
        (!remaining.is_zero()).then_some(remaining)
    })
}

pub(super) async fn registration_revoked(line_id: &str) -> bool {
    revoked().lock().await.contains_key(line_id)
}

pub(super) async fn check_retry_gate(line_id: &str) -> Result<(), LiveStageError> {
    if live_reg_event_retry_remaining_for_line(line_id).await.is_some() {
        let deadline = revoked().lock().await.get(line_id).map(|state| state.not_before);
        if let Some(deadline) = deadline { super::diagnostics_live::network_wait(line_id, deadline).await; }
        return Err(live_stage_error(LOSS_REASON));
    }
    Ok(())
}

/// Called only after a newly accepted REGISTER has installed its captured UICC.
/// A natural refresh of the revoked owner cannot manufacture recovery.
pub(super) async fn accepted_owner(line_id: &str, owner: &Arc<LiveImsSelection>) -> Result<(), LiveStageError> {
    let mut states = revoked().lock().await;
    if let Some(state) = states.get(line_id) {
        if state.owner.same_subscription(owner) &&
            (state.not_before.is_none_or(|deadline| Instant::now() < deadline) || Arc::ptr_eq(&state.owner, owner))
        {
            return Err(live_stage_error(LOSS_REASON));
        }
        states.remove(line_id);
    }
    Ok(())
}

impl VowifiUnregisterFactory {
    pub(super) async fn reg_event_check_owner(&self) -> Result<(), ImsError> {
        let uicc = self.context.uicc.as_ref().ok_or_else(|| ImsError::new("ims_uicc_binding_invalid"))?;
        let current = xcap_binding_cache().lock().await.get(&self.line_id).cloned();
        let owned = current.as_ref().is_some_and(|current| std::ptr::eq(current.profile, self.profile)
            && Arc::ptr_eq(&current.uicc, uicc));
        // Revoked channels may still acknowledge duplicate NOTIFY while their
        // active dialogs drain; they cannot authenticate or start subscriptions.
        let revoked_owner = current.is_none() && revoked().lock().await.get(&self.line_id)
            .is_some_and(|state| Arc::ptr_eq(&state.owner, uicc));
        if !owned && !revoked_owner { return Err(ImsError::new("ims_uicc_binding_changed")); }
        uicc.verify(&self.line_id).await.map_err(ImsError::new)?;
        // Card observation awaits; never use a now-replaced channel's authority.
        let cache = xcap_binding_cache().lock().await;
        let owned = cache.get(&self.line_id).is_some_and(|current| std::ptr::eq(current.profile, self.profile)
            && Arc::ptr_eq(&current.uicc, uicc));
        let revoked_owner = !cache.contains_key(&self.line_id) && revoked().lock().await.get(&self.line_id)
            .is_some_and(|state| Arc::ptr_eq(&state.owner, uicc));
        if !owned && !revoked_owner { return Err(ImsError::new("ims_uicc_binding_changed")); }
        Ok(())
    }

    pub(super) async fn reg_event_proof(
        &self, challenge: &crate::connectivity::core::digest_aka::DigestChallenge, uri: &str,
    ) -> Result<String, ImsError> {
        self.reg_event_check_owner().await?;
        if challenge.stale || registration_revoked(&self.line_id).await {
            return Err(ImsError::new("ims_reg_event_auth_unavailable"));
        }
        let uicc = self.context.uicc.as_ref().ok_or_else(|| ImsError::new("ims_uicc_binding_invalid"))?;
        // This helper takes the original slot/AID/card binding, not the mutable
        // global MWI lookup. Each SUBSCRIBE gets fresh cnonce / nonce count 1.
        let header = build_line_parsed_digest_aka_authorization(&self.line_id, uicc,
            &self.context.identity.private_user, "SUBSCRIBE", uri, challenge.clone())
            .await.map_err(|_| ImsError::new("ims_reg_event_auth_unavailable"))?;
        self.reg_event_check_owner().await?;
        // AUTS alone is not an authentication proof. Do not send a continuation
        // that a bare 2xx could accidentally promote to authenticated state.
        require_subscribe_proof(&header)?;
        Ok(header)
    }

    pub(super) async fn reg_event_loss(&self, retry_after: Option<u32>) {
        // Operator already checked the captured owner before acknowledging.
        // Recheck cache ownership under lock, with no second UICC round trip.
        let Some(uicc) = self.context.uicc.as_ref() else { return; };
        // Serialize with publication of a replacement before touching caches.
        let mut xcap = xcap_binding_cache().lock().await;
        if !xcap.get(&self.line_id).is_some_and(|current| Arc::ptr_eq(&current.uicc, uicc)
            && std::ptr::eq(current.profile, self.profile)) { return; }
        let seconds = retry_after.unwrap_or(30).max(1);
        let deadline = (seconds <= 86_400).then(|| Instant::now().checked_add(Duration::from_secs(u64::from(seconds)))).flatten();
        {
            let mut states = revoked().lock().await;
            let deadline = match states.get(&self.line_id).filter(|state| state.owner.same_subscription(uicc)) {
                Some(state) => match (state.not_before, deadline) {
                    (Some(old), Some(new)) => Some(old.max(new)),
                    _ => None,
                },
                None => deadline,
            };
            states.insert(self.line_id.clone(), Revoked { owner: uicc.clone(), not_before: deadline });
        }
        ims_register_ready_cache().lock().await.remove(&self.line_id);
        // Security-Verify / ESP remain owned by the active channel. Remove only
        // new-service credentials and readiness, never an in-progress dialog.
        xcap.remove(&self.line_id);
        ims_refresh_failure_cache().lock().await.insert(self.line_id.clone(), LiveImsRefreshFailure {
            consecutive_failures: LIVE_IMS_REFRESH_REBUILD_FAILURES,
            last_failure_reason: LOSS_REASON.to_string(),
            rebuild_pending: true,
        });
        let observed_deadline = revoked().lock().await.get(&self.line_id).and_then(|state| state.not_before);
        super::diagnostics_live::network_wait(&self.line_id, observed_deadline).await;
        let runtime = runtime_for_line(&self.line_id);
        if let Some(runtime) = runtime {
            runtime.prepare_live_ims_registration_refresh(LOSS_REASON).await;
        }
    }
}

fn require_subscribe_proof(header: &str) -> Result<(), ImsError> {
    let header = header.to_ascii_lowercase();
    if header.contains("auts=") || header.contains("response=\"\"") || !header.contains("response=") {
        return Err(ImsError::new("ims_reg_event_auth_unavailable"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "reg_event_live_tests.rs"]
mod tests;
