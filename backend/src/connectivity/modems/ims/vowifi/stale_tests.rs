//! Synthetic, device-free IMS Digest stale regressions. Run in Actions only.
use super::*;
use crate::connectivity::core::digest_aka::{self, ChallengeAction};
use crate::connectivity::modems::ims::vowifi::{
    eap_aka, profiles::{derive_standard_3gpp_profile, Standard3gppAccess},
    qmi_uim::UsimAkaApduResult,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const REALM: &str = "ims.mnc001.mcc001.3gppnetwork.org";
const SECURITY: &str = "Security-Server: ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans;spi-c=1001;spi-s=1002;port-c=5062;port-s=5063\r\n";

fn profile() -> &'static CarrierProfile {
    derive_standard_3gpp_profile("001", "01", Standard3gppAccess::WifiEpdg).unwrap()
}

fn nonce(vector: u8, suffix: u8) -> String {
    let mut bytes = vec![vector; 32];
    bytes.push(suffix);
    BASE64_STANDARD.encode(bytes)
}

fn frame(value: &str, security: &str, proxy: bool) -> String {
    format!(
        "SIP/2.0 {}\r\n{}: {value}\r\n{security}Content-Length: 0\r\n\r\n",
        if proxy { "407 Proxy Authentication Required" } else { "401 Unauthorized" },
        if proxy { "Proxy-Authenticate" } else { "WWW-Authenticate" },
    )
}

fn value(vector: u8, suffix: u8, extra: &str) -> String {
    format!("Digest realm=\"{REALM}\", nonce=\"{}\", algorithm=AKAv1-MD5, qop=\"auth\"{extra}", nonce(vector, suffix))
}

fn challenge(vector: u8, suffix: u8, stale: bool) -> LiveDigestChallenge {
    parse_live_digest_challenge(
        &frame(&value(vector, suffix, if stale { ", stale=true" } else { "" }), "", false),
        REALM,
    ).unwrap()
}

fn aka() -> UsimAkaApduResult {
    UsimAkaApduResult {
        res: vec![0x22; 8], ck: vec![0x33; 16], ik: vec![0x44; 16], auts: None,
    }
}

fn state(nc: u32) -> LiveRefreshAuthorization {
    LiveRefreshAuthorization {
        challenge: challenge(0x11, 1, false).shared(),
        password: aka().res.clone(),
        aka: Some(aka()),
        aka_origin: Some(digest_aka::decode_aka_nonce(&nonce(0x11, 1)).unwrap()),
        cnonce: "fixture-original-cnonce".into(),
        nonce_count: nc,
    }
}

fn context(profile: &'static CarrierProfile) -> LiveRegisterRequestContext {
    LiveRegisterRequestContext::new(
        profile,
        LiveImsRegisterIdentity {
            shared: crate::connectivity::core::context::ImsIdentity {
                private_user: format!("001010123456789@{REALM}"),
                public_uri: format!("sip:001010123456789@{REALM}"),
                contact_user: "001010123456789".into(),
                home_domain: REALM.into(),
                contact_user_phone: false,
            },
            shape: "stale_fixture",
        },
        "[::1]:5064".parse().unwrap(),
        "::1".parse().unwrap(),
    ).unwrap()
}

fn bind_context(context: &mut LiveRegisterRequestContext) {
    let card = crate::connectivity::modems::ims::uicc_ims::UiccCardBinding {
        endpoint: "/synthetic/stale-reader".into(), slot: 1, owner: "synthetic-owner".into(),
        iccid: "synthetic-stale-card".into(), imsi: "001010123456789".into(),
    };
    context.uicc = Some(live_identity::selection_for_test("fixture", context.identity.clone(), context.target.clone(), card));
}

