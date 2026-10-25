//! Actions-only two-connection bootstrap/account peers. Synthetic DER roots and
//! native TLS/Common framing; no DNS, SDK execution, device or paid operation.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as native;
use native::runtime_provision::AccountTrustInputs;
use std::future::Future;

const SOURCE_AUTHORITY: &str = "bootstrap.fixture.invalid";
const ACCOUNT_AUTHORITY: &str = "account.fixture.invalid";
const SOURCE_REFERENCE: &str = "#original-bootstrap:tcp -h bootstrap.fixture.invalid -p 443;";
const ACCOUNT_REFERENCE: &[u8] = b"#returned-account:tcp -h account.fixture.invalid -p 9443;";
const ACCOUNT_DOMAIN: &[u8] = b"returned-account-domain.invalid";

struct ConnectionRecord {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    reference: String,
    authority: String,
    root: String,
    sni: bool,
}
struct TwoConnector {
    io: Mutex<[Option<TrackedIo>; 2]>,
    records: Mutex<Vec<ConnectionRecord>>,
    source_dropped: Arc<AtomicBool>,
    pending_destination: bool,
    substitute_destination: bool,
}
impl RuntimeConnector for TwoConnector {
    fn connect<'a>(&'a self, owner: &'a AttemptOwner, scope: &'a RpcConnectionScope,
        input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move {
            let index = {
                let mut records = lock(&self.records);
                let index = records.len();
                if index > 1 { return Err(ConnectError::Io); }
                if index == 1 {
                    // This assertion observes the actual owning actor's IO Drop,
                    // not merely its terminal scope flag or a bootstrap receipt.
                    assert!(self.source_dropped.load(Ordering::SeqCst));
                    assert!(!records[0].scope.is_open());
                    assert!(!records[0].scope.matches(scope));
                    assert_eq!(&records[0].owner, owner);
                }
                records.push(ConnectionRecord { owner: owner.clone(), scope: scope.clone(),
                    reference: input.server_reference.to_owned(), authority: input.authority.to_owned(),
                    root: input.root_der_base64.to_owned(), sni: input.enable_sni });
                index
            };
            let io = lock(&self.io)[index].take().ok_or(ConnectError::Io)?;
            if index == 1 && self.pending_destination { std::future::pending::<()>().await; }
            if index == 1 && self.substitute_destination {
                ConnectedIo::original(owner, &RpcConnectionScope::fresh(), input, io)
            } else { ConnectedIo::original(owner, scope, input, io) }
        })
    }
}
struct Fixture {
    backend: Arc<NativeHostingBackend>,
    events: mpsc::Receiver<BackendEvent>,
    connector: Arc<TwoConnector>,
    source: Peer,
    account: Peer,
    source_dropped: Arc<AtomicBool>,
    account_dropped: Arc<AtomicBool>,
}
fn bootstrap_profile(account_root: &str, handoff_timeout: Duration) -> RuntimeProfile {
    let client = [RpcProperty { key: b"ClientId", value: b"fixture-exact-client-id" },
        RpcProperty { key: b"__appKey", value: b"ignored-imported-key" },
        RpcProperty { key: b"__domain", value: b"ignored-imported-domain" },
        RpcProperty { key: b"__app", value: b"ignored-imported-app" },
        RpcProperty { key: b"DeviceInfo.CC", value: b"ignored-stale-cc" }];
    let mut input = inputs(&client);
    input.authority = SOURCE_AUTHORITY;
    input.domain_name = ""; input.domain_id = ""; input.application_id = ""; input.device_info_cc = "";
    input.login_routing = &[RpcProperty { key: b"id", value: b"#exact-imported-login-routing" },
        RpcProperty { key: b"domain", value: b"exact-imported-routing-domain" }];
    RuntimeProfile::supplied_bootstrap(input, RuntimeBootstrapInputs {
        account_trust: AccountTrust::supplied(AccountTrustInputs { authority: ACCOUNT_AUTHORITY,
            root_der_base64: account_root, enable_sni: false }).unwrap(),
        routing: &[RpcProperty { key: b"ServerGroup", value: b"fixture-original-server-group" }],
        client_config_decode_key: None, handoff_timeout,
    }).unwrap()
}
fn bootstrap_request(root: &str) -> SdkLoginRequest<'_> {
    let mut input = request(root); input.server_address = SOURCE_REFERENCE; input
}
fn peer_io(cert: &Certificates) -> (TrackedIo, Peer, Arc<AtomicBool>) {
    let (client, server) = tokio::io::duplex(65_536);
    let dropped = Arc::new(AtomicBool::new(false));
    let tracked = TrackedIo { io: client, dropped: dropped.clone(), block_writes: Arc::new(AtomicBool::new(false)) };
    let peer = Peer { io: server, server: cert.server(&rustls::version::TLS13), network: PacketStreamDecoder::default(),
        plaintext: PlaintextPacketDecoder::default(), assembler: CallAssembler::default(), received: VecDeque::new() };
    (tracked, peer, dropped)
}
fn two_fixture(source: &Certificates, account: &Certificates, timeout: Duration,
    pending_destination: bool, substitute_destination: bool) -> Fixture {
    let (source_io, source_peer, source_dropped) = peer_io(source);
    let (account_io, account_peer, account_dropped) = peer_io(account);
    let connector = Arc::new(TwoConnector { io: Mutex::new([Some(source_io), Some(account_io)]),
        records: Mutex::new(Vec::new()), source_dropped: source_dropped.clone(), pending_destination, substitute_destination });
    let (backend, events) = NativeHostingBackend::new(bootstrap_profile(&account.root, timeout), connector.clone());
    Fixture { backend, events, connector, source: source_peer, account: account_peer, source_dropped, account_dropped }
}
fn properties(include_cc: bool) -> Vec<RpcProperty<'static>> {
    let mut values = vec![RpcProperty { key: b"DomainName", value: ACCOUNT_DOMAIN },
        RpcProperty { key: b"DomainId", value: b"returned-domain" },
        RpcProperty { key: b"AppId", value: b"returned-app" }];
    if include_cc { values.push(RpcProperty { key: b"CC", value: b"returned-cc" }); }
    values
}
fn bootstrap_result(properties: &[RpcProperty<'_>], reference: &[u8], success: bool) -> Vec<u8> {
    let mut result = Writer::new(); result.boolean(success).unwrap(); result.properties(properties).unwrap();
    result.bytes(reference).unwrap(); result.bytes(b"unclaimed-alive-reference").unwrap();
    result.bytes(b"unclaimed-user-reference").unwrap(); result.bytes(b"").unwrap();
    result.finish().expose_to_transport().to_vec()
}
async fn bootstrap_call(peer: &mut Peer) -> i32 {
    let message = peer.receive().await.unwrap();
    let call = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(call.method, native::rpc_bootstrap::GET_APP_CONFIGS_METHOD.as_bytes());
    assert!(!call.metadata.iter().any(|p| p.key == b"pwd" || p.key == b"token"));
    assert_eq!(call.metadata.iter().find(|p| p.key == b"ServerGroup").unwrap().value, b"fixture-original-server-group");
    let mut args = Reader::versioned(call.arguments).unwrap();
    assert_eq!(args.bytes().unwrap(), b"fixture-original-appkey"); args.finish().unwrap();
    call.call_id
}
async fn account_login(peer: &mut Peer, include_cc: bool) -> i32 {
    let message = peer.receive().await.unwrap();
    let call = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(call.method, native::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
    assert_eq!(call.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, b"fixture-original-pwd");
    assert_eq!(call.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"#exact-imported-login-routing");
    assert_eq!(call.metadata.iter().find(|p| p.key == b"domain").unwrap().value, b"exact-imported-routing-domain");
    assert!(!call.metadata.iter().any(|p| p.key == b"token"));
    let mut args = Reader::versioned(call.arguments).unwrap();
    assert_eq!(args.bytes().unwrap(), b"[username:fixture-account@returned-account-domain.invalid]");
    assert_eq!(args.bytes().unwrap(), b"fixture-original-selector");
    let fields = args.properties().unwrap(); args.finish().unwrap();
    for (key, expected) in [(b"ClientId".as_slice(), b"fixture-exact-client-id".as_slice()),
        (b"DeviceId", b"fixture-original-device"), (b"__appKey", b"fixture-original-appkey"),
        (b"__domain", b"returned-domain"), (b"__app", b"returned-app"), (b"ForceFlag", b"0"),
        (b"DeviceInfo.CC", if include_cc { b"returned-cc".as_slice() } else { b"".as_slice() })] {
        assert_eq!(fields.iter().find(|p| p.key == key).unwrap().value, expected);
    }
    call.call_id
}
fn login_result() -> Vec<u8> {
    let mut result = Writer::new(); result.boolean(true).unwrap(); result.bytes(b"fixture-user").unwrap();
    result.bytes(b"fixture-auth").unwrap(); result.properties(&[]).unwrap(); result.bytes(b"").unwrap();
    result.finish().expose_to_transport().to_vec()
}

#[tokio::test]
async fn two_original_tls_scopes_join_bootstrap_before_account_login_and_only_real_results_grant_callbacks() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    assert_ne!(source_cert.root, account_cert.root);
    let Fixture { backend, mut events, connector, mut source, mut account, source_dropped, account_dropped } =
        two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, false);
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, true)).await;
        source.drain_until_eof().await;
    });
    let (submitted, seen_submission) = oneshot::channel();
    let (allow_ack, allowed) = oneshot::channel();
    let account_task = tokio::spawn(async move {
        let id = account_login(&mut account, true).await; submitted.send(()).unwrap(); allowed.await.unwrap();
        account.reply(id, &login_result()).await;
        account.swap(Some(STATUS_REGISTER), true).await; account.swap(Some(STATUS_UNREGISTER), true).await;
        let message = next_non_poll(&mut account).await;
        let call = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(call.method, native::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        account.reply(call.call_id, &[0, 1, 0, 0, 0, 0]).await; account.drain_until_eof().await;
    });
    let original = owner(&source_cert.root);
    backend.login(original.clone(), bootstrap_request(&source_cert.root)).await.unwrap(); seen_submission.await.unwrap();
    let attempt = backend.original(&original).unwrap();
    assert!(source_dropped.load(Ordering::SeqCst)); assert!(attempt.bootstrap_handle.try_lock().unwrap().is_none());
    assert!(!lock(&attempt.state).configured); assert!(attempt.scope.is_open());
    assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    {
        let records = lock(&connector.records); assert_eq!(records.len(), 2);
        assert_eq!(records[0].reference, SOURCE_REFERENCE); assert_eq!(records[0].authority, SOURCE_AUTHORITY);
        assert_eq!(records[0].root, source_cert.root); assert!(records[0].sni);
        assert_eq!(records[1].reference.as_bytes(), ACCOUNT_REFERENCE); assert_eq!(records[1].authority, ACCOUNT_AUTHORITY);
        assert_eq!(records[1].root, account_cert.root); assert!(!records[1].sni);
        assert!(records[1].scope.matches(&attempt.scope)); assert!(!records[0].scope.matches(&records[1].scope));
    }
    allow_ack.send(()).unwrap();
    let sdk = callback(&mut events).await; assert!(sdk.matches(&original, &attempt.scope));
    assert!(matches!(sdk.payload, BackendEventPayload::Callback(OwnedRpcCallback { callback: Callback::SdkLogin { success: true }, .. })));
    backend.update_status(original.clone(), STATUS_REGISTER, gateway()).await.unwrap();
    let hosted = callback(&mut events).await;
    assert!(matches!(hosted.payload, BackendEventPayload::Callback(OwnedRpcCallback { callback: Callback::HostingAck { success: true }, .. })));
    assert!(hosted.matches(&original, &attempt.scope));
    backend.update_status(original.clone(), STATUS_UNREGISTER, gateway()).await.unwrap();
    backend.logout(original).await.unwrap(); assert!(account_dropped.load(Ordering::SeqCst));
    assert!(lock(&attempt.state).released); source_task.await.unwrap(); account_task.await.unwrap();
}

