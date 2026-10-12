//! Production cellular AKA/IPsec + rspauth path on real TCP loopback streams.
//! The UICC result and namespace/XFRM execution are injected only at existing
//! test seams; the authenticator, channel, selectors, refresh and rollback are real.
use super::*;
use tokio::{net::{TcpListener, TcpStream, TcpSocket}, io::{AsyncReadExt, AsyncWriteExt}};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn fixture() -> (CellularImsLiveSession, CellularImsRuntime, TcpListener, TcpStream) {
    static IDS: AtomicUsize = AtomicUsize::new(0);
    let (live, runtime, _udp_peer) = super::super::tests::test_voice_session().await;
    let mut session = live.session.lock().await.take().unwrap();
    let derived = crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc).unwrap();
    let mut profile = *derived; profile.ims.transport = "tcp";
    session.profile = Box::leak(Box::new(profile));
    session.effective_ims = resolve_effective_ims_profile(session.profile, None);
    session.register_variant = register_variants(session.profile)[0];
    session.device.line_id = format!("n6-live-stream-{}", IDS.fetch_add(1, Ordering::Relaxed));
    session.registration_identity.private_user = "user@ims.example".into();
    session.registration_identity.public_uri = "sip:registered@ims.example".into();
    session.registration_identity.contact_user = "user".into();
    session.identity = session.registration_identity.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
    let (peer, _) = listener.accept().await.unwrap();
    let route = ImsRoute { local_addr: socket.local_addr().unwrap(), pcscf_addr: listener.local_addr().unwrap(), transport: SipTransport::Tcp };
    session.channel = CellularImsSipChannel::from_tcp_for_test(socket, route, session.worker_binding.clone()).unwrap();
    session.channel.configure_outbound(&session.device.line_id, &session.sip_instance, true);
    session.pcscf = route.pcscf_addr;
    (session, (*runtime).clone(), listener, peer)
}

fn initial_auth(session: &CellularImsLiveSession, runtime: &CellularImsRuntime, offered: SecAgree) -> CellularImsRegisterAuthenticator {
    let security = session.register_variant.build_security_offer(offered, session.profile).unwrap();
    CellularImsRegisterAuthenticator::new(
        session.registration_identity.clone(), RequestIds::fresh(1), session.sip_instance.clone(), offered,
        security.clone(), session.channel.route(), session.device.clone(), runtime.clone(), false,
        session.aka_aid.clone(), session.register_variant.policy, session.profile, session.effective_ims.clone(),
        None, None, None, None, Some(security), None, session.xfrm_worker.clone(),
    ).with_auth_binding(session.auth_binding.clone()).with_worker_binding(session.worker_binding.clone())
}

async fn read_frame(socket: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        loop {
            if crate::connectivity::core::sip_frame::complete_frame_len(&bytes).is_some_and(|n| n == bytes.len()) { return bytes; }
            let mut byte = [0]; socket.read_exact(&mut byte).await.unwrap(); bytes.push(byte[0]);
        }
    }).await.unwrap()
}
fn param(header: &str, key: &str) -> String {
    header.trim_start_matches("Digest ").split(',').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == key).then(|| value.trim_matches('"').to_owned())
    }).unwrap()
}
fn success(request: &[u8], invalid: bool) -> Vec<u8> {
    let header = sip::header_value(request, "Authorization").unwrap();
    let challenge = digest_aka::parse_digest_challenge(&header, false).unwrap();
    let proof = crate::connectivity::core::digest_aka::compute_digest_response(
        &param(&header, "username"), &challenge.realm, &aka().res, &challenge.algorithm, "",
        &param(&header, "uri"), &challenge.nonce, challenge.qop.as_deref(), &param(&header, "cnonce"), &param(&header, "nc"),
    ).unwrap();
    let proof = if invalid { "0".repeat(proof.len()) } else { proof };
    format!("SIP/2.0 200 OK\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\nContact: {}\r\nExpires: 600\r\nAuthentication-Info: rspauth=\"{proof}\",qop=auth,nc={},cnonce=\"{}\",nextnonce=\"stream-next\"\r\nContent-Length: 0\r\n\r\n",
        sip::header_value(request, "Via").unwrap(), sip::header_value(request, "Call-ID").unwrap(),
        sip::header_value(request, "CSeq").unwrap(), sip::header_value(request, "Contact").unwrap(),
        param(&header,"nc"), param(&header,"cnonce")).into_bytes()
}

