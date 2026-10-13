//! Pure projection/ownership tests. These observations never drive recovery.
use super::*;
use super::super::register::{RetryAfter, RegisterFailureKind, RegisterFailureStage};

fn token(serial: u64, generation: u64) -> DiagnosticsToken { DiagnosticsToken { serial, generation } }
fn metadata(kind: RegisterFailureKind, status: u16) -> RegisterFailureMetadata {
    RegisterFailureMetadata { kind, sip_status: Some(status), stage: RegisterFailureStage::Authenticated,
        retry_after: RetryAfter::Absent, min_expires: None }
}

#[test]
fn empty_projection_reports_unknown_not_profile_estimates() {
    let mut state = DiagnosticsState::default();
    state.profile("not-executed", FactSource::Derived, "isim");
    let report = state.report();
    assert_eq!(report.version, 1);
    assert!(!report.observed && !report.negotiated_from_success);
    assert_eq!(report.profile_id, None);
    assert_eq!(report.identity_source, IdentityOrigin::Unknown);
    assert_eq!(report.requested_family, IpFamily::Unknown);
    assert_eq!(report.wait.status, WaitStatus::Unknown);
}

#[test]
fn late_attempt_and_old_generation_cannot_overwrite_current_facts() {
    let mut state = DiagnosticsState::default(); let old = token(1, 3); state.begin(old);
    assert!(state.apply(old, 3, |state| state.profile("first", FactSource::Database, "derived")));
    let current = token(2, 3); state.begin(current);
    assert!(!state.apply(old, 3, |state| state.profile("stale", FactSource::CarrierCatalog, "isim")));
    assert!(state.apply(current, 3, |state| state.profile("current", FactSource::Derived, "isim")));
    assert!(!state.apply(current, 4, |state| state.wait_stopped()));
    assert_eq!(state.report().profile_id.as_deref(), Some("current"));
    state.discard_stale(4);
    assert!(!state.report().observed);
    assert!(!state.apply(old, 4, |state| state.wait_ready()));
}

#[test]
fn delayed_begin_or_duplicate_token_cannot_reset_a_newer_attempt() {
    let mut state = DiagnosticsState::default();
    assert!(state.begin(token(4, 2)));
    state.profile("newer", FactSource::Database, "isim");
    assert!(!state.begin(token(3, 2)));
    assert!(!state.begin(token(4, 2)));
    assert!(!state.begin(token(99, 1)));
    assert_eq!(state.report().profile_id.as_deref(), Some("newer"));
    assert!(state.begin(token(5, 2)));
    assert_eq!(state.report().profile_id, None);
}

#[test]
fn actual_isim_and_user_override_sources_do_not_follow_declared_template_labels() {
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0));
    state.profile("profile", FactSource::Database, "carrier_device_model");
    state.identity(IdentityOrigin::Isim, FactSource::Isim, FactSource::SimOverride);
    let report = state.report();
    assert_eq!(report.identity_source, IdentityOrigin::Isim);
    assert_eq!(report.identity_rule_status, IdentityRuleStatus::DeclaredOnly);
    assert_eq!(report.domain_source, FactSource::Isim);
    assert_eq!(report.realm_source, FactSource::SimOverride);
    state.profile("derived-profile", FactSource::Derived, "isim");
    state.identity(IdentityOrigin::UsimDerived, FactSource::Derived, FactSource::Derived);
    assert_eq!(state.report().identity_rule_status, IdentityRuleStatus::StandardDerived);
    assert_eq!(state.report().identity_source, IdentityOrigin::UsimDerived);
}

#[test]
fn requested_dual_stack_does_not_fabricate_a_dual_stack_grant() {
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0));
    state.facts.requested_family = IpFamily::parse("ipv4v6");
    state.facts.granted_family = IpFamily::granted(false, true);
    state.endpoint(FactSource::BearerPco, SipTransport::Tcp, "[2001:db8::1]:5080".parse().unwrap(), 2, 3);
    let report = state.report();
    assert_eq!(report.requested_family, IpFamily::Ipv4v6);
    assert_eq!(report.granted_family, IpFamily::Ipv6);
    assert_eq!(report.signaling_family, IpFamily::Ipv6);
    assert_eq!(report.pcscf_port, Some(5080));
    assert!(!report.socket_ready);
}

