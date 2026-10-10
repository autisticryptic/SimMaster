use super::*;
use crate::connectivity::core::{
    access::ImsChannel,
    context::{ImsRoute, SipTransport},
    ims_failure::{parse_retry_after, RetryAfterInvalid},
    register::{run_register_observed, RegisterAuthenticator, RegisterFailure, RegisterFailureStage},
    ImsError,
};
use std::collections::VecDeque;

fn endpoint(port: u16) -> SocketAddr {
    SocketAddr::from(([192, 0, 2, 1], port))
}

fn failure(status: u16, headers: &str, auth_rounds: u8) -> RegisterFailure {
    RegisterFailure {
        error: ImsError::new(if auth_rounds == 0 {
            "ims_register_initial_unexpected_status"
        } else {
            "ims_register_authenticated_unexpected_status"
        }),
        response: Some(format!("SIP/2.0 {status} Failure\r\n{headers}\r\n").into_bytes()),
        auth_rounds,
    }
}

fn parsed(value: &str) -> RetryAfter {
    parse_retry_after(format!("SIP/2.0 503 Unavailable\r\nRetry-After: {value}\r\n\r\n").as_bytes())
}

#[test]
fn register_retry_parser_accepts_seconds_comment_parameters_and_folding() {
    for value in [
        "30", " 30\t", "00030", "30 (maintenance)",
        "30 (nested (busy) and \\) escaped)",
        "30;duration=60", "30 (busy);duration=60;reason=overload;flag",
        "30;reason=\"busy; try later\";host=[2001:db8::1]",
        "30\r\n\t(maintenance)\r\n ; duration = 60",
    ] {
        assert_eq!(parsed(value), RetryAfter::DelaySeconds(30), "{value:?}");
    }
    assert_eq!(parsed("0"), RetryAfter::DelaySeconds(0));
    assert_eq!(parsed("4294967295"), RetryAfter::DelaySeconds(u32::MAX));
    assert_eq!(parse_retry_after(b"SIP/2.0 503 Unavailable\r\nretry-after : 30\r\n\r\n"),
        RetryAfter::DelaySeconds(30));
}

#[test]
fn register_retry_parser_rejects_dates_signs_lists_garbage_and_overflow() {
    for value in [
        "", "+30", "-30", "busy30", "Wed, 21 Oct 2015 07:28:00 GMT", "1.5",
        "30, 60", "30 60", "30 garbage", "30;", "30;;x", "30;x=", "30;=x",
        "30;duration", "30;duration=-1", "30;duration=\"60\"", "30;duration=60x",
        "30;duration=1;Duration=2", "30;x=1;X=2", "30 (unterminated",
        "30 (one) (two)", "30 (((((too deep)))))", "30;x=\"unterminated",
        "30;x=[not-an-ip]", "30;x=[2001:db8::1", "30\u{7f}", "30\u{b}",
        "4294967296", "18446744073709551616", "30;duration=4294967296",
    ] {
        assert!(matches!(parsed(value), RetryAfter::Invalid(_)), "{value:?}");
        assert_ne!(parsed(value), RetryAfter::DelaySeconds(0));
    }
    assert_eq!(parsed("4294967296"), RetryAfter::Invalid(RetryAfterInvalid::Overflow));
}

#[test]
fn register_retry_parser_rejects_duplicate_headers_and_combined_values() {
    for headers in [
        "Retry-After: 30\r\nRetry-After: 30\r\n",
        "Retry-After: 0\r\nretry-after: 60\r\n",
        "Retry-After: 60\r\nX-Test: ok\r\nRetry-After: 0\r\n",
    ] {
        assert_eq!(failure(503, headers, 0).metadata().retry_after,
            RetryAfter::Invalid(RetryAfterInvalid::Duplicate));
    }
    assert!(matches!(parsed("0, 60"), RetryAfter::Invalid(_)));
}

