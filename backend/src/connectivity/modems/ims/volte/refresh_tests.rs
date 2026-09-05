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

async fn protected_session() -> (VolteLiveSession, VolteRuntime, UdpSocket, UdpSocket) {
    let (live, runtime, server) = super::tests::test_voice_session().await;
    let mut session = live.session.lock().await.take().unwrap();
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
            Some(pcscf.security_client_value()),
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
            .build(ue, session.profile),
    );
    session.xfrm_plan = Some(ipsec::build_install_plan(ip, ip, &ue, &pcscf, &[3; 16]).unwrap());
    let mut authorization = VolteRefreshAuthorization::new(challenge("old-nonce"), aka());
    authorization.nonce_count = 2;
    session.refresh_authorization = Some(authorization);
    (session, (*runtime).clone(), server, client)
}

fn authenticator(
    session: &VolteLiveSession,
    runtime: &VolteRuntime,
    offered: SecAgree,
) -> VolteRegisterAuthenticator {
    let security = session
        .register_variant
        .security_client_offer
        .build(offered, session.profile);
    let mut authorization = session.refresh_authorization.clone().unwrap();
    let uri = sip::register_request_uri_with_target(
        session.profile,
        effective_register_target(&session.effective_ims),
        &session.channel.route(),
    );
    let header = authorization
        .authorization_for(&session.registration_identity, &uri)
        .unwrap();
    VolteRegisterAuthenticator::new(
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
        Some((selected, selected.security_client_value())),
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
    auth.prepare_aka_result(
        challenge("new-nonce"),
        aka(),
        Some((selected, selected.security_client_value())),
        &mut session.channel,
    )
    .await
    .unwrap();
    auth.authenticated_request(b"", 2).await.unwrap();
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
        b"SIP/2.0 401 Unauthorized\r\nSecurity-Server: ipsec-3gpp;alg=hmac-md5-96;ealg=null;spi-c=1;spi-s=0;port-c=33174;port-s=6000\r\n\r\n",
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
                Some(format!("<{}>;tag={expected_from_tag}", expected_identity.public_uri))
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
        assert_eq!(runtime.status().await.public_uri.as_deref(), Some(default_uri));
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
async fn unregister_targets_original_binding_not_originating_default() {
    let (session, runtime, server, _old_client) = protected_session().await;
    let expected_identity = session.registration_identity.clone();
    let expected_from_tag = session.register_ids.from_tag.clone();
    let old_route = session.channel.send_route();
    let live = VolteLiveHandle::new();
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
            Some(format!("<{}>;tag={expected_from_tag}", expected_identity.public_uri))
        );
        assert_eq!(
            sip::header_value(&request, "Expires"),
            Some("0".to_string())
        );
        assert!(sip::header_value(&request, "Security-Verify").is_some());
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
