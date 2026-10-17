//! Re-observe the original PCO provider before an idle registration refresh.
//! Keep a still-listed flow; reselect only from a nonempty, complete same-family
//! observation. Unknown/partial results never remove an endpoint or downgrade
//! its source. Replacement reuses the bearer/worker, not its old SIP/AKA/SA.
use super::*;
use crate::connectivity::core::register::{RegisterFailureKind, RegisterFailureStage};
use crate::connectivity::modems::ims::cellular_ims::pcscf::PcscfSource;

pub(super) struct RetainedSetup {
    pub sip_instance: String,
    pub access_network: Option<ImsAccessNetworkContext>,
    pub media_operator_creator: Arc<dyn OperatorSocketCreator>,
    pub require_protection: bool,
}

fn complete_for_current_family(discovery: &ImsPcscfDiscovery, current: &PcscfEndpoint) -> bool {
    (current.socket.is_ipv4() || discovery.ipv6_complete)
        && (!discovery.candidates.is_empty() || !discovery.domains.is_empty())
}
fn same_endpoint(a: &PcscfEndpoint, b: &PcscfEndpoint) -> bool {
    a.socket == b.socket && a.transport == b.transport && a.tls_authority == b.tls_authority
}
fn may_try_another_endpoint(error: &CellularImsError) -> bool {
    error.register_failure().is_some_and(|metadata|
        metadata.stage == RegisterFailureStage::Initial && metadata.min_expires.is_none()
            && matches!(metadata.kind, RegisterFailureKind::TemporaryEndpoint | RegisterFailureKind::Transport))
}

pub(super) async fn refresh_if_changed(session: &mut CellularImsLiveSession, runtime: &CellularImsRuntime,
    line_id: &str) -> Option<CellularImsRefreshAttempt> {
    // A candidate-list update is not a reg-event revocation. Existing dialogs
    // stay on their owned flow; do not break media or borrow a different source.
    if session.selected_endpoint.source != PcscfSource::BearerPco || !session.voice_calls.is_empty() {
        return None;
    }
    let native = session.native_bearer.as_mut()?;
    let observed = match native.observe_pcscf_update().await {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(error) if error.code() == code::RUNTIME_ALL_PCSCF_FAILED => return None,
        Err(error) => return Some(lost_refresh_attempt(error)),
    };
    if let Err(error) = verify_mm_task_bearer(runtime, session.native_bearer.as_mut()).await {
        return Some(lost_refresh_attempt(error));
    }
    if let Err(error) = verify_ims_registration_material(&session.device, session.auth_binding.as_ref(), &session.provisioning).await {
        return Some(lost_refresh_attempt(CellularImsError::new(error.code())));
    }
    if !complete_for_current_family(&observed, &session.selected_endpoint) { return None; }
    let mut bearer = session.bearer.clone();
    bearer.settings.pcscf = observed.candidates;
    let candidates = match super::super::pcscf::rediscover_retained_pco_in_worker(
        &bearer.settings, &observed.domains, &session.registration_identity.home_domain,
        session.channel.route().local_addr.ip(), &bearer.interface, &session.network_worker,
        session.selected_endpoint.transport).await {
        Ok(candidates) if !candidates.is_empty() => candidates,
        // No fresh resolved peer means uncertainty, not authority to discard
        // this registered binding or fall through to profile/ISIM/system DNS.
        Ok(_) => return None,
        Err(error) if error.code() == code::RUNTIME_ALL_PCSCF_FAILED => return None,
        Err(error) => return Some(lost_refresh_attempt(error)),
    };
    if let Err(error) = verify_mm_task_bearer(runtime, session.native_bearer.as_mut()).await {
        return Some(lost_refresh_attempt(error));
    }
    if let Err(error) = verify_ims_registration_material(&session.device, session.auth_binding.as_ref(), &session.provisioning).await {
        return Some(lost_refresh_attempt(CellularImsError::new(error.code())));
    }
    if remaining_registration_lifetime(&session.registration).is_zero() {
        return Some(lost_refresh_attempt(CellularImsError::new("ims_pcscf_reselection_expired")));
    }
    if let Some(current) = candidates.iter().find(|endpoint| same_endpoint(endpoint, &session.selected_endpoint)) {
        session.selected_endpoint = current.clone();
        return None;
    }
    Some(reselect(session, runtime, line_id, bearer, candidates).await)
}

