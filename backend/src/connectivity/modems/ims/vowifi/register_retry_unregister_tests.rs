//! R4 through the installed socket owner and production unregister factory.
//! Synthetic loopback only; no ePDG, TUN, extra SA, or device access.
use super::*;
use crate::connectivity::core::{
    register::{RegisterFailureKind, RegisterFailureStage, RetryAfter},
    register_retry::{RetryAdmission, RetryStopReason},
    registration_diagnostics::{AuthStage, RegistrationProcedure, WaitStatus},
};
use crate::connectivity::modems::ims::{uicc_ims::UiccCardBinding, vowifi::{
    operator::{disconnect_line, install_registered_channel, operator_link_for_line, RegisteredVoiceContext},
    runtime::VowifiRuntime,
}};

async fn installed_fixture() -> (Arc<VowifiUnregisterFactory>, UdpSocket, Arc<VowifiRuntime>, SocketAddr) {
    let (mut factory, channel, peer) = fixture(false).await;
    let route = channel.route();
    let runtime = Arc::new(VowifiRuntime::for_line(&factory.line_id));
    reg_event_live::bind_reg_event_runtime(&factory.line_id, &runtime);
    Arc::get_mut(&mut factory).unwrap().context.diagnostic_scope =
        diagnostics_live::Scope::begin(&factory.line_id, factory.profile).await;
    let context = RegisteredVoiceContext {
        line_id: factory.line_id.clone(), profile_id: factory.profile.meta.profile_id,
        identity: factory.context.identity.shared.clone(), route,
        registration: RegisteredImsContext::from_response(ImsRegistrationAccess::Vowifi,
            b"SIP/2.0 200 OK\r\nExpires: 3600\r\n\r\n", 3600),
        security_verify: Some(VERIFY.into()), pani: "IEEE-802.11".into(), user_agent: "r4-fixture".into(),
        expires_at: Instant::now() + Duration::from_secs(60), tcp_keepalive_interval: None,
        options_ping_interval: None, unregister: Some(factory.clone()), media_route_installer: None,
        media_interface: None, media_operator_creator: crate::connectivity::core::media::test_operator_socket_creator(),
    };
    install_registered_channel(context, channel).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while !operator_link_for_line(&factory.line_id).is_available() { tokio::task::yield_now().await; }
    }).await.unwrap();
    (factory, peer, runtime, route.pcscf_addr)
}

fn logical_endpoint(factory: &VowifiUnregisterFactory) -> SocketAddr {
    SocketAddr::new(factory.context.route_addr, factory.profile.ims.local_port)
}

fn assert_zero_expiry(request: &[u8]) {
    assert!(request.starts_with(b"REGISTER "));
    assert_eq!(sip_frame::header_value(request, "Expires").as_deref(), Some("0"));
    // RFC 3261 permits the binding expiry to come from the global Expires
    // header. A Contact override is optional, but must not retain a lease.
    let contact = sip_frame::header_value(request, "Contact").unwrap();
    for expires in contact.split(';').filter_map(|part| part.trim().strip_prefix("expires=")) {
        assert_eq!(expires.trim_matches('"'), "0");
    }
    assert_eq!(sip_frame::header_value(request, "Security-Verify").as_deref(), Some(VERIFY));
}

async fn answer_unregister(peer: UdpSocket, status: &str, headers: &str, challenges: u8) -> UdpSocket {
    let (mut request, source) = receive(&peer).await;
    assert_zero_expiry(&request);
    for round in 0..challenges {
        let challenge = format!("WWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"{}\", algorithm=AKAv1-MD5, qop=\"auth\", stale=true\r\n", nonce(round + 2));
        peer.send_to(&response(&request, "401 Unauthorized", &challenge), source).await.unwrap();
        let (next, next_source) = receive(&peer).await;
        assert_eq!(next_source, source, "no new protected socket or SA");
        request = next;
        assert_zero_expiry(&request);
    }
    let headers = if status == "200 OK" { format!("{headers}{}", info(&request, false)) } else { headers.to_owned() };
    peer.send_to(&response(&request, status, &headers), source).await.unwrap();
    peer
}

async fn quiet(peer: &UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(80), peer.recv_from(&mut [0u8; 8192])).await.is_err(),
        "shutdown must not retry deregistration or start another registration");
}

