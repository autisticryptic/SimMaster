//! Hardware-free regressions for the protected REGISTER refresh procedure.
use super::*;
use crate::connectivity::modems::ims::vowifi::qmi_uim::UsimAkaApduResult;
use tokio::net::UdpSocket;

fn aka() -> UsimAkaApduResult {
    UsimAkaApduResult {
        res: vec![1; 8],
        ck: vec![2; 16],
        ik: vec![3; 16],
        auts: None,
    }
}

fn challenge(nonce: &str) -> digest_aka::DigestChallenge {
    digest_aka::DigestChallenge {
        realm: "ims.example".into(),
        nonce: nonce.into(),
        algorithm: "AKAv1-MD5".into(),
        qop: Some("auth".into()),
        opaque: None,
        proxy: false,
    }
}

async fn protected_session() -> (
    CellularImsLiveSession,
    CellularImsRuntime,
    UdpSocket,
    UdpSocket,
) {
    // Legacy MD5/null fixture remains explicit, not a derived default.
    let mut profile = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
    profile.ims.register.security_client_mechanisms = &["hmac-md5-96/null/esp/trans"];
    protected_session_with_profile(Box::leak(Box::new(profile))).await
}

async fn protected_session_with_profile(
    profile: &'static CarrierProfile,
) -> (
    CellularImsLiveSession,
    CellularImsRuntime,
    UdpSocket,
    UdpSocket,
) {
    let (live, runtime, server) = super::tests::test_voice_session().await;
    let mut session = live.session.lock().await.take().unwrap();
    session.profile = profile;
    session.effective_ims = resolve_effective_ims_profile(session.profile, None);
    // Model the real USIM trace: REGISTER uses a temporary IMPU, whereas
    // P-Associated-URI selects an MSISDN identity for originating services.
    session.registration_identity.public_uri = "sip:234330000000001@ims.example".into();
    assert_ne!(
        session.registration_identity.public_uri,
        session.identity.public_uri
    );
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (port_c, port_s) = session.channel.reserve_security_ports_for_test(0);
    let ue = SecAgree {
        spi_c: 0x1001,
        spi_s: 0x1002,
        port_c,
        port_s,
    };
    let pcscf = SecAgree {
        spi_c: 0x2001,
        spi_s: 0x2002,
        port_c: client.local_addr().unwrap().port(),
        port_s: server.local_addr().unwrap().port(),
    };
    let ip = session.channel.route().local_addr.ip();
    let server_offer = CellularImsSecurityClientOffer::Full
        .build(pcscf, profile)
        .unwrap();
    let algorithms = select_security_server(profile, &[server_offer.clone()])
        .unwrap()
        .unwrap()
        .algorithms;
    session
        .channel
        .activate_security_in_worker(
            ImsRoute {
                local_addr: SocketAddr::new(ip, port_c),
                pcscf_addr: server.local_addr().unwrap(),
                transport: SipTransport::Udp,
            },
            SocketAddr::new(ip, port_s),
            client.local_addr().unwrap(),
            Some(server_offer),
            &session.xfrm_worker,
        )
        .await
        .unwrap();
    session.channel.commit_security();
    session.security_binding = ue;
    session.security_client = Some(
        session
            .register_variant
            .security_client_offer
            .build(ue, session.profile)
            .unwrap(),
    );
    session.xfrm_plan = Some(
        ipsec::build_install_plan_with_algs(ip, ip, &ue, &pcscf, &[3; 16], &[2; 16], algorithms)
            .unwrap(),
    );
    let mut authorization = CellularImsRefreshAuthorization::new(challenge("old-nonce"), aka());
    authorization.nonce_count = 2;
    session.refresh_authorization = Some(authorization);
    (session, (*runtime).clone(), server, client)
}

fn authenticator(
    session: &CellularImsLiveSession,
    runtime: &CellularImsRuntime,
    offered: SecAgree,
) -> CellularImsRegisterAuthenticator {
    let security = session.register_variant.build_security_offer(offered, session.profile).unwrap();
    let mut authorization = session.refresh_authorization.clone().unwrap();
    let uri = sip::register_request_uri_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        &session.channel.route(),
    );
    let header = authorization
        .authorization_for(&session.registration_identity, &uri)
        .unwrap();
    CellularImsRegisterAuthenticator::new(
        session.registration_identity.clone(),
        RequestIds::fresh(10),
        session.sip_instance.clone(),
        offered,
        security.clone(),
        session.channel.route(),
        session.device.clone(),
        runtime.clone(),
        false,
        Vec::new(),
        session.register_variant.policy,
        session.profile,
        session.effective_ims.clone(),
        None,
        None,
        Some(header),
        Some(authorization),
        Some(security),
        session.channel.security_verify().map(str::to_string),
        session.xfrm_worker.clone(),
    )
    .with_worker_binding(session.worker_binding.clone())
    .with_security_mechanism(session.register_variant.security_mechanism)
}

async fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = vec![0; 8192];
    let (n, peer) = tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    buffer.truncate(n);
    (buffer, peer)
}

fn response(request: &[u8], status: &str, extra: &str) -> Vec<u8> {
    format!("SIP/2.0 {status}\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\n{extra}Content-Length: 0\r\n\r\n",
        sip::header_value(request, "Via").unwrap(),
        sip::header_value(request, "Call-ID").unwrap(),
        sip::header_value(request, "CSeq").unwrap(),
    ).into_bytes()
}

#[tokio::test]
async fn second_sha1_offer_registers_on_frozen_tuple_and_rolls_back_without_touching_old_sa() {
    use crate::connectivity::modems::ims::vowifi::profiles::{
        derive_standard_3gpp_profile, Standard3gppAccess,
    };
    let profile = derive_standard_3gpp_profile("234", "33", Standard3gppAccess::LteEpc).unwrap();
    assert!(profile.ims.register.strict_security_server_offer);
    let (mut session, runtime, server, _old_client) = protected_session_with_profile(profile).await;
    let old_route = session.channel.send_route();
    let old_verify = session.channel.security_verify().unwrap().to_string();
    assert!(session
        .xfrm_plan
        .as_ref()
        .unwrap()
        .states
        .iter()
        .all(|s| s.algs.enc == "cbc(aes)" && s.enc_key == vec![2; 16]));
    let (port_c, port_s) = session
        .channel
        .reserve_security_ports_for_test(session.security_binding.port_s);
    let offered = offered_refresh_security(session.security_binding, port_c);
    assert_eq!(port_s, offered.port_s);
    let mut auth = authenticator(&session, &runtime, offered);
    let original = auth.initial_security_client.clone().unwrap();
    assert_eq!(original.split(", ").count(), 2);
    assert!(original.split(", ").nth(1).unwrap().contains("ealg=null"));
    let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let selected = SecAgree {
        spi_c: 0x7001,
        spi_s: 0x7002,
        port_c: peer.local_addr().unwrap().port(),
        port_s: server.local_addr().unwrap().port(),
    };
    let server_header = selected
        .security_client_value()
        .replace("hmac-md5-96", "hmac-sha-1-96");
    auth.prepare_aka_result(
        challenge("second-offer-nonce"),
        aka(),
        select_security_server(profile, &[server_header.clone()]).unwrap(),
        &mut session.channel,
    )
    .await
    .unwrap();
    let request = auth.authenticated_request(b"", 2).await.unwrap();
    assert_eq!(
        sip::header_value(&request, "Security-Client"),
        Some(original)
    );
    assert_eq!(
        sip::header_value(&request, "Security-Verify"),
        Some(server_header)
    );
    assert_eq!(auth.offered_security_binding, offered);
    for state in &auth.xfrm_plan.as_ref().unwrap().states {
        assert_eq!(state.algs.auth, "hmac(sha1)");
        assert_eq!(state.algs.enc, "cipher_null");
        assert!(state.enc_key.is_empty());
        assert_eq!(state.auth_key, aka().ik);
    }
    assert_eq!(
        session.channel.send_route().pcscf_addr,
        server.local_addr().unwrap()
    );
    session.channel.send_sip(&request).await.unwrap();
    assert_eq!(receive(&server).await.1.port(), offered.port_c);
    let ok = response(&request, "200 OK", "Expires: 3600\r\n");
    peer.send_to(&ok, session.channel.route().local_addr)
        .await
        .unwrap();
    assert_eq!(
        session
            .channel
            .recv_sip(Duration::from_secs(1))
            .await
            .unwrap(),
        ok
    );
    auth.rollback_security(&mut session.channel).await;
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    assert!(session
        .xfrm_plan
        .as_ref()
        .unwrap()
        .states
        .iter()
        .all(|s| s.algs.enc == "cbc(aes)"));
}

#[tokio::test]
async fn retained_security_ports_cannot_be_reused_for_a_different_frozen_offer() {
    let (mut session, _runtime, _server, _client) = protected_session().await;
    let (send, receive) = session
        .channel
        .reserve_security_ports_for_test(session.security_binding.port_s);
    let other = |port: u16| if port == 65535 { port - 1 } else { port + 1 };
    assert_eq!(
        session
            .channel
            .reserve_security_send_port_in_worker_at(&session.xfrm_worker, other(send), receive)
            .await
            .unwrap_err()
            .code(),
        code::CHANNEL_SEND_PORT_MISMATCH
    );
    assert_eq!(
        session
            .channel
            .reserve_security_receive_port_in_worker_at(&session.xfrm_worker, other(receive))
            .await
            .unwrap_err()
            .code(),
        code::CHANNEL_RECEIVE_PORT_MISMATCH
    );
    assert_eq!(
        session
            .channel
            .reserve_security_send_port_in_worker_at(&session.xfrm_worker, send, receive)
            .await
            .unwrap(),
        send
    );
    assert_eq!(
        session
            .channel
            .reserve_security_receive_port_in_worker_at(&session.xfrm_worker, receive)
            .await
            .unwrap(),
        receive
    );
}

