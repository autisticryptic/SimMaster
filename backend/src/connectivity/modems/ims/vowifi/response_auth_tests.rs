//! Device-free production WiFi REGISTER response-authentication gates.
//! Synthetic loopback peers only; compiled/executed in Actions, never locally.
use super::*;
#[path = "register_retry_refresh_tests.rs"]
mod retry_recovery_tests;
#[path = "register_retry_unregister_tests.rs"]
mod retry_unregister_tests;
#[path = "initial_min_expires_tests.rs"]
mod initial_min_expires_tests;
use crate::connectivity::core::{
    digest_aka::{self, DigestRequestProof},
    register::run_unregister,
    registration::UnregisterResult,
};
use crate::connectivity::modems::ims::vowifi::{
    operator::{RegisteredUnregister, RegisteredUnregisterAdapter},
    profiles::{derive_standard_3gpp_profile, Standard3gppAccess},
    qmi_uim::UsimAkaApduResult,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::UdpSocket;

const REALM: &str = "ims.mnc001.mcc001.3gppnetwork.org";
const VERIFY: &str = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans;spi-c=1001;spi-s=1002;port-c=5062;port-s=5063";

fn nonce(suffix: u8) -> String {
    let mut bytes = vec![0x11; 32];
    bytes.push(suffix);
    BASE64_STANDARD.encode(bytes)
}

fn challenge(suffix: u8, stale: bool, proxy: bool) -> LiveDigestChallenge {
    parse_live_digest_challenge(&format!(
        "SIP/2.0 {}\r\n{}: Digest realm=\"{REALM}\", nonce=\"{}\", algorithm=AKAv1-MD5, qop=\"auth\", stale={}\r\n\r\n",
        if proxy { "407 Proxy Authentication Required" } else { "401 Unauthorized" },
        if proxy { "Proxy-Authenticate" } else { "WWW-Authenticate" },
        nonce(suffix), stale,
    ), REALM).unwrap()
}

fn context(line: &str, local: SocketAddr, remote: SocketAddr) -> (LiveRegisterRequestContext, &'static CarrierProfile) {
    let profile = derive_standard_3gpp_profile("001", "01", Standard3gppAccess::WifiEpdg).unwrap();
    let mut context = LiveRegisterRequestContext::new(profile, LiveImsRegisterIdentity {
        shared: crate::connectivity::core::context::ImsIdentity {
            private_user: format!("001010123456789@{REALM}"),
            public_uri: format!("sip:001010123456789@{REALM}"),
            contact_user: "001010123456789".into(),
            home_domain: REALM.into(), contact_user_phone: false,
        },
        shape: "rspauth_fixture",
    }, local, remote.ip()).unwrap();
    let card = crate::connectivity::modems::ims::uicc_ims::UiccCardBinding {
        endpoint: format!("/synthetic/{line}"), slot: 1, owner: "synthetic-owner".into(),
        iccid: "synthetic-rspauth-card".into(), imsi: "001010123456789".into(),
    };
    context.uicc = Some(live_identity::selection_for_test(line, context.identity.clone(), context.target.clone(), card));
    context.bind_test_worker(line);
    (context, profile)
}

fn authorization(context: &LiveRegisterRequestContext, profile: &'static CarrierProfile, proxy: bool) -> LiveRefreshAuthorization {
    let mut state = LiveRefreshAuthorization {
        challenge: challenge(1, false, proxy).shared(),
        password: vec![0x22; 8],
        aka: Some(UsimAkaApduResult { res: vec![0x22; 8], ck: vec![0x33; 16], ik: vec![0x44; 16], auts: None }),
        aka_origin: Some(digest_aka::decode_aka_nonce(&nonce(1)).unwrap()),
        cnonce: "synthetic-rspauth-cnonce".into(), nonce_count: 0, request_proof: None,
    };
    state.authorization(&context.identity.private_user, &context.request_uri(profile, live_register_header_variants(profile)[0])).unwrap();
    state
}

async fn fixture(proxy: bool) -> (Arc<VowifiUnregisterFactory>, SipChannel, UdpSocket) {
    static ID: AtomicUsize = AtomicUsize::new(0);
    let line = format!("n4-wifi-rspauth-{}", ID.fetch_add(1, Ordering::Relaxed));
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let remote = server.local_addr().unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.connect(remote).await.unwrap();
    let local = client.local_addr().unwrap();
    let (context, profile) = context(&line, local, remote);
    let previous = authorization(&context, profile, proxy);
    let mut channel = SipChannel::new(SipChannelSocket::Udp(client), vec![], crate::connectivity::core::context::ImsRoute {
        local_addr: local, pcscf_addr: remote, transport: crate::connectivity::core::context::SipTransport::Udp,
    }, Some(VERIFY.into()));
    channel.configure_outbound(&line, &context.instance_id, true);
    let factory = Arc::new(VowifiUnregisterFactory {
        line_id: line, profile, context, variant: live_register_header_variants(profile)[0],
        next_cseq: std::sync::atomic::AtomicU32::new(8),
        expires_seconds: std::sync::atomic::AtomicU32::new(3600),
        refresh_authorization: std::sync::Mutex::new(Some(previous)),
        unregister_authorization: std::sync::Mutex::new(None),
        unregister_resync_sent: std::sync::atomic::AtomicBool::new(false),
        security_verify: Some(VERIFY.into()),
    });
    (factory, channel, server)
}

async fn receive(server: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut bytes = vec![0; 16384];
    let (len, peer) = tokio::time::timeout(Duration::from_secs(3), server.recv_from(&mut bytes)).await.unwrap().unwrap();
    bytes.truncate(len);
    (bytes, peer)
}

fn response(request: &[u8], status: &str, headers: &str) -> Vec<u8> {
    format!("SIP/2.0 {status}\r\nVia: {}\r\nFrom: {}\r\nTo: {}\r\nCall-ID: {}\r\nCSeq: {}\r\n{headers}Content-Length: 0\r\n\r\n",
        sip_frame::header_value(request, "Via").unwrap(),
        sip_frame::header_value(request, "From").unwrap(),
        sip_frame::header_value(request, "To").unwrap(),
        sip_frame::header_value(request, "Call-ID").unwrap(),
        sip_frame::header_value(request, "CSeq").unwrap(),
    ).into_bytes()
}

// Only parses our generated, simple synthetic fields, not production input.
fn param(header: &str, name: &str) -> String {
    header.trim_start_matches("Digest ").split(',').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.trim_matches('"').to_owned())
    }).unwrap()
}

