//! R1 regressions through the real refresh controller and owned loopback SIP.
//! No hardware, altered production budgets, or local execution.
use super::*;
use crate::connectivity::core::{
    register::{RegisterFailureKind, RegisterFailureStage, RegisterMinExpiresFailure},
    registration_diagnostics as facts,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

fn profile() -> &'static CarrierProfile {
    crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc,
    ).unwrap()
}
async fn fixture() -> (CellularImsLiveSession, CellularImsRuntime, UdpSocket, UdpSocket) {
    let (mut session, runtime, server, client) = protected_session_with_profile(profile()).await;
    assert!(session.register_variant.security_mechanism.is_none());
    session.channel.reserve_security_ports_for_test(session.security_binding.port_s);
    let token = runtime.begin_diagnostics().await;
    session.diagnostic_scope = token;
    let runtime = runtime.with_diagnostics(token);
    (session, runtime, server, client)
}
fn db() -> Database { Database::new(std::path::PathBuf::from(":memory:")).unwrap() }
fn cseq(request: &[u8]) -> u32 {
    sip::header_value(request, "CSeq").unwrap().split_whitespace().next().unwrap().parse().unwrap()
}
fn nc(request: &[u8]) -> u32 {
    let header = sip::header_value(request, "Authorization").unwrap();
    let value = header.split(',').find_map(|part| part.trim().strip_prefix("nc=")).unwrap();
    u32::from_str_radix(value.trim_matches('"'), 16).unwrap()
}
fn unchanged_shape(first: &[u8], next: &[u8]) {
    for name in ["Call-ID", "Security-Client", "Security-Verify", "P-Access-Network-Info", "Route"] {
        assert_eq!(sip::header_values(first, name), sip::header_values(next, name), "changed {name} during lease negotiation");
    }
    assert_eq!(cseq(next), cseq(first) + 1);
    assert_eq!(nc(next), nc(first) + 1);
    assert!(sip::header_value(next, "Security-Verify").is_some());
}
async fn quiet(server: &UdpSocket) {
    let mut bytes = [0u8; 8192];
    assert!(tokio::time::timeout(Duration::from_millis(100), server.recv_from(&mut bytes)).await.is_err(),
        "a stopped exchange reopened the candidate ladder");
}

#[tokio::test]
async fn refresh_423_then_format_status_cannot_reopen_header_candidates() {
    for status in [400u16, 415, 420, 421, 494] {
        let (mut session, runtime, server, _client) = fixture().await;
        let old_registration = session.registration.clone();
        let old_route = session.channel.send_route();
        let old_verify = session.channel.security_verify().unwrap().to_owned();
        let old_binding = session.security_binding;
        let old_variant = session.register_variant;
        let old_cseq = session.next_register_cseq;
        let old_nc = session.refresh_authorization.as_ref().unwrap().nonce_count;
        let peer = tokio::spawn(async move {
            let (first, source) = receive(&server).await;
            server.send_to(&response(&first, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
            let (next, next_source) = receive(&server).await;
            assert_eq!(next_source, source); unchanged_shape(&first, &next);
            assert_eq!(sip::header_value(&next, "Expires").as_deref(), Some("7200"));
            server.send_to(&response(&next, &format!("{status} Rejected"), ""), source).await.unwrap();
            quiet(&server).await;
        });
        let result = refresh_live_registration(&mut session, &runtime, "r1-negotiated-format", &db()).await;
        peer.await.unwrap();
        assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::NetworkRejected));
        let metadata = *result.error.as_ref().unwrap().register_failure().unwrap();
        assert_eq!(metadata.sip_status, Some(status));
        assert_eq!(metadata.min_expires, Some(RegisterMinExpiresFailure::Negotiated));
        assert!(!metadata.permits_variant_fallback());
        assert_eq!(session.registration, old_registration);
        assert_eq!(session.channel.send_route(), old_route);
        assert_eq!(session.channel.security_verify(), Some(old_verify.as_str()));
        assert_eq!(session.security_binding, old_binding);
        assert_eq!(session.register_variant, old_variant);
        assert_eq!(session.next_register_cseq, old_cseq + 2);
        assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, old_nc + 2);
        assert!(session.retired_xfrm_plan.is_none());
        assert!(runtime.status().await.diagnostics.failure.unwrap().lease_negotiation_observed);
    }
}