#[tokio::test]
async fn stale_mm_authenticator_rejects_aka_and_min_expires_before_io() {
    let (mut session, runtime, _server, _old_client) = protected_session().await;
    let task = runtime.for_generation(runtime.generation());
    let mut auth = authenticator(&session, &task, session.security_binding);
    runtime.reset_runtime("sim_changed").await;
    // Invalid input deliberately proves cancellation wins before parsing or AKA.
    assert_eq!(
        auth.prepare_authenticated_channel(b"not a challenge", &mut session.channel)
            .await
            .unwrap_err()
            .code(),
        code::RUNTIME_NOT_RUNNING
    );
    assert_eq!(
        auth.authenticated_request(b"", 2).await.unwrap_err().code(),
        code::RUNTIME_NOT_RUNNING
    );
    assert_eq!(
        auth.rebuild_register_with_min_expires(b"", 2, 3600, false)
            .await
            .unwrap_err()
            .code(),
        code::RUNTIME_NOT_RUNNING
    );
}

#[tokio::test]
async fn challenged_refresh_freezes_offer_and_rolls_back_sockets_and_nonce_on_timeout() {
    let (mut session, runtime, server, old_client) = protected_session().await;
    let old_route = session.channel.send_route();
    let old_verify = session.channel.security_verify().unwrap().to_string();
    let old_binding = session.security_binding;
    let (port_c, port_s) = session
        .channel
        .reserve_security_ports_for_test(old_binding.port_s);
    let offered = offered_refresh_security(old_binding, port_c);
    assert_eq!(port_s, old_binding.port_s);
    let mut auth = authenticator(&session, &runtime, offered);
    let offered_header = auth.initial_security_client.clone().unwrap();
    // SM1 remains on the old socket/SA, although it already advertises the new offer.
    let initial = sip::build_register_from_profile_with_target_visited_and_access(
        session.profile,
        effective_register_target(&session.effective_ims),
        sip::RegisterPhase::Refresh,
        &session.registration_identity,
        &session.channel.route(),
        &auth.ids,
        3600,
        auth.initial_authorization.as_deref(),
        Some(&offered_header),
        Some(&old_verify),
        &session.sip_instance,
        session.register_variant.policy,
        None,
        None,
    );
    session.channel.send_sip(&initial).await.unwrap();
    assert_eq!(receive(&server).await.1, old_route.local_addr);

    let new_client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let selected = SecAgree {
        spi_c: 0x3001,
        spi_s: 0x3002,
        port_c: new_client.local_addr().unwrap().port(),
        port_s: server.local_addr().unwrap().port(),
    };
    auth.prepare_aka_result(
        challenge("new-nonce"),
        aka(),
        select_security_server(session.profile, &[selected.security_client_value()]).unwrap(),
        &mut session.channel,
    )
    .await
    .unwrap();
    let authenticated = auth.authenticated_request(b"", 2).await.unwrap();
    for request in [&initial, &authenticated] {
        assert_eq!(
            sip::header_value(request, "To"),
            Some(format!("<{}>", session.registration_identity.public_uri))
        );
        assert_eq!(
            sip::header_value(request, "From"),
            Some(format!(
                "<{}>;tag={}",
                session.registration_identity.public_uri, auth.ids.from_tag
            ))
        );
    }
    assert_eq!(
        sip::header_value(&initial, "Security-Client"),
        sip::header_value(&authenticated, "Security-Client")
    );
    assert_eq!(auth.offered_security_binding, offered);
    assert_eq!(
        session.channel.send_route().local_addr.port(),
        offered.port_c
    );
    assert_eq!(
        session.channel.route().local_addr.port(),
        old_binding.port_s
    );
    session.channel.send_sip(&authenticated).await.unwrap();
    assert_eq!(receive(&server).await.1.port(), offered.port_c);
    // During the tentative exchange an error/NOTIFY can still arrive on the old SA.
    old_client
        .send_to(b"old protected frame", session.channel.route().local_addr)
        .await
        .unwrap();
    assert_eq!(
        session
            .channel
            .recv_sip(Duration::from_secs(1))
            .await
            .unwrap(),
        b"old protected frame"
    );
    assert!(session
        .channel
        .recv_sip(Duration::from_millis(20))
        .await
        .is_err());
    auth.rollback_security(&mut session.channel).await;
    assert!(auth.xfrm_plan.is_none());
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    let old_auth = auth.refresh_authorization_after_failure().unwrap();
    assert_eq!(old_auth.challenge.nonce, "old-nonce");
    assert_eq!(
        old_auth.nonce_count, 3,
        "do not reuse an emitted nonce-count"
    );
    session
        .channel
        .send_sip(b"retry on old association")
        .await
        .unwrap();
    assert_eq!(receive(&server).await.1, old_route.local_addr);
    old_client
        .send_to(
            b"old channel still receives",
            session.channel.route().local_addr,
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .channel
            .recv_sip(Duration::from_secs(1))
            .await
            .unwrap(),
        b"old channel still receives"
    );
}