fn info(request: &[u8], proxy: bool) -> String {
    let header = sip_frame::header_value(request, if proxy { "Proxy-Authorization" } else { "Authorization" }).unwrap();
    let challenge = digest_aka::parse_digest_challenge(&header, proxy).unwrap();
    let nc = param(&header, "nc");
    let cnonce = param(&header, "cnonce");
    let rspauth = digest_aka::compute_digest_response(
        &param(&header, "username"), &challenge.realm, &[0x22; 8], &challenge.algorithm,
        "", &param(&header, "uri"), &challenge.nonce, Some("auth"), &cnonce, &nc,
    ).unwrap();
    format!("{}: rspauth=\"{rspauth}\", qop=auth, nc={nc}, cnonce=\"{cnonce}\", nextnonce=\"{}\"\r\n",
        if proxy { "Proxy-Authentication-Info" } else { "Authentication-Info" }, nonce(3))
}

fn challenge_response(request: &[u8], proxy: bool) -> Vec<u8> {
    response(request, if proxy { "407 Proxy Authentication Required" } else { "401 Unauthorized" }, &format!(
        "{}: Digest realm=\"{REALM}\", nonce=\"{}\", algorithm=AKAv1-MD5, qop=\"auth\", stale=true\r\n",
        if proxy { "Proxy-Authenticate" } else { "WWW-Authenticate" }, nonce(2),
    ))
}

fn assert_retained(factory: &VowifiUnregisterFactory, nc: u32) {
    let retained = factory.refresh_authorization.lock().unwrap();
    let retained = retained.as_ref().unwrap();
    assert_eq!(retained.challenge.nonce, nonce(1));
    assert_eq!(retained.nonce_count, nc);
    assert_eq!(retained.cnonce, "synthetic-rspauth-cnonce");
    assert_eq!(factory.expires_seconds.load(Ordering::Relaxed), 3600);
    assert_eq!(factory.security_verify.as_deref(), Some(VERIFY));
}

