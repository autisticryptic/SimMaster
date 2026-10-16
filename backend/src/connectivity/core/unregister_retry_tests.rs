//! I/O-free policy tests; Rust execution belongs to Actions.
use super::*;
use crate::connectivity::core::register::{RegisterFailure, RegisterFailureStage};
use crate::connectivity::core::ImsError;

fn endpoint(port: u16) -> SocketAddr { SocketAddr::from(([192, 0, 2, 1], port)) }
fn failure(status: u16, headers: &str) -> RegisterFailureMetadata {
    RegisterFailure {
        error: ImsError::new("ims_register_initial_unexpected_status"),
        response: Some(format!("SIP/2.0 {status} Failure\r\n{headers}\r\n").into_bytes()), auth_rounds: 0,
    }.metadata()
}

#[test]
fn unregister_wait_survives_shorter_endpoints_pause_and_late_success() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    state.observe_failure(endpoint(5070), &failure(503, "Retry-After: 5\r\n"), now);
    let expected = RetryAdmission::Deferred { not_before: now + Duration::from_secs(120) };
    assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(503, "Retry-After: 120\r\n"), now), expected);
    assert_eq!(state.pause_restart(now), expected);
    state.registered();
    assert_eq!(state.restart_admission(now + Duration::from_secs(119)), expected);
    assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(503, "Retry-After: 1\r\n"), now), expected);
    assert_eq!(state.restart_admission(now + Duration::from_secs(120)), RetryAdmission::Allowed);
}

#[test]
fn unregister_wait_preserves_default_zero_and_terminal_response_delays() {
    let now = Instant::now();
    for (status, header, seconds) in [(503, "", 30), (503, "Retry-After: 0\r\n", 1), (403, "Retry-After: 90\r\n", 90)] {
        let mut state = EndpointRetryState::default();
        let expected = RetryAdmission::Deferred { not_before: now + Duration::from_secs(seconds) };
        assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(status, header), now), expected);
        assert_eq!(state.restart_admission(now), expected);
    }
}

#[test]
fn unregister_invalid_duplicate_and_excessive_waits_remain_stopped() {
    let now = Instant::now();
    for (header, reason) in [
        ("Retry-After: invalid\r\n", RetryStopReason::InvalidRetryAfter),
        ("Retry-After: 0\r\nRetry-After: 60\r\n", RetryStopReason::InvalidRetryAfter),
        ("Retry-After: 86401\r\n", RetryStopReason::RetryAfterTooLong),
    ] {
        let mut state = EndpointRetryState::default();
        let expected = RetryAdmission::Stopped(reason);
        assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(503, header), now), expected);
        state.registered();
        assert_eq!(state.pause_restart(now + Duration::from_secs(90000)), expected);
    }
}

#[test]
fn unregister_rejection_without_wait_does_not_invent_or_clear_a_delay() {
    let now = Instant::now();
    for status in [401, 403, 423] {
        let mut state = EndpointRetryState::default();
        assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(status, ""), now), RetryAdmission::Allowed);
        let expected = state.observe_unregister_failure(endpoint(5070), &failure(503, "Retry-After: 120\r\n"), now);
        assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(status, ""), now), expected);
    }
}

#[test]
fn unregister_transport_does_not_reuse_a_saved_challenge_retry_after() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let metadata = RegisterFailureMetadata { sip_status: Some(401), stage: RegisterFailureStage::Authenticated,
        kind: RegisterFailureKind::Transport, retry_after: RetryAfter::DelaySeconds(7200), min_expires: None };
    assert_eq!(state.observe_unregister_failure(endpoint(5060), &metadata, now),
        RetryAdmission::Deferred { not_before: now + TRANSPORT_RETRY_DELAY });
}

#[test]
fn unregister_and_reg_event_waits_merge_without_shortening_either() {
    let now = Instant::now();
    let mut state = EndpointRetryState::default();
    let expected = state.observe_registration_event(endpoint(5070), Some(300), now);
    assert_eq!(state.observe_unregister_failure(endpoint(5060), &failure(503, "Retry-After: 120\r\n"), now), expected);
    state.registered();
    assert_eq!(state.restart_admission(now + Duration::from_secs(121)), expected);
    assert_eq!(state.restart_admission(now + Duration::from_secs(300)), RetryAdmission::Allowed);
}
