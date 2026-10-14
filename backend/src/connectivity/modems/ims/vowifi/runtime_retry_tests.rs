//! Retry control evidence is separate from passive execution diagnostics.
use super::*;
use crate::connectivity::core::register_retry::RetryAdmission;
use super::super::executor::RegisterRecovery;

fn failure(expiry: Option<Instant>, attempted: bool) -> ExecutorStageResult {
    ExecutorStageResult { stage: "ims_register", status: "failed", readiness_key: "ims_registered",
        reason: Some("ims_register_initial_unexpected_status".into()), soak_observation: None,
        register_recovery: Some(RegisterRecovery {
            admission: RetryAdmission::Deferred { not_before: Instant::now() + Duration::from_secs(120) },
            retained_until: expiry, attempted,
        }),
    }
}

#[tokio::test]
async fn temporary_refresh_restores_only_existing_services_until_original_expiry() {
    let runtime = VowifiRuntime::for_line("r2-runtime-retention");
    runtime.seed_registered_for_reg_event_test().await;
    runtime.snapshot.write().await.live_readiness.voice_ready = false;
    runtime.prepare_live_ims_registration_refresh("vowifi_registration_refresh_due").await;
    let expiry = Instant::now() + Duration::from_secs(60);
    runtime.snapshot.write().await.apply_stage_result(ExecutorStage::ImsRegister, &failure(Some(expiry), true));
    let snapshot = runtime.snapshot().await;
    assert!(snapshot.readiness().ims_registered && snapshot.readiness().sms_ready);
    assert!(!snapshot.readiness().voice_ready, "a successful voice stage was never observed");
    assert_eq!(snapshot.live_readiness.retained_until, Some(expiry));
    assert!(snapshot.status_response().register_recovery.unwrap().attempted);
    let json = serde_json::to_string(&snapshot.status_response()).unwrap();
    assert!(!json.contains("register_recovery") && !json.contains("retained_until"));
    runtime.snapshot.write().await.live_readiness.retained_until = Some(Instant::now() - Duration::from_secs(1));
    let expired = runtime.snapshot().await;
    assert!(!expired.live_readiness.ims_registered && !expired.readiness().sms_ready);
    assert!(expired.live_readiness.esp_ready, "lease expiry is not evidence that ESP disappeared");
}

#[tokio::test]
async fn diagnostic_history_cannot_preserve_registration_without_operator_lease_evidence() {
    let runtime = VowifiRuntime::for_line("r2-runtime-no-estimates");
    runtime.seed_registered_for_reg_event_test().await;
    let token = runtime.begin_ims_diagnostics().await.unwrap();
    runtime.observe_ims_diagnostics(token, |state| {
        state.facts.negotiated_from_success = true;
        state.wait_until(Instant::now() + Duration::from_secs(120));
    }).await;
    runtime.prepare_live_ims_registration_refresh("vowifi_registration_refresh_due").await;
    runtime.snapshot.write().await.apply_stage_result(ExecutorStage::ImsRegister, &failure(None, true));
    let snapshot = runtime.snapshot().await;
    assert!(!snapshot.readiness().ims_registered);
    assert!(snapshot.status_response().diagnostics.negotiated_from_success);
}

#[tokio::test]
async fn late_refresh_stage_cannot_republish_a_reg_event_revoked_binding() {
    let runtime = VowifiRuntime::for_line("r2-runtime-revoked");
    runtime.seed_registered_for_reg_event_test().await;
    runtime.prepare_live_ims_registration_refresh("vowifi_registration_refresh_due").await;
    let generation = runtime.live_generation.load(Ordering::SeqCst);
    let revocation_serial = runtime.registration_revocation_serial.load(Ordering::SeqCst);
    let mut late = runtime.snapshot().await;
    late.apply_stage_result(ExecutorStage::ImsRegister, &failure(Some(Instant::now() + Duration::from_secs(60)), true));
    assert!(late.readiness().ims_registered);
    runtime.revoke_live_ims_registration("ims_reg_event_binding_terminated").await;
    let current = runtime.publish_live_stages(generation, revocation_serial, late).await;
    assert!(!current.readiness().ims_registered && !current.readiness().sms_ready);
    assert_eq!(current.degraded_reason.as_deref(), Some("ims_reg_event_binding_terminated"));
    assert!(current.live_readiness.retained_until.is_none());
}

#[tokio::test]
async fn reset_or_never_registered_runtime_does_not_accept_retention_from_an_old_lease() {
    let runtime = VowifiRuntime::for_line("r2-runtime-reset");
    runtime.seed_registered_for_reg_event_test().await;
    runtime.prepare_live_ims_registration_refresh("vowifi_registration_refresh_due").await;
    runtime.reset_runtime("new_subscription").await;
    runtime.snapshot.write().await.apply_stage_result(ExecutorStage::ImsRegister,
        &failure(Some(Instant::now() + Duration::from_secs(60)), false));
    assert!(!runtime.snapshot().await.readiness().ims_registered);
}