#[tokio::test]
async fn successful_rollover_keeps_old_inbound_path_until_next_procedure() {
    let (mut session, runtime, server, old_client) = protected_session().await;
    let (port_c, _) = session
        .channel
        .reserve_security_ports_for_test(session.security_binding.port_s);
    let offered = offered_refresh_security(session.security_binding, port_c);
    let mut auth = authenticator(&session, &runtime, offered);
    let new_client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let selected = SecAgree {
        spi_c: 0x3001,
        spi_s: 0x3002,
        port_c: new_client.local_addr().unwrap().port(),
        port_s: server.local_addr().unwrap().port(),
    };
    let server_values = vec![
        "tls;q=0.9".to_string(),
        format!("{};q=0.8", selected.security_client_value()),
    ];
    auth.prepare_aka_result(
        challenge("new-nonce"),
        aka(),
        select_security_server(session.profile, &server_values).unwrap(),
        &mut session.channel,
    )
    .await
    .unwrap();
    let authenticated = auth.authenticated_request(b"", 2).await.unwrap();
    assert_eq!(
        sip::header_value(&authenticated, "Security-Verify"),
        Some(server_values.join(", "))
    );
    let incoming = session.channel.route().local_addr;
    new_client
        .send_to(b"SIP/2.0 200 OK\r\n\r\n", incoming)
        .await
        .unwrap();
    let ok = session
        .channel
        .recv_sip(Duration::from_secs(1))
        .await
        .unwrap();
    let next_auth = auth.refresh_authorization_after_success(&ok).unwrap();
    assert_eq!(next_auth.challenge.nonce, "new-nonce");
    assert_eq!(next_auth.nonce_count, 1);
    session.channel.commit_security();
    session.channel.commit_security(); // idempotent; no second replacement
    session.channel.rollback_security(); // cannot roll back a committed exchange
    assert_eq!(
        session.channel.send_route().local_addr.port(),
        offered.port_c
    );
    for (peer, frame) in [
        (&old_client, b"delayed old NOTIFY".as_slice()),
        (&new_client, b"new OPTIONS".as_slice()),
    ] {
        peer.send_to(frame, incoming).await.unwrap();
        assert_eq!(
            session
                .channel
                .recv_sip(Duration::from_secs(1))
                .await
                .unwrap(),
            frame
        );
    }
    session.channel.discard_retired_security();
    new_client
        .send_to(b"new channel survives retirement", incoming)
        .await
        .unwrap();
    assert_eq!(
        session
            .channel
            .recv_sip(Duration::from_secs(1))
            .await
            .unwrap(),
        b"new channel survives retirement"
    );
}

#[tokio::test]
async fn invalid_zero_spi_challenge_is_rejected_before_aka_without_mutating_channel() {
    let (mut session, runtime, server, _) = protected_session().await;
    let old_route = session.channel.send_route();
    let old_verify = session.channel.security_verify().unwrap().to_string();
    let (port_c, _) = session
        .channel
        .reserve_security_ports_for_test(session.security_binding.port_s);
    let mut auth = authenticator(
        &session,
        &runtime,
        offered_refresh_security(session.security_binding, port_c),
    );
    // Captured failure shape: syntactically present but unusable Security-Server.
    let error = auth.prepare_authenticated_channel(
        b"SIP/2.0 401 Unauthorized\r\nSecurity-Server: ipsec-3gpp;alg=hmac-md5-96;ealg=null;prot=esp;mod=trans;spi-c=1;spi-s=0;port-c=33174;port-s=6000\r\n\r\n",
        &mut session.channel,
    ).await.unwrap_err();
    assert_eq!(error.code(), code::SECURITY_SERVER_INVALID);
    auth.rollback_security(&mut session.channel).await;
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    assert!(auth.xfrm_plan.is_none());
    session.channel.send_sip(b"still protected").await.unwrap();
    assert_eq!(receive(&server).await.1, old_route.local_addr);
}