#[test]
fn register_retry_parser_bounds_headers_values_parameters_and_body() {
    assert_eq!(parsed(&format!("30 ({})", "x".repeat(1024))),
        RetryAfter::Invalid(RetryAfterInvalid::ValueTooLarge));
    let huge = format!("SIP/2.0 503 Unavailable\r\nX-Huge: {}\r\nRetry-After: 0\r\n\r\n", "x".repeat(32768));
    assert_eq!(parse_retry_after(huge.as_bytes()), RetryAfter::Invalid(RetryAfterInvalid::HeadersTooLarge));
    let many = format!("SIP/2.0 503 Unavailable\r\n{}Retry-After: 0\r\n\r\n", "X: a\r\n".repeat(130));
    assert_eq!(parse_retry_after(many.as_bytes()), RetryAfter::Invalid(RetryAfterInvalid::TooManyHeaders));
    let many_params = format!("30{}", (0..17).map(|i| format!(";p{i}=a")).collect::<String>());
    assert!(matches!(parsed(&many_params), RetryAfter::Invalid(_)));
    assert_eq!(parse_retry_after(b"SIP/2.0 503 Unavailable\r\n\r\nRetry-After: 0\r\n"), RetryAfter::Absent);
    assert!(matches!(parse_retry_after(b"SIP/2.0 503 Unavailable\r\nRetry-After: 30"), RetryAfter::Invalid(_)));
    assert!(matches!(parse_retry_after(b"SIP/2.0 503 Unavailable\r\nRetry-After: 30 (\xff)\r\n\r\n"), RetryAfter::Invalid(_)));
}

#[test]
fn register_retry_metadata_classifies_status_stage_without_warning_or_detail() {
    for status in [408, 500, 502, 503, 504] {
        for rounds in [0, 1] {
            let metadata = failure(status, "Retry-After: 30\r\nWarning: 399 peer \"not subscribed\"\r\n", rounds).metadata();
            assert_eq!(metadata.kind, RegisterFailureKind::TemporaryEndpoint);
            assert_eq!(metadata.sip_status, Some(status));
            assert_eq!(metadata.stage, if rounds == 0 { RegisterFailureStage::Initial } else { RegisterFailureStage::Authenticated });
            assert!(!metadata.permits_variant_fallback());
        }
    }
    for status in [402, 403, 404, 604] {
        assert_eq!(failure(status, "", 0).metadata().kind, RegisterFailureKind::IdentityRejected);
    }
    for status in [401, 407] {
        assert_eq!(failure(status, "", 2).metadata().kind, RegisterFailureKind::AuthenticationRejected);
    }
    for status in [400, 415, 420, 421, 494] {
        assert!(failure(status, "", 0).metadata().permits_variant_fallback());
        assert!(!failure(status, "", 1).metadata().permits_variant_fallback());
        assert!(!failure(status, "Retry-After: 30\r\n", 0).metadata().permits_variant_fallback());
    }
    assert!(!failure(423, "Min-Expires: 600\r\n", 0).metadata().permits_variant_fallback());
    assert_eq!(failure(423, "", 0).metadata().kind, RegisterFailureKind::Negotiation);
}

#[test]
fn register_retry_metadata_keeps_saved_challenge_distinct_from_send_outcome() {
    let saved = failure(401, "Retry-After: 90\r\n", 1).response;
    let transport = RegisterFailure {
        error: ImsError::new("ims_register_authenticated_receive_failed"),
        response: saved.clone(), auth_rounds: 1,
    }.metadata();
    assert_eq!(transport.kind, RegisterFailureKind::Transport);
    assert_eq!(transport.stage, RegisterFailureStage::Authenticated);
    assert_eq!(transport.sip_status, Some(401));
    let local = RegisterFailure {
        error: ImsError::new(crate::connectivity::modems::ims::cellular_ims::errors::code::SECURITY_SERVER_INVALID),
        response: saved, auth_rounds: 1,
    }.metadata();
    assert_eq!(local.kind, RegisterFailureKind::LocalFailure);
    assert_eq!(local.stage, RegisterFailureStage::Authentication);
    assert!(!local.permits_variant_fallback());
}