#[tokio::test]
async fn absent_bootstrap_cc_clears_stale_client_property_on_actual_account_login() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend, mut events, mut source, mut account, account_dropped, .. } =
        two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, false);
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id, &bootstrap_result(&properties(false), ACCOUNT_REFERENCE, true)).await; source.drain_until_eof().await;
    });
    let account_task = tokio::spawn(async move {
        let id = account_login(&mut account, false).await; account.reply(id, &login_result()).await; account.drain_until_eof().await;
    });
    let original = owner(&source_cert.root); backend.login(original.clone(), bootstrap_request(&source_cert.root)).await.unwrap();
    let sdk = callback(&mut events).await;
    assert!(matches!(sdk.payload, BackendEventPayload::Callback(OwnedRpcCallback { callback: Callback::SdkLogin { success: true }, .. })));
    backend.retire(&original); backend.logout(original).await.unwrap(); assert!(account_dropped.load(Ordering::SeqCst));
    source_task.await.unwrap(); account_task.await.unwrap();
}

#[tokio::test]
async fn unsupported_bootstrap_reference_or_invalid_domain_never_connects_destination_or_emits_sdk_success() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    for invalid_domain in [false, true] {
        let Fixture { backend, mut events, connector, mut source, source_dropped, .. } =
            two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, false);
        let source_task = tokio::spawn(async move {
            let id = bootstrap_call(&mut source).await;
            let mut values = properties(true);
            if invalid_domain { values[0].value = b"ENC(not-a-domain-name)"; }
            let reference = if invalid_domain { ACCOUNT_REFERENCE } else { b"#returned-account:tcp -h wrong.fixture.invalid -p 9443;".as_slice() };
            source.reply(id, &bootstrap_result(&values, reference, true)).await; source.drain_until_eof().await;
        });
        let original = owner(&source_cert.root);
        assert_eq!(backend.login(original.clone(), bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Unavailable));
        let attempt = backend.original(&original).unwrap(); assert!(!attempt.scope.is_open());
        assert!(!lock(&attempt.state).released && !lock(&attempt.state).configured);
        assert!(attempt.bootstrap_handle.try_lock().unwrap().is_some()); assert_eq!(lock(&connector.records).len(), 1);
        assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
        backend.logout(original).await.unwrap(); assert!(source_dropped.load(Ordering::SeqCst)); source_task.await.unwrap();
    }
}