async fn assert_next_registration_gated(factory: &VowifiUnregisterFactory, admission: RetryAdmission) {
    // Exercise the real new-registration entry, before it can establish ESP.
    let error = attempt_live_ims_registration(&factory.line_id, factory.profile,
        &ImsAccessNetworkRuntime::default(), false).await.unwrap_err();
    assert_eq!(error.retry_admission, Some(admission));
    assert_eq!(error.reason, if matches!(admission, RetryAdmission::Stopped(_)) {
        register_retry_live::STOPPED
    } else { register_retry_live::DEFERRED });
}

#[tokio::test]
async fn installed_unregister_failures_gate_next_registration_after_cleanup() {
    for (status, headers, seconds, stop) in [
        ("503 Service Unavailable", "Retry-After: 120\r\n", 120, None),
        ("503 Service Unavailable", "", 30, None),
        ("503 Service Unavailable", "Retry-After: 0\r\n", 1, None),
        ("503 Service Unavailable", "Retry-After: 86401\r\n", 0, Some(RetryStopReason::RetryAfterTooLong)),
        ("503 Service Unavailable", "Retry-After: 1\r\nRetry-After: 2\r\n", 0, Some(RetryStopReason::InvalidRetryAfter)),
        ("503 Service Unavailable", "Retry-After: invalid\r\n", 0, Some(RetryStopReason::InvalidRetryAfter)),
        ("403 Forbidden", "Retry-After: 120\r\n", 120, None),
    ] {
        let (factory, peer, runtime, protected_endpoint) = installed_fixture().await;
        let scope = register_retry_live::Scope::for_owner(&factory.line_id, factory.context.uicc.as_ref().unwrap()).await.unwrap();
        assert_ne!(logical_endpoint(&factory), protected_endpoint, "fixture must distinguish logical and protected ports");
        let sender = tokio::spawn(answer_unregister(peer, status, headers, 0));
        let started = Instant::now();
        assert_eq!(disconnect_line(&factory.line_id).await, UnregisterResult::Rejected);
        let peer = sender.await.unwrap();
        let admission = scope.check(logical_endpoint(&factory)).await;
        match (admission, stop) {
            (RetryAdmission::Stopped(actual), Some(expected)) => assert_eq!(actual, expected),
            (RetryAdmission::Deferred { not_before }, None) => {
                assert!(not_before >= started + Duration::from_secs(seconds));
                assert!(not_before <= Instant::now() + Duration::from_secs(seconds));
            },
            _ => panic!("unexpected unregister admission: {admission:?}"),
        }
        assert!(scope.check(protected_endpoint).await.is_allowed(), "ESP port is not the retry key");
        let report = runtime.snapshot().await.status_response().diagnostics;
        assert_eq!(report.procedure, RegistrationProcedure::Unregister);
        let failure = report.failure.unwrap();
        assert_eq!(failure.sip_status, Some(if status.starts_with("503") { 503 } else { 403 }));
        assert_eq!(failure.auth_stage, Some(AuthStage::Initial));
        assert_eq!(report.wait.status, if stop.is_some() { WaitStatus::Stopped } else { WaitStatus::Waiting });
        assert!(!operator_link_for_line(&factory.line_id).is_available());
        assert_retained(&factory, 2);
        assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 9);
        clear_live_runtime_for_line(&factory.line_id).await;
        assert_eq!(register_retry_live::restart_admission(&factory.line_id).await, admission);
        assert_next_registration_gated(&factory, admission).await;
        assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 9);
        quiet(&peer).await;
    }
}

#[tokio::test]
async fn installed_challenged_unregister_503_records_final_response_and_blocks_restart() {
    let (factory, peer, runtime, _) = installed_fixture().await;
    let sender = tokio::spawn(answer_unregister(peer, "503 Service Unavailable", "Retry-After: 120\r\n", 1));
    assert_eq!(disconnect_line(&factory.line_id).await, UnregisterResult::Rejected);
    let peer = sender.await.unwrap();
    let failure = runtime.snapshot().await.status_response().diagnostics.failure.unwrap();
    assert_eq!(failure.sip_status, Some(503), "not the preceding 401");
    assert_eq!(failure.auth_stage, Some(AuthStage::Authenticated));
    assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 10);
    assert_retained(&factory, 2);
    clear_live_runtime_for_line(&factory.line_id).await;
    let admission = register_retry_live::restart_admission(&factory.line_id).await;
    assert!(matches!(admission, RetryAdmission::Deferred { .. }));
    assert_next_registration_gated(&factory, admission).await;
    quiet(&peer).await;
}

