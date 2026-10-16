//! Real shared REGISTER driver + production WiFi authenticator on loopback.
//! Only UICC results and the pre-existing protected transport are fixtures;
//! no modem, TUN, external gateway or new ESP installation is performed here.
use super::*;
use crate::connectivity::core::register::{run_register_observed_with_budget, RegisterFailureKind, RegisterMinExpiresFailure, RegisterNegotiationBudget};

struct InitialFixture {
    line: String,
    context: LiveRegisterRequestContext,
    profile: &'static CarrierProfile,
    variant: LiveRegisterHeaderVariant,
    channel: SipChannel,
    server: UdpSocket,
    aka_calls: Arc<AtomicUsize>,
}

async fn initial_fixture() -> InitialFixture {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let line = format!("initial-min-expires-{}", SEQ.fetch_add(1, Ordering::Relaxed));
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let local = client.local_addr().unwrap();
    let remote = server.local_addr().unwrap();
    client.connect(remote).await.unwrap();
    let (mut context, profile) = context(&line, local, remote);
    let calls = Arc::new(AtomicUsize::new(0));
    context.uicc = Some(live_identity::with_aka_for_test(context.uicc.as_ref().unwrap(),
        UsimAkaApduResult { res: vec![0x22; 8], ck: vec![0x33; 16], ik: vec![0x44; 16], auts: None }, calls.clone()));
    let channel = SipChannel::new(SipChannelSocket::Udp(client), vec![], crate::connectivity::core::context::ImsRoute {
        local_addr: local, pcscf_addr: remote, transport: crate::connectivity::core::context::SipTransport::Udp,
    }, None);
    InitialFixture { line, context, profile, variant: live_register_header_variants(profile)[0], channel, server, aka_calls: calls }
}

fn first_challenge(request: &[u8], proxy: bool) -> Vec<u8> {
    response(request, if proxy { "407 Proxy Authentication Required" } else { "401 Unauthorized" }, &format!(
        "{}: Digest realm=\"{REALM}\", nonce=\"{}\", algorithm=AKAv1-MD5, qop=\"auth\", opaque=\"fixture-opaque\"\r\n",
        if proxy { "Proxy-Authenticate" } else { "WWW-Authenticate" }, nonce(1),
    ))
}

fn cseq(request: &[u8]) -> u32 {
    sip_frame::header_value(request, "CSeq").unwrap().split_whitespace().next().unwrap().parse().unwrap()
}
fn auth_nc(request: &[u8], proxy: bool) -> String {
    param(&sip_frame::header_value(request, if proxy { "Proxy-Authorization" } else { "Authorization" }).unwrap(), "nc")
}
async fn no_more(server: &UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(80), server.recv_from(&mut [0u8; 16384])).await.is_err(), "no compatibility/profile/ESP replay after lease failure");
}
fn authenticator<'a>(line: &'a str, profile: &'static CarrierProfile, context: &'a mut LiveRegisterRequestContext,
    variant: LiveRegisterHeaderVariant,
) -> VowifiRegisterAuthenticator<'a> {
    VowifiRegisterAuthenticator { line_id: line, profile, gateway: None, context, variant,
        last_security_server: None, last_security_verify: None, resync_sent: false, last_error: None }
}

