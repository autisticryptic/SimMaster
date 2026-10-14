//! R2 same-subscription gates and the production endpoint-pass controller.
//! Only synthetic card observations and loopback SIP; executed in Actions.
use super::*;
use crate::connectivity::core::{context::{ImsIdentity, ImsRoute, SipTransport}, register::{RegisterAuthenticator, RegisterFailureStage}};
use crate::connectivity::modems::ims::uicc_ims::UiccCardBinding;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::UdpSocket;

fn line() -> String {
    static IDS: AtomicUsize = AtomicUsize::new(0);
    format!("r2-retry-scope-{}", IDS.fetch_add(1, Ordering::Relaxed))
}
fn owner(line: &str, card_id: &str, device: &str) -> Arc<LiveImsSelection> {
    let profile = profiles::derive_standard_3gpp_profile("001", "01", profiles::Standard3gppAccess::WifiEpdg).unwrap();
    let realm = profile.ims.realm;
    let identity = LiveImsRegisterIdentity { shared: ImsIdentity {
        private_user: format!("001010000000001@{realm}"), public_uri: format!("sip:001010000000001@{realm}"),
        contact_user: "001010000000001".into(), home_domain: realm.into(), contact_user_phone: false,
    }, shape: "r2-fixture" };
    let target = live_ims_target(line, profile);
    live_identity::selection_for_test(line, identity, target, UiccCardBinding {
        endpoint: format!("/synthetic/{device}"), slot: 1, owner: format!("owner-{device}"),
        iccid: card_id.into(), imsi: "001010000000001".into(),
    })
}
fn endpoint() -> SocketAddr { "192.0.2.1:5060".parse().unwrap() }
fn temporary(seconds: u32) -> RegisterFailureMetadata {
    RegisterFailureMetadata { sip_status: Some(503), stage: RegisterFailureStage::Initial,
        kind: RegisterFailureKind::TemporaryEndpoint, retry_after: RetryAfter::DelaySeconds(seconds), min_expires: None }
}

#[tokio::test]
async fn ordinary_wait_survives_clear_profile_worker_and_control_port_reenumeration() {
    let line = line(); let original = owner(&line, "card-one", "before");
    let first = Scope::for_owner(&line, &original).await.unwrap();
    let wait = first.observe(endpoint(), &temporary(120)).await;
    first.pause_restart().await;
    record_live_ims_refresh_failure(&line, "temporary-fixture").await;
    record_live_ims_refresh_failure(&line, "temporary-fixture").await;
    clear_live_runtime_for_line(&line).await;
    assert_eq!(live_ims_refresh_failure_count_for_line(&line).await, 2);
    let new = owner(&line, "card-one", "after");
    let again = Scope::for_owner(&line, &new).await.unwrap();
    assert!(Arc::ptr_eq(&first.state, &again.state));
    assert_eq!(again.check(endpoint()).await, wait);
    assert_eq!(restart_admission(&line).await, wait);
    assert_eq!(live_ims_refresh_failure_count_for_line(&line).await, 2);
    assert_eq!(check_restart_gate(&line).await.unwrap_err().reason, DEFERRED);
    assert!(again.accepted(endpoint()).await.is_err(), "late success cannot erase unelapsed wait");
}

#[tokio::test]
async fn proven_new_sim_replaces_scope_but_late_old_writer_only_touches_old_state() {
    let line = line(); let original = owner(&line, "card-one", "before");
    let old = Scope::for_owner(&line, &original).await.unwrap();
    old.observe(endpoint(), &temporary(120)).await; old.pause_restart().await;
    record_live_ims_refresh_failure(&line, "old-subscription-failure").await;
    let new_owner = owner(&line, "card-two", "after");
    assert_eq!(restart_admission(&line).await, RetryAdmission::Allowed);
    assert_eq!(live_ims_refresh_failure_count_for_line(&line).await, 0);
    let new = Scope::for_owner(&line, &new_owner).await.unwrap();
    assert!(!Arc::ptr_eq(&old.state, &new.state));
    old.observe(endpoint(), &temporary(1000)).await;
    assert!(new.check(endpoint()).await.is_allowed());
    assert_eq!(Scope::for_owner(&line, &original).await.unwrap_err().code(), "ims_uicc_binding_changed");
}

