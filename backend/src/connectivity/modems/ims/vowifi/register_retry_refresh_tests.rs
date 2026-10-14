//! R2 through the actual retained-credential factory and installed operator.
use super::*;
use crate::connectivity::core::{register::{RegisterFailureKind, RegisterFailureStage, RetryAfter}, register_retry::RetryAdmission};
use crate::connectivity::modems::ims::vowifi::{runtime::VowifiRuntime, operator::{
    RegisteredVoiceContext, install_registered_channel, refresh_registered_for_line, operator_link_for_line,
}};
use tokio::{net::{TcpListener, TcpStream}, io::{AsyncReadExt, AsyncWriteExt}};

async fn observed_fixture() -> (Arc<VowifiUnregisterFactory>, SipChannel, UdpSocket, Arc<VowifiRuntime>) {
    let (mut factory, channel, peer) = fixture(false).await;
    let runtime = Arc::new(VowifiRuntime::for_line(&factory.line_id));
    reg_event_live::bind_reg_event_runtime(&factory.line_id, &runtime);
    let diagnostics = diagnostics_live::Scope::begin(&factory.line_id, factory.profile).await;
    Arc::get_mut(&mut factory).unwrap().context.diagnostic_scope = diagnostics;
    (factory, channel, peer, runtime)
}
async fn assert_quiet(peer: &UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(80), peer.recv_from(&mut [0u8; 8192])).await.is_err());
}

#[tokio::test]
async fn temporary_refresh_wait_keeps_original_lease_sa_and_consumed_nonce_without_resending() {
    for (header, stopped) in [("Retry-After: 120\r\n", false), ("Retry-After: 0\r\n", false), ("", false),
        ("Retry-After: 86401\r\n", true), ("Retry-After: 1\r\nRetry-After: 2\r\n", true)] {
        let (factory, mut channel, peer, runtime) = observed_fixture().await;
        let original_expiry = Instant::now() + Duration::from_secs(60);
        let route = channel.route();
        let sender = tokio::spawn(async move {
            let (request, source) = receive(&peer).await;
            peer.send_to(&response(&request, "503 Service Unavailable", header), source).await.unwrap();
            peer
        });
        let failure = factory.refresh_observed(&mut channel, original_expiry).await.unwrap_err();
        let peer = sender.await.unwrap();
        assert_eq!(failure.retained_until, Some(original_expiry));
        assert_eq!(failure.metadata.unwrap().kind, RegisterFailureKind::TemporaryEndpoint);
        assert_eq!(failure.metadata.unwrap().sip_status, Some(503));
        assert_eq!(matches!(failure.retry_admission.unwrap(), RetryAdmission::Stopped(_)), stopped);
        assert_eq!(channel.route(), route); assert_eq!(channel.security_verify(), Some(VERIFY));
        assert_retained(&factory, 2);
        let cseq = factory.next_cseq.load(Ordering::Relaxed);
        let gated = factory.refresh_observed(&mut channel, original_expiry).await.unwrap_err();
        assert_eq!(gated.metadata, None, "checking a wait is not another rejected REGISTER");
        assert_eq!(gated.retained_until, Some(original_expiry));
        assert_eq!(factory.next_cseq.load(Ordering::Relaxed), cseq);
        assert_retained(&factory, 2);
        assert!(factory.unregister_admitted().await.is_err(), "Expires:0 cannot jump a pending endpoint gate");
        let report = runtime.snapshot().await.status_response().diagnostics;
        assert_eq!(report.failure.unwrap().sip_status, Some(503));
        assert_eq!(report.wait.status, if stopped { crate::connectivity::core::registration_diagnostics::WaitStatus::Stopped }
            else { crate::connectivity::core::registration_diagnostics::WaitStatus::Waiting });
        assert_quiet(&peer).await;
        let expired = factory.refresh_observed(&mut channel, Instant::now() - Duration::from_secs(1)).await.unwrap_err();
        assert_eq!(expired.code(), "vowifi_registration_expired");
        assert_eq!(expired.retained_until, None);
        assert_eq!(factory.next_cseq.load(Ordering::Relaxed), cseq);
    }
}