#[tokio::test]
async fn initial_aka_423_rebuilds_real_proof_without_authentication_or_channel_restart() {
    for proxy in [false, true] {
        let mut f = initial_fixture().await;
        let peer = tokio::spawn(async move {
            let (initial, source) = receive(&f.server).await;
            f.server.send_to(&first_challenge(&initial, proxy), source).await.unwrap();
            let (proof, source2) = receive(&f.server).await;
            assert_eq!(source2, source);
            assert_eq!(cseq(&proof), 2);
            assert_eq!(auth_nc(&proof, proxy), "00000001");
            f.server.send_to(&response(&proof, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
            let (retry, source3) = receive(&f.server).await;
            assert_eq!(source3, source);
            assert_eq!(cseq(&retry), 3);
            assert_eq!(auth_nc(&retry, proxy), "00000002");
            assert_eq!(sip_frame::header_value(&retry, "Expires").as_deref(), Some("7200"));
            assert_eq!(sip_frame::header_value(&retry, "Call-ID"), sip_frame::header_value(&initial, "Call-ID"));
            let authorization = sip_frame::header_value(&retry, if proxy { "Proxy-Authorization" } else { "Authorization" }).unwrap();
            assert_eq!(param(&authorization, "opaque"), "fixture-opaque");
            let success = response(&retry, "200 OK", &format!("Expires: 7200\r\n{}", info(&retry, proxy)));
            f.server.send_to(&success, source).await.unwrap();
            retry
        });
        let request = f.context.build_initial_request(f.profile, f.variant);
        let mut budget = RegisterNegotiationBudget::default();
        let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
        let result = run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.unwrap();
        assert_eq!(result.auth_rounds, 1);
        drop(auth);
        assert_eq!(f.aka_calls.load(Ordering::Acquire), 1);
        assert!(!budget.permits_candidate_expansion());
        assert_eq!(f.context.min_expires, Some(7200));
        assert!(!f.context.channel_transferred, "shared success gate precedes installed binding publication");
        let sent = peer.await.unwrap();
        validate_live_register_request(f.context.refresh_authorization.as_ref(), &sent).unwrap();
        f.context.refresh_authorization.as_mut().unwrap().apply_success(&result.response).unwrap();
        assert_eq!(f.context.refresh_authorization.as_ref().unwrap().nonce_count, 0);
        assert_eq!(f.context.refresh_authorization.as_ref().unwrap().challenge.nonce, nonce(3));
    }
}

#[tokio::test]
async fn pre_auth_and_post_auth_423_keep_one_two_round_budget_and_strict_increase() {
    let mut f = initial_fixture().await;
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&f.server).await;
        f.server.send_to(&response(&first, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        let (initial_retry, _) = receive(&f.server).await;
        assert_eq!(cseq(&initial_retry), 2);
        f.server.send_to(&first_challenge(&initial_retry, false), source).await.unwrap();
        let (proof, _) = receive(&f.server).await;
        assert_eq!(cseq(&proof), 3);
        assert_eq!(sip_frame::header_value(&proof, "Expires").as_deref(), Some("7200"));
        f.server.send_to(&response(&proof, "423 Interval Too Brief", "Min-Expires: 8000\r\n"), source).await.unwrap();
        let (retry, _) = receive(&f.server).await;
        assert_eq!(cseq(&retry), 4);
        assert_eq!(auth_nc(&retry, false), "00000002");
        f.server.send_to(&response(&retry, "423 Interval Too Brief", "Min-Expires: 9000\r\n"), source).await.unwrap();
        no_more(&f.server).await;
    });
    let request = f.context.build_initial_request(f.profile, f.variant);
    let mut budget = RegisterNegotiationBudget::default();
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    let failure = run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.unwrap_err();
    assert_eq!(budget.failure_metadata(&failure).min_expires, Some(RegisterMinExpiresFailure::Exhausted));
    assert_eq!(failure.auth_rounds, 1);
    assert!(!budget.permits_candidate_expansion());
    drop(auth);
    assert_eq!(f.aka_calls.load(Ordering::Acquire), 1);
    assert_eq!(f.context.min_expires, Some(8000));
    peer.await.unwrap();
}

#[tokio::test]
async fn initial_authenticated_invalid_duplicate_or_excessive_floor_sends_no_retry() {
    for (header, expected) in [
        ("", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 0\r\n", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 3600\r\n", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 7200\r\nMin-Expires: 7200\r\n", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 86401\r\n", RegisterMinExpiresFailure::Unsupported),
    ] {
        let mut f = initial_fixture().await;
        let peer = tokio::spawn(async move {
            let (initial, source) = receive(&f.server).await;
            f.server.send_to(&first_challenge(&initial, false), source).await.unwrap();
            let (proof, _) = receive(&f.server).await;
            f.server.send_to(&response(&proof, "423 Interval Too Brief", header), source).await.unwrap();
            no_more(&f.server).await;
        });
        let request = f.context.build_initial_request(f.profile, f.variant);
        let mut budget = RegisterNegotiationBudget::default();
        let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
        let failure = run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.unwrap_err();
        assert_eq!(budget.failure_metadata(&failure).min_expires, Some(expected));
        assert!(!budget.permits_candidate_expansion());
        drop(auth);
        assert_eq!(f.context.refresh_authorization.as_ref().unwrap().nonce_count, 1);
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn initial_423_invalid_rspauth_never_commits_nextnonce_or_publishes_binding() {
    let mut f = initial_fixture().await;
    let peer = tokio::spawn(async move {
        let (initial, source) = receive(&f.server).await;
        f.server.send_to(&first_challenge(&initial, false), source).await.unwrap();
        let (proof, _) = receive(&f.server).await;
        f.server.send_to(&response(&proof, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        let (retry, _) = receive(&f.server).await;
        let wrong = info(&retry, false).replace("cnonce=\"", "cnonce=\"tampered-");
        f.server.send_to(&response(&retry, "200 OK", &wrong), source).await.unwrap();
        no_more(&f.server).await;
    });
    let request = f.context.build_initial_request(f.profile, f.variant);
    let mut budget = RegisterNegotiationBudget::default();
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    let failure = run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.unwrap_err();
    assert_eq!(budget.failure_metadata(&failure).kind, RegisterFailureKind::AuthenticationRejected);
    assert_eq!(budget.failure_metadata(&failure).min_expires, Some(RegisterMinExpiresFailure::Negotiated));
    drop(auth);
    let state = f.context.refresh_authorization.as_ref().unwrap();
    assert_eq!(state.challenge.nonce, nonce(1));
    assert_eq!(state.nonce_count, 2);
    assert!(!f.context.channel_transferred);
    peer.await.unwrap();
}

#[tokio::test]
async fn protected_423_handoff_and_stale_retry_retain_the_exact_original_channel() {
    let mut f = initial_fixture().await;
    let local = f.channel.route().local_addr;
    let remote = f.channel.route().pcscf_addr;
    let dummy = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut initial = SipChannel::new(SipChannelSocket::Udp(dummy), vec![], f.channel.route(), None);
    // Already-established channel represents the owned ESP mapping. The real
    // production handoff must move it for 423, with no new gateway available.
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(remote).await.unwrap();
    let protected_local = client.local_addr().unwrap();
    let protected = SipChannel::new(SipChannelSocket::Udp(client), vec![], crate::connectivity::core::context::ImsRoute {
        local_addr: protected_local, pcscf_addr: remote, transport: crate::connectivity::core::context::SipTransport::Udp,
    }, Some(VERIFY.into()));
    assert!(retain_protected_register_response(&mut initial, protected, 423));
    assert_eq!(initial.route().local_addr, protected_local);
    assert_ne!(initial.route().local_addr, local);
    f.context.local_addr = protected_local;
    f.context.protected_header_port = Some(5063);
    let mut previous = authorization(&f.context, f.profile, false);
    let auth_header = previous.authorization(&f.context.identity.private_user, &f.context.request_uri(f.profile, f.variant)).unwrap();
    f.context.refresh_authorization = Some(previous);
    let request = f.context.build_authorized_request(f.profile, f.variant, 1, &auth_header, Some(VERIFY));
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&f.server).await;
        assert_eq!(source, protected_local);
        f.server.send_to(&challenge_response(&first, false), source).await.unwrap();
        let (stale, source2) = receive(&f.server).await;
        assert_eq!(source2, source);
        assert_eq!(auth_nc(&stale, false), "00000001");
        f.server.send_to(&response(&stale, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        let (retry, source3) = receive(&f.server).await;
        assert_eq!(source3, source);
        assert_eq!(auth_nc(&retry, false), "00000002");
        assert_eq!(sip_frame::header_value(&retry, "Security-Verify").as_deref(), Some(VERIFY));
        assert!(sip_frame::header_value(&retry, "Via").unwrap().contains(":5063"));
        f.server.send_to(&response(&retry, "200 OK", &info(&retry, false)), source).await.unwrap();
    });
    let mut budget = RegisterNegotiationBudget::default();
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    let result = run_register_observed_with_budget(&mut initial, request.as_bytes(), &mut auth, &mut budget).await.unwrap();
    assert_eq!(result.auth_rounds, 1);
    assert_eq!(initial.security_verify(), Some(VERIFY));
    assert_eq!(initial.route().local_addr, protected_local);
    assert_eq!(f.aka_calls.load(Ordering::Acquire), 0, "stale and 423 never rerun UICC");
    peer.await.unwrap();
}

#[tokio::test]
async fn initial_423_worker_replacement_refuses_request_before_nonce_or_send() {
    let mut f = initial_fixture().await;
    let worker = f.context.worker_binding.as_ref().unwrap().worker().clone();
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&f.server).await;
        f.server.send_to(&first_challenge(&first, false), source).await.unwrap();
        let (proof, _) = receive(&f.server).await;
        worker.advance_test_generation().await;
        f.server.send_to(&response(&proof, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        no_more(&f.server).await;
    });
    let request = f.context.build_initial_request(f.profile, f.variant);
    let mut budget = RegisterNegotiationBudget::default();
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    let failure = run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.unwrap_err();
    assert_eq!(failure.error.code(), "vowifi_runtime_ue_worker_generation_changed");
    drop(auth);
    assert_eq!(f.context.refresh_authorization.as_ref().unwrap().nonce_count, 1);
    assert!(!f.context.channel_transferred);
    peer.await.unwrap();
}

#[tokio::test]
async fn two_aka_rounds_and_two_lease_rounds_are_independent_and_never_reset_each_other() {
    let mut f = initial_fixture().await;
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&f.server).await;
        f.server.send_to(&first_challenge(&first, false), source).await.unwrap();
        let (proof, _) = receive(&f.server).await;
        f.server.send_to(&response(&proof, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        let (retry, _) = receive(&f.server).await;
        assert_eq!(cseq(&retry), 3);
        f.server.send_to(&first_challenge(&retry, false), source).await.unwrap();
        let (second_proof, _) = receive(&f.server).await;
        assert_eq!(cseq(&second_proof), 4);
        f.server.send_to(&response(&second_proof, "423 Interval Too Brief", "Min-Expires: 8000\r\n"), source).await.unwrap();
        let (second_retry, _) = receive(&f.server).await;
        assert_eq!(cseq(&second_retry), 5);
        f.server.send_to(&first_challenge(&second_retry, false), source).await.unwrap();
        no_more(&f.server).await;
    });
    let request = f.context.build_initial_request(f.profile, f.variant);
    let mut budget = RegisterNegotiationBudget::default();
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    let failure = run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.unwrap_err();
    assert_eq!(failure.auth_rounds, 2);
    assert_eq!(failure.error.code(), "ims_register_auth_rejected");
    assert_eq!(budget.failure_metadata(&failure).min_expires, Some(RegisterMinExpiresFailure::Negotiated));
    assert_eq!(f.aka_calls.load(Ordering::Acquire), 2);
    peer.await.unwrap();
}

#[tokio::test]
async fn zero_expiry_cannot_rebuild_a_positive_lease_or_use_an_unsent_proof() {
    let mut f = initial_fixture().await;
    assert_eq!(rebuild_initial_authenticated_register(&mut f.context, f.profile, f.variant, 2, 7200, None).unwrap_err().code(), "digest_request_auth_context_missing");
    let request = f.context.build_register_request_with_expires(f.profile, f.variant, 1, None, None, 0);
    let peer = tokio::spawn(async move {
        let (zero, source) = receive(&f.server).await;
        assert_eq!(sip_frame::header_value(&zero, "Expires").as_deref(), Some("0"));
        f.server.send_to(&response(&zero, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        no_more(&f.server).await;
    });
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    assert_eq!(run_unregister(&mut f.channel, request.as_bytes(), &mut auth).await, UnregisterResult::Rejected);
    assert_eq!(f.aka_calls.load(Ordering::Acquire), 0);
    peer.await.unwrap();
}

#[tokio::test]
async fn auts_only_423_is_not_a_credential_proof_and_cannot_negotiate_success() {
    let mut f = initial_fixture().await;
    f.context.uicc = Some(live_identity::with_aka_for_test(f.context.uicc.as_ref().unwrap(),
        UsimAkaApduResult { res: vec![], ck: vec![], ik: vec![], auts: Some(vec![0x55; 14]) }, f.aka_calls.clone()));
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&f.server).await;
        f.server.send_to(&first_challenge(&first, false), source).await.unwrap();
        let (auts, _) = receive(&f.server).await;
        assert!(sip_frame::header_value(&auts, "Authorization").unwrap().contains("auts="));
        f.server.send_to(&response(&auts, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        no_more(&f.server).await;
    });
    let request = f.context.build_initial_request(f.profile, f.variant);
    let mut budget = RegisterNegotiationBudget::default();
    let mut auth = authenticator(&f.line, f.profile, &mut f.context, f.variant);
    assert!(run_register_observed_with_budget(&mut f.channel, request.as_bytes(), &mut auth, &mut budget).await.is_err());
    assert_eq!(auth.last_error.as_ref().unwrap().reason, "ims_aka_resync_unexpected_status");
    drop(auth);
    assert!(f.context.refresh_authorization.is_none());
    assert!(f.context.min_expires.is_none());
    peer.await.unwrap();
}