#[tokio::test]
async fn refresh_invalid_and_unsupported_floors_stop_without_shape_or_sa_changes() {
    for (headers, expected) in [
        ("", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 0\r\n", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 3600\r\n", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 7200\r\nMin-Expires: 8000\r\n", RegisterMinExpiresFailure::Invalid),
        ("Min-Expires: 86401\r\n", RegisterMinExpiresFailure::Unsupported),
    ] {
        let (mut session, runtime, server, _client) = fixture().await;
        let old_registration = session.registration.clone(); let old_route = session.channel.send_route();
        let old_cseq = session.next_register_cseq;
        let peer = tokio::spawn(async move {
            let (request, source) = receive(&server).await;
            server.send_to(&response(&request, "423 Interval Too Brief", headers), source).await.unwrap();
            quiet(&server).await;
        });
        let result = refresh_live_registration(&mut session, &runtime, "r1-invalid-floor", &db()).await;
        peer.await.unwrap();
        assert!(matches!(result.outcome, RegistrationRefreshResult::RebuildAccess(_)));
        assert_eq!(result.error.unwrap().register_failure().unwrap().min_expires, Some(expected));
        assert_eq!(session.next_register_cseq, old_cseq + 1);
        assert_eq!(session.registration, old_registration);
        assert_eq!(session.channel.send_route(), old_route);
        assert!(session.channel.security_verify().is_some());
    }
}

#[tokio::test]
async fn refresh_floor_rounds_are_bounded_and_a_repeated_floor_is_not_new_budget() {
    for (floors, expected) in [
        (vec![7200, 10800, 14400], RegisterMinExpiresFailure::Exhausted),
        (vec![7200, 7200], RegisterMinExpiresFailure::Invalid),
        (vec![7200, 7000], RegisterMinExpiresFailure::Invalid),
    ] {
        let (mut session, runtime, server, _client) = fixture().await;
        let count = floors.len() as u32;
        let old_cseq = session.next_register_cseq;
        let old_nc = session.refresh_authorization.as_ref().unwrap().nonce_count;
        let old_registration = session.registration.clone();
        let peer = tokio::spawn(async move {
            let mut previous: Option<Vec<u8>> = None;
            for floor in floors {
                let (request, source) = receive(&server).await;
                if let Some(old) = previous.as_ref() { unchanged_shape(old, &request); }
                server.send_to(&response(&request, "423 Interval Too Brief", &format!("Min-Expires: {floor}\r\n")), source).await.unwrap();
                previous = Some(request);
            }
            quiet(&server).await;
        });
        let result = refresh_live_registration(&mut session, &runtime, "r1-round-cap", &db()).await;
        peer.await.unwrap();
        assert_eq!(result.error.unwrap().register_failure().unwrap().min_expires, Some(expected));
        assert_eq!(session.registration, old_registration);
        assert_eq!(session.next_register_cseq, old_cseq + count);
        assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, old_nc + count);
    }
}

#[tokio::test]
async fn refresh_423_then_503_retains_only_original_lease_and_enforces_existing_wait() {
    let (mut session, runtime, server, _client) = fixture().await;
    let old_registration = session.registration.clone(); let old_route = session.channel.send_route();
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&server).await;
        server.send_to(&response(&first, "423 Interval Too Brief", "Min-Expires: 7200\r\n"), source).await.unwrap();
        let (next, next_source) = receive(&server).await;
        assert_eq!(source, next_source); unchanged_shape(&first, &next);
        server.send_to(&response(&next, "503 Service Unavailable", "Retry-After: 120\r\n"), source).await.unwrap();
        server
    });
    let result = refresh_live_registration(&mut session, &runtime, "r1-wait", &db()).await;
    let server = peer.await.unwrap();
    assert_eq!(result.outcome, RegistrationRefreshResult::Retry);
    let metadata = *result.error.unwrap().register_failure().unwrap();
    assert_eq!(metadata.min_expires, Some(RegisterMinExpiresFailure::Negotiated));
    assert_eq!(metadata.kind, RegisterFailureKind::TemporaryEndpoint);
    assert_eq!(session.registration, old_registration);
    assert_eq!(session.channel.send_route(), old_route);
    let cseq = session.next_register_cseq;
    let nc = session.refresh_authorization.as_ref().unwrap().nonce_count;
    let gated = refresh_live_registration(&mut session, &runtime, "r1-wait", &db()).await;
    assert_eq!(gated.outcome, RegistrationRefreshResult::Retry);
    assert_eq!(session.next_register_cseq, cseq);
    assert_eq!(session.refresh_authorization.as_ref().unwrap().nonce_count, nc);
    assert_eq!(runtime.status().await.diagnostics.wait.status, facts::WaitStatus::Waiting);
    quiet(&server).await;
}