#[tokio::test]
async fn initial_challenge_builder_captures_exact_nc_one_and_protected_headers() {
    let (factory, _channel, _server) = fixture(false).await;
    let mut context = factory.context.clone();
    context.refresh_authorization = factory.refresh_authorization.lock().unwrap().clone();
    let incoming = parse_live_digest_challenge(&format!(
        "SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"{}\", algorithm=AKAv1-MD5, qop=\"auth\", stale=true\r\nSecurity-Server: {VERIFY}\r\n\r\n", nonce(2)), REALM).unwrap();
    let material = build_live_register_auth_material(&factory.line_id, factory.profile, &mut context, &incoming, factory.variant).await.unwrap();
    context.refresh_authorization = material.refresh_authorization;
    context.protected_header_port = Some(5062);
    let request = context.build_authorized_request(factory.profile, factory.variant, 2, &material.authorization, Some(VERIFY));
    validate_live_register_request(context.refresh_authorization.as_ref(), request.as_bytes()).unwrap();
    assert!(request.contains("nc=00000001"));
    assert_eq!(context.refresh_authorization.as_ref().unwrap().nonce_count, 1);
    let good = response(request.as_bytes(), "200 OK", &info(request.as_bytes(), false));
    validate_live_register_success(context.refresh_authorization.as_ref(), false, &good).unwrap();
    context.refresh_authorization.as_mut().unwrap().apply_success(&good).unwrap();
    assert_eq!(context.refresh_authorization.as_ref().unwrap().nonce_count, 0);
    // Protected owned exchange commits before returning to the shared driver;
    // the last proof must survive nextnonce/nc mutation for its second gate.
    validate_live_register_success(context.refresh_authorization.as_ref(), false, &good).unwrap();
    assert_eq!(context.refresh_authorization.as_ref().unwrap().challenge.nonce, nonce(3));
    assert!(validate_live_register_request(context.refresh_authorization.as_ref(), request.replacen("REGISTER sip:", "REGISTER sip:tampered-", 1).as_bytes()).is_err());
}

#[test]
fn initial_empty_or_auts_never_borrows_a_prior_proof() {
    validate_live_register_success(None, false, b"SIP/2.0 200 OK\r\n\r\n").unwrap();
    for header in ["Authentication-Info: nextnonce=next", "Proxy-Authentication-Info: rspauth=00"] {
        let wire = format!("SIP/2.0 200 OK\r\n{header}\r\n\r\n");
        let error = validate_live_register_success(None, false, wire.as_bytes()).unwrap_err();
        assert_eq!(error.code(), "digest_response_auth_context_missing");
        assert!(register_reason_blocks_profile_fallback(error.code()));
    }
    assert_eq!(validate_live_register_success(None, true, b"SIP/2.0 200 OK\r\n\r\n").unwrap_err().code(), "ims_aka_resync_unexpected_status");
}

