//! Real cellular unregister and cleanup on an owned synthetic worker/channel.
//! Executed only in Actions; no device or real namespace is accessed.
use super::*;
use crate::connectivity::core::register_retry::{RetryAdmission, RetryStopReason};

async fn fixture() -> (CellularImsLiveHandle, CellularImsRuntime, UdpSocket, UdpSocket, UiccCardBinding, SocketAddr) {
    let profile = crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc).unwrap();
    let (mut session, runtime, server, client) = protected_session_with_profile(profile).await;
    let live = CellularImsLiveHandle::new();
    let card = session.auth_binding.clone().unwrap();
    let endpoint = session.pcscf;
    session.endpoint_retries = live.retry_scope.lock().await.for_card(&card);
    session.diagnostic_scope = runtime.begin_diagnostics().await;
    *live.session.lock().await = Some(session);
    (live, runtime, server, client, card, endpoint)
}

async fn answer(server: UdpSocket, status: &str, headers: &str) -> UdpSocket {
    let (request, source) = receive(&server).await;
    assert_eq!(sip::header_value(&request, "Expires").as_deref(), Some("0"));
    assert!(sip::header_value(&request, "Authorization").is_some());
    assert!(sip::header_value(&request, "Security-Verify").is_some());
    server.send_to(&response(&request, status, headers), source).await.unwrap();
    server
}
async fn quiet(server: &UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(80), server.recv_from(&mut [0u8; 8192])).await.is_err());
}

#[tokio::test]
async fn unregister_response_wait_survives_real_cleanup_and_same_sim_scope_recreation() {
    let (live, runtime, server, _client, mut card, endpoint) = fixture().await;
    let captured = live.retry_scope.lock().await.for_card(&card);
    let sender = tokio::spawn(answer(server, "503 Service Unavailable", "Retry-After: 120\r\n"));
    let started = Instant::now();
    assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::Rejected);
    let server = sender.await.unwrap();
    let admission = captured.lock().await.restart_admission(Instant::now());
    assert!(matches!(admission, RetryAdmission::Deferred { not_before } if not_before >= started + Duration::from_secs(120)));
    {
        let sessions = live.session.lock().await;
        let session = sessions.as_ref().unwrap();
        assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, 3);
        assert_eq!(session.registration.lease.expires_seconds, 3600);
        assert_eq!(session.refresh_authorization.as_ref().unwrap().challenge.nonce, "old-nonce");
    }
    let facts = runtime.status().await.diagnostics;
    assert_eq!(facts.procedure, reg_diag::RegistrationProcedure::Unregister);
    assert_eq!(facts.failure.unwrap().response_retry_after_seconds, Some(120));
    assert_eq!(facts.wait.status, reg_diag::WaitStatus::Waiting);
    cleanup_live_session(&live).await;
    assert!(live.session.lock().await.is_none());
    card.owner = "replacement-owner".into();
    card.endpoint = "renumbered-control".into();
    let next = live.retry_scope.lock().await.for_card(&card);
    assert!(Arc::ptr_eq(&captured, &next));
    assert_eq!(next.lock().await.restart_admission(Instant::now()), admission);
    assert_eq!(next.lock().await.check(endpoint, Instant::now()), admission);
    assert_eq!(live_retry::admission_error(admission, Instant::now()).unwrap().code(), code::REGISTER_RETRY_DEFERRED);
    quiet(&server).await;
}

#[tokio::test]
async fn unregister_wait_uses_existing_default_zero_and_invalid_header_policy() {
    for (status, headers, seconds, stopped) in [
        ("503 Service Unavailable", "", 30, None),
        ("503 Service Unavailable", "Retry-After: 0\r\n", 1, None),
        ("403 Forbidden", "Retry-After: 120\r\n", 120, None),
        ("503 Service Unavailable", "Retry-After: invalid\r\n", 0, Some(RetryStopReason::InvalidRetryAfter)),
        ("503 Service Unavailable", "Retry-After: 0\r\nRetry-After: 1\r\n", 0, Some(RetryStopReason::InvalidRetryAfter)),
        ("503 Service Unavailable", "Retry-After: 86401\r\n", 0, Some(RetryStopReason::RetryAfterTooLong)),
    ] {
        let (live, runtime, server, _client, card, endpoint) = fixture().await;
        let state = live.retry_scope.lock().await.for_card(&card);
        let sender = tokio::spawn(answer(server, status, headers));
        let started = Instant::now();
        assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::Rejected);
        let server = sender.await.unwrap();
        let admission = state.lock().await.restart_admission(Instant::now());
        match (admission, stopped) {
            (RetryAdmission::Deferred { not_before }, None) => assert!(not_before >= started + Duration::from_secs(seconds)),
            (RetryAdmission::Stopped(reason), Some(expected)) => assert_eq!(reason, expected),
            _ => panic!("unexpected unregister admission: {admission:?}"),
        }
        assert_eq!(state.lock().await.check(endpoint, Instant::now()), admission);
        assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::AccessLost);
        cleanup_live_session(&live).await;
        assert_eq!(state.lock().await.restart_admission(Instant::now()), admission);
        quiet(&server).await;
    }
}

#[tokio::test]
async fn unregister_success_or_rejection_without_wait_does_not_invent_restart_backoff() {
    for (status, headers, expected) in [
        ("200 OK", "Expires: 0\r\nRetry-After: 120\r\n", UnregisterResult::Confirmed),
        ("403 Forbidden", "", UnregisterResult::Rejected),
        ("423 Interval Too Brief", "Min-Expires: 7200\r\n", UnregisterResult::Rejected),
    ] {
        let (live, runtime, server, _client, card, _) = fixture().await;
        let state = live.retry_scope.lock().await.for_card(&card);
        let sender = tokio::spawn(answer(server, status, headers));
        assert_eq!(unregister_live_session(&live, &runtime).await, expected);
        let server = sender.await.unwrap();
        cleanup_live_session(&live).await;
        assert_eq!(state.lock().await.restart_admission(Instant::now()), RetryAdmission::Allowed);
        quiet(&server).await;
    }
}

#[tokio::test]
async fn late_unregister_response_cannot_poison_replacement_sim_retry_scope() {
    let (live, runtime, server, _client, card, endpoint) = fixture().await;
    let old = live.retry_scope.lock().await.for_card(&card);
    let live_for_peer = live.clone();
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        let mut new_card = card;
        new_card.iccid = "new-synthetic-card".into();
        let new_state = live_for_peer.retry_scope.lock().await.for_card(&new_card);
        assert!(!Arc::ptr_eq(&old, &new_state));
        server.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
        (server, new_state, old)
    });
    assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::Rejected);
    let (server, new_state, old) = sender.await.unwrap();
    assert!(matches!(old.lock().await.check(endpoint, Instant::now()), RetryAdmission::Deferred { .. }));
    assert_eq!(new_state.lock().await.restart_admission(Instant::now()), RetryAdmission::Allowed);
    cleanup_live_session(&live).await;
    assert_eq!(new_state.lock().await.restart_admission(Instant::now()), RetryAdmission::Allowed);
    quiet(&server).await;
}