#[test]
fn register_retry_exhausted_batch_gates_bearer_restart_without_clearing_endpoint_waits() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    state.observe_failure(endpoint(5060), &failure(503, "Retry-After: 60\r\n", 0).metadata(), now);
    // During the current discovery batch, a distinct alternate is still usable.
    assert_eq!(state.restart_admission(now), RetryAdmission::Allowed);
    assert!(state.check(endpoint(5070), now).is_allowed());
    assert_eq!(state.pause_restart(now), RetryAdmission::Deferred { not_before: now + Duration::from_secs(60) });
    assert!(!state.restart_admission(now + Duration::from_secs(59)).is_allowed());
    assert!(state.restart_admission(now + Duration::from_secs(60)).is_allowed());
    state.observe_failure(endpoint(5060), &failure(503, "Retry-After: 60\r\n", 0).metadata(), now);
    state.pause_restart(now);
    state.registered();
    assert!(state.restart_admission(now).is_allowed());
    assert!(!state.check(endpoint(5060), now).is_allowed());
}

#[test]
fn register_retry_endpoint_deadline_survives_profile_and_header_changes() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let first = state.observe_failure(endpoint(5060), &failure(503, "Retry-After: 60\r\n", 0).metadata(), now);
    assert_eq!(first, RetryAdmission::Deferred { not_before: now + Duration::from_secs(60) });
    // Profile/source/variant labels are deliberately absent from the key/API.
    for _profile in ["catalog", "custom", "derived"] {
        for _variant in ["original", "without-pani", "without-route"] {
            assert!(!state.check(endpoint(5060), now + Duration::from_secs(59)).is_allowed());
        }
    }
    assert!(state.check(endpoint(5070), now).is_allowed());
    assert!(state.check(endpoint(5060), now + Duration::from_secs(60)).is_allowed());
}

#[test]
fn register_retry_new_observation_never_shortens_existing_deadline() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    state.observe_failure(endpoint(5060), &failure(503, "Retry-After: 120\r\n", 0).metadata(), now);
    for seconds in [0, 1, 30] {
        let later = state.observe_failure(endpoint(5060), &failure(503, &format!("Retry-After: {seconds}\r\n"), 0).metadata(), now + Duration::from_secs(5));
        assert_eq!(later, RetryAdmission::Deferred { not_before: now + Duration::from_secs(120) });
    }
}

#[test]
fn register_retry_absent_and_legal_zero_are_bounded_nonzero_delays() {
    let now = Instant::now();
    for (header, expected) in [("", 30), ("Retry-After: 0\r\n", 1)] {
        let mut state = EndpointRetryState::default();
        let admission = state.observe_failure(endpoint(5060), &failure(503, header, 0).metadata(), now);
        assert_eq!(admission.remaining(now), Some(Duration::from_secs(expected)));
    }
}

#[test]
fn register_retry_invalid_or_unrepresentable_delay_stops_without_short_clamp() {
    let now = Instant::now();
    for (value, reason) in [
        ("-1", RetryStopReason::InvalidRetryAfter),
        ("4294967296", RetryStopReason::InvalidRetryAfter),
        ("86401", RetryStopReason::RetryAfterTooLong),
        ("4294967295", RetryStopReason::RetryAfterTooLong),
    ] {
        let mut state = EndpointRetryState::default();
        let metadata = failure(503, &format!("Retry-After: {value}\r\n"), 0).metadata();
        assert_eq!(state.observe_failure(endpoint(5060), &metadata, now), RetryAdmission::Stopped(reason));
        assert_eq!(state.check(endpoint(5060), now + MAX_ENDPOINT_RETRY_DELAY * 2), RetryAdmission::Stopped(reason));
        assert!(state.check(endpoint(5070), now).is_allowed());
        assert!(matches!(state.observe_failure(endpoint(5060), &failure(503, "Retry-After: 0\r\n", 0).metadata(), now), RetryAdmission::Stopped(_)));
    }
}