#[tokio::test]
async fn negative_bootstrap_result_is_not_a_login_ack_and_cleanup_keeps_original_join() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend, mut events, connector, mut source, source_dropped, .. } =
        two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, false);
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, false)).await; source.drain_until_eof().await;
    });
    let original = owner(&source_cert.root);
    assert_eq!(backend.login(original.clone(), bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Unavailable));
    let attempt = backend.original(&original).unwrap(); assert!(!lock(&attempt.state).configured);
    assert!(!attempt.scope.is_open()); assert!(!lock(&attempt.state).released);
    assert_eq!(lock(&connector.records).len(), 1); assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    backend.logout(original).await.unwrap(); assert!(source_dropped.load(Ordering::SeqCst)); source_task.await.unwrap();
}

#[tokio::test]
async fn unmatched_bootstrap_call_id_never_hands_off_or_restarts_original_rpc_deadline() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend, mut events, connector, mut source, source_dropped, .. } =
        two_fixture(&source_cert, &account_cert, Duration::from_millis(300), false, false);
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id + 1, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, true)).await; source.drain_until_eof().await;
    });
    let original = owner(&source_cert.root);
    assert_eq!(tokio::time::timeout(Duration::from_secs(1), backend.login(original.clone(), bootstrap_request(&source_cert.root))).await.unwrap(), Err(BackendFailure::Disconnected));
    assert_eq!(lock(&connector.records).len(), 1); assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    backend.logout(original).await.unwrap(); assert!(source_dropped.load(Ordering::SeqCst)); source_task.await.unwrap();
}

