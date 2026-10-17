//! Production provider -> DNS selection -> REGISTER -> retained-handle move.
//! Only the kernel/device interfaces are synthetic. Run on Actions, not locally.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::hardware::devices::transport::{ImsBearerHandle, ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint, TransportFuture};
use crate::services::ue_worker::{NetConfigOp, NetStatusSnapshot, UeSocketKind};

struct Control {
    result: std::sync::Mutex<Result<Option<ImsPcscfDiscovery>, ImsBearerError>>,
    reads: AtomicUsize,
    releases: AtomicUsize,
    replace_worker: AtomicBool,
    worker: UeWorkerHandle,
}
struct Handle(Arc<Control>);
impl ImsBearerHandle for Handle {
    fn check_liveness(&mut self) -> Result<(), ImsBearerError> { Ok(()) }
    fn discover_pcscf(&mut self) -> TransportFuture<'_, Result<Option<ImsPcscfDiscovery>, ImsBearerError>> {
        Box::pin(async move {
            self.0.reads.fetch_add(1, Ordering::SeqCst);
            let result = self.0.result.lock().unwrap().clone();
            if self.0.replace_worker.load(Ordering::SeqCst) { self.0.worker.advance_test_generation().await; }
            result
        })
    }
    fn release(self: Box<Self>) -> std::pin::Pin<Box<dyn std::future::Future<Output=()> + Send + 'static>> {
        self.0.releases.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}
