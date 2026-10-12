//! Real cellular REGISTER preparation paths, with synthetic SIM bindings and
//! loopback-owned protected channels. No UICC/APDU or real XFRM is executed.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};

fn nonce(vector: u8, suffix: u8) -> String {
    let mut bytes = vec![vector; 32]; bytes.push(suffix); STANDARD.encode(bytes)
}
fn synthetic_profile() -> &'static CarrierProfile {
    crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc,
    ).unwrap()
}

#[tokio::test]
async fn auts_register_preserves_qop_opaque_proxy_kind_and_protected_flow() {
    for (proxy, qop) in [(false, true), (true, true), (false, false), (true, false)] {
        let (mut session, runtime, _server, _client) = protected_session_with_profile(synthetic_profile()).await;
        let old_route = session.channel.send_route();
        let old_verify = session.channel.security_verify().unwrap().to_owned();
        let mut auth = authenticator(&session, &runtime, session.security_binding);
        let mut digest = challenge(&nonce(1, 1));
        digest.proxy = proxy;
        digest.qop = qop.then(|| "auth".to_owned());
        digest.opaque = Some("server,\"tag\\value".into());
        let mut material = aka(); material.auts = Some(vec![7; 14]);
        auth.prepare_aka_result(digest.clone(), material.clone(), None, &mut session.channel).await.unwrap();
        let frame = auth.authenticated_request(&[], 2).await.unwrap();
        let header = sip::header_value(&frame, digest.authorization_header_name()).unwrap();
        assert!(header.contains("response=\"\""));
        assert!(header.contains(&format!("auts=\"{}\"", STANDARD.encode([7; 14]))));
        assert!(header.contains("opaque=\"server,\\\"tag\\\\value\""));
        if qop {
            assert!(header.contains("qop=auth,nc=00000001,cnonce=\""));
            assert!(!header.contains("cnonce=\"\""));
        } else {
            assert!(!header.contains("qop=") && !header.contains("cnonce="));
        }
        assert_eq!(sip::header_value(&frame, "Security-Verify"), Some(old_verify.clone()));
        assert_eq!(session.channel.send_route(), old_route);
        assert!(auth.pending.as_ref().unwrap().refresh_authorization.is_none());
        let repeated = auth.prepare_aka_result(digest, material, None, &mut session.channel).await.unwrap_err();
        assert_eq!(repeated.code(), "ims_aka_resync_repeated");
    }
}

#[tokio::test]
async fn auts_initial_register_does_not_fabricate_security_verify_or_credentials() {
    let (session, runtime, _server, _client) = protected_session_with_profile(synthetic_profile()).await;
    let mut plain = CellularImsSipChannel::bind(ImsRoute { local_addr: "127.0.0.1:0".parse().unwrap(), ..session.channel.route() }, None, None).unwrap();
    let mut auth = authenticator(&session, &runtime, session.security_binding);
    auth.refresh_authorization = None;
    let mut material = aka(); material.auts = Some(vec![1; 14]);
    auth.prepare_aka_result(challenge(&nonce(1, 1)), material, None, &mut plain).await.unwrap();
    let frame = auth.authenticated_request(&[], 2).await.unwrap();
    assert!(sip::header_value(&frame, "Authorization").unwrap().contains("auts="));
    assert!(sip::header_value(&frame, "Security-Verify").is_none());
    assert!(sip::header_value(&frame, "Security-Client").is_some());
    assert!(auth.refresh_authorization().is_none());
}

#[tokio::test]
async fn auts_invalid_material_is_rejected_before_preparing_a_request() {
    let (mut session, runtime, _server, _client) = protected_session_with_profile(synthetic_profile()).await;
    for length in [0, 13, 15] {
        let mut auth = authenticator(&session, &runtime, session.security_binding);
        let mut material = aka(); material.auts = Some(vec![1; length]);
        let error = auth.prepare_aka_result(challenge(&nonce(1, 1)), material, None, &mut session.channel).await.unwrap_err();
        assert_eq!(error.code(), code::AKA_MATERIAL_INVALID);
        assert!(auth.pending.is_none() && !auth.resync_sent);
    }
}

