//! REGISTER response-authentication regressions through cellular production
//! adapters and loopback-owned channels. Executed only in GitHub Actions.
use super::*;
use crate::connectivity::core::digest_aka as shared_digest;
use std::collections::HashMap;

fn profile() -> &'static CarrierProfile {
    crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc,
    ).unwrap()
}

// The synthetic UE worker deliberately cannot create real namespace sockets.
// Pre-reserve the next SM1 offer using the existing test-only socket adapter,
// just as the other real-refresh regressions do. Production reservation and
// activation still check those exact descriptors/ports; no gate is bypassed.
async fn refresh_session() -> (CellularImsLiveSession, CellularImsRuntime, UdpSocket, UdpSocket) {
    let (mut session, runtime, server, client) = protected_session_with_profile(profile()).await;
    let server_port = session.security_binding.port_s;
    session.channel.reserve_security_ports_for_test(server_port);
    (session, runtime, server, client)
}

// Only the locally generated synthetic headers below are parsed here. The
// production parser's escaping, duplicate and size limits have separate tests.
fn sent_params(request: &[u8]) -> (bool, HashMap<String, String>) {
    let (proxy, header) = if let Some(value) = sip::header_value(request, "Proxy-Authorization") {
        (true, value)
    } else {
        (false, sip::header_value(request, "Authorization").unwrap())
    };
    let params = header.strip_prefix("Digest ").unwrap().split(',').map(|part| {
        let (name, value) = part.trim().split_once('=').unwrap();
        (name.to_owned(), value.trim_matches('"').to_owned())
    }).collect();
    (proxy, params)
}

fn auth_info(request: &[u8], tamper: bool, nextnonce: Option<&str>) -> String {
    let (proxy, params) = sent_params(request);
    let material = aka();
    let password = shared_digest::aka_digest_password(&params["algorithm"], &shared_digest::AkaMaterial {
        res: &material.res, ck: &material.ck, ik: &material.ik,
    }).unwrap();
    let proof = shared_digest::compute_digest_response(
        &params["username"], &params["realm"], &password, &params["algorithm"],
        "", &params["uri"], &params["nonce"], params.get("qop").map(String::as_str),
        params.get("cnonce").map_or("", String::as_str),
        params.get("nc").map_or("00000001", String::as_str),
    ).unwrap();
    let proof = if tamper { "0".repeat(proof.len()) } else { proof };
    let mut value = format!("{}: rspauth=\"{proof}\"", if proxy { "Proxy-Authentication-Info" } else { "Authentication-Info" });
    if let Some(qop) = params.get("qop") {
        value.push_str(&format!(", qop={qop}, nc={}, cnonce=\"{}\"", params["nc"], params["cnonce"]));
    }
    if let Some(nonce) = nextnonce { value.push_str(&format!(", nextnonce=\"{nonce}\"")); }
    value.push_str("\r\n");
    value
}

fn initial_request(auth: &CellularImsRegisterAuthenticator) -> Vec<u8> {
    sip::build_register_from_profile_with_target_visited_and_access(
        auth.profile, effective_register_target(&auth.effective_ims), sip::RegisterPhase::Refresh,
        &auth.identity, &auth.route, &auth.ids, auth.expires_seconds,
        auth.initial_authorization.as_deref(), auth.initial_security_client.as_deref(),
        auth.refresh_security_verify.as_deref(), &auth.sip_instance, auth.register_policy, None, None,
    )
}