#[tokio::test]
async fn cancellation_during_bootstrap_join_retains_source_handle_and_never_admits_destination() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend, events: _events, connector, mut source, source_dropped, .. } =
        two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, false);
    let (reply_sent, observed_reply) = oneshot::channel();
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, true)).await;
        reply_sent.send(()).unwrap(); source.drain_until_eof().await;
    });
    let original = owner(&source_cert.root);
    {
        let login = backend.login(original.clone(), bootstrap_request(&source_cert.root)); tokio::pin!(login);
        tokio::select! { result = login.as_mut() => panic!("bootstrap Join must still be pending: {result:?}"), _ = observed_reply => {} }
        let source_scope = backend.original(&original).unwrap().bootstrap_scope.clone().unwrap();
        std::future::poll_fn(|cx| {
            if let Poll::Ready(result) = login.as_mut().poll(cx) { panic!("original Join not yet run: {result:?}"); }
            if source_scope.is_open() { Poll::Pending } else { Poll::Ready(()) }
        }).await;
        // Current-thread executor: scope was closed by THIS poll, but the
        // owning source actor cannot finish its Join until we yield again.
        assert!(!source_dropped.load(Ordering::SeqCst));
    }
    let attempt = backend.original(&original).unwrap(); assert!(!attempt.scope.is_open());
    assert!(attempt.bootstrap_handle.try_lock().unwrap().is_some()); assert!(attempt.handle.try_lock().unwrap().is_none());
    assert!(!lock(&attempt.state).released); assert_eq!(lock(&connector.records).len(), 1);
    {
        let cleanup = backend.logout(original.clone()); tokio::pin!(cleanup);
        std::future::poll_fn(|cx| { assert!(cleanup.as_mut().poll(cx).is_pending()); Poll::Ready(()) }).await;
    }
    assert!(attempt.bootstrap_handle.try_lock().unwrap().is_some()); assert!(!lock(&attempt.state).released);
    backend.logout(original.clone()).await.unwrap(); assert!(lock(&attempt.state).released);
    assert!(source_dropped.load(Ordering::SeqCst)); assert!(attempt.bootstrap_handle.try_lock().unwrap().is_none());
    assert_eq!(backend.login(original, bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Unavailable));
    source_task.await.unwrap();
}