#[tokio::test]
async fn stale_same_vector_recomputes_proof_without_uicc_or_sa_replacement() {
    let (mut session, runtime, _server, _client) = protected_session_with_profile(synthetic_profile()).await;
    session.refresh_authorization.as_mut().unwrap().challenge = challenge(&nonce(1, 1));
    session.refresh_authorization.as_mut().unwrap().aka_origin = Some(crate::connectivity::core::digest_aka::decode_aka_nonce(&nonce(1, 1)).unwrap());
    let old_route = session.channel.send_route();
    let old_verify = session.channel.security_verify().unwrap().to_owned();
    let mut auth = authenticator(&session, &runtime, session.security_binding);
    let old = auth.refresh_authorization.clone().unwrap();
    let incoming = format!("SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"ims.example\",nonce=\"{}\",algorithm=AKAv1-MD5,qop=auth,stale=true,opaque=\"new-tag\"\r\n\r\n", nonce(1, 2));
    // The fixture has no actual UICC. A successful production prepare here
    // proves that this valid stale renewal never attempted an APDU/AKA call.
    auth.prepare_authenticated_channel(incoming.as_bytes(), &mut session.channel).await.unwrap();
    let frame = auth.authenticated_request(incoming.as_bytes(), 2).await.unwrap();
    let header = sip::header_value(&frame, "Authorization").unwrap();
    assert!(header.contains(&format!("nonce=\"{}\"", nonce(1, 2))));
    assert!(header.contains("nc=00000001") && header.contains("opaque=\"new-tag\""));
    assert!(!header.contains("stale=") && !header.contains("response=\"\""));
    assert_eq!(session.channel.send_route(), old_route);
    assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
    assert!(auth.xfrm_plan.is_none());
    let retained = auth.refresh_authorization_after_failure().unwrap();
    assert_eq!(retained.challenge, old.challenge);
    assert_eq!(retained.nonce_count, old.nonce_count);
    let repeated = auth.prepare_authenticated_channel(incoming.as_bytes(), &mut session.channel).await.unwrap_err();
    assert_eq!(repeated.code(), "digest_stale_nonce_unchanged");
}

#[test]
fn stale_nextnonce_does_not_authenticate_a_new_aka_vector() {
    let mut auth = CellularImsRefreshAuthorization::new(challenge(&nonce(1, 1)), aka());
    let origin = auth.aka_origin.clone();
    let identity = ImsIdentity { private_user: "user@ims.example".into(), public_uri: "sip:user@ims.example".into(), contact_user: "user".into(), home_domain: "ims.example".into(), contact_user_phone: false };
    auth.authorization_for(&identity, "sip:ims.example").unwrap();
    auth.apply_success_authentication_info(format!("SIP/2.0 200 OK\r\nAuthentication-Info: nextnonce=\"{}\"\r\n\r\n", nonce(2, 1)).as_bytes()).unwrap();
    auth.authorization_for(&identity, "sip:ims.example").unwrap();
    assert_eq!(auth.aka_origin, origin);
    let mut incoming = challenge(&nonce(2, 2)); incoming.stale = true;
    assert_eq!(crate::connectivity::core::digest_aka::challenge_action(&incoming, Some(&auth.challenge), auth.aka_origin.as_ref()).unwrap(), crate::connectivity::core::digest_aka::ChallengeAction::Authenticate);
}

#[tokio::test]
async fn stale_cannot_borrow_credentials_after_initial_empty_aka_or_auts() {
    let (mut session, runtime, _server, _client) = protected_session_with_profile(synthetic_profile()).await;
    session.refresh_authorization.as_mut().unwrap().challenge = challenge(&nonce(1, 1));
    session.refresh_authorization.as_mut().unwrap().aka_origin = Some(crate::connectivity::core::digest_aka::decode_aka_nonce(&nonce(1, 1)).unwrap());
    let incoming = format!("SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"ims.example\",nonce=\"{}\",algorithm=AKAv1-MD5,qop=auth,stale=true\r\n\r\n", nonce(1, 2));
    let mut empty = authenticator(&session, &runtime, session.security_binding);
    empty.refresh_authorization = None;
    assert_eq!(empty.prepare_authenticated_channel(incoming.as_bytes(), &mut session.channel).await.unwrap_err().code(), "digest_stale_without_context");
    let mut resync = authenticator(&session, &runtime, session.security_binding);
    let mut material = aka(); material.auts = Some(vec![1; 14]);
    resync.prepare_aka_result(challenge(&nonce(1, 1)), material, None, &mut session.channel).await.unwrap();
    assert_eq!(resync.prepare_authenticated_channel(incoming.as_bytes(), &mut session.channel).await.unwrap_err().code(), "digest_stale_without_context");
}