#[test]
fn register_retry_capacity_overflow_stops_scope_instead_of_evicting() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let metadata = failure(503, "Retry-After: 60\r\n", 0).metadata();
    for i in 0..MAX_ENDPOINT_RETRY_ENTRIES {
        assert!(matches!(state.observe_failure(endpoint(5060 + i as u16), &metadata, now), RetryAdmission::Deferred { .. }));
    }
    assert_eq!(state.entries.len(), MAX_ENDPOINT_RETRY_ENTRIES);
    assert_eq!(state.observe_failure(endpoint(6000), &metadata, now), RetryAdmission::Stopped(RetryStopReason::CapacityExceeded));
    assert_eq!(state.entries.len(), MAX_ENDPOINT_RETRY_ENTRIES);
    assert_eq!(state.check(endpoint(7000), now + Duration::from_secs(120)), RetryAdmission::Stopped(RetryStopReason::CapacityExceeded));
}

#[test]
fn register_retry_expired_entries_are_pruned_before_capacity_is_checked() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let metadata = failure(503, "Retry-After: 1\r\n", 0).metadata();
    for i in 0..MAX_ENDPOINT_RETRY_ENTRIES {
        state.observe_failure(endpoint(5060 + i as u16), &metadata, now);
    }
    assert!(matches!(state.observe_failure(endpoint(6000), &metadata, now + Duration::from_secs(1)), RetryAdmission::Deferred { .. }));
    assert_eq!(state.entries.len(), 1);
}

#[test]
fn register_retry_transport_has_own_backoff_not_saved_challenge_retry_after() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let mut transport = failure(401, "Retry-After: 900\r\n", 1);
    transport.error = ImsError::new("ims_register_authenticated_receive_failed");
    assert_eq!(state.observe_failure(endpoint(5060), &transport.metadata(), now).remaining(now), Some(TRANSPORT_RETRY_DELAY));
}

#[test]
fn register_retry_temporary_refresh_retains_lease_and_retries_same_flow() {
    let now = Instant::now();
    for status in [408, 500, 502, 503, 504] {
        for rounds in [0, 1] {
            let mut state = EndpointRetryState::default();
            let decision = state.observe_refresh_failure(endpoint(5060), &failure(status, "Retry-After: 30\r\n", rounds).metadata(), now, Duration::from_secs(90));
            assert_eq!(decision, RefreshRetryDecision::RetryAfter(Duration::from_secs(30)));
            assert!(!state.check(endpoint(5060), now + Duration::from_secs(29)).is_allowed());
        }
    }
}

#[test]
fn register_retry_refresh_not_before_at_or_beyond_expiry_only_retains_until_expiry() {
    let now = Instant::now();
    for value in ["60", "61", "86401", "bad"] {
        let mut state = EndpointRetryState::default();
        let decision = state.observe_refresh_failure(endpoint(5060), &failure(503, &format!("Retry-After: {value}\r\n"), 0).metadata(), now, Duration::from_secs(60));
        assert_eq!(decision, RefreshRetryDecision::RetainUntilExpiry(Duration::from_secs(60)));
    }
}

#[test]
fn register_retry_expired_identity_auth_and_dead_flow_never_keep_binding() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    for status in [403, 401, 407, 423, 430, 494] {
        assert_eq!(state.observe_refresh_failure(endpoint(5060), &failure(status, "", 1).metadata(), now, Duration::from_secs(60)), RefreshRetryDecision::InvalidateBinding);
    }
    let mut dead = failure(401, "", 1);
    dead.error = ImsError::new("ims_outbound_flow_failed");
    assert_eq!(dead.metadata().kind, RegisterFailureKind::FlowFailed);
    assert_eq!(state.observe_refresh_failure(endpoint(5060), &dead.metadata(), now, Duration::from_secs(60)), RefreshRetryDecision::InvalidateBinding);
    assert_eq!(state.observe_refresh_failure(endpoint(5060), &failure(503, "Retry-After: 30\r\n", 0).metadata(), now, Duration::ZERO), RefreshRetryDecision::InvalidateBinding);
}

