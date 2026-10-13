//! Passive WiFi runtime publication and cross-attempt/SIM isolation.
use super::*;
use crate::connectivity::core::registration_diagnostics::{FactSource, IdentityOrigin, IpFamily, FailureScope, AuthStage, WaitStatus};
use std::sync::atomic::{AtomicUsize, Ordering};

fn profile() -> &'static CarrierProfile {
    profiles::derive_standard_3gpp_profile("001", "01", profiles::Standard3gppAccess::WifiEpdg).unwrap()
}
fn runtime() -> (String, Arc<VowifiRuntime>) {
    static IDS: AtomicUsize = AtomicUsize::new(0);
    let line = format!("n7-diagnostics-{}", IDS.fetch_add(1, Ordering::Relaxed));
    let runtime = Arc::new(VowifiRuntime::for_line(&line));
    super::super::reg_event_live::bind_reg_event_runtime(&line, &runtime);
    (line, runtime)
}

#[tokio::test]
async fn wifi_execution_facts_reach_status_without_changing_readiness_or_configuration() {
    let (line, runtime) = runtime();
    runtime.seed_registered_for_reg_event_test().await;
    let before = runtime.snapshot().await;
    let scope = Scope::begin(&line, profile()).await;
    scope.update(|state| {
        state.identity(IdentityOrigin::Isim, FactSource::Isim, FactSource::SimOverride);
        state.facts.requested_family = IpFamily::Ipv4v6;
        state.facts.granted_family = IpFamily::Ipv6;
        state.endpoint(FactSource::AccessConfiguration, crate::connectivity::core::context::SipTransport::Tcp,
            "[2001:db8::1]:5088".parse().unwrap(), 1, 2);
    }).await;
    let status = runtime.snapshot().await.status_response();
    assert!(status.diagnostics.observed);
    assert_eq!(status.diagnostics.profile_id.as_deref(), Some(profile().meta.profile_id));
    assert_eq!(status.diagnostics.identity_source, IdentityOrigin::Isim);
    assert_eq!(status.diagnostics.domain_source, FactSource::Isim);
    assert_eq!(status.diagnostics.requested_family, IpFamily::Ipv4v6);
    assert_eq!(status.diagnostics.granted_family, IpFamily::Ipv6);
    assert_eq!(status.diagnostics.pcscf_port, Some(5088));
    assert!(!status.diagnostics.negotiated_from_success);
    assert_eq!(status.readiness, before.status_response().readiness, "telemetry does not change registration authority");
}

#[tokio::test]
async fn replaced_attempt_and_reset_generation_reject_old_wifi_diagnostic_writers() {
    let (line, runtime) = runtime();
    let old = Scope::begin(&line, profile()).await;
    let new = Scope::begin(&line, profile()).await;
    old.update(|state| state.facts.profile_id = Some("late-old".into())).await;
    assert_eq!(runtime.snapshot().await.status_response().diagnostics.profile_id.as_deref(), Some(profile().meta.profile_id));
    new.update(|state| state.facts.granted_family = IpFamily::Ipv4).await;
    assert_eq!(runtime.snapshot().await.status_response().diagnostics.granted_family, IpFamily::Ipv4);
    runtime.reset_runtime("new_subscription").await;
    new.update(|state| state.wait_stopped()).await;
    assert!(!runtime.snapshot().await.status_response().diagnostics.observed);
}

#[tokio::test]
async fn destroyed_runtime_cannot_publish_into_a_new_runtime_with_the_same_line_id() {
    let (line, original) = runtime();
    let old = Scope::begin(&line, profile()).await;
    drop(original);
    let replacement = Arc::new(VowifiRuntime::for_line(&line));
    super::super::reg_event_live::bind_reg_event_runtime(&line, &replacement);
    old.update(|state| state.profile("stale-runtime", FactSource::Database, "derived")).await;
    assert!(!replacement.snapshot().await.status_response().diagnostics.observed);
}