#[tokio::test]
async fn original_handoff_deadline_spans_bootstrap_release_and_pending_destination_connect() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend, events: _events, connector, mut source, source_dropped, account_dropped, .. } =
        two_fixture(&source_cert, &account_cert, Duration::from_millis(600), true, false);
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await; tokio::time::sleep(Duration::from_millis(300)).await;
        source.reply(id, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, true)).await; source.drain_until_eof().await;
    });
    let original = owner(&source_cert.root);
    assert_eq!(tokio::time::timeout(Duration::from_secs(1), backend.login(original.clone(), bootstrap_request(&source_cert.root))).await.unwrap(), Err(BackendFailure::Disconnected));
    let attempt = backend.original(&original).unwrap(); assert_eq!(lock(&connector.records).len(), 2);
    assert!(!attempt.scope.is_open()); assert!(!lock(&attempt.state).released);
    assert!(source_dropped.load(Ordering::SeqCst)); assert!(account_dropped.load(Ordering::SeqCst));
    backend.logout(original).await.unwrap(); assert!(lock(&attempt.state).released); source_task.await.unwrap();
}

#[tokio::test]
async fn foreign_destination_scope_drops_connected_io_without_login_and_requires_original_cleanup() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend, events: _events, connector, mut source, source_dropped, account_dropped, .. } =
        two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, true);
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, true)).await; source.drain_until_eof().await;
    });
    let original = owner(&source_cert.root);
    assert_eq!(backend.login(original.clone(), bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Disconnected));
    let attempt = backend.original(&original).unwrap(); assert_eq!(lock(&connector.records).len(), 2);
    assert!(source_dropped.load(Ordering::SeqCst) && account_dropped.load(Ordering::SeqCst));
    assert!(!lock(&attempt.state).configured && !lock(&attempt.state).released);
    assert_eq!(backend.login(owner(&source_cert.root), bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Unavailable));
    backend.logout(original).await.unwrap(); assert!(lock(&attempt.state).released); source_task.await.unwrap();
}

#[tokio::test]
async fn cancelled_bootstrap_connect_retires_both_prebound_scopes_and_never_reimports_owner() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let connector = Arc::new(PendingConnector { scope: Mutex::new(None), calls: AtomicUsize::new(0) });
    let (backend, _events) = NativeHostingBackend::new(bootstrap_profile(&account_cert.root, Duration::from_secs(3)), connector.clone());
    let original = owner(&source_cert.root);
    assert!(tokio::time::timeout(Duration::from_millis(30), backend.login(original.clone(), bootstrap_request(&source_cert.root))).await.is_err());
    let attempt = backend.original(&original).unwrap();
    assert!(!attempt.scope.is_open() && !attempt.bootstrap_scope.as_ref().unwrap().is_open());
    assert!(!lock(&attempt.state).released); assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
    backend.logout(original.clone()).await.unwrap(); assert!(lock(&attempt.state).released);
    assert_eq!(backend.login(original, bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Unavailable));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn closed_original_backend_consumer_prevents_bootstrap_connect_and_grants_no_release_until_cleanup() {
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let connector = Arc::new(PendingConnector { scope: Mutex::new(None), calls: AtomicUsize::new(0) });
    let (backend, events) = NativeHostingBackend::new(bootstrap_profile(&account_cert.root, Duration::from_secs(3)), connector.clone());
    drop(events); let original = owner(&source_cert.root);
    assert_eq!(backend.login(original.clone(), bootstrap_request(&source_cert.root)).await, Err(BackendFailure::Disconnected));
    let attempt = backend.original(&original).unwrap(); assert!(!lock(&attempt.state).released);
    assert!(!attempt.scope.is_open() && !attempt.bootstrap_scope.as_ref().unwrap().is_open());
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    backend.logout(original).await.unwrap(); assert!(lock(&attempt.state).released);
}