#[test]
fn offered_and_successfully_negotiated_algorithms_remain_distinct_and_redacted() {
    let offer = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;spi-c=33333;auth_key=SECRET_KEY,ipsec-3gpp;alg=hmac-sha1-96;ealg=null;nonce=SECRET_NONCE";
    let chosen = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=null;spi-c=44444";
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0));
    state.offer(Some(offer));
    assert_eq!(state.report().offered_ipsec.as_ref().unwrap().len(), 2);
    assert!(state.report().negotiated_ipsec.is_empty());
    assert!(!state.report().negotiated_from_success);
    state.accepted(SipTransport::Tcp, Some(chosen), Some("AKAv2-SHA-256"), Some(false));
    assert_eq!(state.report().negotiated_ipsec.len(), 1);
    assert_eq!(state.report().negotiated_ipsec[0].encryption, EncryptionAlgorithm::Null);
    assert_eq!(state.report().digest_algorithm, Some(DigestAlgorithm::Akav2Sha256));
    assert_eq!(state.report().rspauth_verified, Some(false));
    let json = serde_json::to_string(&state.report()).unwrap();
    for forbidden in ["SECRET_KEY", "SECRET_NONCE", "33333", "44444", "spi-c", "auth_key"] { assert!(!json.contains(forbidden)); }
    for invalid in ["ipsec-3gpp;alg=secret-unknown;ealg=aes-cbc", "ipsec-3gpp;alg=hmac-sha1-96;alg=hmac-md5-96;ealg=null", "ipsec-3gpp;alg=hmac-sha1-96;ealg=unknown"] {
        assert!(ipsec_algorithms(invalid).is_empty());
    }
}

#[test]
fn tls_confirmation_requires_observation_and_does_not_claim_register_success() {
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0));
    state.endpoint(FactSource::Dns, SipTransport::Tls, "192.0.2.10:5061".parse().unwrap(), 1, 1);
    assert_eq!(state.report().tls_verification, TlsVerification::Pending);
    state.established_tls(Some(TlsAlgorithms { version: Some("TLSv1_3".into()), cipher_suite: Some("TLS13_AES_128_GCM_SHA256".into()) }));
    assert_eq!(state.report().tls_verification, TlsVerification::Verified);
    assert!(!state.report().negotiated_from_success);
    state.accepted(SipTransport::Tls, None, Some("AKAv1-MD5"), None);
    assert_eq!(state.report().security_mode, SecurityMode::Tls);
    assert!(state.report().negotiated_from_success);
}

#[test]
fn actual_wait_is_monotonic_not_rebuilt_from_a_response_header_on_every_read() {
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0));
    let now = Instant::now(); state.wait_until(now + Duration::from_secs(120));
    let first = state.report_at(now); let later = state.report_at(now + Duration::from_secs(90));
    assert_eq!(first.wait.retry_after_seconds, Some(120));
    assert_eq!(later.wait.retry_after_seconds, Some(30));
    assert_eq!(first.wait.not_before, later.wait.not_before);
    assert_eq!(state.report_at(now + Duration::from_secs(121)).wait.status, WaitStatus::Ready);
    state.wait_stopped();
    assert_eq!(state.report_at(now + Duration::from_secs(99999)).wait.status, WaitStatus::Stopped);
    assert_eq!(state.report().wait.retry_after_seconds, None);
}

#[test]
fn received_retry_after_is_distinct_from_the_actually_enforced_gate() {
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0)); state.wait_ready();
    let mut failure = metadata(RegisterFailureKind::TemporaryEndpoint, 503); failure.retry_after = RetryAfter::DelaySeconds(300);
    state.failure("ims_register_initial_unexpected_status", Some(failure));
    let report = state.report();
    assert_eq!(report.failure.unwrap().response_retry_after_seconds, Some(300));
    assert_eq!(report.wait.status, WaitStatus::Ready, "a received header alone is not evidence of a scheduled wait");
    state.wait_until(Instant::now() + Duration::from_secs(300));
    assert_eq!(state.report().wait.status, WaitStatus::Waiting);
}

#[test]
fn failure_projection_preserves_authenticated_status_and_drops_raw_detail() {
    let mut state = DiagnosticsState::default(); state.begin(token(1, 0));
    state.facts.procedure = RegistrationProcedure::Refresh;
    state.endpoint(FactSource::Profile, SipTransport::Udp, "192.0.2.1:5060".parse().unwrap(), 1, 2);
    state.failure("digest_response_auth_mismatch:SECRET-IMPI:nonce=SECRET", Some(metadata(RegisterFailureKind::AuthenticationRejected, 200)));
    let failure = state.report().failure.unwrap();
    assert_eq!(failure.scope, FailureScope::Authentication);
    assert_eq!(failure.auth_stage, Some(AuthStage::Authenticated));
    assert_eq!(failure.procedure, RegistrationProcedure::Refresh);
    assert_eq!(failure.endpoint_index, Some(1));
    assert_eq!(failure.sip_status, Some(200));
    let json = serde_json::to_string(&failure).unwrap(); assert!(!json.contains("SECRET"));
    state.endpoint(FactSource::Dns, SipTransport::Udp, "192.0.2.2:5060".parse().unwrap(), 2, 2);
    assert_eq!(state.report().failure.unwrap().endpoint_index, Some(1), "old failure does not silently move to the next endpoint");
    state.start_discovery(); assert_eq!(state.report().endpoint_index, None);
}

#[test]
fn unsafe_identity_shaped_reasons_are_not_echoed_as_reason_codes() {
    for reason in ["sip:private@example", "123456789012345", "ims_bad\r\nAuthorization", "ims_bad secret"] {
        assert_eq!(FailureFact::new(reason, None).reason_code, None);
    }
    assert_eq!(FailureFact::new("ims_uicc_binding_changed", None).scope, FailureScope::Ownership);
}