#[test]
fn register_retry_presend_refresh_gate_uses_original_expiry_not_new_profile_lease() {
    let now = Instant::now();
    let admission = RetryAdmission::Deferred { not_before: now + Duration::from_secs(90) };
    assert_eq!(refresh_wait_decision(admission, now, Duration::from_secs(10)), Some(RefreshRetryDecision::RetainUntilExpiry(Duration::from_secs(10))));
    assert_eq!(refresh_wait_decision(admission, now, Duration::from_secs(180)), Some(RefreshRetryDecision::RetryAfter(Duration::from_secs(90))));
    assert_eq!(refresh_wait_decision(RetryAdmission::Allowed, now, Duration::ZERO), Some(RefreshRetryDecision::InvalidateBinding));
    assert_eq!(refresh_wait_decision(RetryAdmission::Allowed, now, Duration::from_secs(10)), None);
}

#[test]
fn register_retry_mapped_address_and_flow_label_cannot_bypass_endpoint_gate() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let metadata = failure(503, "Retry-After: 60\r\n", 0).metadata();
    state.observe_failure(endpoint(5060), &metadata, now);
    assert!(!state.check("[::ffff:192.0.2.1]:5060".parse().unwrap(), now).is_allowed());
    let address = "2001:db8::1".parse().unwrap();
    let original = SocketAddr::V6(std::net::SocketAddrV6::new(address, 5060, 1, 0));
    let another_label = SocketAddr::V6(std::net::SocketAddrV6::new(address, 5060, 2, 0));
    state.observe_failure(original, &metadata, now);
    assert!(!state.check(another_label, now).is_allowed());
}

#[test]
fn register_retry_refresh_transport_keeps_old_binding_but_not_past_expiry() {
    let now = Instant::now();
    let mut transport = failure(401, "", 1);
    transport.error = ImsError::new("ims_register_authenticated_receive_failed");
    let mut state = EndpointRetryState::default();
    assert_eq!(state.observe_refresh_failure(endpoint(5060), &transport.metadata(), now,
        Duration::from_secs(60)), RefreshRetryDecision::RetryAfter(TRANSPORT_RETRY_DELAY));
    assert_eq!(state.observe_refresh_failure(endpoint(5060), &transport.metadata(), now,
        Duration::from_secs(1)), RefreshRetryDecision::RetainUntilExpiry(Duration::from_secs(1)));
}

// Synthetic in-memory exchange: no socket, SIM, bearer or protocol simulation.
struct MockChannel {
    responses: VecDeque<Vec<u8>>,
    sends: usize,
}

impl ImsChannel for MockChannel {
    async fn send_sip(&mut self, _frame: &[u8]) -> Result<(), ImsError> {
        self.sends += 1;
        Ok(())
    }
    async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.responses.pop_front().ok_or(ImsError::new("mock_transport_closed"))
    }
    fn route(&self) -> ImsRoute {
        ImsRoute { local_addr: endpoint(5090), pcscf_addr: endpoint(5060), transport: SipTransport::Tcp }
    }
    fn security_verify(&self) -> Option<&str> { None }
}

struct MockAuthenticator { calls: u8 }

impl RegisterAuthenticator<MockChannel> for MockAuthenticator {
    async fn authenticated_request(&mut self, _response: &[u8], _cseq: u32) -> Result<Vec<u8>, ImsError> {
        self.calls += 1;
        Ok(b"REGISTER sip:ims.example SIP/2.0\r\nContent-Length: 0\r\n\r\n".to_vec())
    }
}