#[tokio::test]
async fn installed_unregister_wait_does_not_shorten_an_unrelated_restart_or_replace_a_stop() {
    for prior_seconds in [30, 600, 86401] {
        let (factory, peer, _, _) = installed_fixture().await;
        let endpoint = logical_endpoint(&factory);
        let scope = register_retry_live::Scope::for_owner(&factory.line_id, factory.context.uicc.as_ref().unwrap()).await.unwrap();
        let unrelated: SocketAddr = "192.0.2.99:5060".parse().unwrap();
        let prior = scope.observe(unrelated, &temporary(prior_seconds)).await;
        scope.pause_restart().await;
        assert!(scope.check(endpoint).await.is_allowed());
        let sender = tokio::spawn(answer_unregister(peer, "503 Service Unavailable", "Retry-After: 120\r\n", 0));
        assert_eq!(disconnect_line(&factory.line_id).await, UnregisterResult::Rejected);
        let peer = sender.await.unwrap();
        let endpoint_wait = scope.check(endpoint).await;
        assert!(matches!(endpoint_wait, RetryAdmission::Deferred { .. }));
        let expected = if prior_seconds == 30 { endpoint_wait } else { prior };
        assert_eq!(register_retry_live::restart_admission(&factory.line_id).await, expected);
        clear_live_runtime_for_line(&factory.line_id).await;
        assert_next_registration_gated(&factory, expected).await;
        quiet(&peer).await;
    }
}

#[tokio::test]
async fn installed_unregister_success_and_no_wait_rejections_do_not_defer_registration() {
    for (status, headers, challenges, expected) in [
        ("200 OK", "Expires: 0\r\n", 0, UnregisterResult::Confirmed),
        ("200 OK", "Expires: 0\r\nRetry-After: 120\r\n", 1, UnregisterResult::Confirmed),
        ("403 Forbidden", "", 0, UnregisterResult::Rejected),
        ("401 Unauthorized", "", 2, UnregisterResult::Rejected),
        ("423 Interval Too Brief", "Min-Expires: 600\r\n", 0, UnregisterResult::Rejected),
        ("423 Interval Too Brief", "Min-Expires: 600\r\n", 1, UnregisterResult::Rejected),
    ] {
        let (factory, peer, runtime, _) = installed_fixture().await;
        let sender = tokio::spawn(answer_unregister(peer, status, headers, challenges));
        assert_eq!(disconnect_line(&factory.line_id).await, expected);
        let peer = sender.await.unwrap();
        assert!(!operator_link_for_line(&factory.line_id).is_available());
        assert_retained(&factory, 2);
        assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 9 + u32::from(challenges));
        let report = runtime.snapshot().await.status_response().diagnostics;
        assert_ne!(report.wait.status, WaitStatus::Waiting);
        assert_ne!(report.wait.status, WaitStatus::Stopped);
        if status == "200 OK" { assert!(report.failure.is_none()); }
        if status == "401 Unauthorized" {
            assert_eq!(report.failure.unwrap().auth_stage, Some(AuthStage::Authenticated));
        }
        clear_live_runtime_for_line(&factory.line_id).await;
        assert_eq!(live_register_retry_remaining_for_line(&factory.line_id).await, None);
        assert!(check_live_register_retry_gate(&factory.line_id).await.is_ok());
        quiet(&peer).await;
    }
}

fn replacement_owner(factory: &VowifiUnregisterFactory, same_sim: bool) -> Arc<LiveImsSelection> {
    live_identity::selection_for_test(&factory.line_id, factory.context.identity.clone(), factory.context.target.clone(),
        UiccCardBinding { endpoint: format!("/synthetic/{}/replacement", factory.line_id), slot: 1,
            owner: "replacement-owner".into(),
            iccid: if same_sim { "synthetic-rspauth-card" } else { "replacement-card" }.into(),
            imsi: "001010123456789".into() })
}

fn temporary(seconds: u32) -> RegisterFailureMetadata {
    RegisterFailureMetadata { sip_status: Some(503), stage: RegisterFailureStage::Initial,
        kind: RegisterFailureKind::TemporaryEndpoint, retry_after: RetryAfter::DelaySeconds(seconds), min_expires: None }
}