#[tokio::test]
async fn unavailable_subscription_observation_does_not_clear_an_existing_wait() {
    let line = line(); let selected = owner(&line, "card-one", "before");
    let scope = Scope::for_owner(&line, &selected).await.unwrap();
    let wait = scope.observe(endpoint(), &temporary(120)).await; scope.pause_restart().await;
    // Invalid slot returns before any system/device access in capture_card_binding.
    register_line_sim_device(&line, "/synthetic/unavailable", 0, "unavailable");
    assert_eq!(restart_admission(&line).await, wait);
}

#[tokio::test]
async fn invalid_and_long_waits_stop_instead_of_becoming_zero_or_short_clamps() {
    for retry_after in [RetryAfter::DelaySeconds(86401), RetryAfter::Invalid(crate::connectivity::core::register::RetryAfterInvalid::Duplicate)] {
        let line = line(); let selected = owner(&line, "card-one", "before");
        let scope = Scope::for_owner(&line, &selected).await.unwrap();
        let metadata = RegisterFailureMetadata { retry_after, ..temporary(1) };
        assert!(matches!(scope.observe(endpoint(), &metadata).await, RetryAdmission::Stopped(_)));
        scope.pause_restart().await;
        assert_eq!(live_register_retry_remaining_for_line(&line).await, Some(Duration::MAX));
        let new = owner(&line, "card-one", "after");
        assert!(matches!(Scope::for_owner(&line, &new).await.unwrap().check(endpoint()).await, RetryAdmission::Stopped(_)));
    }
}

#[test]
fn only_real_temporary_or_final_waits_stop_the_inner_header_ladder() {
    assert!(response_requires_wait(temporary(1)));
    assert!(response_requires_wait(RegisterFailureMetadata { kind: RegisterFailureKind::Negotiation,
        sip_status: Some(421), ..temporary(30) }));
    assert!(!response_requires_wait(RegisterFailureMetadata { kind: RegisterFailureKind::Transport,
        sip_status: Some(401), ..temporary(300) }));
    let err = owned_transport_error(ImsError::new("ims_channel_read_timeout"), false);
    assert_eq!(err.register_failure.unwrap().kind, RegisterFailureKind::Transport);
    assert_eq!(err.register_failure.unwrap().retry_after, RetryAfter::Absent);
    assert_eq!(err.register_failure.unwrap().sip_status, None);
    assert_eq!(owned_transport_error(ImsError::new("ims_outbound_flow_failed"), true).register_failure.unwrap().kind,
        RegisterFailureKind::FlowFailed);
}

struct Auth;
fn request(cseq: u32) -> Vec<u8> {
    format!("REGISTER sip:ims.example.test SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1;branch=z9hG4bK-r2-{cseq}\r\nCall-ID: r2-loopback\r\nCSeq: {cseq} REGISTER\r\nExpires: 3600\r\nContent-Length: 0\r\n\r\n").into_bytes()
}
impl RegisterAuthenticator<SipChannel> for Auth {
    async fn authenticated_request(&mut self, _: &[u8], cseq: u32) -> Result<Vec<u8>, ImsError> { Ok(request(cseq)) }
}
async fn exchange(endpoint: SocketAddr) -> Result<(), LiveStageError> {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap(); socket.connect(endpoint).await.unwrap();
    let route = ImsRoute { local_addr: socket.local_addr().unwrap(), pcscf_addr: endpoint, transport: SipTransport::Udp };
    let mut channel = SipChannel::new(SipChannelSocket::Udp(socket), vec![], route, None);
    crate::connectivity::core::register::run_register_observed(&mut channel, &request(1), &mut Auth).await
        .map(|_| ()).map_err(|error| map_shared_register_failure(&error))
}
async fn peer(responses: Vec<(u16, &'static str)>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let server = UdpSocket::bind("127.0.0.1:0").await.unwrap(); let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for (status, extra) in responses {
            let mut buffer = [0u8; 8192];
            let (n, remote) = tokio::time::timeout(Duration::from_secs(3), server.recv_from(&mut buffer)).await.unwrap().unwrap();
            let request = &buffer[..n];
            let response = format!("SIP/2.0 {status} Fixture\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\n{extra}Content-Length: 0\r\n\r\n",
                sip_frame::header_value(request, "Via").unwrap(), sip_frame::header_value(request, "Call-ID").unwrap(),
                sip_frame::header_value(request, "CSeq").unwrap());
            server.send_to(response.as_bytes(), remote).await.unwrap();
        }
    });
    (addr, task)
}