#[tokio::test]
async fn stored_independent_account_runs_real_bootstrap_handoff_and_preserves_private_configuration_after_join() {
    use native::{account_runtime::AccountManager, account_store, account_test_support::{config_json, QueuedFactory}};
    let source_cert = Certificates::generate("DNS:bootstrap.fixture.invalid");
    let account_cert = Certificates::generate("DNS:account.fixture.invalid");
    let Fixture { backend: unused, events: unused_events, connector, mut source, mut account, source_dropped, account_dropped } =
        two_fixture(&source_cert, &account_cert, Duration::from_secs(3), false, false);
    drop(unused); drop(unused_events);
    let factory = Arc::new(QueuedFactory::default()); factory.add(connector.clone());
    let database = Arc::new(crate::platform::db::Database::new(std::path::PathBuf::from(":memory:")).unwrap());
    let manager = AccountManager::with_factory(database.clone(), factory.clone());
    let mut config = config_json(&source_cert.root);
    config["credentials"]["sdk"]["server_address"] = serde_json::json!(SOURCE_REFERENCE);
    config["profile"]["authority"] = serde_json::json!(SOURCE_AUTHORITY);
    for name in ["domain_name", "domain_id", "application_id", "device_info_cc"] { config["profile"].as_object_mut().unwrap().remove(name); }
    config["profile"]["login_routing"] = serde_json::json!([
        {"key": b"id", "value": b"#exact-imported-login-routing"}, {"key": b"domain", "value": b"exact-imported-routing-domain"}]);
    config["bootstrap"] = serde_json::json!({"account_authority": ACCOUNT_AUTHORITY, "account_root_der_base64": account_cert.root,
        "account_enable_sni": false, "routing": [{"key": b"ServerGroup", "value": b"fixture-original-server-group"}], "handoff_timeout_ms": 3000});
    let imported: native::account_config::AccountConfig = serde_json::from_value(config).unwrap();
    let expected = imported.encode_private().unwrap();
    let line = manager.create(imported, 100).await.unwrap().account.line_id;
    let source_task = tokio::spawn(async move {
        let id = bootstrap_call(&mut source).await;
        source.reply(id, &bootstrap_result(&properties(true), ACCOUNT_REFERENCE, true)).await; source.drain_until_eof().await;
    });
    let account_task = tokio::spawn(async move {
        let id = account_login(&mut account, true).await; account.reply(id, &login_result()).await;
        account.swap(Some(STATUS_REGISTER), true).await; account.swap(Some(STATUS_UNREGISTER), true).await;
        let message = next_non_poll(&mut account).await; let call = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(call.method, native::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        account.reply(call.call_id, &[0, 1, 0, 0, 0, 0]).await; account.drain_until_eof().await;
    });
    let start = manager.start(&line, 1, 100).await.unwrap(); assert!(start.io_cleanup_required);
    let hosted = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let view = manager.get(&line, 100).await.unwrap();
            if view.phase == "hosted" { break view; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    assert!(!hosted.incoming_call_ready && !hosted.incoming_message_ready && !hosted.outbound_authorized);
    assert_eq!(factory.build_count(), 1); assert_eq!(lock(&connector.records).len(), 2);
    assert!(source_dropped.load(Ordering::SeqCst));
    let stopped = manager.stop(&line, 1, 100).await.unwrap(); assert!(!stopped.io_cleanup_required);
    assert!(account_dropped.load(Ordering::SeqCst));
    let stored = database.with_conn(|conn| account_store::load(conn, &line)).unwrap().unwrap();
    assert_eq!(stored.config.encode_private().unwrap().as_slice(), expected.as_slice());
    assert!(stored.config.profile.domain_name.is_none()); assert!(stored.config.bootstrap.is_some());
    source_task.await.unwrap(); account_task.await.unwrap();
}