#[tokio::test]
async fn register_retry_shared_driver_preserves_temporary_response_and_stops_exchange() {
    for challenged in [false, true] {
        let mut responses = VecDeque::new();
        if challenged { responses.push_back(failure(401, "", 0).response.unwrap()); }
        responses.push_back(failure(503, "Retry-After: 60 (busy);duration=120\r\n", 0).response.unwrap());
        let mut channel = MockChannel { responses, sends: 0 };
        let mut auth = MockAuthenticator { calls: 0 };
        let result = run_register_observed(&mut channel, b"REGISTER sip:ims.example SIP/2.0\r\n\r\n", &mut auth).await.unwrap_err();
        assert_eq!(result.metadata().sip_status, Some(503));
        assert_eq!(result.metadata().retry_after, RetryAfter::DelaySeconds(60));
        assert_eq!(result.metadata().kind, RegisterFailureKind::TemporaryEndpoint);
        assert_eq!(channel.sends, if challenged { 2 } else { 1 });
        assert_eq!(auth.calls, u8::from(challenged));
    }
}

#[tokio::test]
async fn register_retry_shared_driver_saved_401_transport_is_not_auth_rejection() {
    let mut channel = MockChannel { responses: VecDeque::from([failure(401, "", 0).response.unwrap()]), sends: 0 };
    let mut auth = MockAuthenticator { calls: 0 };
    let failure = run_register_observed(&mut channel, b"REGISTER sip:ims.example SIP/2.0\r\n\r\n", &mut auth).await.unwrap_err();
    assert_eq!(failure.metadata().sip_status, Some(401));
    assert_eq!(failure.metadata().kind, RegisterFailureKind::Transport);
    assert_eq!(failure.metadata().stage, RegisterFailureStage::Authenticated);
    assert_eq!(channel.sends, 2);
}

#[tokio::test]
async fn register_retry_mock_profile_loop_skips_deferred_endpoint_then_uses_alternate() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let mut total_sends = 0;
    let mut registered = false;
    // Same logical P-CSCF is offered by two profiles, followed by a genuinely
    // different endpoint. This mirrors the live owner's check/run/observe API.
    for (remote, status) in [(endpoint(5060), 503), (endpoint(5060), 200), (endpoint(5070), 200)] {
        if !state.check(remote, now).is_allowed() { continue; }
        let mut channel = MockChannel {
            responses: VecDeque::from([failure(status, "Retry-After: 60\r\n", 0).response.unwrap()]),
            sends: 0,
        };
        let mut auth = MockAuthenticator { calls: 0 };
        match run_register_observed(&mut channel, b"REGISTER sip:ims.example SIP/2.0\r\n\r\n", &mut auth).await {
            Ok(_) => registered = true,
            Err(failure) => { state.observe_failure(remote, &failure.metadata(), now); }
        }
        total_sends += channel.sends;
    }
    assert_eq!(total_sends, 2, "the second profile must not send to the deferred endpoint");
    assert!(registered);
    assert!(!state.check(endpoint(5060), now).is_allowed());
}

#[tokio::test]
async fn register_retry_shared_driver_auth_rounds_stay_bounded() {
    for challenge in [401, 407] {
        let mut channel = MockChannel { responses: VecDeque::from(vec![failure(challenge, "", 0).response.unwrap(); 4]), sends: 0 };
        let mut auth = MockAuthenticator { calls: 0 };
        let failure = run_register_observed(&mut channel, b"REGISTER sip:ims.example SIP/2.0\r\n\r\n", &mut auth).await.unwrap_err();
        assert_eq!(failure.metadata().kind, RegisterFailureKind::AuthenticationRejected);
        assert_eq!(failure.auth_rounds, 2);
        assert_eq!(channel.sends, 3);
        assert_eq!(auth.calls, 2);
    }
}