#[tokio::test]
async fn real_initial_503_uses_an_eligible_alternate_without_retrying_the_deferred_endpoint() {
    let line = line(); let selected = owner(&line, "card-one", "before");
    let scope = Scope::for_owner(&line, &selected).await.unwrap();
    let (first, a) = peer(vec![(503, "Retry-After: 120\r\n")]).await;
    let (second, b) = peer(vec![(200, "Expires: 3600\r\n")]).await;
    let calls = AtomicUsize::new(0);
    run_candidates(&scope, &[first, first, second], |endpoint| { calls.fetch_add(1, Ordering::SeqCst); exchange(endpoint) }).await.unwrap();
    a.await.unwrap(); b.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!scope.check(first).await.is_allowed());
    assert!(scope.check(second).await.is_allowed());
    assert!(restart_admission(&line).await.is_allowed());
}

#[tokio::test]
async fn ordinary_pre_auth_404_compatibility_is_preserved_without_a_server_wait() {
    let line = line(); let selected = owner(&line, "card-one", "before");
    let scope = Scope::for_owner(&line, &selected).await.unwrap();
    let (first, a) = peer(vec![(404, "")]).await;
    let (second, b) = peer(vec![(200, "Expires: 3600\r\n")]).await;
    run_candidates(&scope, &[first, second], exchange).await.unwrap();
    a.await.unwrap(); b.await.unwrap();
    assert!(scope.check(first).await.is_allowed());
    assert!(restart_admission(&line).await.is_allowed());
}

#[tokio::test]
async fn real_authenticated_503_preserves_final_response_not_the_prior_401_and_stops_alternates() {
    let line = line(); let selected = owner(&line, "card-one", "before");
    let scope = Scope::for_owner(&line, &selected).await.unwrap();
    let (first, a) = peer(vec![(401, "WWW-Authenticate: Digest realm=ims.example.test,nonce=fixture\r\n"),
        (503, "Retry-After: 120\r\n")]).await;
    let calls = AtomicUsize::new(0);
    let error = run_candidates(&scope, &[first, "127.0.0.1:9".parse().unwrap()], |endpoint| {
        calls.fetch_add(1, Ordering::SeqCst); exchange(endpoint)
    }).await.unwrap_err();
    a.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let metadata = error.register_failure.unwrap();
    assert_eq!(metadata.sip_status, Some(503)); assert_eq!(metadata.stage, RegisterFailureStage::Authenticated);
    assert_eq!(metadata.kind, RegisterFailureKind::TemporaryEndpoint);
    assert_eq!(metadata.retry_after, RetryAfter::DelaySeconds(120));
    assert!(!restart_admission(&line).await.is_allowed());
    assert!(register_reason_blocks_profile_fallback(&error.reason));
}

#[tokio::test]
async fn real_421_retry_after_stops_dynamic_headers_and_a_new_profile_pass() {
    let line = line(); let selected = owner(&line, "card-one", "before");
    let scope = Scope::for_owner(&line, &selected).await.unwrap();
    let (first, a) = peer(vec![(421, "Require: sec-agree\r\nRetry-After: 120\r\n")]).await;
    let error = run_candidates(&scope, &[first], exchange).await.unwrap_err(); a.await.unwrap();
    assert!(error.server_required_sec_agree);
    let profile = profiles::derive_standard_3gpp_profile("001", "01", profiles::Standard3gppAccess::WifiEpdg).unwrap();
    assert!(next_dynamic_live_register_variant(profile, live_register_header_variants(profile)[0], &error).is_none());
    assert!(register_reason_blocks_profile_fallback(&error.reason));
    assert!(check_restart_gate(&line).await.is_err());
}