#[tokio::test]
async fn diagnostics_wifi_preflight_binding_failure_is_visible_without_emitting_sip() {
    use crate::connectivity::core::registration_diagnostics as facts;
    let (mut factory, mut channel, server) = fixture(false).await;
    let runtime = Arc::new(crate::connectivity::modems::ims::vowifi::runtime::VowifiRuntime::for_line(&factory.line_id));
    reg_event_live::bind_reg_event_runtime(&factory.line_id, &runtime);
    let scope = diagnostics_live::Scope::begin(&factory.line_id, factory.profile).await;
    let mutable = Arc::get_mut(&mut factory).unwrap();
    mutable.context.diagnostic_scope = scope;
    mutable.context.uicc = None;
    assert_eq!(factory.refresh(&mut channel).await.unwrap_err().code(), "ims_uicc_binding_invalid");
    let report = runtime.snapshot().await.status_response().diagnostics;
    assert_eq!(report.failure.unwrap().scope, facts::FailureScope::Ownership);
    assert!(!report.negotiated_from_success);
    assert_eq!(server.try_recv_from(&mut [0u8; 2048]).unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
}

#[tokio::test]
async fn diagnostics_real_wifi_refresh_projects_sent_offer_validated_success_and_actual_auth_failure() {
    use crate::connectivity::core::registration_diagnostics as facts;
    for valid in [true, false] {
        let (mut factory, mut channel, server) = fixture(false).await;
        let runtime = Arc::new(crate::connectivity::modems::ims::vowifi::runtime::VowifiRuntime::for_line(&factory.line_id));
        reg_event_live::bind_reg_event_runtime(&factory.line_id, &runtime);
        let scope = diagnostics_live::Scope::begin(&factory.line_id, factory.profile).await;
        Arc::get_mut(&mut factory).unwrap().context.diagnostic_scope = scope;
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            let offered = facts::ipsec_algorithms(&sip_frame::header_values(&request, "Security-Client").join(","));
            let headers = info(&request, false);
            let headers = if valid { headers } else { headers.replace("cnonce=\"", "cnonce=\"invalid-") };
            server.send_to(&response(&request, "200 OK", &format!("Expires: 4800\r\n{headers}")), source).await.unwrap();
            offered
        });
        let result = factory.refresh(&mut channel).await;
        let offered = peer.await.unwrap();
        let snapshot = runtime.snapshot().await;
        let report = snapshot.status_response().diagnostics;
        assert_eq!(report.procedure, facts::RegistrationProcedure::Refresh);
        assert_eq!(report.offered_ipsec, Some(offered));
        assert_eq!(report.negotiated_from_success, valid);
        if valid {
            assert_eq!(result.unwrap().lease.expires_seconds, 4800);
            assert_eq!(report.rspauth_verified, Some(true));
            assert_eq!(report.negotiated_ipsec, facts::ipsec_algorithms(VERIFY));
            assert_eq!(report.wait.status, facts::WaitStatus::Ready);
        } else {
            assert!(result.unwrap_err().code().starts_with("digest_response_auth_"));
            let failure = report.failure.as_ref().unwrap();
            assert_eq!(failure.scope, facts::FailureScope::Authentication);
            assert_eq!(failure.sip_status, Some(200));
            assert!(report.negotiated_ipsec.is_empty());
            assert_retained(&factory, 2);
        }
        assert!(!snapshot.live_readiness.ims_registered, "diagnostics never grant runtime readiness");
        let json = serde_json::to_string(&report).unwrap();
        for secret in [&factory.context.identity.private_user, &nonce(1), &nonce(3)] { assert!(!json.contains(secret.as_str())); }
    }
}

#[tokio::test]
async fn registered_direct_refresh_optional_and_verified_nextnonce_paths() {
    for mode in 0..3 {
        let (factory, mut channel, server) = fixture(false).await;
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            assert!(String::from_utf8_lossy(&request).contains("nc=00000002"));
            let auth_info = match mode { 0 => info(&request, false), 1 => format!("Authentication-Info: nextnonce=\"{}\"\r\n", nonce(3)), _ => String::new() };
            server.send_to(&response(&request, "200 OK", &format!("Expires: 4800\r\n{auth_info}")), source).await.unwrap();
        });
        let registered = factory.refresh(&mut channel).await.unwrap();
        peer.await.unwrap();
        assert_eq!(registered.lease.expires_seconds, 4800);
        let state = factory.refresh_authorization.lock().unwrap();
        let state = state.as_ref().unwrap();
        assert_eq!(state.nonce_count, if mode == 2 { 2 } else { 0 });
        assert_eq!(state.challenge.nonce, if mode == 2 { nonce(1) } else { nonce(3) });
        assert!(state.request_proof.is_some());
    }
}

#[tokio::test]
async fn registered_invalid_rspauth_precedes_outbound_and_lease_commit() {
    for mode in 0..4 {
        let (factory, mut channel, server) = fixture(false).await;
        let old_route = channel.route();
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            let good = info(&request, false);
            let bad = match mode {
                0 => {
                    let start = good.find("rspauth=\"").unwrap() + "rspauth=\"".len();
                    let mut changed = good.clone();
                    changed.replace_range(start..start + 32, &"0".repeat(32));
                    changed
                },
                1 => good.replacen("cnonce=\"", "cnonce=\"changed-", 1),
                2 => format!("{good}Authentication-Info: nextnonce=duplicate\r\n"),
                _ => good.replace("Authentication-Info:", "Proxy-Authentication-Info:"),
            };
            // If outbound processing ran first, this missing required binding
            // would poison the coordinator instead of preserving its old flow.
            server.send_to(&response(&request, "200 OK", &format!("Expires: 9999\r\nRequire: outbound\r\n{bad}")), source).await.unwrap();
            server
        });
        let error = factory.refresh(&mut channel).await.unwrap_err();
        let server = peer.await.unwrap();
        assert!(error.code().starts_with("digest_response_auth_"));
        assert!(register_reason_blocks_profile_fallback(error.code()));
        assert_retained(&factory, 2);
        assert_eq!(channel.route(), old_route);
        assert_eq!(channel.security_verify(), Some(VERIFY));
        assert!(!channel.outbound_active());
        assert!(!crate::connectivity::core::ims_registration_coordinator::for_line(&factory.line_id).multiple_registration_blocked());
        channel.send_all(b"OPTIONS sip:loopback.invalid SIP/2.0\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        let (probe, source) = receive(&server).await;
        assert!(probe.starts_with(b"OPTIONS "));
        assert_eq!(source, old_route.local_addr, "failed authentication must not replace/close the old socket");
    }
}