fn factory(previous: Option<LiveRefreshAuthorization>) -> VowifiUnregisterFactory {
    let profile = profile();
    VowifiUnregisterFactory {
        line_id: "n2-wifi-synthetic".into(),
        profile,
        context: context(profile),
        variant: live_register_header_variants(profile)[0],
        next_cseq: std::sync::atomic::AtomicU32::new(4),
        expires_seconds: std::sync::atomic::AtomicU32::new(3600),
        refresh_authorization: std::sync::Mutex::new(previous),
        unregister_authorization: std::sync::Mutex::new(None),
        unregister_resync_sent: std::sync::atomic::AtomicBool::new(false),
        security_verify: Some("ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc".into()),
    }
}

#[test]
fn stale_parser_matches_shared_strict_quoted_case_and_escaped_opaque() {
    for (extra, expected) in [("", false), (", stale=false", false), (", StAlE=\"TrUe\"", true), (", stale=TRUE", true)] {
        let value = value(0x11, 2, &format!(r#"{extra}, opaque="escaped\"comma,slash\\end""#));
        let shared = digest_aka::parse_digest_challenge(&value, true).unwrap();
        let parsed = parse_live_digest_challenge(&frame(&value, "", true), REALM).unwrap();
        assert_eq!(parsed.stale, expected);
        assert_eq!(parsed.shared(), shared);
        assert_eq!(parsed.opaque.as_deref(), Some("escaped\"comma,slash\\end"));
        assert_eq!(parsed.authorization_header_name(), "Proxy-Authorization");
    }
    for extra in [
        ", stale=maybe", ", stale=1", ", stale=\"\"", ", stale=\"true",
        ", stale=true, STALE=false", ", stale=true, stale=true",
        ", nonce=\"duplicate\"", ", realm=\"duplicate\"", ", opaque=x, OPAQUE=y",
        ", stale=true,", ", stale=true,, x=y", ", stale=true\r\nx=y",
    ] {
        let value = value(0x11, 2, extra);
        assert!(digest_aka::parse_digest_challenge(&value, false).is_err());
        // Feed the header value directly so SIP unfolding cannot mask controls.
        assert!(parse_live_digest_challenge_value("", REALM, "www_authenticate", &value).is_err());
    }
    let oversized = value(0x11, 2, &format!(", opaque=\"{}\"", "x".repeat(16 * 1024)));
    assert!(parse_live_digest_challenge_value("", REALM, "www_authenticate", &oversized).is_err());
}

#[test]
fn stale_nonce_scope_rejects_missing_unsent_same_nonce_and_changed_scope() {
    let previous = state(7);
    let incoming = challenge(0x11, 2, true);
    assert_eq!(live_digest_challenge_action(&incoming, Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    assert_eq!(live_digest_challenge_action(&incoming, None).unwrap_err().code(), "digest_stale_without_context");
    assert_eq!(live_digest_challenge_action(&incoming, Some(&state(0))).unwrap_err().code(), "digest_stale_without_context");
    assert_eq!(live_digest_challenge_action(&challenge(0x11, 1, true), Some(&previous)).unwrap_err().code(), "digest_stale_nonce_unchanged");
    for mutation in 0..4 {
        let mut changed = incoming.clone();
        match mutation {
            0 => changed.realm = "different.invalid".into(),
            1 => changed.algorithm = "AKAv2-MD5".into(),
            2 => changed.header_kind = "proxy_authenticate",
            _ => changed.qop = None,
        }
        assert_eq!(live_digest_challenge_action(&changed, Some(&previous)).unwrap_err().code(), "digest_stale_scope_changed");
    }
    let mut changed_opaque = incoming;
    changed_opaque.opaque = Some("new,opaque".into());
    assert_eq!(live_digest_challenge_action(&changed_opaque, Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    assert_eq!(live_digest_challenge_action(&challenge(0x11, 1, false), None).unwrap(), ChallengeAction::Authenticate);
}

#[test]
fn stale_refresh_new_nc_one_preserves_consumed_original_nc_and_security() {
    let factory = factory(Some(state(7)));
    let mut auth = VowifiRefreshAuthenticator {
        factory: &factory, authorization: Some(state(7)), resync_sent: false, expires: 3600,
    };
    let initial = String::from_utf8(auth.request().unwrap()).unwrap();
    assert!(initial.contains("nc=00000008"));
    let old = auth.authorization.as_ref().unwrap().clone();
    let mut incoming = challenge(0x11, 2, true);
    incoming.opaque = Some("new\"opaque".into());
    assert_eq!(live_digest_challenge_action(&incoming, Some(&old)).unwrap(), ChallengeAction::ReuseCredentials);
    auth.authorization = Some(old.renewed(&incoming).unwrap());
    let request = String::from_utf8(auth.request().unwrap()).unwrap();
    assert!(request.contains("nc=00000001"));
    assert!(request.contains(&format!("nonce=\"{}\"", incoming.nonce)));
    assert!(request.contains("opaque=\"new\\\"opaque\""));
    assert!(request.contains("Security-Verify: ipsec-3gpp;"));
    assert!(request.contains("Expires: 3600"));
    assert_ne!(auth.authorization.as_ref().unwrap().cnonce, old.cnonce);
    assert_eq!(auth.authorization.as_ref().unwrap().password, old.password);
    // A failed exchange discards the tentative nonce/password, not consumed nc.
    drop(auth);
    factory.remember_emitted(&Some(state(3))); // A stale local snapshot cannot rewind nc.
    let retained = factory.refresh_authorization.lock().unwrap();
    let retained = retained.as_ref().unwrap();
    assert_eq!(retained.nonce_count, 8);
    assert_eq!(retained.challenge, old.challenge);
    assert_eq!(retained.cnonce, old.cnonce);
}

#[test]
fn stale_nextnonce_preserves_aka_provenance_instead_of_borrowing_new_vector() {
    let mut original = state(8);
    let origin = original.aka_origin.clone();
    original.apply_success(format!("SIP/2.0 200 OK\r\nAuthentication-Info: nextnonce=\"{}\"\r\n\r\n", nonce(0x12, 1)).as_bytes());
    original.authorization("user", "sip:ims.invalid").unwrap();
    assert_eq!(original.aka_origin, origin);
    assert_eq!(live_digest_challenge_action(&challenge(0x12, 2, true), Some(&original)).unwrap(), ChallengeAction::Authenticate);
    assert_eq!(live_digest_challenge_action(&challenge(0x11, 2, true), Some(&original)).unwrap(), ChallengeAction::ReuseCredentials);
}

#[test]
fn stale_nonce_count_never_wraps_and_nextnonce_is_not_yet_sent() {
    let mut original = state(u32::MAX);
    assert_eq!(original.authorization("user", "sip:ims.invalid").unwrap_err().code(), "vowifi_refresh_nonce_count_exhausted");
    assert_eq!(original.nonce_count, u32::MAX);
    let mut original = state(8);
    original.apply_success(format!("SIP/2.0 200 OK\r\nAuthentication-Info: nextnonce=\"{}\"\r\n\r\n", nonce(0x11, 2)).as_bytes());
    assert_eq!(original.nonce_count, 0);
    assert_eq!(live_digest_challenge_action(&challenge(0x11, 3, true), Some(&original)).unwrap_err().code(), "digest_stale_without_context");
    assert!(original.authorization("user", "sip:ims.invalid").unwrap().contains("nc=00000001"));
    assert_eq!(live_digest_challenge_action(&challenge(0x11, 3, true), Some(&original)).unwrap(), ChallengeAction::ReuseCredentials);
}

#[tokio::test]
async fn stale_initial_reuses_aka_without_uicc_and_fresh_vector_authenticates() {
    let previous = state(1);
    let calls = AtomicUsize::new(0);
    for (vector, expected) in [(0x11, ChallengeAction::ReuseCredentials), (0x12, ChallengeAction::Authenticate)] {
        let incoming = challenge(vector, 2, true);
        let action = live_digest_challenge_action(&incoming, Some(&previous)).unwrap();
        assert_eq!(action, expected);
        let result = live_register_aka_result(action, Some(&previous), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(aka())
        }).await.unwrap();
        assert_eq!(result.res, aka().res);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    // Exercise the full production builder with an explicit synthetic card
    // observation. Missing binding must fail even when AKA keys are cached.
    let profile = profile();
    let variant = live_register_header_variants(profile)[0];
    let mut context = context(profile);
    context.refresh_authorization = Some(previous.clone());
    let incoming = parse_live_digest_challenge(&frame(&value(0x11, 2, ", stale=true"), SECURITY, false), REALM).unwrap();
    let missing_binding = build_live_register_auth_material("fixture", profile, &mut context, &incoming, variant).await.err().unwrap();
    assert_eq!(missing_binding.reason, "ims_uicc_binding_invalid");
    bind_context(&mut context);
    let material = build_live_register_auth_material("fixture", profile, &mut context, &incoming, variant).await.unwrap();
    assert!(material.authorization.contains("nc=00000001"));
    let new_state = material.refresh_authorization.unwrap();
    assert_eq!(new_state.nonce_count, 1, "matches the precomputed proof, pending socket write");
    assert_eq!(new_state.password, previous.password);
    assert_ne!(new_state.cnonce, previous.cnonce);
    assert!(new_state.aka.is_some());
    assert_eq!(format!("{new_state:?}"), "LiveRefreshAuthorization(<redacted>)");
    assert_eq!(context.refresh_authorization.as_ref().unwrap().nonce_count, 1);
    let missing_security = build_live_register_auth_material(
        "fixture", profile, &mut context, &challenge(0x11, 2, true), variant,
    ).await.err().unwrap();
    assert_eq!(missing_security.reason, "ims_security_server_offer_missing");

    let incoming = challenge(0x12, 2, true);
    context.uicc = None;
    let error = build_live_register_auth_material("fixture", profile, &mut context, &incoming, variant).await.err().unwrap();
    assert_eq!(error.reason, "ims_uicc_binding_invalid", "fresh vector reaches the bound-UICC gate");
}

#[tokio::test]
async fn stale_empty_initial_and_auts_cannot_reuse_credentials_or_replay_uicc() {
    let calls = AtomicUsize::new(0);
    let mut auts = state(1);
    auts.aka.as_mut().unwrap().auts = Some(vec![0x55; 14]);
    auts.aka.as_mut().unwrap().res.clear();
    let mut missing = state(1);
    missing.aka = None;
    for previous in [None, Some(state(0)), Some(auts), Some(missing)] {
        let result = live_register_aka_result(ChallengeAction::ReuseCredentials, previous.as_ref(), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(aka())
        }).await;
        assert_eq!(result.unwrap_err().reason, "ims_digest_stale_credentials_missing");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let profile = profile();
    let mut context = context(profile); // The initial AKA-empty header has no proof state.
    let error = build_live_register_auth_material("fixture", profile, &mut context, &challenge(0x11, 2, true), live_register_header_variants(profile)[0]).await.err().unwrap();
    assert_eq!(error.reason, "ims_digest_stale_without_context");
}

#[tokio::test]
async fn stale_refresh_and_unregister_reject_scope_before_uicc_or_sa_changes() {
    let factory = factory(Some(state(7)));
    let previous = state(8);
    for (value, security, expected) in [
        (value(0x11, 1, ", stale=true"), "", "digest_stale_nonce_unchanged"),
        (value(0x11, 2, ", stale=true").replace(REALM, "foreign.invalid"), "", "digest_stale_scope_changed"),
        (value(0x11, 2, ", stale=true"), SECURITY, "vowifi_refresh_security_update_required"),
        (value(0x11, 2, ", stale=true"), "Security-Server: broken-offer\r\n", "vowifi_refresh_security_update_required"),
    ] {
        let error = factory.challenged_authorization(frame(&value, security, false).as_bytes(), Some(&previous)).await.unwrap_err();
        assert_eq!(error.code(), expected, "no UICC binding in this fixture");
    }
    let response = frame(&value(0x11, 2, ", stale=true"), "", false);
    // Even a committed old proof cannot supply context after an AUTS request.
    let error = factory.challenged_authorization(response.as_bytes(), None).await.unwrap_err();
    assert_eq!(error.code(), "digest_stale_without_context");
    use crate::connectivity::modems::ims::vowifi::operator::RegisteredUnregister;
    let error = RegisteredUnregister::authenticated_request(&factory, response.as_bytes(), 5).await.unwrap_err();
    assert_eq!(error.code(), "digest_stale_without_context");
    assert_eq!(factory.refresh_authorization.lock().unwrap().as_ref().unwrap().nonce_count, 7);
}

#[tokio::test]
async fn stale_plain_md5_keeps_explicit_policy_and_md5_aka_vector_boundary() {
    let mut configured = *profile();
    configured.ims.register.use_plain_digest_placeholder = false;
    let disabled = Box::leak(Box::new(configured));
    let parse_md5 = |nonce: &str| parse_live_digest_challenge(
        &frame(&format!("Digest realm=\"{REALM}\", nonce=\"{nonce}\", algorithm=MD5, stale=true"), "", false), REALM,
    ).unwrap();
    let incoming = parse_md5("00112233445566778899aabbccddeeff");
    assert_eq!(incoming.nonce_kind, LiveDigestNonceKind::PlainDigest);
    assert_eq!(reject_plain_digest_when_disabled(disabled, &incoming).unwrap_err().reason, "ims_digest_plain_md5_disabled");
    let mut configured = *disabled;
    configured.ims.register.use_plain_digest_placeholder = true;
    let enabled = Box::leak(Box::new(configured));
    assert!(reject_plain_digest_when_disabled(enabled, &incoming).is_ok());

    let mut previous = state(1);
    previous.challenge.algorithm = "MD5".into();
    previous.challenge.qop = None;
    assert_eq!(live_digest_challenge_action(&parse_md5(&nonce(0x11, 2)), Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    assert_eq!(live_digest_challenge_action(&parse_md5(&nonce(0x12, 2)), Some(&previous)).unwrap(), ChallengeAction::Authenticate);
    let retained_password = previous.password.clone();
    let mut context = context(enabled);
    bind_context(&mut context);
    context.refresh_authorization = Some(previous);
    let material = build_live_register_auth_material(
        "fixture", enabled, &mut context, &incoming, live_register_header_variants(enabled)[0],
    ).await.unwrap();
    assert_eq!(material.refresh_authorization.unwrap().password, retained_password);
    let denied = build_live_register_auth_material(
        "fixture", disabled, &mut context, &incoming, live_register_header_variants(disabled)[0],
    ).await.err().unwrap();
    assert_eq!(denied.reason, "ims_digest_plain_md5_disabled");
}

#[tokio::test]
async fn stale_protected_round_retains_channel_for_absent_or_unchanged_security() {
    use tokio::net::UdpSocket;
    for security in ["", SECURITY] {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let remote = server.local_addr().unwrap();
        let first = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        first.connect(remote).await.unwrap();
        let first_addr = first.local_addr().unwrap();
        let protected = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        protected.connect(remote).await.unwrap();
        let protected_addr = protected.local_addr().unwrap();
        let verify = SECURITY.trim().strip_prefix("Security-Server: ").unwrap().to_owned();
        let route = |local_addr| crate::connectivity::core::context::ImsRoute { local_addr, pcscf_addr: remote, transport: crate::connectivity::core::context::SipTransport::Udp };
        let mut channel = SipChannel::new(SipChannelSocket::Udp(first), vec![], route(first_addr), None);
        let protected = SipChannel::new(SipChannelSocket::Udp(protected), vec![], route(protected_addr), Some(verify.clone()));
        retain_protected_register_channel(&mut channel, protected);
        assert_eq!(channel.route(), route(protected_addr));
        let profile = profile();
        let variant = live_register_header_variants(profile)[0];
        let mut context = context(profile);
        bind_context(&mut context);
        context.local_addr = protected_addr;
        context.route_addr = remote.ip();
        context.protected_header_port = Some(protected_addr.port());
        context.refresh_authorization = Some(state(1));
        let incoming = parse_live_digest_challenge(&frame(&value(0x11, 2, ", stale=true"), security, false), REALM).unwrap();
        let peer = tokio::spawn(async move {
            let mut bytes = [0u8; 16384];
            let (n, source) = tokio::time::timeout(Duration::from_secs(3), server.recv_from(&mut bytes)).await.unwrap().unwrap();
            assert_eq!(source, protected_addr);
            let request = &bytes[..n];
            assert_eq!(sip_frame::header_value(request, "CSeq").as_deref(), Some("3 REGISTER"));
            assert!(sip_frame::header_value(request, "Authorization").unwrap().contains("nc=00000001"));
            assert!(sip_frame::header_value(request, "Security-Verify").is_some());
            let response = format!("SIP/2.0 200 OK\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\nContent-Length: 0\r\n\r\n", sip_frame::header_value(request, "Via").unwrap(), sip_frame::header_value(request, "Call-ID").unwrap(), sip_frame::header_value(request, "CSeq").unwrap());
            server.send_to(response.as_bytes(), source).await.unwrap();
            server
        });
        let result = run_retained_stale_register_exchange("fixture", profile, &mut channel, &mut context, &incoming, variant, 3).await.unwrap();
        assert!(result.starts_with("SIP/2.0 200"));
        let server = peer.await.unwrap();
        assert_eq!(channel.route(), route(protected_addr));
        assert_eq!(channel.security_verify(), Some(verify.as_str()));
        assert_eq!(context.refresh_authorization.as_ref().unwrap().nonce_count, 1);
        // An explicitly changed tuple must not install a replacement policy.
        let changed = parse_live_digest_challenge(&frame(&value(0x11, 3, ", stale=true"), &SECURITY.replace("spi-c=1001", "spi-c=2001"), false), REALM).unwrap();
        let error = run_retained_stale_register_exchange("fixture", profile, &mut channel, &mut context, &changed, variant, 4).await.unwrap_err();
        assert_eq!(error.reason, "ims_digest_stale_security_changed");
        assert!(register_reason_blocks_profile_fallback(&error.reason));
        let fresh = challenge(0x12, 3, true);
        let error = run_retained_stale_register_exchange("fixture", profile, &mut channel, &mut context, &fresh, variant, 4).await.unwrap_err();
        assert_eq!(error.reason, "ims_register_protected_challenge_requires_recovery");
        assert!(register_reason_blocks_profile_fallback(&error.reason));
        assert_eq!(server.try_recv_from(&mut [0u8; 8192]).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}

#[test]
fn stale_auts_final_200_cannot_commit_refresh_or_unregister_success() {
    use crate::connectivity::modems::ims::vowifi::operator::RegisteredUnregister;
    let factory = factory(Some(state(7)));
    let auth = VowifiRefreshAuthenticator { factory: &factory, authorization: None, resync_sent: true, expires: 3600 };
    assert_eq!(RegisterAuthenticator::<SipChannel>::validate_success(&auth, b"SIP/2.0 200 OK\r\n\r\n").unwrap_err().code(), "ims_aka_resync_unexpected_status");
    factory.unregister_resync_sent.store(true, Ordering::Relaxed);
    assert_eq!(RegisteredUnregister::validate_success(&factory, b"SIP/2.0 200 OK\r\n\r\n").unwrap_err().code(), "ims_aka_resync_unexpected_status");
    *factory.unregister_authorization.lock().unwrap() = Some(state(1));
    assert!(RegisteredUnregister::validate_success(&factory, b"SIP/2.0 200 OK\r\n\r\n").is_ok());
}

#[test]
fn stale_auts_keeps_qop_opaque_but_never_creates_reusable_proof() {
    let profile = profile();
    let context = context(profile);
    let mut incoming = challenge(0x11, 2, false);
    incoming.header_kind = "proxy_authenticate";
    incoming.opaque = Some("escaped\"opaque\\value".into());
    let header = build_digest_resync_authorization_header(
        &context, &incoming, "sip:ims.invalid", &[0x55; 14],
    ).unwrap();
    assert!(header.starts_with("Proxy-Authorization: Digest "));
    assert!(header.contains("response=\"\""));
    assert!(header.contains("auts=\""));
    assert!(header.contains("qop=auth"));
    assert!(header.contains("nc=00000001"));
    assert!(header.contains("cnonce=\""));
    assert!(header.contains("opaque=\"escaped\\\"opaque\\\\value\""));
    assert!(context.refresh_authorization.is_none());
    incoming.stale = true;
    assert_eq!(live_digest_challenge_action(&incoming, context.refresh_authorization.as_ref()).unwrap_err().code(), "digest_stale_without_context");
}

#[test]
fn stale_unregister_new_nonce_remains_tentative_and_zero_expiry() {
    let factory = factory(Some(state(7)));
    let mut original = Some(state(7));
    let request = String::from_utf8(factory.authorized_request(0, &mut original).unwrap()).unwrap();
    factory.remember_emitted(&original);
    assert!(request.contains("nc=00000008"));
    assert!(request.contains("Expires: 0"));
    let old = original.unwrap();
    let mut next = Some(old.renewed(&challenge(0x11, 2, true)).unwrap());
    let request = String::from_utf8(factory.authorized_request(0, &mut next).unwrap()).unwrap();
    factory.remember_emitted(&next);
    assert!(request.contains("nc=00000001"));
    assert!(request.contains("Expires: 0"));
    assert!(request.contains("Security-Verify: ipsec-3gpp;"));
    *factory.unregister_authorization.lock().unwrap() = next;
    let pending = factory.unregister_authorization.lock().unwrap();
    assert_eq!(pending.as_ref().unwrap().nonce_count, 1);
    let committed = factory.refresh_authorization.lock().unwrap();
    assert_eq!(committed.as_ref().unwrap().nonce_count, 8);
    assert_eq!(committed.as_ref().unwrap().challenge, old.challenge);
}

#[test]
fn stale_ims_state_does_not_change_eap_mac_or_auts_behavior() {
    // EAP Request/AKA Challenge with RAND, AUTN, and a deliberately invalid MAC.
    let mut packet = vec![1, 7, 0, 68, 23, 1, 0, 0];
    for (kind, data) in [(1u8, [0x11; 16]), (2u8, [0x11; 16]), (11u8, [0; 16])] {
        packet.extend_from_slice(&[kind, 5, 0, 0]);
        packet.extend_from_slice(&data);
    }
    let eap = eap_aka::parse_challenge(&packet).unwrap();
    let sync_before = eap_aka::build_sync_failure_response(&eap, &[0x55; 14]).unwrap();
    let previous = state(7);
    let incoming = challenge(0x11, 2, true);
    assert_eq!(live_digest_challenge_action(&incoming, Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    let mut renewed = previous.renewed(&incoming).unwrap();
    renewed.authorization("ims-user", "sip:ims.invalid").unwrap();
    assert_eq!(eap_aka::build_challenge_response(&eap, "0001010123456789@nai.epc.mnc001.mcc001.3gppnetwork.org", &aka()).unwrap_err(), eap_aka::EapAkaProtocolError::InvalidMac);
    let sync_after = eap_aka::build_sync_failure_response(&eap, &[0x55; 14]).unwrap();
    assert_eq!(sync_before.expose_for_ike_encryption(), sync_after.expose_for_ike_encryption());
    assert_eq!(sync_after.expose_for_ike_encryption()[5], 4);
    assert!(sync_after.msk_for_ike_auth().is_none());
}