#[tokio::test]
async fn direct_200_refresh_discards_offer_without_replacing_active_association() {
    let (mut session, runtime, server, _) = protected_session().await;
    let old_route = session.channel.send_route();
    let old_binding = session.security_binding;
    let old_security_client = session.security_client.clone();
    let old_verify = session.channel.security_verify().unwrap().to_string();
    session
        .channel
        .reserve_security_ports_for_test(old_binding.port_s);
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let peer = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        assert_eq!(source, old_route.local_addr);
        let offered =
            ipsec::parse_security_server(&sip::header_value(&request, "Security-Client").unwrap())
                .unwrap();
        assert_ne!(offered.port_c, old_binding.port_c);
        assert_eq!(offered.port_s, old_binding.port_s);
        assert_eq!(
            sip::header_value(&request, "Security-Verify"),
            Some(old_verify)
        );
        server
            .send_to(&response(&request, "200 OK", "Expires: 3600\r\n"), source)
            .await
            .unwrap();
        server
    });
    let result = refresh_live_registration(&mut session, &runtime, "refresh-test", &db).await;
    let _server = peer.await.unwrap();
    assert!(matches!(
        result.outcome,
        RegistrationRefreshResult::Refreshed(_)
    ));
    assert_eq!(session.security_binding, old_binding);
    assert_eq!(session.security_client, old_security_client);
    assert_eq!(session.channel.send_route(), old_route);
    assert!(session.retired_xfrm_plan.is_none());
    assert_eq!(
        session.refresh_authorization.as_ref().unwrap().nonce_count,
        3
    );
    assert_eq!(session.next_register_cseq, 3);
}

#[tokio::test]
async fn refresh_keeps_registered_aor_when_associated_default_identity_changes() {
    let (mut session, runtime, mut server, _old_client) = protected_session().await;
    let registered_identity = session.registration_identity.clone();
    let old_route = session.channel.send_route();
    let old_binding = session.security_binding;
    let old_verify = session.channel.security_verify().unwrap().to_string();
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();

    // A second 200 can change the default again. Neither update is permission
    // to re-register a different AoR on the original Call-ID/contact/SA.
    for default_uri in [
        "sip:+441234567891@ims.example",
        "sip:+441234567892@ims.example",
    ] {
        session
            .channel
            .reserve_security_ports_for_test(old_binding.port_s);
        let expected_identity = registered_identity.clone();
        let expected_from_tag = session.register_ids.from_tag.clone();
        let expected_call_id = session.register_ids.call_id.clone();
        let expected_cseq = session.next_register_cseq;
        let expected_verify = old_verify.clone();
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            assert_eq!(source, old_route.local_addr);
            assert_eq!(
                sip::header_value(&request, "To"),
                Some(format!("<{}>", expected_identity.public_uri))
            );
            assert_eq!(
                sip::header_value(&request, "From"),
                Some(format!(
                    "<{}>;tag={expected_from_tag}",
                    expected_identity.public_uri
                ))
            );
            assert_eq!(
                sip::header_value(&request, "Call-ID"),
                Some(expected_call_id)
            );
            assert_eq!(
                sip::header_value(&request, "CSeq"),
                Some(format!("{expected_cseq} REGISTER"))
            );
            assert_eq!(
                sip::header_value(&request, "Security-Verify"),
                Some(expected_verify)
            );
            assert!(sip::header_value(&request, "Contact")
                .unwrap()
                .starts_with(&format!("<sip:{}@", expected_identity.contact_user)));
            let extra = format!("Expires: 3600\r\nP-Associated-URI: <{default_uri}>\r\n");
            server
                .send_to(&response(&request, "200 OK", &extra), source)
                .await
                .unwrap();
            server
        });
        let result =
            refresh_live_registration(&mut session, &runtime, "identity-refresh-test", &db).await;
        server = peer.await.unwrap();
        assert!(matches!(
            result.outcome,
            RegistrationRefreshResult::Refreshed(_)
        ));
        assert_eq!(session.registration_identity, registered_identity);
        assert_eq!(session.identity.public_uri, default_uri);
        assert_eq!(
            runtime.status().await.public_uri.as_deref(),
            Some(default_uri)
        );
        assert_eq!(session.channel.send_route(), old_route);
        assert_eq!(session.security_binding, old_binding);
        assert!(session.retired_xfrm_plan.is_none());
        // Originating service requests must still use the network's default,
        // rather than accidentally exposing the temporary registration IMPU.
        let options = sip::build_options(
            &session.identity,
            &session.channel.route(),
            None,
            1,
            session.channel.security_verify(),
        );
        assert_eq!(
            sip::header_value(&options, "To"),
            Some(format!("<{default_uri}>"))
        );
    }
    assert_eq!(runtime.status().await.register_refresh_count, 2);
}