#[tokio::test]
async fn invalid_refresh_preserves_an_existing_outbound_binding() {
    use crate::connectivity::core::ims_access::ImsAccess;
    let (factory, mut channel, server) = fixture(false).await;
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&server).await;
        server.send_to(&response(&first, "200 OK", "Expires: 3600\r\n"), source).await.unwrap();
        let (last, next_source) = receive(&server).await;
        assert_eq!(source, next_source);
        let invalid = info(&last, false).replace("nc=00000003", "nc=00000001");
        server.send_to(&response(&last, "200 OK", &format!("Expires: 0\r\nRequire: outbound\r\n{invalid}")), source).await.unwrap();
    });
    factory.refresh(&mut channel).await.unwrap();
    let coordinator = crate::connectivity::core::ims_registration_coordinator::for_line(&factory.line_id);
    let old_instance = coordinator.binding_instance(ImsAccess::Wlan).expect("first production refresh installs a live flow");
    let error = factory.refresh(&mut channel).await.unwrap_err();
    peer.await.unwrap();
    assert_eq!(error.code(), "digest_response_auth_mismatch");
    assert_retained(&factory, 3);
    assert_eq!(coordinator.binding_instance(ImsAccess::Wlan).as_deref(), Some(old_instance.as_str()));
    assert!(!coordinator.multiple_registration_blocked());
    assert_eq!(channel.security_verify(), Some(VERIFY));
}

#[tokio::test]
async fn enabled_plain_md5_placeholder_retains_nonempty_request_proof() {
    let (factory, _channel, _server) = fixture(false).await;
    let mut configured = *factory.profile;
    configured.ims.register.use_plain_digest_placeholder = true;
    let profile = Box::leak(Box::new(configured));
    // Keep this fixture within the existing nonce decoder contract: base64
    // of a short value, not a full RAND/AUTN and not malformed opaque text.
    let incoming = parse_live_digest_challenge(&format!(
        "SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"cGxhaW4=\", algorithm=MD5\r\n\r\n"), REALM).unwrap();
    let mut context = factory.context.clone();
    let variant = live_register_header_variants(profile)[0];
    let material = build_live_register_auth_material(&factory.line_id, profile, &mut context, &incoming, variant).await.unwrap();
    assert!(!material.authorization.contains("response=\"\""));
    assert!(material.refresh_authorization.as_ref().unwrap().password.is_empty());
    let request = context.build_authorized_request(profile, variant, 2, &material.authorization, None);
    validate_live_register_request(material.refresh_authorization.as_ref(), request.as_bytes()).unwrap();
    validate_live_register_success(material.refresh_authorization.as_ref(), false, b"SIP/2.0 200 OK\r\n\r\n").unwrap();
    let digest = digest_aka::compute_digest_response(
        &context.identity.private_user, REALM, &[], "MD5", "",
        &context.request_uri(profile, variant), &incoming.nonce, None, "", "00000001",
    ).unwrap();
    let success = response_minimal(&format!("Authentication-Info: rspauth=\"{digest}\", nextnonce=\"plain-next\"\r\n"));
    let mut state = material.refresh_authorization.unwrap();
    state.apply_success(&success).unwrap();
    assert_eq!(state.nonce_count, 0);
    assert_eq!(state.challenge.nonce, "plain-next");
    configured.ims.register.use_plain_digest_placeholder = false;
    assert_eq!(reject_plain_digest_when_disabled(Box::leak(Box::new(configured)), &incoming).unwrap_err().reason, "ims_digest_plain_md5_disabled");
}