#[tokio::test]
async fn rspauth_direct_refresh_accepts_supported_algorithms_and_proxy_scopes() {
    for (algorithm, proxy, qop) in [
        ("AKAv1-MD5", false, true), ("AKAv1-MD5", true, false),
        ("AKAv2-MD5", true, true), ("AKAv2-SHA-256", false, true),
    ] {
        let (mut session, runtime, server, _client) = refresh_session().await;
        let state = session.refresh_authorization.as_mut().unwrap();
        state.challenge.algorithm = algorithm.into();
        state.challenge.proxy = proxy;
        state.challenge.qop = qop.then(|| "auth".into());
        let origin = state.aka_origin.clone();
        let old_route = session.channel.send_route();
        let old_binding = session.security_binding;
        let old_verify = session.channel.security_verify().unwrap().to_owned();
        let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            let headers = format!("{}Expires: 7200\r\n", auth_info(&request, false, Some("accepted-nextnonce")));
            server.send_to(&response(&request, "200 OK", &headers), source).await.unwrap();
        });
        let result = refresh_live_registration(&mut session, &runtime, "rspauth-direct", &db).await;
        peer.await.unwrap();
        assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_)));
        let accepted = session.refresh_authorization.as_ref().unwrap();
        assert_eq!(accepted.challenge.nonce, "accepted-nextnonce");
        assert_eq!(accepted.nonce_count, 0);
        assert_eq!(accepted.aka_origin, origin);
        assert_eq!(session.registration.lease.expires_seconds, 7200);
        assert_eq!(session.channel.send_route(), old_route);
        assert_eq!(session.security_binding, old_binding);
        assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
        assert!(session.retired_xfrm_plan.is_none());
        assert_eq!(runtime.status().await.register_refresh_count, 1);
    }
}

#[tokio::test]
async fn rspauth_invalid_direct_refresh_preserves_nonce_lease_identity_and_sa() {
    let (mut session, runtime, server, _client) = refresh_session().await;
    let old_registration = session.registration.clone();
    let old_identity = session.identity.clone();
    let old_route = session.channel.send_route();
    let old_binding = session.security_binding;
    let old_verify = session.channel.security_verify().unwrap().to_owned();
    let old_nc = session.refresh_authorization.as_ref().unwrap().nonce_count;
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let peer = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        let headers = format!("{}Expires: 86400\r\nP-Associated-URI: <sip:untrusted@ims.example>\r\n",
            auth_info(&request, true, Some("untrusted-nextnonce")));
        server.send_to(&response(&request, "200 OK", &headers), source).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(80), server.recv_from(&mut [0; 8192])).await.is_err(),
            "bad server proof must not reopen header/profile retries");
    });
    let result = refresh_live_registration(&mut session, &runtime, "rspauth-invalid", &db).await;
    peer.await.unwrap();
    assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::AuthenticationRejected));
    let error = result.error.unwrap();
    assert_eq!(error.code(), code::DIGEST_RESPONSE_AUTH_FAILED);
    assert_eq!(error.register_failure().unwrap().kind, crate::connectivity::core::register::RegisterFailureKind::AuthenticationRejected);
    assert_eq!(session.registration, old_registration);
    assert_eq!(session.identity, old_identity);
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.security_binding, old_binding);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    assert!(session.retired_xfrm_plan.is_none());
    let retained = session.refresh_authorization.as_ref().unwrap();
    assert_eq!(retained.challenge.nonce, "old-nonce");
    assert_eq!(retained.nonce_count, old_nc + 1, "an emitted nonce-count stays consumed");
    assert_eq!(runtime.status().await.register_refresh_count, 0);
}