#[tokio::test]
async fn replaced_worker_blocks_old_factory_without_erasing_same_sim_wait_or_consuming_nonce() {
    let (factory, mut channel, peer, _) = observed_fixture().await;
    let expiry = Instant::now() + Duration::from_secs(60);
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&peer).await;
        peer.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
        peer
    });
    factory.refresh_observed(&mut channel, expiry).await.unwrap_err();
    let peer = sender.await.unwrap();
    let cseq = factory.next_cseq.load(Ordering::Relaxed);
    let mut replacement_context = factory.context.clone();
    replacement_context.bind_test_worker(&factory.line_id);
    let failed = factory.refresh_observed(&mut channel, expiry).await.unwrap_err();
    assert_eq!(failed.code(), "vowifi_runtime_ue_worker_generation_changed");
    assert_eq!(failed.retained_until, None);
    assert_eq!(factory.next_cseq.load(Ordering::Relaxed), cseq);
    assert_retained(&factory, 2);
    assert!(live_register_retry_remaining_for_line(&factory.line_id).await.unwrap() > Duration::from_secs(119));
    assert_quiet(&peer).await;
}

#[tokio::test]
async fn refresh_after_actual_wait_uses_next_nonce_count_and_only_verified_success_releases_restart_gate() {
    let (factory, mut channel, peer, _) = observed_fixture().await;
    let expiry = Instant::now() + Duration::from_secs(60);
    let sender = tokio::spawn(async move {
        let (first, source) = receive(&peer).await;
        peer.send_to(&response(&first, "503 Service Unavailable", "Retry-After: 1\r\n"), source).await.unwrap();
        let (second, next_source) = receive(&peer).await;
        assert_eq!(next_source, source);
        let header = sip_frame::header_value(&second, "Authorization").unwrap();
        assert_eq!(param(&header, "nc"), "00000003");
        peer.send_to(&response(&second, "200 OK", &format!("Expires: 4800\r\n{}", info(&second, false))), source).await.unwrap();
    });
    let failed = factory.refresh_observed(&mut channel, expiry).await.unwrap_err();
    let not_before = match failed.retry_admission.unwrap() { RetryAdmission::Deferred { not_before } => not_before, _ => panic!("finite wait expected") };
    tokio::time::sleep(not_before.saturating_duration_since(Instant::now()) + Duration::from_millis(10)).await;
    let registration = factory.refresh_observed(&mut channel, expiry).await.unwrap();
    sender.await.unwrap();
    assert_eq!(registration.lease.expires_seconds, 4800);
    assert_eq!(live_register_retry_remaining_for_line(&factory.line_id).await, None);
    assert_eq!(factory.refresh_authorization.lock().unwrap().as_ref().unwrap().challenge.nonce, nonce(3));
    assert_eq!(channel.security_verify(), Some(VERIFY));
}

#[tokio::test]
async fn refresh_challenged_503_preserves_actual_stage_wait_and_old_credential_state() {
    let (factory, mut channel, peer, _) = observed_fixture().await;
    let expiry = Instant::now() + Duration::from_secs(60);
    let sender = tokio::spawn(async move {
        let (first, source) = receive(&peer).await;
        peer.send_to(&challenge_response(&first, false), source).await.unwrap();
        let (authenticated, source) = receive(&peer).await;
        peer.send_to(&response(&authenticated, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
    });
    let failed = factory.refresh_observed(&mut channel, expiry).await.unwrap_err(); sender.await.unwrap();
    let metadata = failed.metadata.unwrap();
    assert_eq!(metadata.sip_status, Some(503)); assert_eq!(metadata.stage, RegisterFailureStage::Authenticated);
    assert_eq!(metadata.retry_after, RetryAfter::DelaySeconds(120));
    assert_eq!(failed.retained_until, Some(expiry));
    assert_retained(&factory, 2);
}

#[tokio::test]
async fn refresh_identity_rejection_with_retry_after_does_not_retain_the_binding() {
    let (factory, mut channel, peer, _) = observed_fixture().await;
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&peer).await;
        peer.send_to(&response(&request, "403 Forbidden", "Retry-After: 120\r\n"), source).await.unwrap();
    });
    let failed = factory.refresh_observed(&mut channel, Instant::now() + Duration::from_secs(60)).await.unwrap_err();
    sender.await.unwrap();
    assert_eq!(failed.metadata.unwrap().kind, RegisterFailureKind::IdentityRejected);
    assert_eq!(failed.retained_until, None);
    assert_eq!(failed.code(), "vowifi_refresh_rejected");
    assert!(live_register_retry_remaining_for_line(&factory.line_id).await.is_some());
}