#[tokio::test]
async fn registered_challenged_refresh_uses_last_emitted_scope_and_rolls_back_failure() {
    for (proxy, valid) in [(false, true), (true, true), (false, false), (true, false)] {
        let (factory, mut channel, server) = fixture(proxy).await;
        let peer = tokio::spawn(async move {
            let (first, source) = receive(&server).await;
            let old_info = info(&first, proxy);
            server.send_to(&challenge_response(&first, proxy), source).await.unwrap();
            let (last, next_source) = receive(&server).await;
            assert_eq!(next_source, source);
            let header = sip_frame::header_value(&last, if proxy { "Proxy-Authorization" } else { "Authorization" }).unwrap();
            assert_eq!(param(&header, "nc"), "00000001");
            assert_eq!(param(&header, "nonce"), nonce(2));
            let auth_info = if valid { info(&last, proxy) } else { old_info };
            server.send_to(&response(&last, "200 OK", &format!("Expires: 4800\r\n{auth_info}")), source).await.unwrap();
        });
        let result = factory.refresh(&mut channel).await;
        peer.await.unwrap();
        if valid {
            assert_eq!(result.unwrap().lease.expires_seconds, 4800);
            let state = factory.refresh_authorization.lock().unwrap();
            assert_eq!(state.as_ref().unwrap().nonce_count, 0);
            assert_eq!(state.as_ref().unwrap().challenge.nonce, nonce(3));
        } else {
            assert_eq!(result.unwrap_err().code(), "digest_response_auth_mismatch");
            assert_retained(&factory, 2);
            assert_eq!(channel.security_verify(), Some(VERIFY));
        }
    }
}