async fn reselect(session: &mut CellularImsLiveSession, runtime: &CellularImsRuntime, line_id: &str,
    bearer: BearerConnection, candidates: Vec<PcscfEndpoint>) -> CellularImsRefreshAttempt {
    let retries = session.endpoint_retries.clone();
    let restart = retries.lock().await.restart_admission(Instant::now());
    if !restart.is_allowed() { return wait_attempt(remaining_registration_lifetime(&session.registration), runtime, restart).await; }
    let Some(card) = session.auth_binding.clone() else {
        return lost_refresh_attempt(CellularImsError::new("ims_uicc_binding_invalid"));
    };
    let identity = DeviceIdentity {
        diagnostic_required_security: false, ims: session.registration_identity.clone(), profile: session.profile,
        effective_ims: session.effective_ims.clone(),
        effective_device_identity: resolve_effective_device_identity(None, Some(&session.device.equipment_identifier)),
        visited_network_header: session.visited_network_header.clone(), dynamic_visited_network_fallback: session.dynamic_visited_network_fallback,
        aka_aid: session.aka_aid.clone(), usim_aid: String::new(), isim_aid: None,
        isim_pcscf: session.provisioning.material.as_ref().map(|value| value.pcscf.clone()).unwrap_or_default(),
        auth_binding: card, provisioning: session.provisioning.clone(), source: "retained_registration",
    };
    let setup = RetainedSetup { sip_instance: session.sip_instance.clone(), access_network: session.access_network.clone(),
        media_operator_creator: session.media_operator_creator.clone(), require_protection: session.channel.security_verify().is_some() };
    let access_runtime = ImsAccessNetworkRuntime::default();
    let deadline = tokio::time::Instant::now() + CELLULAR_IMS_STREAM_CONNECT_BUDGET;
    let local = session.channel.route().local_addr.ip();
    let mut last_error = None;
    for (index, endpoint) in candidates.iter().enumerate() {
        let admission = retries.lock().await.check(endpoint.socket, Instant::now());
        if !admission.is_allowed() { continue; }
        runtime.observe_diagnostics(|facts| facts.endpoint(reg_diag::FactSource::BearerPco,
            endpoint.transport.sip_transport(), endpoint.socket, index + 1, candidates.len())).await;
        match connect_family(runtime, &bearer, &identity, local, endpoint, deadline, index + 1 < candidates.len(),
            &retries, &session.device, session.video_capability_enabled, &access_runtime, &session.worker_binding,
            session.native_bearer.as_mut(), Some(&setup)).await {
            Ok(mut replacement) => {
                // Move the actual retained handles exactly once. The old SIP
                // session must NOT run bearer/profile/network cleanup here.
                replacement.native_bearer = session.native_bearer.take();
                replacement.ims_profile_lease = session.ims_profile_lease.take();
                replacement.pcscf_reporting_cid = session.pcscf_reporting_cid;
                replacement.data_slot_mode = session.data_slot_mode;
                let old = std::mem::replace(session, replacement);
                for plan in old.xfrm_plan.iter().chain(old.retired_xfrm_plan.iter()) {
                    if old.worker_binding.is_current() { ipsec::uninstall_plan_in_worker(plan, &old.xfrm_worker).await; }
                }
                drop(old); // Only original sockets/old SAs, never its PDN.
                retries.lock().await.registered();
                runtime.observe_diagnostics(|facts| facts.wait_ready()).await;
                runtime.record_attempt(CellularImsStage::Registered, Some(session.ip_family), "succeeded", None,
                    Some("pcscf_reselected_same_bearer".into())).await;
                tracing::info!(line_id, "IMS P-CSCF reselected on retained bearer; not a natural refresh");
                return CellularImsRefreshAttempt { outcome: RegistrationRefreshResult::Refreshed(session.registration.clone()),
                    error: None, retry_after: None };
            }
            Err(error) => {
                if let Some(metadata) = error.register_failure() {
                    retries.lock().await.observe_failure(endpoint.socket, metadata, Instant::now());
                }
                if !may_try_another_endpoint(&error) {
                    let admission = retries.lock().await.pause_restart(Instant::now());
                    observe_retry_diagnostics(runtime, admission).await;
                    // Authentication/security/423 failure never authorizes a
                    // new endpoint, old cleartext refresh, or reset budget.
                    return lost_refresh_attempt(error);
                }
                last_error = Some(error);
            }
        }
    }
    let admission = retries.lock().await.pause_restart(Instant::now());
    let mut attempt = wait_attempt(remaining_registration_lifetime(&session.registration), runtime, admission).await;
    if last_error.is_some() { attempt.error = last_error; }
    attempt
}

async fn wait_attempt(remaining: Duration, runtime: &CellularImsRuntime,
    admission: super::super::register_retry::RetryAdmission) -> CellularImsRefreshAttempt {
    observe_retry_diagnostics(runtime, admission).await;
    if remaining.is_zero() { return lost_refresh_attempt(CellularImsError::new("ims_pcscf_reselection_expired")); }
    let delay = admission.remaining(Instant::now()).unwrap_or(remaining).min(remaining).max(Duration::from_millis(1));
    CellularImsRefreshAttempt { outcome: RegistrationRefreshResult::Retry,
        error: Some(CellularImsError::new("ims_pcscf_reselection_pending")), retry_after: Some(delay) }
}

#[cfg(test)]
#[path = "pcscf_refresh_tests.rs"]
mod tests;