async fn tcp_request(socket: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        loop {
            if sip_frame::complete_frame_len(&bytes).is_some_and(|length| length == bytes.len()) { return bytes; }
            let mut byte = [0]; socket.read_exact(&mut byte).await.unwrap(); bytes.push(byte[0]);
            assert!(bytes.len() < 32768);
        }
    }).await.unwrap()
}

#[tokio::test]
async fn real_tcp_eof_during_refresh_is_dead_flow_not_a_lease_preserving_timeout() {
    let (factory, _udp_channel, _udp_peer, _) = observed_fixture().await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let route = crate::connectivity::core::context::ImsRoute { local_addr: socket.local_addr().unwrap(),
        pcscf_addr: listener.local_addr().unwrap(), transport: crate::connectivity::core::context::SipTransport::Tcp };
    let mut channel = SipChannel::new(SipChannelSocket::Tcp(socket), vec![], route, Some(VERIFY.into()));
    let sender = tokio::spawn(async move {
        let (mut peer, _) = listener.accept().await.unwrap();
        tcp_request(&mut peer).await;
        peer.shutdown().await.unwrap();
    });
    let failed = factory.refresh_observed(&mut channel, Instant::now() + Duration::from_secs(60)).await.unwrap_err();
    sender.await.unwrap();
    assert_eq!(failed.code(), "ims_outbound_flow_failed");
    assert_eq!(failed.metadata.unwrap().kind, RegisterFailureKind::FlowFailed);
    assert_eq!(failed.retained_until, None);
}

#[tokio::test]
async fn installed_operator_keeps_receiving_during_wait_then_expires_at_original_deadline() {
    let (factory, channel, peer, _) = observed_fixture().await;
    let line = factory.line_id.clone(); let route = channel.route();
    let expiry = Instant::now() + Duration::from_millis(1500);
    let context = RegisteredVoiceContext {
        line_id: line.clone(), profile_id: factory.profile.meta.profile_id,
        identity: factory.context.identity.shared.clone(), route,
        registration: RegisteredImsContext::from_response(ImsRegistrationAccess::Vowifi,
            b"SIP/2.0 200 OK\r\nExpires: 3600\r\n\r\n", 3600),
        security_verify: Some(VERIFY.into()), pani: "IEEE-802.11".into(), user_agent: "r2-fixture".into(),
        expires_at: expiry, tcp_keepalive_interval: None, options_ping_interval: None,
        unregister: Some(factory.clone()), media_route_installer: None, media_interface: None,
        media_operator_creator: crate::connectivity::core::media::test_operator_socket_creator(),
    };
    install_registered_channel(context, channel).await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while !operator_link_for_line(&line).is_available() { tokio::task::yield_now().await; }
    }).await.unwrap();
    let sender = tokio::spawn(async move {
        let (request, source) = receive(&peer).await;
        peer.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
        peer
    });
    let failure = refresh_registered_for_line(&line, factory.profile.meta.profile_id).await.unwrap().unwrap_err();
    let peer = sender.await.unwrap();
    assert_eq!(failure.retained_until, Some(expiry));
    assert!(operator_link_for_line(&line).is_available());
    let options = b"OPTIONS sip:flow@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bK-r2-options\r\nFrom: <sip:peer@ims.example.test>;tag=remote\r\nTo: <sip:flow@ims.example.test>\r\nCall-ID: r2-options\r\nCSeq: 1 OPTIONS\r\nContent-Length: 0\r\n\r\n";
    peer.send_to(options, route.local_addr).await.unwrap();
    assert_eq!(sip_frame::parse_status(&receive(&peer).await.0).unwrap(), 200);
    tokio::time::timeout(Duration::from_secs(3), async {
        while operator_link_for_line(&line).is_available() { tokio::time::sleep(Duration::from_millis(20)).await; }
    }).await.unwrap();
    assert!(Instant::now() >= expiry);
    assert!(live_register_retry_remaining_for_line(&line).await.unwrap() > Duration::from_secs(115));
    assert_eq!(factory.expires_seconds.load(Ordering::Relaxed), 3600);
}