#[tokio::test]
async fn hint_restriction_is_retained_in_real_refresh_and_423_rebuild() {
    use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};
    let profile = derive_standard_3gpp_profile("460", "02", Standard3gppAccess::LteEpc).unwrap();
    let (mut session, runtime, server, _old_client) = protected_session_with_profile(profile).await;
    session.register_variant = CellularImsRegisterVariant { security_mechanism: Some(0), ..register_variants(profile)[0] };
    session.security_client = Some(session.register_variant.build_security_offer(session.security_binding, profile).unwrap());
    let old_port = session.security_binding.port_s;
    session.channel.reserve_security_ports_for_test(old_port);
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&server).await;
        let offer = sip::header_value(&first, "Security-Client").unwrap();
        assert!(!offer.contains(',')); assert!(offer.contains("ealg=aes-cbc"));
        server.send_to(&response(&first, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        let (second, peer) = receive(&server).await;
        assert_eq!(source, peer);
        assert_eq!(sip::header_value(&second, "Security-Client"), Some(offer));
        assert_eq!(sip::header_value(&first, "Security-Verify"), sip::header_value(&second, "Security-Verify"));
        assert_eq!(sip::header_value(&second, "Expires").as_deref(), Some("7200"));
        server.send_to(&response(&second, "200 OK", "Expires: 7200\r\n"), source).await.unwrap();
    });
    let result = refresh_live_registration(&mut session, &runtime, "hint-refresh-test", &db).await;
    peer.await.unwrap();
    assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_)));
    assert_eq!(session.register_variant.security_mechanism, Some(0));
}

#[tokio::test]
async fn hint_authenticator_rejects_changed_challenge_before_aka_and_keeps_auts_offer() {
    use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};
    let profile = derive_standard_3gpp_profile("460", "02", Standard3gppAccess::LteEpc).unwrap();
    let (mut session, runtime, _server, _old_client) = protected_session_with_profile(profile).await;
    session.register_variant = CellularImsRegisterVariant { security_mechanism: Some(0), ..register_variants(profile)[0] };
    let mut auth = authenticator(&session, &runtime, session.security_binding);
    let frozen = auth.offered_security.clone();
    assert!(!frozen.contains(','));
    // The deliberately invalid nonce would fail differently if the code went
    // past the singleton check toward AKA. No real UIM call is possible here.
    let outside = b"SIP/2.0 401 Unauthorized\r\nSecurity-Server: ipsec-3gpp;alg=hmac-sha-1-96;ealg=null;spi-c=50001;spi-s=50002;port-c=5068;port-s=5069\r\nWWW-Authenticate: Digest realm=\"ims.example\",nonce=\"invalid\",algorithm=AKAv1-MD5\r\n\r\n";
    assert_eq!(auth.prepare_authenticated_channel(outside, &mut session.channel).await.unwrap_err().code(), code::SECURITY_SERVER_INVALID);
    let mut resync = aka(); resync.auts = Some(vec![7; 14]);
    auth.prepare_aka_result(challenge("resync-nonce"), resync, None, &mut session.channel).await.unwrap();
    let prepared = auth.pending.as_ref().unwrap();
    assert_eq!(prepared.security_client.as_deref(), Some(frozen.as_str()));
    assert!(prepared.security_verify.is_some());
    assert!(prepared.register_policy.require_sec_agree);
}

#[tokio::test]
async fn hint_refresh_and_unregister_reject_an_outside_singleton_challenge() {
    use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};
    let profile = derive_standard_3gpp_profile("460", "02", Standard3gppAccess::LteEpc).unwrap();
    for unregister in [false, true] {
        let (mut session, runtime, server, _old_client) = protected_session_with_profile(profile).await;
        session.register_variant = CellularImsRegisterVariant { security_mechanism: Some(0), ..register_variants(profile)[0] };
        session.security_client = Some(session.register_variant.build_security_offer(session.security_binding, profile).unwrap());
        session.channel.reserve_security_ports_for_test(session.security_binding.port_s);
        let original_route = session.channel.send_route();
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            let offer = sip::header_value(&request, "Security-Client").unwrap();
            assert!(!offer.contains(',')); assert!(offer.contains("ealg=aes-cbc"));
            let challenge = "Security-Server: ipsec-3gpp;alg=hmac-sha-1-96;ealg=null;spi-c=50001;spi-s=50002;port-c=5068;port-s=5069\r\nWWW-Authenticate: Digest realm=\"ims.example\",nonce=\"invalid\",algorithm=AKAv1-MD5\r\n";
            server.send_to(&response(&request, "401 Unauthorized", challenge), source).await.unwrap();
            let mut bytes = [0; 8192];
            assert!(tokio::time::timeout(Duration::from_millis(100), server.recv_from(&mut bytes)).await.is_err(), "must not send another request after outside-singleton challenge");
        });
        if unregister {
            let live = CellularImsLiveHandle::new(); *live.session.lock().await = Some(session);
            assert_eq!(unregister_live_session(&live, &runtime).await, UnregisterResult::Rejected);
            assert_eq!(live.session.lock().await.as_ref().unwrap().channel.send_route(), original_route);
        } else {
            let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
            let result = refresh_live_registration(&mut session, &runtime, "hint-reject-refresh", &db).await;
            assert!(matches!(result.outcome, RegistrationRefreshResult::Retry));
            assert_eq!(session.channel.send_route(), original_route);
            assert_eq!(session.register_variant.security_mechanism, Some(0));
        }
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn confirmed_fallback_requirement_rejects_missing_security_before_aka() {
    use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};
    let profile = derive_standard_3gpp_profile("001", "01", Standard3gppAccess::LteEpc).unwrap();
    let (session, runtime, _server, _old_client) = protected_session_with_profile(profile).await;
    for (status, required) in [(421, "Require: sec-agree\r\n"), (421, "Proxy-Require: sec-agree\r\n"), (494, "")] {
        let first = register_variants(profile)[0];
        let mut state = register_fallback::RegisterFallbackState::new(first);
        state.observe(profile, first, &RegisterFailure {
            error: ImsError::new("ims_register_initial_unexpected_status"), auth_rounds: 0,
            response: Some(format!("SIP/2.0 {status} Required\r\n{required}\r\n").into_bytes()),
        }).unwrap();
        for (challenge_status, header) in [(401, "WWW-Authenticate"), (407, "Proxy-Authenticate")] {
            let mut plain = CellularImsSipChannel::bind(ImsRoute {
                local_addr: "127.0.0.1:0".parse().unwrap(), ..session.channel.route()
            }, None, None).unwrap();
            let mut auth = authenticator(&session, &runtime, session.security_binding)
                .with_required_security(state.requires_protection(profile));
            // Invalid nonce guarantees that even a broken gate cannot call UIM;
            // the expected missing-security error must precede nonce decoding.
            let challenge = format!("SIP/2.0 {challenge_status} Challenge\r\n{header}: Digest realm=\"fixture.invalid\",nonce=\"invalid\",algorithm=AKAv1-MD5\r\n\r\n");
            let error = auth.prepare_authenticated_channel(challenge.as_bytes(), &mut plain).await.unwrap_err();
            assert_eq!(error.code(), code::SECURITY_SERVER_MISSING);
            assert!(auth.pending.is_none() && auth.xfrm_plan.is_none());
            assert!(plain.security_verify().is_none());
        }
    }
}