fn discovery(addresses: &[&str], complete: bool) -> ImsPcscfDiscovery {
    ImsPcscfDiscovery { candidates: addresses.iter().map(|value| value.parse().unwrap()).collect(),
        domains: vec![], ipv6_complete: complete, context_id: None, source: "synthetic_retained_pco" }
}
fn profile() -> &'static CarrierProfile {
    let mut profile = *crate::connectivity::modems::ims::vowifi::profiles::derive_standard_3gpp_profile(
        "001", "01", crate::connectivity::modems::ims::vowifi::profiles::Standard3gppAccess::LteEpc).unwrap();
    profile.ims.register.initial_authorization = "none";
    profile.ims.register.sec_agree_mode = "supported";
    profile.ims.register.require_sec_agree_headers = false;
    profile.ims.register.proxy_require_sec_agree_headers = false;
    profile.ims.register.include_pani_initial = false;
    profile.ims.register.include_pani_authenticated = false;
    profile.ims.register.enable_cellular_network_info = false;
    Box::leak(Box::new(profile))
}
struct Fixture {
    session: CellularImsLiveSession,
    runtime: CellularImsRuntime,
    old_peer: tokio::net::UdpSocket,
    protected_client: Option<tokio::net::UdpSocket>,
    control: Arc<Control>,
    operations: Arc<std::sync::Mutex<Vec<NetConfigOp>>>,
}
#[cfg(unix)]
async fn fixture(protected: bool) -> Fixture {
    let profile = profile();
    let (mut session, runtime, old_peer, protected_client) = if protected {
        let (session, runtime, peer, client) = super::super::refresh_tests::protected_session_with_profile(profile).await;
        (session, runtime, peer, Some(client))
    } else {
        let (live, runtime, old) = super::super::tests::test_voice_session().await;
        drop(old);
        let mut session = live.session.lock().await.take().unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:5060").await.unwrap();
        session.channel = CellularImsSipChannel::bind(ImsRoute { local_addr: "127.0.0.1:0".parse().unwrap(),
            pcscf_addr: peer.local_addr().unwrap(), transport: SipTransport::Udp }, None, None).unwrap();
        session.pcscf = peer.local_addr().unwrap();
        session.profile = profile; session.effective_ims = resolve_effective_ims_profile(profile, None);
        (session, (*runtime).clone(), peer, None)
    };
    session.selected_endpoint.source = PcscfSource::BearerPco;
    session.selected_endpoint.socket = SocketAddr::new("127.0.0.1".parse().unwrap(), 5060);
    session.video_capability_enabled = true;
    session.dynamic_visited_network_fallback = true;
    let operations = Arc::new(std::sync::Mutex::new(vec![]));
    session.network_worker.enable_test_registration_io(NetStatusSnapshot {
        interfaces: vec!["lo".into()], addresses: vec!["127.0.0.1".into()], default_routes: vec![],
    }, operations.clone(), |spec| {
        assert_eq!(spec.kind, UeSocketKind::Udp);
        let socket = std::net::UdpSocket::bind(spec.bind.unwrap())?;
        socket.set_nonblocking(true)?;
        if let Some(remote) = spec.connect { socket.connect(remote)?; }
        Ok(socket.into())
    }).await;
    let control = Arc::new(Control { result: std::sync::Mutex::new(Ok(Some(discovery(&["127.0.0.2"], true)))),
        reads: AtomicUsize::new(0), releases: AtomicUsize::new(0), replace_worker: AtomicBool::new(false),
        worker: session.network_worker.clone() });
    session.native_bearer = Some(NativeImsBearer::retained_for_test(session.bearer.clone(), session.worker_binding.clone(), Box::new(Handle(control.clone()))));
    Fixture { session, runtime, old_peer, protected_client, control, operations }
}
async fn receive(peer: &tokio::net::UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut frame = vec![0; 16384];
    let (len, from) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut frame)).await.unwrap().unwrap();
    frame.truncate(len); (frame, from)
}
fn response(request: &[u8], status: &str, headers: &str) -> Vec<u8> {
    format!("SIP/2.0 {status}\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\n{headers}Content-Length: 0\r\n\r\n",
        sip::header_value(request, "Via").unwrap(), sip::header_value(request, "Call-ID").unwrap(), sip::header_value(request, "CSeq").unwrap()).into_bytes()
}
async fn quiet(peer: &tokio::net::UdpSocket) {
    assert!(tokio::time::timeout(Duration::from_millis(70), peer.recv_from(&mut [0u8; 2048])).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn same_bearer_reselection_uses_real_registration_and_never_reactivates_pdn() {
    let mut f = fixture(false).await;
    let new_peer = tokio::net::UdpSocket::bind("127.0.0.2:5060").await.unwrap();
    let old_local = f.session.channel.route().local_addr;
    let old_call_id = f.session.register_ids.call_id.clone();
    f.session.mwi_subscription = Some(MwiSubscription { ids: SubscribeIds {
        branch: "retired-branch".into(), from_tag: "retired-tag".into(), to_tag: None,
        call_id: "retired-mwi".into(), cseq: 4 }, refresh_at: tokio::time::Instant::now() + Duration::from_secs(600), authenticated: false });
    let old_bearer = f.session.bearer.path.clone(); let old_instance = f.session.sip_instance.clone();
    let binding = f.session.worker_binding.clone();
    let sender = tokio::spawn(async move {
        let (request, from) = receive(&new_peer).await;
        assert_ne!(from, old_local);
        assert!(sip::header_value(&request, "Authorization").is_none(), "no old Digest proof crosses peers");
        assert!(sip::header_value(&request, "Security-Verify").is_none());
        new_peer.send_to(&response(&request, "200 OK", "Expires: 1800\r\n"), from).await.unwrap();
        new_peer
    });
    let result = refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.unwrap();
    assert!(matches!(result.outcome, RegistrationRefreshResult::Refreshed(_)));
    assert_eq!(f.session.pcscf, "127.0.0.2:5060".parse().unwrap());
    assert_eq!(f.session.bearer.path, old_bearer); assert_eq!(f.session.sip_instance, old_instance);
    assert!(f.session.worker_binding.matches(&binding));
    assert_ne!(f.session.register_ids.call_id, old_call_id);
    assert!(f.session.mwi_subscription.is_none(), "old subscription cannot follow the new registration");
    assert!(f.session.video_capability_enabled && f.session.dynamic_visited_network_fallback);
    assert_eq!(f.control.releases.load(Ordering::SeqCst), 0);
    assert_eq!(f.runtime.status().await.register_refresh_count, 0, "reselection is not natural renewal");
    assert!(f.operations.lock().unwrap().iter().all(|op| matches!(op, NetConfigOp::RouteReplace { .. })));
    quiet(&f.old_peer).await; quiet(&sender.await.unwrap()).await;
    super::super::super::native_bearer::release_native_ims_bearer(f.session.native_bearer.take().unwrap()).await;
    assert_eq!(f.control.releases.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn unchanged_or_reordered_candidates_keep_the_existing_protected_flow() {
    let mut f = fixture(true).await;
    let old = f.session.channel.send_route(); let old_verify = f.session.channel.security_verify().unwrap().to_owned();
    // Logical PCO port is independent of negotiated protected ports.
    *f.control.result.lock().unwrap() = Ok(Some(discovery(&["127.0.0.2", "127.0.0.1"], true)));
    assert!(refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.is_none());
    assert_eq!(f.session.channel.send_route(), old); assert_eq!(f.session.channel.security_verify(), Some(old_verify.as_str()));
    assert!(f.operations.lock().unwrap().is_empty());
    assert_eq!(f.control.releases.load(Ordering::SeqCst), 0); quiet(&f.old_peer).await;
}

#[cfg(unix)]
#[tokio::test]
async fn protected_reselection_cannot_accept_a_new_unprotected_success() {
    let mut f = fixture(true).await;
    let old = f.session.registration.clone(); let old_route = f.session.channel.send_route();
    let new_peer = tokio::net::UdpSocket::bind("127.0.0.2:5060").await.unwrap();
    let sender = tokio::spawn(async move {
        let (request, from) = receive(&new_peer).await;
        new_peer.send_to(&response(&request, "200 OK", "Expires: 7200\r\n"), from).await.unwrap(); new_peer
    });
    let result = refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.unwrap();
    assert!(matches!(result.outcome, RegistrationRefreshResult::RebuildAccess(_)));
    assert_eq!(f.session.registration, old); assert_eq!(f.session.channel.send_route(), old_route);
    assert!(f.session.channel.security_verify().is_some()); assert_eq!(f.control.releases.load(Ordering::SeqCst), 0);
    quiet(&sender.await.unwrap()).await;
}

#[cfg(unix)]
#[tokio::test]
async fn temporary_new_peer_failure_keeps_old_lease_and_same_sim_retry_wait() {
    let mut f = fixture(false).await; let old = f.session.registration.clone();
    let new_peer = tokio::net::UdpSocket::bind("127.0.0.2:5060").await.unwrap();
    let sender = tokio::spawn(async move {
        let (request, from) = receive(&new_peer).await;
        new_peer.send_to(&response(&request, "503 Service Unavailable", "Retry-After: 120\r\n"), from).await.unwrap(); new_peer
    });
    let result = refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.unwrap();
    let peer = sender.await.unwrap();
    assert_eq!(result.outcome, RegistrationRefreshResult::Retry); assert_eq!(f.session.registration, old);
    assert_eq!(f.runtime.status().await.pcscf, Some(f.session.pcscf.to_string()), "runtime must not advertise the rejected peer as registered");
    assert!(!f.session.endpoint_retries.lock().await.restart_admission(Instant::now()).is_allowed());
    let again = refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.unwrap();
    assert_eq!(again.outcome, RegistrationRefreshResult::Retry);
    assert_eq!(f.control.releases.load(Ordering::SeqCst), 0); quiet(&peer).await; quiet(&f.old_peer).await;
}

#[cfg(unix)]
#[tokio::test]
async fn unavailable_or_empty_observation_does_not_invent_endpoint_removal() {
    let mut f = fixture(false).await;
    for value in [Ok(None), Ok(Some(discovery(&[], true))), Err(ImsBearerError { kind: ImsBearerErrorKind::PcscfUnavailable,
        hint: ImsBearerFailureHint::None, detail: "synthetic_unavailable".into() })] {
        *f.control.result.lock().unwrap() = value;
        assert!(refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.is_none());
    }
    assert!(f.operations.lock().unwrap().is_empty()); quiet(&f.old_peer).await;
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_non_pco_source_never_queries_or_switches_to_provider_results() {
    let mut f = fixture(false).await;
    for source in [PcscfSource::Environment, PcscfSource::Profile, PcscfSource::Isim, PcscfSource::BearerDhcp, PcscfSource::StandardDns] {
        f.session.selected_endpoint.source = source;
        assert!(refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.is_none());
    }
    assert_eq!(f.control.reads.load(Ordering::SeqCst), 0); quiet(&f.old_peer).await;
}

#[cfg(unix)]
#[tokio::test]
async fn provider_worker_change_stops_before_dns_routes_or_register() {
    let mut f = fixture(false).await;
    f.control.replace_worker.store(true, Ordering::SeqCst);
    let result = refresh_if_changed(&mut f.session, &f.runtime, "volte-dialog-matrix").await.unwrap();
    assert_eq!(result.error.unwrap().code(), code::RUNTIME_UE_WORKER_GENERATION_CHANGED);
    assert!(f.operations.lock().unwrap().is_empty()); quiet(&f.old_peer).await;
}

#[test]
fn partial_ipv6_and_changed_tls_authority_never_relabel_a_valid_binding() {
    let mut endpoint = PcscfEndpoint { socket: "[2001:db8::1]:5061".parse().unwrap(), source: PcscfSource::BearerPco,
        host: None, transport: PcscfTransport::Tls, tls_authority: Some("original.example.test".into()),
        ttl: Some(30), srv_priority: None, srv_weight: None };
    assert!(!complete_for_current_family(&discovery(&["192.0.2.1"], false), &endpoint));
    let old = endpoint.clone(); endpoint.ttl = Some(5); assert!(same_endpoint(&old, &endpoint));
    endpoint.tls_authority = Some("other.example.test".into()); assert!(!same_endpoint(&old, &endpoint));
    endpoint.tls_authority = old.tls_authority.clone(); endpoint.transport = PcscfTransport::Udp;
    assert!(!same_endpoint(&old, &endpoint));
}