#[tokio::test]
async fn late_installed_unregister_result_updates_only_captured_subscription_not_new_diagnostics() {
    for same_sim in [true, false] {
        let (factory, peer, runtime, _) = installed_fixture().await;
        let endpoint = logical_endpoint(&factory);
        let old = register_retry_live::Scope::for_owner(&factory.line_id, factory.context.uicc.as_ref().unwrap()).await.unwrap();
        let line = factory.line_id.clone();
        let disconnect = tokio::spawn(async move { disconnect_line(&line).await });
        let (request, source) = receive(&peer).await;
        assert_zero_expiry(&request);
        // Receipt proves admission and its Scope capture already happened.
        // Replace both the diagnostics snapshot and reader/card observation
        // before releasing the network response to the old socket owner.
        runtime.reset_runtime("r4-replacement").await;
        let diagnostic = diagnostics_live::Scope::begin(&factory.line_id, factory.profile).await;
        diagnostic.update(|state| state.facts.procedure = RegistrationProcedure::Initial).await;
        let owner = replacement_owner(&factory, same_sim);
        let current = register_retry_live::Scope::for_owner(&factory.line_id, &owner).await.unwrap();
        let new_wait = if same_sim { None } else {
            Some(current.observe_unregister_failure(endpoint, &temporary(45)).await)
        };
        peer.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
        assert_eq!(disconnect.await.unwrap(), UnregisterResult::Rejected);
        let old_wait = old.check(endpoint).await;
        assert!(matches!(old_wait, RetryAdmission::Deferred { not_before } if not_before > Instant::now() + Duration::from_secs(115)));
        let expected = new_wait.unwrap_or(old_wait);
        assert_eq!(current.check(endpoint).await, expected);
        assert_eq!(register_retry_live::restart_admission(&factory.line_id).await, expected);
        let report = runtime.snapshot().await.status_response().diagnostics;
        assert_eq!(report.procedure, RegistrationProcedure::Initial);
        assert!(report.failure.is_none(), "old observer cannot publish into replacement snapshot");
        assert_ne!(report.wait.status, WaitStatus::Waiting);
        assert!(factory.unregister_admitted().await.is_err(), "old reader mapping must still fail admission");
        clear_live_runtime_for_line(&factory.line_id).await;
        assert_next_registration_gated(&factory, expected).await;
        quiet(&peer).await;
    }
}

#[tokio::test]
async fn unregister_diagnostics_hook_is_passive_and_control_survives_missing_runtime() {
    let (mut factory, _channel, peer) = fixture(false).await;
    let runtime = Arc::new(VowifiRuntime::for_line(&factory.line_id));
    reg_event_live::bind_reg_event_runtime(&factory.line_id, &runtime);
    runtime.seed_registered_for_reg_event_test().await;
    Arc::get_mut(&mut factory).unwrap().context.diagnostic_scope =
        diagnostics_live::Scope::begin(&factory.line_id, factory.profile).await;
    let before = runtime.snapshot().await.status_response().readiness;
    let observer = factory.unregister_admitted().await.unwrap().unwrap();
    let endpoint = logical_endpoint(&factory);
    let scope = register_retry_live::Scope::for_owner(&factory.line_id, factory.context.uicc.as_ref().unwrap()).await.unwrap();
    let admission = RetryAdmission::Deferred { not_before: Instant::now() + Duration::from_secs(120) };
    factory.diagnostics_unregister_started().await;
    factory.diagnostics_unregister_finished(Some("ims_register_initial_unexpected_status"), Some(temporary(120)), Some(admission)).await;
    assert_eq!(scope.check(endpoint).await, RetryAdmission::Allowed);
    assert_eq!(register_retry_live::restart_admission(&factory.line_id).await, RetryAdmission::Allowed);
    assert_eq!(runtime.snapshot().await.status_response().readiness, before);
    assert_retained(&factory, 1);
    assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 8);
    drop(runtime);
    let actual = observer.observe(temporary(120)).await;
    assert_eq!(scope.check(endpoint).await, actual);
    assert_eq!(register_retry_live::restart_admission(&factory.line_id).await, actual);
    assert!(matches!(actual, RetryAdmission::Deferred { .. }));
    // Neither hook sends SIP, consumes auth state, or changes the lease/SA.
    assert_retained(&factory, 1);
    assert_eq!(factory.next_cseq.load(Ordering::Relaxed), 8);
    clear_live_runtime_for_line(&factory.line_id).await;
    assert_next_registration_gated(&factory, actual).await;
    quiet(&peer).await;
}