async fn stale_refresh(tamper: bool) {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let mut vector = vec![0x41; 32];
    vector.push(1);
    let old_nonce = STANDARD.encode(&vector);
    *vector.last_mut().unwrap() = 2;
    let new_nonce = STANDARD.encode(&vector);
    let (mut session, runtime, server, _client) = refresh_session().await;
    let state = session.refresh_authorization.as_mut().unwrap();
    state.challenge.nonce = old_nonce.clone();
    state.aka_origin = Some(shared_digest::decode_aka_nonce(&old_nonce).unwrap());
    let old_registration = session.registration.clone();
    let old_route = session.channel.send_route();
    let old_verify = session.channel.security_verify().unwrap().to_owned();
    let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&server).await;
        let extra = format!("WWW-Authenticate: Digest realm=\"ims.example\",nonce=\"{new_nonce}\",algorithm=AKAv1-MD5,qop=auth,stale=true\r\n");
        server.send_to(&response(&first, "401 Unauthorized", &extra), source).await.unwrap();
        let (second, new_source) = receive(&server).await;
        assert_eq!(source, new_source);
        let (_, params) = sent_params(&second);
        assert_eq!(params["nonce"], new_nonce);
        assert_eq!(params["nc"], "00000001");
        let extra = format!("{}Expires: 7200\r\n", auth_info(&second, tamper, Some("after-stale")));
        server.send_to(&response(&second, "200 OK", &extra), new_source).await.unwrap();
    });
    let result = refresh_live_registration(&mut session, &runtime, "rspauth-stale", &db).await;
    peer.await.unwrap();
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    assert!(session.retired_xfrm_plan.is_none());
    if tamper {
        assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::AuthenticationRejected));
        assert_eq!(session.registration, old_registration);
        let retained = session.refresh_authorization.as_ref().unwrap();
        assert_eq!(retained.challenge.nonce, old_nonce);
        assert_eq!(retained.nonce_count, 3);
        assert_eq!(runtime.status().await.register_refresh_count, 0);
    } else {
        assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_)));
        let accepted = session.refresh_authorization.as_ref().unwrap();
        assert_eq!(accepted.challenge.nonce, "after-stale");
        assert_eq!(accepted.nonce_count, 0);
        assert_eq!(session.registration.lease.expires_seconds, 7200);
    }
}

#[tokio::test]
async fn rspauth_stale_refresh_uses_newly_emitted_proof() { stale_refresh(false).await; }

#[tokio::test]
async fn rspauth_failed_stale_refresh_does_not_commit_tentative_credentials() { stale_refresh(true).await; }

#[tokio::test]
async fn rspauth_423_refresh_uses_incremented_nc_and_rejects_prior_request_proof() {
    for stale_response in [false, true] {
        let (mut session, runtime, server, _client) = refresh_session().await;
        let old_registration = session.registration.clone();
        let db = Database::new(std::path::PathBuf::from(":memory:")).unwrap();
        let peer = tokio::spawn(async move {
            let (first, source) = receive(&server).await;
            server.send_to(&response(&first, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
            let (second, new_source) = receive(&server).await;
            assert_eq!(source, new_source);
            let (_, old_params) = sent_params(&first);
            let (_, new_params) = sent_params(&second);
            assert_eq!(old_params["nc"], "00000003");
            assert_eq!(new_params["nc"], "00000004");
            let proof_request = if stale_response { &first } else { &second };
            let extra = format!("{}Expires: 7200\r\n", auth_info(proof_request, false, Some("after-423")));
            server.send_to(&response(&second, "200 OK", &extra), new_source).await.unwrap();
        });
        let result = refresh_live_registration(&mut session, &runtime, "rspauth-423", &db).await;
        peer.await.unwrap();
        if stale_response {
            assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::AuthenticationRejected));
            assert_eq!(session.registration, old_registration);
            assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, 4);
            assert_eq!(session.refresh_authorization.as_ref().unwrap().challenge.nonce, "old-nonce");
        } else {
            assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_)));
            assert_eq!(session.registration.lease.expires_seconds, 7200);
            assert_eq!(session.refresh_authorization.as_ref().unwrap().challenge.nonce, "after-423");
        }
    }
}

#[tokio::test]
async fn rspauth_unregister_never_confirms_an_invalid_server_proof() {
    for tamper in [false, true] {
        let (session, runtime, server, _client) = protected_session_with_profile(profile()).await;
        let live = CellularImsLiveHandle::new();
        *live.session.lock().await = Some(session);
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            assert_eq!(sip::header_value(&request, "Expires").as_deref(), Some("0"));
            assert!(sip::header_value(&request, "Security-Verify").is_some());
            let extra = format!("{}Expires: 0\r\n", auth_info(&request, tamper, Some("not-a-new-lease")));
            server.send_to(&response(&request, "200 OK", &extra), source).await.unwrap();
        });
        let result = unregister_live_session(&live, &runtime).await;
        peer.await.unwrap();
        assert_eq!(result, if tamper { UnregisterResult::Rejected } else { UnregisterResult::Confirmed });
        let guard = live.session.lock().await;
        let state = guard.as_ref().unwrap().refresh_authorization.as_ref().unwrap();
        assert_eq!(state.challenge.nonce, "old-nonce", "removal does not install another registration nonce");
        assert_eq!(state.nonce_count, 3);
    }
}