#[tokio::test]
async fn refresh_423_rebuilds_proof_without_resetting_consumed_nc_or_lease_budget() {
    for valid in [true, false] {
        let (factory, mut channel, server) = fixture(false).await;
        let peer = tokio::spawn(async move {
            let (first, source) = receive(&server).await;
            let old_info = info(&first, false);
            server.send_to(&response(&first, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
            let (last, _) = receive(&server).await;
            let first_auth = sip_frame::header_value(&first, "Authorization").unwrap();
            let last_auth = sip_frame::header_value(&last, "Authorization").unwrap();
            assert_eq!(param(&first_auth, "nc"), "00000002");
            assert_eq!(param(&last_auth, "nc"), "00000003");
            assert_ne!(param(&first_auth, "response"), param(&last_auth, "response"));
            assert_eq!(sip_frame::header_value(&last, "Expires").as_deref(), Some("7200"));
            let auth_info = if valid { info(&last, false) } else { old_info };
            server.send_to(&response(&last, "200 OK", &format!("Expires: 7200\r\n{auth_info}")), source).await.unwrap();
        });
        let result = factory.refresh(&mut channel).await;
        peer.await.unwrap();
        if valid {
            assert_eq!(result.unwrap().lease.expires_seconds, 7200);
            assert_eq!(factory.refresh_authorization.lock().unwrap().as_ref().unwrap().nonce_count, 0);
        } else {
            assert_eq!(result.unwrap_err().code(), "digest_response_auth_mismatch");
            assert_retained(&factory, 3);
        }
    }
}

#[tokio::test]
async fn unregister_production_adapter_checks_zero_expiry_direct_and_challenged() {
    for (challenged, valid) in [(false, true), (false, false), (true, true), (true, false)] {
        let (factory, mut channel, server) = fixture(false).await;
        let initial = factory.initial_request().unwrap();
        let mut adapter = RegisteredUnregisterAdapter { factory: factory.clone() };
        let peer = tokio::spawn(async move {
            let (mut request, source) = receive(&server).await;
            assert_eq!(sip_frame::header_value(&request, "Expires").as_deref(), Some("0"));
            let old_info = info(&request, false);
            if challenged {
                server.send_to(&challenge_response(&request, false), source).await.unwrap();
                request = receive(&server).await.0;
                assert_eq!(sip_frame::header_value(&request, "Expires").as_deref(), Some("0"));
                assert_eq!(param(&sip_frame::header_value(&request, "Authorization").unwrap(), "nc"), "00000001");
            }
            let auth_info = if valid { info(&request, false) } else if challenged { old_info } else { old_info.replace("nc=00000002", "nc=00000001") };
            server.send_to(&response(&request, "200 OK", &format!("Expires: 0\r\n{auth_info}")), source).await.unwrap();
        });
        let result = run_unregister(&mut channel, &initial, &mut adapter).await;
        peer.await.unwrap();
        assert_eq!(result, if valid { UnregisterResult::Confirmed } else { UnregisterResult::Rejected });
        // Deregistration never commits a new refresh nonce/lease, even if the
        // validated final 200 supplies nextnonce.
        assert_retained(&factory, 2);
    }
}

#[tokio::test]
async fn adapter_request_hooks_reject_tampered_uri_and_authorization_before_send() {
    let (factory, _channel, _server) = fixture(false).await;
    let mut refresh = VowifiRefreshAuthenticator { factory: &factory, authorization: factory.refresh_authorization.lock().unwrap().clone(), resync_sent: false, expires: 3600 };
    let request = String::from_utf8(refresh.request().unwrap()).unwrap();
    refresh.validate_request(request.as_bytes()).unwrap();
    for changed in [request.replacen("REGISTER sip:", "REGISTER sip:wrong-", 1), request.replace("nc=00000002", "nc=00000009"), request.replace("Authorization:", "Proxy-Authorization:")] {
        assert_eq!(refresh.validate_request(changed.as_bytes()).unwrap_err().code(), "digest_request_auth_mismatch");
    }
    let initial = String::from_utf8(factory.initial_request().unwrap()).unwrap();
    let adapter = RegisteredUnregisterAdapter { factory: factory.clone() };
    adapter.validate_request(initial.as_bytes()).unwrap();
    assert_eq!(adapter.validate_request(initial.replace("Authorization:", "Proxy-Authorization:").as_bytes()).unwrap_err().code(), "digest_request_auth_mismatch");
}

#[tokio::test]
async fn auts_clears_registered_response_context_but_keeps_committed_credentials() {
    let (factory, _channel, _server) = fixture(false).await;
    let previous = factory.refresh_authorization.lock().unwrap().clone();
    let mut refresh = VowifiRefreshAuthenticator { factory: &factory, authorization: previous, resync_sent: false, expires: 3600 };
    let request = refresh.request().unwrap();
    let prior_success = response(&request, "200 OK", &info(&request, false));
    refresh.authorization = None; // Production challenged_authorization's AUTS result.
    refresh.resync_sent = true;
    assert_eq!(refresh.validate_success(&prior_success).unwrap_err().code(), "ims_aka_resync_unexpected_status");
    factory.unregister_resync_sent.store(true, Ordering::Relaxed);
    assert_eq!(factory.validate_success(&prior_success).unwrap_err().code(), "ims_aka_resync_unexpected_status");
    let stale = challenge_response(&request, false);
    assert_eq!(factory.challenged_authorization(&stale, None).await.unwrap_err().code(), "digest_stale_without_context");
    assert_retained(&factory, 2);
}

#[tokio::test]
async fn retained_protected_exchange_rejects_invalid_response_without_replacing_flow() {
    for valid in [true, false] {
        let (factory, mut channel, server) = fixture(false).await;
        let route = channel.route();
        let mut context = factory.context.clone();
        context.refresh_authorization = factory.refresh_authorization.lock().unwrap().clone();
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            assert!(sip_frame::header_value(&request, "Security-Verify").is_some());
            let mut auth_info = info(&request, false);
            if !valid { auth_info = auth_info.replace("Authentication-Info:", "Proxy-Authentication-Info:"); }
            server.send_to(&response(&request, "200 OK", &format!("Expires: 9999\r\n{auth_info}")), source).await.unwrap();
        });
        let result = run_retained_stale_register_exchange(&factory.line_id, factory.profile, &mut channel, &mut context, &challenge(2, true, false), factory.variant, 9).await;
        peer.await.unwrap();
        if valid {
            let wire = result.unwrap();
            validate_live_register_success(context.refresh_authorization.as_ref(), false, wire.as_bytes()).unwrap();
            context.refresh_authorization.as_mut().unwrap().apply_success(wire.as_bytes()).unwrap();
            assert_eq!(context.refresh_authorization.as_ref().unwrap().nonce_count, 0);
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.reason, "digest_response_auth_scope_mismatch");
            assert!(live_register_error_is_terminal(&error));
            assert_eq!(context.refresh_authorization.as_ref().unwrap().challenge.nonce, nonce(2));
            assert!(!context.channel_transferred);
        }
        assert_eq!(channel.route(), route);
        assert_eq!(channel.security_verify(), Some(VERIFY));
        assert!(!channel.outbound_active());
    }
}