async fn prepare_protected(session: &mut CellularImsLiveSession, runtime: &CellularImsRuntime, listener: &TcpListener)
    -> (CellularImsRegisterAuthenticator, TcpStream, TcpSocket, Vec<u8>)
{
    use base64::Engine as _;
    let (port_c, port_s) = session.channel.reserve_stream_ports_for_test().await;
    let offered = offered_security(port_c, port_s);
    let mut auth = initial_auth(session, runtime, offered);
    let peer_client = TcpSocket::new_v4().unwrap(); peer_client.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let selected = SecAgree { spi_c: 0x3101, spi_s: 0x3102,
        port_c: peer_client.local_addr().unwrap().port(), port_s: listener.local_addr().unwrap().port() };
    let server = selected.security_client_value().replace("hmac-md5-96", "hmac-sha-1-96");
    let challenge = challenge(&base64::engine::general_purpose::STANDARD.encode([0x11; 32]));
    let frozen = auth.offered_security.clone();
    auth.prepare_aka_result(challenge, aka(), select_security_server(session.profile, &[server.clone()]).unwrap(), &mut session.channel).await.unwrap();
    let (peer, observed) = listener.accept().await.unwrap();
    assert_eq!(observed.port(), port_c);
    let frame = auth.authenticated_request(b"", 2).await.unwrap();
    assert_eq!(sip::header_value(&frame, "Security-Client"), Some(frozen));
    assert_eq!(sip::header_value(&frame, "Security-Verify"), Some(server));
    assert!(sip::header_value(&frame,"Via").unwrap().starts_with("SIP/2.0/TCP "));
    assert!(sip::header_value(&frame,"Via").unwrap().contains(&format!(":{port_s};")));
    let plan = auth.xfrm_plan.as_ref().unwrap();
    assert_eq!(plan.states.len(), 4);
    assert!(plan.states.iter().all(|state| state.transport == ipsec::XfrmTransport::Tcp));
    assert!(plan.policies.iter().all(|policy| policy.join(" ").contains("proto tcp")));
    (auth, peer, peer_client, frame)
}

#[tokio::test]
async fn real_tcp_authenticated_register_checks_rspauth_before_committing_four_direction_sa() {
    for invalid in [false, true] {
        let (mut session, runtime, listener, mut original_peer) = fixture().await;
        let original_route = session.channel.send_route();
        let (mut auth, mut peer, _peer_client, frame) = prepare_protected(&mut session, &runtime, &listener).await;
        let task = tokio::spawn(async move {
            let request = read_frame(&mut peer).await;
            peer.write_all(&success(&request, invalid)).await.unwrap();
            peer
        });
        let result = run_register_observed(&mut session.channel, &frame, &mut auth).await;
        let _peer = task.await.unwrap();
        if invalid {
            let failure = result.unwrap_err();
            assert_eq!(failure.error.code(), "digest_response_auth_mismatch");
            assert!(auth.refresh_authorization_after_success(failure.response.as_deref().unwrap()).is_err());
            auth.rollback_security(&mut session.channel).await;
            assert!(auth.xfrm_plan.is_none());
            assert_eq!(session.channel.send_route(), original_route);
            assert!(session.channel.security_verify().is_none());
            let probe = sip::build_options(&session.identity, &session.channel.route(), None, 10, None);
            session.channel.send_sip(&probe).await.unwrap();
            assert_eq!(read_frame(&mut original_peer).await, probe);
        } else {
            let result = result.unwrap();
            let accepted = auth.refresh_authorization_after_success(&result.response).unwrap().unwrap();
            assert_eq!(accepted.challenge.nonce, "stream-next");
            assert_eq!(accepted.nonce_count, 0);
            session.channel.outbound_registered(&result.response, 600).unwrap();
            session.channel.commit_security();
            assert!(session.channel.security_verify().is_some());
            assert!(session.channel.reg_event_binding().is_some());
            assert_ne!(session.channel.send_route(), original_route);
        }
    }
}

#[tokio::test]
async fn real_tcp_direct_refresh_reuses_established_connection_and_does_not_commit_bad_response() {
    for invalid in [false, true] {
        let (mut session, runtime, listener, _original_peer) = fixture().await;
        let (mut auth, mut peer, _peer_client, frame) = prepare_protected(&mut session, &runtime, &listener).await;
        let task = tokio::spawn(async move {
            let initial = read_frame(&mut peer).await;
            peer.write_all(&success(&initial, false)).await.unwrap();
            let refresh = read_frame(&mut peer).await;
            assert_eq!(param(&sip::header_value(&refresh,"Authorization").unwrap(),"nonce"), "stream-next");
            assert!(sip::header_value(&refresh,"Security-Verify").is_some());
            peer.write_all(&success(&refresh, invalid)).await.unwrap();
            peer
        });
        let registered = run_register_observed(&mut session.channel, &frame, &mut auth).await.unwrap();
        session.refresh_authorization = auth.refresh_authorization_after_success(&registered.response).unwrap();
        session.security_binding = auth.offered_security_binding;
        session.security_client = auth.initial_security_client.clone();
        session.xfrm_plan = auth.xfrm_plan.take();
        session.register_ids = auth.ids.clone(); session.next_register_cseq = auth.last_cseq + 1;
        let artifacts = session.channel.outbound_registered(&registered.response, 600).unwrap();
        session.registration = RegisteredImsContext::from_artifacts(ImsRegistrationAccess::CellularIms, artifacts, 600);
        session.channel.commit_security();
        let old_registration = session.registration.clone(); let old_route = session.channel.send_route();
        let old_verify = session.channel.security_verify().unwrap().to_owned();
        session.channel.reserve_stream_ports_for_test().await;
        let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
        let result = refresh_live_registration(&mut session, &runtime, "n6-refresh", &db).await;
        let _peer = task.await.unwrap();
        assert_eq!(session.channel.send_route(), old_route);
        assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
        assert!(session.retired_xfrm_plan.is_none());
        assert_eq!(session.xfrm_plan.as_ref().unwrap().states.len(), 4);
        if invalid {
            assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::AuthenticationRejected));
            assert_eq!(session.registration, old_registration);
            assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, 1);
        } else { assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_))); }
    }
}