#[tokio::test]
async fn unregister_targets_original_binding_not_originating_default() {
    let (session, runtime, server, _old_client) = protected_session().await;
    let expected_identity = session.registration_identity.clone();
    let expected_from_tag = session.register_ids.from_tag.clone();
    let expected_cseq = session.next_register_cseq;
    let expected_call_id = session.register_ids.call_id.clone();
    let uri = sip::register_request_uri_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        &session.channel.route(),
    );
    let expected_authorization = session
        .refresh_authorization
        .clone()
        .unwrap()
        .authorization_for(&session.registration_identity, &uri)
        .unwrap();
    let old_route = session.channel.send_route();
    let live = CellularImsLiveHandle::new();
    *live.session.lock().await = Some(session);
    let peer = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        assert_eq!(source, old_route.local_addr);
        assert_eq!(
            sip::header_value(&request, "To"),
            Some(format!("<{}>", expected_identity.public_uri))
        );
        assert_eq!(
            sip::header_value(&request, "From"),
            Some(format!(
                "<{}>;tag={expected_from_tag}",
                expected_identity.public_uri
            ))
        );
        assert_eq!(
            sip::header_value(&request, "Expires"),
            Some("0".to_string())
        );
        assert!(sip::header_value(&request, "Security-Verify").is_some());
        // A registrar should not need to challenge an empty initial Digest on
        // a binding which already has AKA credentials. Verify the exact proof,
        // including the incremented nonce-count and original registration IMPU.
        assert_eq!(
            sip::header_value(&request, "Authorization"),
            Some(
                expected_authorization
                    .strip_prefix("Authorization: ")
                    .unwrap()
                    .to_string()
            )
        );
        assert!(expected_authorization.contains("nc=00000003"));
        assert_eq!(
            sip::header_value(&request, "Call-ID"),
            Some(expected_call_id)
        );
        assert_eq!(
            sip::header_value(&request, "CSeq"),
            Some(format!("{expected_cseq} REGISTER"))
        );
        server
            .send_to(&response(&request, "200 OK", "Expires: 0\r\n"), source)
            .await
            .unwrap();
    });
    assert_eq!(
        unregister_live_session(&live, &runtime).await,
        UnregisterResult::Confirmed
    );
    peer.await.unwrap();
}