#[tokio::test]
async fn refresh_authenticated_400_does_not_reopen_the_header_or_aka_budget() {
    let (mut session, runtime, server, _client) = fixture().await;
    let mut vector = vec![0x41; 32]; vector.push(1);
    let original_nonce = STANDARD.encode(&vector);
    *vector.last_mut().unwrap() = 2;
    let next_nonce = STANDARD.encode(&vector);
    let state = session.refresh_authorization.as_mut().unwrap();
    state.challenge.nonce = original_nonce.clone();
    state.aka_origin = Some(crate::connectivity::core::digest_aka::decode_aka_nonce(&original_nonce).unwrap());
    let old_registration = session.registration.clone(); let old_route = session.channel.send_route();
    let old_cseq = session.next_register_cseq;
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&server).await;
        server.send_to(&response(&first, "401 Unauthorized", &format!(
            "WWW-Authenticate: Digest realm=\"ims.example\",nonce=\"{next_nonce}\",algorithm=AKAv1-MD5,qop=\"auth\",stale=true\r\n")), source).await.unwrap();
        let (authenticated, next_source) = receive(&server).await;
        assert_eq!(source, next_source);
        assert!(sip::header_value(&authenticated, "Security-Verify").is_some());
        server.send_to(&response(&authenticated, "400 Bad Request", ""), source).await.unwrap();
        quiet(&server).await;
    });
    let result = refresh_live_registration(&mut session, &runtime, "r1-authenticated-stop", &db()).await;
    peer.await.unwrap();
    assert_eq!(result.outcome, RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::NetworkRejected));
    let metadata = *result.error.unwrap().register_failure().unwrap();
    assert_eq!(metadata.stage, RegisterFailureStage::Authenticated);
    assert_eq!(metadata.min_expires, None);
    assert!(!metadata.permits_variant_fallback());
    assert_eq!(session.next_register_cseq, old_cseq + 2);
    assert_eq!(session.registration, old_registration);
    assert_eq!(session.channel.send_route(), old_route);
}

#[tokio::test]
async fn existing_non_ipsec_refresh_keeps_initial_header_compatibility_without_423() {
    // Start with the existing explicitly non-IPsec fixture; never remove an SA
    // or turn a protected session into plaintext to make this test pass.
    let (live, runtime, server) = super::super::tests::test_voice_session().await;
    let mut session = live.session.lock().await.take().unwrap();
    assert!(session.channel.security_verify().is_none());
    session.profile = profile(); session.effective_ims = resolve_effective_ims_profile(profile(), None);
    session.register_variant = register_variants(profile())[0];
    session.refresh_authorization = Some(CellularImsRefreshAuthorization::new(challenge("existing-nonce"), aka()));
    let old_route = session.channel.send_route(); let old_cseq = session.next_register_cseq;
    let peer = tokio::spawn(async move {
        let (first, source) = receive(&server).await;
        server.send_to(&response(&first, "400 Bad Request", ""), source).await.unwrap();
        let (second, second_source) = receive(&server).await;
        assert_eq!(source, second_source); assert_eq!(cseq(&second), cseq(&first) + 1);
        assert_eq!(nc(&second), nc(&first) + 1);
        server.send_to(&response(&second, "200 OK", "Expires: 3600\r\n"), source).await.unwrap();
    });
    let result = refresh_live_registration(&mut session, &runtime, "r1-ordinary-compatibility", &db()).await;
    peer.await.unwrap();
    assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_)));
    assert_eq!(session.next_register_cseq, old_cseq + 2);
    assert_eq!(session.channel.send_route(), old_route);
    assert!(session.channel.security_verify().is_none());
}