#[tokio::test]
async fn rspauth_request_context_checks_final_uri_and_header_before_send() {
    let (session, runtime, _server, _client) = protected_session_with_profile(profile()).await;
    let auth = authenticator(&session, &runtime, session.security_binding);
    let request = initial_request(&auth);
    auth.validate_request(&request).unwrap();
    let altered = String::from_utf8(request).unwrap().replacen("REGISTER sip:", "REGISTER sips:", 1);
    assert_eq!(auth.validate_request(altered.as_bytes()).unwrap_err().code(), "digest_request_auth_mismatch");
}

#[tokio::test]
async fn rspauth_initial_no_proof_and_auts_cannot_borrow_prior_context() {
    let (mut session, runtime, _server, _client) = protected_session_with_profile(profile()).await;
    let mut auth = authenticator(&session, &runtime, session.security_binding);
    let mut empty = authenticator(&session, &runtime, session.security_binding);
    empty.refresh_authorization = None;
    empty.validate_success(b"SIP/2.0 200 OK\r\n\r\n").unwrap();
    assert_eq!(empty.validate_success(b"SIP/2.0 200 OK\r\nAuthentication-Info: nextnonce=\"unsolicited\"\r\n\r\n").unwrap_err().code(),
        "digest_response_auth_context_missing");
    let mut material = aka(); material.auts = Some(vec![1; 14]);
    auth.prepare_aka_result(challenge("resync-nonce"), material, None, &mut session.channel).await.unwrap();
    let request = auth.authenticated_request(b"", 2).await.unwrap();
    auth.validate_request(&request).unwrap();
    assert!(auth.emitted_authorization().is_none());
    assert_eq!(auth.validate_success(b"SIP/2.0 200 OK\r\n\r\n").unwrap_err().code(), "ims_aka_resync_unexpected_status");
    assert!(auth.refresh_authorization_after_success(b"SIP/2.0 200 OK\r\n\r\n").is_err());
}

#[tokio::test]
async fn rspauth_tentative_security_is_not_committed_on_invalid_response() {
    let (mut session, runtime, server, _old_client) = protected_session_with_profile(profile()).await;
    let old_route = session.channel.send_route();
    let old_verify = session.channel.security_verify().unwrap().to_owned();
    let (port_c, _) = session.channel.reserve_security_ports_for_test(session.security_binding.port_s);
    let offered = offered_refresh_security(session.security_binding, port_c);
    let mut auth = authenticator(&session, &runtime, offered);
    let new_peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let selected = SecAgree { spi_c: 0x3001, spi_s: 0x3002,
        port_c: new_peer.local_addr().unwrap().port(), port_s: server.local_addr().unwrap().port() };
    let verify = selected.security_client_value().replace("hmac-md5-96", "hmac-sha-1-96");
    auth.prepare_aka_result(challenge("new-challenge"), aka(),
        select_security_server(session.profile, &[verify]).unwrap(), &mut session.channel).await.unwrap();
    let request = auth.authenticated_request(b"", 2).await.unwrap();
    assert!(auth.xfrm_plan.is_some());
    let peer = tokio::spawn(async move {
        let (request, source) = receive(&server).await;
        let extra = format!("{}Expires: 7200\r\n", auth_info(&request, true, Some("untrusted")));
        server.send_to(&response(&request, "200 OK", &extra), source).await.unwrap();
    });
    let failure = run_register_observed(&mut session.channel, &request, &mut auth).await.unwrap_err();
    peer.await.unwrap();
    assert_eq!(failure.error.code(), "digest_response_auth_mismatch");
    assert!(auth.refresh_authorization_after_success(failure.response.as_deref().unwrap()).is_err());
    auth.rollback_security(&mut session.channel).await;
    assert!(auth.xfrm_plan.is_none());
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    let original = auth.refresh_authorization_after_failure().unwrap();
    assert_eq!(original.challenge.nonce, "old-nonce");
    assert_eq!(original.nonce_count, 3);
}
