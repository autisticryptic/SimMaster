//! Runtime projection isolation: no SIM/device/network operations here.
use super::*;
use crate::connectivity::core::{registration_diagnostics as diag, register::{RegisterFailureMetadata, RegisterFailureKind, RegisterFailureStage, RetryAfter}};

#[tokio::test]
async fn cellular_diagnostic_projection_does_not_mutate_registration_or_counters() {
    let runtime = CellularImsRuntime::new();
    let before = runtime.status().await;
    let token = runtime.begin_diagnostics().await;
    let writer = runtime.with_diagnostics(token);
    writer.observe_diagnostics(|state| {
        state.profile("actual-profile", diag::FactSource::Database, "carrier_device_model");
        state.identity(diag::IdentityOrigin::Isim, diag::FactSource::Isim, diag::FactSource::SimOverride);
        state.facts.requested_family = diag::IpFamily::Ipv4v6;
        state.facts.granted_family = diag::IpFamily::Ipv4;
        state.endpoint(diag::FactSource::BearerPco, crate::connectivity::core::context::SipTransport::Udp,
            "192.0.2.1:5078".parse().unwrap(), 1, 2);
    }).await;
    let status = runtime.status().await;
    assert_eq!(status.registered, before.registered);
    assert_eq!(status.sent_count, before.sent_count);
    assert_eq!(status.reconnect_count, before.reconnect_count);
    assert_eq!(status.profile_id, before.profile_id);
    assert_eq!(status.diagnostics.profile_id.as_deref(), Some("actual-profile"));
    assert_eq!(status.diagnostics.identity_source, diag::IdentityOrigin::Isim);
    assert_eq!(status.diagnostics.identity_rule_status, diag::IdentityRuleStatus::DeclaredOnly);
    assert_eq!(status.diagnostics.requested_family, diag::IpFamily::Ipv4v6);
    assert_eq!(status.diagnostics.granted_family, diag::IpFamily::Ipv4);
    assert_eq!(status.diagnostics, runtime.snapshot().await.diagnostics.report());
}

#[tokio::test]
async fn cellular_diagnostic_tokens_survive_neither_replacement_attempt_nor_runtime_reset() {
    let runtime = CellularImsRuntime::new();
    let old = runtime.with_diagnostics(runtime.begin_diagnostics().await);
    let current = runtime.with_diagnostics(runtime.begin_diagnostics().await);
    old.observe_diagnostics(|state| state.profile("old", diag::FactSource::Database, "derived")).await;
    assert_eq!(runtime.status().await.diagnostics.profile_id, None);
    current.observe_diagnostics(|state| state.profile("new", diag::FactSource::Derived, "isim")).await;
    assert_eq!(runtime.status().await.diagnostics.profile_id.as_deref(), Some("new"));
    runtime.reset_runtime("new_card").await;
    current.observe_diagnostics(|state| state.wait_stopped()).await;
    assert!(!runtime.status().await.diagnostics.observed);
    let next = runtime.with_diagnostics(runtime.begin_diagnostics().await);
    old.observe_diagnostics(|state| state.profile("very-old", diag::FactSource::Database, "isim")).await;
    next.observe_diagnostics(|state| state.profile("next", diag::FactSource::Derived, "isim")).await;
    assert_eq!(runtime.status().await.diagnostics.profile_id.as_deref(), Some("next"));
}

#[tokio::test]
async fn cellular_diagnostic_error_adapter_uses_canonical_tls_and_owner_classifications() {
    let runtime = CellularImsRuntime::new();
    let writer = runtime.with_diagnostics(runtime.begin_diagnostics().await);
    for code in [super::super::errors::code::TLS_HANDSHAKE_FAILED, super::super::errors::code::TLS_AUTHORITY_INVALID,
        super::super::errors::code::TLS_HANDSHAKE_TIMEOUT] {
        let error = CellularImsError::new(code);
        writer.record_attempt(CellularImsStage::Pcscf, None, "failed", Some(&error), None).await;
        assert_eq!(runtime.status().await.diagnostics.tls_verification, diag::TlsVerification::Failed);
    }
    let error = CellularImsError::new(super::super::errors::code::RUNTIME_UE_WORKER_GENERATION_CHANGED);
    writer.record_attempt(CellularImsStage::RegisterRefresh, None, "failed", Some(&error), None).await;
    assert_eq!(runtime.status().await.diagnostics.failure.unwrap().scope, diag::FailureScope::Ownership);
}

#[tokio::test]
async fn cellular_structured_failure_and_network_wait_reach_api_without_secret_detail() {
    let runtime = CellularImsRuntime::new();
    let writer = runtime.with_diagnostics(runtime.begin_diagnostics().await);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let error = super::super::errors::CellularImsError::with_detail(super::super::errors::code::REGISTER_AUTH_UNEXPECTED_STATUS,
        "SECRET_IMPI_123456789:Authorization=SECRET_NONCE")
        .with_register_failure(RegisterFailureMetadata {
            kind: RegisterFailureKind::TemporaryEndpoint, stage: RegisterFailureStage::Authenticated,
            sip_status: Some(503), retry_after: RetryAfter::DelaySeconds(120), min_expires: None,
        }).with_retry_not_before(deadline);
    writer.record_attempt(CellularImsStage::RegisterRefresh, Some("ipv4"), "failed", Some(&error), None).await;
    let status = runtime.status().await;
    let diagnostics = status.diagnostics;
    assert_eq!(diagnostics.failure.as_ref().unwrap().scope, diag::FailureScope::Endpoint);
    assert_eq!(diagnostics.failure.as_ref().unwrap().sip_status, Some(503));
    assert_eq!(diagnostics.wait.status, diag::WaitStatus::Waiting);
    assert!(diagnostics.wait.retry_after_seconds.unwrap() >= 119);
    assert_eq!(diagnostics.failure.as_ref().unwrap().response_retry_after_seconds, Some(120));
    let json = serde_json::to_string(&diagnostics).unwrap();
    assert!(!json.contains("SECRET") && !json.contains("123456789"));
}