#[tokio::test]
async fn unregister_nonce_exhaustion_does_not_send_empty_digest_or_claim_success() {
    let (mut session, runtime, server, _old_client) = protected_session().await;
    session.refresh_authorization.as_mut().unwrap().nonce_count = u32::MAX;
    let old_route = session.channel.send_route();
    let live = CellularImsLiveHandle::new();
    *live.session.lock().await = Some(session);
    assert_eq!(
        unregister_live_session(&live, &runtime).await,
        UnregisterResult::Rejected
    );
    let mut buffer = [0; 8192];
    assert_eq!(
        server.try_recv_from(&mut buffer).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(
        live.session
            .lock()
            .await
            .as_ref()
            .unwrap()
            .channel
            .send_route(),
        old_route
    );
}

#[tokio::test]
async fn temporary_refresh_honors_retry_after_on_the_existing_protected_binding() {
    let (mut session, runtime, server, _client) = protected_session().await;
    let old_route = session.channel.send_route();
    let old_binding = session.security_binding;
    let old_verify = session.channel.security_verify().unwrap().to_owned();
    let old_registered = session.registration.registered_at;
    let old_lease = session.registration.lease.clone();
    session.channel.reserve_security_ports_for_test(old_binding.port_s);
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let peer = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        server.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
        server
    });
    let attempt = refresh_live_registration(&mut session, &runtime, "temporary-refresh", &db).await;
    assert_eq!(attempt.outcome, RegistrationRefreshResult::Retry);
    assert!(attempt.retry_after.unwrap() >= Duration::from_secs(119));
    let server = peer.await.unwrap();
    let cseq = session.next_register_cseq;
    let gated = refresh_live_registration(&mut session, &runtime, "temporary-refresh", &db).await;
    assert_eq!(gated.outcome, RegistrationRefreshResult::Retry);
    assert_eq!(session.next_register_cseq, cseq);
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.security_binding, old_binding);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    assert_eq!(session.registration.registered_at, old_registered);
    assert_eq!(session.registration.lease, old_lease);
    assert_eq!(server.try_recv_from(&mut [0u8; 8192]).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[tokio::test]
async fn deferred_refresh_expiry_never_sends_early_or_extends_the_old_lease() {
    let (mut session, runtime, server, _client) = protected_session().await;
    session.registration.registered_at = std::time::SystemTime::now()
        - session.registration.lease.expires_after + Duration::from_secs(10);
    let failure = RegisterFailure { error: ImsError::new("ims_register_initial_unexpected_status"),
        response: Some(b"SIP/2.0 503 Service Unavailable\r\nRetry-After: 120\r\n\r\n".to_vec()), auth_rounds: 0 };
    session.endpoint_retries.lock().await.observe_failure(session.pcscf, &failure.metadata(), Instant::now());
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let attempt = refresh_live_registration(&mut session, &runtime, "expired-refresh", &db).await;
    assert_eq!(attempt.outcome, RegistrationRefreshResult::Retry);
    assert!(attempt.retry_after.unwrap() <= Duration::from_secs(10));
    session.registration.registered_at = std::time::SystemTime::now() - session.registration.lease.expires_after - Duration::from_secs(1);
    let expired = refresh_live_registration(&mut session, &runtime, "expired-refresh", &db).await;
    assert_eq!(expired.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::Expired));
    assert!(!session.endpoint_retries.lock().await.restart_admission(Instant::now()).is_allowed());
    assert_eq!(server.try_recv_from(&mut [0u8; 8192]).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[tokio::test]
async fn repeated_timeouts_never_downgrade_refresh_to_plaintext() {
    let (mut session, runtime, server, _client) = protected_session().await;
    let old_route = session.channel.send_route();
    let old_binding = session.security_binding;
    let old_verify = session.channel.security_verify().unwrap().to_string();
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let observer = tokio::spawn(async move {
        let mut buffer = vec![0; 8192];
        loop {
            let (n, source) = server.recv_from(&mut buffer).await.unwrap();
            if tx.send((buffer[..n].to_vec(), source)).is_err() {
                break;
            }
        }
    });
    // Exercise the real RFC 3261 Timer E/F, including the third attempt where
    // the old implementation abandoned the protected channel. No modem needed.
    for attempt in 0..3 {
        session
            .channel
            .reserve_security_ports_for_test(old_binding.port_s);
        let result = refresh_live_registration(&mut session, &runtime, "refresh-test", &db).await;
        assert_eq!(result.outcome, RegistrationRefreshResult::Retry);
        assert!(result.retry_after.is_some());
        assert_eq!(session.channel.send_route(), old_route);
        assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
        assert_eq!(session.security_binding, old_binding);
        let mut first = None;
        let mut count = 0;
        while let Ok((frame, source)) = rx.try_recv() {
            assert_eq!(source, old_route.local_addr);
            assert_eq!(
                sip::header_value(&frame, "To"),
                Some(format!("<{}>", session.registration_identity.public_uri))
            );
            assert_eq!(
                sip::header_value(&frame, "Security-Verify"),
                Some(old_verify.clone())
            );
            assert_eq!(
                sip::header_value(&frame, "CSeq"),
                Some(format!("{} REGISTER", 2 + attempt))
            );
            if let Some(initial) = &first {
                assert_eq!(&frame, initial, "retransmissions must be byte-identical");
            } else {
                first = Some(frame);
            }
            count += 1;
        }
        assert!(count >= 2, "must actually retransmit on the old tuple");
    }
    observer.abort();
    assert_eq!(session.next_register_cseq, 5);
    assert_eq!(
        session.refresh_authorization.as_ref().unwrap().nonce_count,
        5
    );
}