#[tokio::test]
async fn ike_authentication_rejection_is_not_misreported_as_an_ordinary_bearer_failure() {
    let (line, runtime) = runtime();
    let scope = Scope::begin(&line, profile()).await;
    scope.failure(&live_stage_error("ike_auth_notify_authentication_failed"), FailureScope::Bearer).await;
    let failure = runtime.snapshot().await.status_response().diagnostics.failure.unwrap();
    assert_eq!(failure.scope, FailureScope::Authentication);
    assert_eq!(failure.sip_status, None, "IKE failure must not fabricate a SIP response");
}

#[tokio::test]
async fn wifi_rspauth_failure_projects_actual_200_not_the_previous_401() {
    let (line, runtime) = runtime();
    let scope = Scope::begin(&line, profile()).await;
    let failure = response_auth_error(ImsError::new("digest_response_auth_mismatch"),
        b"SIP/2.0 200 OK\r\nAuthentication-Info: rspauth=\"redacted\"\r\nContent-Length: 0\r\n\r\n");
    scope.failure(&failure, FailureScope::Endpoint).await;
    let report = runtime.snapshot().await.status_response().diagnostics;
    let failure = report.failure.unwrap();
    assert_eq!(failure.sip_status, Some(200));
    assert_eq!(failure.auth_stage, Some(AuthStage::Authenticated));
    assert_eq!(failure.scope, FailureScope::Authentication);
    assert!(!serde_json::to_string(&failure).unwrap().contains("redacted"));
}

#[tokio::test]
async fn wifi_reg_event_wait_projection_preserves_deadline_and_never_performs_recovery() {
    let (line, runtime) = runtime();
    runtime.seed_registered_for_reg_event_test().await;
    network_wait(&line, Some(Instant::now() + Duration::from_secs(120))).await;
    let state = runtime.snapshot().await;
    let report = state.status_response().diagnostics;
    assert_eq!(report.wait.status, WaitStatus::Waiting);
    assert!(report.wait.retry_after_seconds.unwrap() >= 119);
    assert_eq!(report.failure.unwrap().scope, FailureScope::Binding);
    assert!(state.live_readiness.ims_registered, "only the real reg-event callback may revoke readiness");
    network_wait(&line, None).await;
    assert_eq!(runtime.snapshot().await.status_response().diagnostics.wait.status, WaitStatus::Stopped);
}

#[tokio::test]
async fn unknown_runtime_diagnostics_remain_unobserved_without_creating_runtime() {
    let scope = Scope::begin("n7-no-runtime", profile()).await;
    assert!(scope.runtime.is_none() && scope.token.is_none());
    scope.update(|_| panic!("missing runtime must not publish or start work")).await;
}

#[tokio::test]
async fn stage_resuming_after_reset_cannot_begin_a_new_epoch_diagnostic_attempt() {
    let (line, runtime) = runtime();
    run_with_owner(&runtime, 0, async {
        runtime.reset_runtime("new_subscription_during_stage").await;
        let late = Scope::begin(&line, profile()).await;
        late.update(|state| state.profile("late-stage", FactSource::Database, "isim")).await;
        assert!(late.token.is_none());
    }).await;
    assert!(!runtime.snapshot().await.status_response().diagnostics.observed);
}

#[tokio::test]
async fn stage_owner_does_not_attach_to_replacement_runtime_with_matching_line_and_epoch() {
    let (line, original) = runtime();
    let replacement = Arc::new(VowifiRuntime::for_line(&line));
    super::super::reg_event_live::bind_reg_event_runtime(&line, &replacement);
    run_with_owner(&original, 0, async {
        let late = Scope::for_profile(&line, profile()).await;
        assert!(late.token.is_none());
        late.update(|_| panic!("old owner must not claim a replacement runtime")).await;
    }).await;
    assert!(!replacement.snapshot().await.status_response().diagnostics.observed);
}