#[test]
fn response_auth_failure_mapping_is_terminal_even_without_challenge_rounds() {
    for code in ["digest_response_auth_invalid", "digest_response_auth_mismatch", "digest_response_auth_context_missing", "digest_response_auth_scope_mismatch", "digest_request_auth_mismatch"] {
        let failure = RegisterFailure { error: ImsError::new(code), response: Some(b"SIP/2.0 200 OK\r\n\r\n".to_vec()), auth_rounds: 0 };
        let mapped = map_shared_register_failure(&failure);
        assert!(mapped.reason.contains(code));
        assert!(live_register_error_is_terminal(&mapped));
        assert!(register_reason_blocks_profile_fallback(&mapped.reason));
        assert_eq!(classify_vowifi_register_error(code), RegistrationLossReason::AuthenticationRejected);
    }
}

#[tokio::test]
async fn rollback_retains_last_emitted_original_proof_without_rewinding_it() {
    let (factory, _channel, _server) = fixture(false).await;
    let before = factory.refresh_authorization.lock().unwrap().clone();
    let mut refresh = VowifiRefreshAuthenticator {
        factory: &factory, authorization: before.clone(), resync_sent: false, expires: 3600,
    };
    let request = refresh.request().unwrap();
    factory.refresh_authorization.lock().unwrap().as_ref().unwrap().request_proof.as_ref().unwrap()
        .validate_request(&request).unwrap();
    factory.remember_emitted(&before);
    let retained = factory.refresh_authorization.lock().unwrap();
    assert_eq!(retained.as_ref().unwrap().nonce_count, 2);
    retained.as_ref().unwrap().request_proof.as_ref().unwrap().validate_request(&request).unwrap();
}

#[test]
fn owned_exchange_errors_preserve_response_authentication_metadata() {
    for code in ["digest_response_auth_invalid", "digest_response_auth_mismatch", "digest_response_auth_context_missing", "digest_response_auth_scope_mismatch"] {
        let wrapped = format!("ims_register_authenticated_stop:{code}:sip_status=200");
        assert_eq!(live_digest_error_code(&wrapped), Some(code));
        let failure = RegisterFailure { error: ImsError::new(live_digest_error_code(&wrapped).unwrap()),
            response: Some(b"SIP/2.0 401 Unauthorized\r\n\r\n".to_vec()), auth_rounds: 1 };
        assert_eq!(failure.metadata().kind, crate::connectivity::core::register::RegisterFailureKind::AuthenticationRejected);
    }
    assert_eq!(live_digest_error_code("digest_response_auth_invalid_extra"), None);
    assert_eq!(live_digest_error_code("ims_register_read_failed"), None);
}

#[test]
fn request_proof_retained_after_nextnonce_cannot_authenticate_a_later_request() {
    let mut state = LiveRefreshAuthorization {
        challenge: challenge(1, false, false).shared(), password: vec![0x22; 8], aka: None, aka_origin: None,
        cnonce: "synthetic-rspauth-cnonce".into(), nonce_count: 0, request_proof: None,
    };
    let header = state.authorization("synthetic", "sip:ims.invalid").unwrap();
    let request = format!("REGISTER sip:ims.invalid SIP/2.0\r\n{header}\r\n\r\n");
    state.request_proof.as_ref().unwrap().validate_request(request.as_bytes()).unwrap();
    let success = response_minimal(&info(request.as_bytes(), false));
    state.apply_success(&success).unwrap();
    assert_eq!(state.nonce_count, 0);
    validate_live_register_success(Some(&state), false, &success).unwrap();
    state.authorization("synthetic", "sip:ims.invalid").unwrap();
    assert!(validate_live_register_success(Some(&state), false, &success).is_err());
    assert_eq!(state.nonce_count, 1);
    // Proof Debug, not secret credential state, remains safe for diagnostics.
    let proof: &DigestRequestProof = state.request_proof.as_ref().unwrap();
    assert!(!format!("{proof:?}").contains("synthetic"));
}

fn response_minimal(headers: &str) -> Vec<u8> {
    format!("SIP/2.0 200 OK\r\n{headers}\r\n").into_bytes()
}
