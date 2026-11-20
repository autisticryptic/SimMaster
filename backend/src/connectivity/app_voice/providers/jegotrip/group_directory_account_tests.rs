//! SOURCE-only Actions regressions for current AccountManager/Common directory READ.
//! Mounted under im_account_tests::directory; execution only in original Actions.
//! Synthetic local TLS/Common fixtures only; no SDK/device/Provider execution.
use super::*;
use n::{account_runtime::{AccountBackendFactory, DirectoryReadView, PreparedDirectoryRead},
    runtime_backend::{BackendEvent, NativeHostingBackend}, runtime_connector::TcpRuntimeConnector,
    rpc_pending::RpcConnectionScope, rpc_im::ImQuery, session::AttemptOwner,
    rpc_stream::{MAX_MAP_ENTRIES, MAX_STREAM_BYTES}};
use std::{sync::{Arc, Mutex}, time::Instant};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::rustls::ServerConfig;

// Observe the ACTUAL stored-profile backend, not a fabricated controller/run.
// This is the same construction as AccountManager's explicit native factory.
// Capturing its Arc permits existing original-scope budget/poll test hooks;
// it supplies no UID, token, SQL lease, binding, readiness or business permit.
#[derive(Default)]
struct ObservedFactory { builds: Mutex<Vec<Arc<NativeHostingBackend>>> }
impl AccountBackendFactory for ObservedFactory {
    fn build(&self, cfg: &AccountConfig) -> Result<(Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>), &'static str> {
        let connector = TcpRuntimeConnector::new(Duration::from_millis(cfg.profile.connect_timeout_ms))
            .map_err(|_| "app_account_configuration_invalid")?;
        let (backend, events) = NativeHostingBackend::new(cfg.runtime_profile()?, Arc::new(connector));
        self.builds.lock().unwrap().push(backend.clone());
        Ok((backend, events))
    }
}
impl ObservedFactory {
    fn current(&self) -> Arc<NativeHostingBackend> { self.builds.lock().unwrap().last().unwrap().clone() }
    fn count(&self) -> usize { self.builds.lock().unwrap().len() }
}
fn directory_config(root: &str, port: u16, timeout: u64, enabled: bool, info: bool) -> AccountConfig {
    let mut cfg = config(root, port, timeout);
    let im = cfg.profile.native_receivers.as_mut().unwrap().im_messages.as_mut().unwrap();
    im.directory_queries = enabled; im.receive_info = info;
    assert!(cfg.profile.client_ex && cfg.profile.im_sync.is_none());
    assert!(cfg.profile.im_send.is_none() && cfg.profile.im_receipts.is_none());
    assert!(cfg.profile.im_user_read.is_none() && cfg.profile.im_read_status.is_none());
    cfg
}
fn relations_body(accepted: bool, reported: bool, reason: &[u8]) -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(accepted).unwrap(); w.boolean(reported).unwrap();
    for (label, flags, first, second) in [
        (b"opaque-first-label".as_slice(), 0xf123_4567, b"not-a-route\0".as_slice(), b"not-a-uid".as_slice()),
        (b"opaque-second-label".as_slice(), 0x8000_0000, b"Org/1-2".as_slice(), b"tcp -h foreign -p 1".as_slice()),
    ] {
        w.u32(1).unwrap(); w.bytes(label).unwrap(); w.u32(flags).unwrap();
        w.bytes(first).unwrap(); w.bytes(second).unwrap();
        w.properties(&[RpcProperty { key: b"grant", value: b"opaque-not-authority" }]).unwrap();
    }
    w.u32(2).unwrap(); w.bytes(b"opaque-name").unwrap(); w.bytes(b"opaque-other-name").unwrap();
    w.i64(i64::MIN).unwrap(); w.bytes(reason).unwrap();
    w.properties(&[RpcProperty { key: b"token", value: b"opaque-footer-not-token" }]).unwrap();
    w.finish().expose_to_transport().to_vec()
}
fn properties_body(accepted: bool, reason: &[u8]) -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(accepted).unwrap();
    w.properties(&[RpcProperty { key: b"P:P_MR", value: b"9223372036854775807" },
        RpcProperty { key: b"WRITE", value: b"opaque-not-permission" }]).unwrap();
    w.bytes(reason).unwrap(); w.properties(&[RpcProperty { key: b"grant", value: b"opaque-footer" }]).unwrap();
    w.finish().expose_to_transport().to_vec()
}
fn relations_view(view: &DirectoryReadView, target: &str, accepted: bool, reported: bool) {
    assert_eq!(serde_json::to_value(view).unwrap(), serde_json::json!({
        "operation":"relations", "target_kind":target, "accepted":accepted, "reported":reported,
        "original_time":"-9223372036854775808", "first_relations":1, "second_relations":1, "names":2, "properties":0
    }));
    assert_eq!(format!("{view:?}"), "DirectoryReadView(<redacted>)");
}
fn properties_view(view: &DirectoryReadView, accepted: bool) {
    assert_eq!(serde_json::to_value(view).unwrap(), serde_json::json!({
        "operation":"properties", "target_kind":"group", "accepted":accepted,
        "first_relations":0, "second_relations":0, "names":0, "properties":2
    }));
    assert_eq!(format!("{view:?}"), "DirectoryReadView(<redacted>)");
}

// Commands and acknowledgements are paired. A peer write is never called an
// actor ACK. The disabled Info DIRECT response is an ordered receive barrier:
// it consumes no message and follows preceding replies on this original FD.
enum Step {
    Read { group: Option<&'static str>, since: i64, properties: bool, body: Vec<u8>,
        seen: oneshot::Sender<i32>, release: oneshot::Receiver<()> },
    Late { id: i32, seen: oneshot::Sender<()> },
    Barrier { seen: oneshot::Sender<()> },
    Finish { joined: oneshot::Sender<()> },
}
fn observe_id(last: &mut Option<i32>, id: i32) {
    if let Some(previous) = *last { assert_eq!(id, previous + 1, "no invisible business ID allocation or automatic resend"); }
    *last = Some(id);
}
async fn ordinary_poll(peer: &mut CommonPeer, packet: &[u8], last: &mut Option<i32>) {
    assert_eq!(packet[0], 0, "unexpected native reply/business frame");
    let call = decode_request(&packet[1..]).unwrap(); observe_id(last, call.call_id);
    assert_eq!(call.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(), "only original ordinary polls are allowed");
    assert_eq!(call.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
    let mut r = Reader::versioned(call.arguments).unwrap();
    assert_eq!(r.bytes().unwrap(), b"fixture-user"); assert_eq!(r.bytes().unwrap(), b"fixture-original-selector");
    let payload = n::rpc_batch::read_payload(&mut r).unwrap(); r.finish().unwrap();
    assert!(payload.requests.is_empty(), "no nested directory, WRITE, hosting replay or other business request");
    assert!(payload.results.is_empty(), "directory DATA is not a notification/result ACK");
    peer.send(&[reply(call.call_id, &n::runtime_backend::tests::swap_result(None, true, payload.fields))]).await;
}
async fn negative_barrier(peer: &mut CommonPeer, last: &mut Option<i32>) {
    // receive_info is disabled in fixtures using this barrier. This is valid
    // original MoreMessage DATA, not a malformed decoder/transport shortcut.
    peer.send(&[info::info_packet(90, 7, false, info::BODY)]).await;
    loop {
        let p = peer.next().await.unwrap();
        if p[0] == 1 {
            let r = decode_reply(&p[1..]).unwrap(); assert_eq!(r.call_id, 90); assert_eq!(r.result_code, 0);
            assert_eq!(r.result_stream, [0; 6]); return;
        }
        Box::pin(ordinary_poll(peer, &p, last)).await;
    }
}
async fn directory_packet(peer: &mut CommonPeer, group: Option<&str>, since: i64, properties: bool, last: &mut Option<i32>) -> i32 {
    loop {
        let p = peer.next().await.unwrap(); assert_eq!(p[0], 0);
        let call = decode_request(&p[1..]).unwrap();
        if call.method == n::rpc_batch::SWAP_CALLS_METHOD.as_bytes() {
            Box::pin(ordinary_poll(peer, &p, last)).await; continue;
        }
        observe_id(last, call.call_id);
        assert_eq!(call.method, if properties { b"getPropsPrefixs.GroupServer.Group".as_slice() } else { b"getRelations2.GroupServer.Group".as_slice() });
        assert_eq!(call.metadata.len(), 2, "literal original object/token only, no invented routing/domain/magic");
        assert_eq!(call.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"#Group");
        assert_eq!(call.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
        assert_eq!(&call.arguments[..5], &[0, 0, 1, 0, 0], "exact original StreamData and method versions [0]");
        let mut r = Reader::versioned(call.arguments).unwrap();
        assert_eq!(r.bytes().unwrap(), group.unwrap_or("fixture-user").as_bytes(), "matched current login UID, never phone/selector/Org box");
        if properties { assert_eq!(r.u32().unwrap(), 1); assert_eq!(r.bytes().unwrap(), b"", "exact one-entry empty prefix set"); }
        else { assert_eq!(r.i64().unwrap(), since); }
        r.finish().unwrap(); return call.call_id;
    }
}
async fn no_extra_connection(watch: &TcpListener) {
    // Poll once, without introducing a slept read-as-ACK or a new IO deadline.
    tokio::select! { biased;
        incoming = watch.accept() => panic!("unexpected second SDK/Common connection: {}", incoming.is_ok()),
        _ = std::future::ready(()) => {}
    }
}
async fn serve(listener: TcpListener, server: Arc<ServerConfig>, mut steps: mpsc::Receiver<Step>, runs: usize) {
    let original = listener.into_std().unwrap();
    let watch = TcpListener::from_std(original.try_clone().unwrap()).unwrap();
    for _ in 0..runs {
        let listener = TcpListener::from_std(original.try_clone().unwrap()).unwrap();
        let mut peer = Box::pin(CommonPeer::open(listener, server.clone())).await;
        let p = peer.next().await.unwrap(); assert_eq!(p[0], 0);
        let login = decode_request(&p[1..]).unwrap(); assert_eq!(login.method, n::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
        peer.send(&[reply(login.call_id, &login_result())]).await;
        let (_, status) = Box::pin(hosting_call(&mut peer)).await; assert_eq!(status, b"4");
        let mut last = None;
        loop {
            Box::pin(no_extra_connection(&watch)).await;
            match steps.recv().await.expect("paired original peer step") {
                Step::Read { group, since, properties, body, seen, release } => {
                    let id = Box::pin(directory_packet(&mut peer, group, since, properties, &mut last)).await;
                    seen.send(id).unwrap(); release.await.unwrap();
                    // Empty body intentionally withholds a fully submitted READ.
                    if !body.is_empty() { peer.send(&[reply(id, &body)]).await; }
                }
                Step::Late { id, seen } => {
                    peer.send(&[reply(id, &[0xff, 0, 0xff])]).await;
                    Box::pin(negative_barrier(&mut peer, &mut last)).await; seen.send(()).unwrap();
                }
                Step::Barrier { seen } => { Box::pin(negative_barrier(&mut peer, &mut last)).await; seen.send(()).unwrap(); }
                Step::Finish { joined } => {
                    // Only teardown uses the unchanged strict parent helper:
                    // actual status3, logout result, physical EOF, then owning Join.
                    Box::pin(finish_account(&mut peer)).await;
                    Box::pin(no_extra_connection(&watch)).await; joined.send(()).unwrap(); break;
                }
            }
        }
    }
}
struct Fixture {
    temp: TempDatabase, manager: AccountManager, factory: Arc<ObservedFactory>, line: String, revision: u64,
    steps: mpsc::Sender<Step>, serving: tokio::task::JoinHandle<()>, runs: usize,
}
impl Fixture {
    async fn new(enabled: bool, info: bool, timeout: u64, runs: usize) -> Self {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
        let temp = TempDatabase::new(); let factory = Arc::new(ObservedFactory::default());
        let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
        let created = Box::pin(manager.create(directory_config(&cert.root, port, timeout, enabled, info), 100)).await.unwrap();
        assert!(!created.incoming_message_ready && !created.incoming_call_ready && !created.outbound_authorized);
        assert_eq!(created.directory_queries_supported, None, "stored support is not runtime readiness");
        assert_eq!(factory.count(), 0, "configuration CRUD opens no SDK connection");
        let (steps, receiving) = mpsc::channel(8);
        let serving = tokio::spawn(Box::pin(bounded(serve(listener, server, receiving, runs))));
        Self { temp, manager, factory, line: created.account.line_id, revision: created.account.revision, steps, serving, runs }
    }
    async fn start(&self, enabled: bool) {
        bounded(Box::pin(self.manager.start(&self.line, self.revision, 100))).await.unwrap();
        bounded(Box::pin(async {
            loop {
                let view = Box::pin(self.manager.get(&self.line, 100)).await.unwrap();
                if view.phase == "hosted" && view.incoming_message_ready {
                    assert_eq!(view.directory_queries_supported, Some(enabled));
                    assert!(!view.incoming_call_ready && !view.outbound_authorized);
                    assert_eq!(view.im_send_supported, Some(false));
                    assert_eq!(view.im_user_read_supported, Some(false));
                    assert_eq!(view.im_read_status_supported, Some(false));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })).await;
        bounded(Box::pin(self.manager.im_send_actor_barrier_for_test(&self.line, self.revision))).await.unwrap();
        assert_eq!(self.resources(), (0, 0)); self.no_business();
    }
    fn resources(&self) -> (usize, usize) { self.manager.im_send_resources_for_test(&self.line).unwrap() }
    fn prepare(&self, group: Option<&str>, since: i64, properties: bool) -> PreparedDirectoryRead {
        self.manager.prepare_native_directory(&self.line, self.revision, group, since, properties, true).unwrap()
    }
    fn original_query(&self) -> (Arc<NativeHostingBackend>, AttemptOwner, RpcConnectionScope, Arc<ImQuery>) {
        let backend = self.factory.current();
        let (owner, scope, query) = backend.reserve_directory_query(None, -1, false, Instant::now() + Duration::from_secs(10)).unwrap();
        (backend, owner, scope, query)
    }
    fn no_business(&self) {
        let db = &self.temp.database;
        for table in ["app_voice_im_messages", "app_voice_im_receive_cursors", "app_voice_offline_cursors",
            "app_voice_im_read_receipts", "app_voice_im_user_reads", "app_voice_im_read_status_actions", "app_voice_im_outbox"] {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            let rows: i64 = db.with_conn(|c| c.query_row(&sql, [], |r| r.get(0)).map_err(|_| "fixture directory SQL")).unwrap();
            assert_eq!(rows, 0, "directory READ must not create {table}");
        }
        assert!(self.manager.im_messages(&self.line, 128).unwrap().is_empty());
        assert!(self.manager.messages(&self.line, 128).unwrap().is_empty());
        assert!(self.manager.im_send_history(&self.line, 128).unwrap().is_empty());
        assert!(db.get_call_history_for_line(&self.line, 128, 0).unwrap().is_empty());
        assert!(db.get_recent_app_events(Some(&self.line), 128).unwrap().is_empty());
    }
    async fn plan(&self, group: Option<&'static str>, since: i64, properties: bool, body: Vec<u8>) -> (oneshot::Receiver<i32>, oneshot::Sender<()>) {
        let (seen, observed) = oneshot::channel(); let (release, waiting) = oneshot::channel();
        self.steps.send(Step::Read { group, since, properties, body, seen, release: waiting }).await.unwrap();
        (observed, release)
    }
    async fn read(&self, group: Option<&'static str>, since: i64, properties: bool, body: Vec<u8>) -> (i32, DirectoryReadView) {
        let (observed, release) = Box::pin(self.plan(group, since, properties, body)).await;
        let mut reading = Box::pin(self.manager.read_native_directory(&self.line, self.revision, group, since, properties, true));
        let id = tokio::select! {
            result = reading.as_mut() => panic!("READ finished before actual submitted frame: {result:?}"),
            result = bounded(observed) => result.unwrap(),
        };
        release.send(()).unwrap(); let view = bounded(reading).await.unwrap(); self.no_business(); (id, view)
    }
    async fn barrier(&self) {
        let (seen, observed) = oneshot::channel(); self.steps.send(Step::Barrier { seen }).await.unwrap();
        bounded(observed).await.unwrap(); self.no_business();
    }
    async fn stop_run(&self) {
        let (joined, observing) = oneshot::channel(); self.steps.send(Step::Finish { joined }).await.unwrap();
        let stopped = bounded(Box::pin(self.manager.stop(&self.line, self.revision, 100))).await.unwrap();
        assert!(!stopped.io_cleanup_required); bounded(observing).await.unwrap();
        assert!(!self.temp.database.with_conn(|c| n::account_runs::ownership_pending(c, &self.line)).unwrap());
        assert_eq!(Box::pin(self.manager.get(&self.line, 100)).await.unwrap().phase, "stopped");
        self.no_business();
    }
    async fn finish(self) {
        Box::pin(self.stop_run()).await;
        assert_eq!(self.factory.count(), self.runs, "only explicit starts may build a new SDK run");
        assert_eq!(self.resources(), (0, 0)); bounded(self.serving).await.unwrap();
    }
}

#[tokio::test]
async fn directory_account_current_uid_and_explicit_groups_return_only_opaque_counts_without_grants() {
    Box::pin(async {
        let f = Box::pin(Fixture::new(true, false, 10000, 1)).await; Box::pin(f.start(true)).await;
        let (_, view) = Box::pin(f.read(None, i64::MAX, false, relations_body(true, false, b""))).await;
        relations_view(&view, "current_user", true, false);
        let (_, view) = Box::pin(f.read(Some("1-2"), -1, false, relations_body(false, true, b""))).await;
        relations_view(&view, "group", false, true);
        let (_, view) = Box::pin(f.read(Some("1+2"), 0, false, relations_body(true, true, b"opaque-refusal"))).await;
        relations_view(&view, "group", false, true);
        for (group, accepted, reason) in [("1-2", true, b"".as_slice()), ("1+2", true, b"opaque-refusal".as_slice())] {
            let (_, view) = Box::pin(f.read(Some(group), -1, true, properties_body(accepted, reason))).await;
            properties_view(&view, reason.is_empty());
        }
        Box::pin(f.barrier()).await;
        let view = Box::pin(f.manager.get(&f.line, 100)).await.unwrap();
        assert_eq!(view.phase, "hosted"); assert_eq!(view.directory_queries_supported, Some(true));
        assert!(!view.incoming_call_ready && !view.outbound_authorized);
        assert_eq!(f.resources(), (0, 0)); Box::pin(f.finish()).await;
    }).await;
}

#[tokio::test]
async fn directory_account_independent_default_disabled_and_consent_revision_request_guards_are_no_io() {
    Box::pin(async {
        for enabled in [false, true] {
            // Info does not enable directory; directory does not require Info or im_sync.
            let f = Box::pin(Fixture::new(enabled, !enabled, 10000, 1)).await; Box::pin(f.start(enabled)).await;
            let prepare = |revision, group, since, properties, consent| f.manager.prepare_native_directory(&f.line, revision, group, since, properties, consent);
            assert_eq!(prepare(f.revision, None, -1, false, false).unwrap_err(), "app_account_directory_consent_required");
            assert_eq!(prepare(0, None, -1, false, true).unwrap_err(), "app_account_revision_invalid");
            assert_eq!(prepare(f.revision + 1, None, -1, false, true).unwrap_err(), "app_account_revision_conflict");
            for (group, since, properties) in [(None, -2, false), (None, -1, true), (Some("1-2"), 0, true),
                (Some("Org/1-2"), -1, false), (Some("1_2"), -1, false), (Some("fixture-user"), -1, false),
                (Some("01-2"), -1, false), (Some("1-0"), -1, false), (Some("1-2\0"), -1, false)] {
                assert_eq!(prepare(f.revision, group, since, properties, true).unwrap_err(), "app_account_directory_request_invalid");
            }
            let too_long = format!("1-{}", "2".repeat(507));
            assert_eq!(prepare(f.revision, Some(&too_long), -1, false, true).unwrap_err(), "app_account_directory_request_invalid");
            if !enabled {
                for (group, properties) in [(None, false), (Some("1-2"), false), (Some("1+2"), true)] {
                    assert_eq!(prepare(f.revision, group, -1, properties, true).unwrap_err(), "app_account_directory_not_installed");
                    assert_eq!(bounded(Box::pin(f.manager.read_native_directory(&f.line, f.revision, group, -1, properties, true))).await.unwrap_err(), "app_account_directory_not_installed");
                }
                // Info is enabled here: use ordinary polls/strict teardown as the
                // no-IO witness, not the disabled-Info barrier from other cases.
            } else { Box::pin(f.barrier()).await; }
            assert_eq!(f.resources(), (0, 0)); assert_eq!(f.factory.count(), 1); f.no_business();
            Box::pin(f.finish()).await;
        }
    }).await;
}

#[tokio::test]
async fn directory_account_controller_wait_charges_before_await_and_never_renews_preparation_deadline() {
    Box::pin(async {
        let f = Box::pin(Fixture::new(true, false, 100, 1)).await; Box::pin(f.start(true)).await;
        let (held, ready) = oneshot::channel(); let (release, releasing) = oneshot::channel();
        let mut holding = Box::pin(f.manager.im_send_controller_pause_for_test(&f.line, held, releasing));
        tokio::select! { result = holding.as_mut() => panic!("controller prematurely released: {result:?}"), result = bounded(ready) => result.unwrap() }
        let began = Instant::now(); let prepared = f.prepare(Some("1-2"), -1, false);
        assert_eq!(f.resources(), (1, 67), "original group.len()+64 is charged synchronously");
        assert_eq!(format!("{prepared:?}"), "PreparedDirectoryRead(<redacted>)");
        let mut reading = Box::pin(f.manager.finish_native_directory(prepared));
        tokio::select! { biased;
            result = reading.as_mut() => panic!("held controller must await its original deadline: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }
        assert_eq!(f.resources(), (1, 67)); f.no_business();
        assert_eq!(tokio::time::timeout(Duration::from_millis(180), reading).await.unwrap().unwrap_err(), "app_account_directory_timeout");
        assert!(began.elapsed() >= Duration::from_millis(100)); assert_eq!(f.resources(), (0, 0));
        // Even an UNPOLLED finish future cannot start a fresh 100ms window.
        let prepared = f.prepare(None, -1, false); assert_eq!(f.resources(), (1, 64));
        tokio::time::sleep(Duration::from_millis(120)).await; // deadline expiry only, never an IO/ACK witness
        assert_eq!(tokio::time::timeout(Duration::from_millis(40), Box::pin(f.manager.finish_native_directory(prepared))).await.unwrap().unwrap_err(), "app_account_directory_timeout");
        assert_eq!(f.resources(), (0, 0)); f.no_business();
        release.send(()).unwrap(); bounded(holding).await.unwrap();
        Box::pin(f.barrier()).await; Box::pin(f.finish()).await;
    }).await;
}

#[tokio::test]
async fn directory_account_parent_stop_revokes_controller_wait_and_refunds_only_last_original_input_holder() {
    Box::pin(async {
        let f = Box::pin(Fixture::new(true, false, 10000, 1)).await; Box::pin(f.start(true)).await;
        let (_, _, scope, input) = f.original_query(); let last_input = input.clone();
        let (held, ready) = oneshot::channel(); let (release, releasing) = oneshot::channel();
        let mut holding = Box::pin(f.manager.im_send_controller_pause_for_test(&f.line, held, releasing));
        tokio::select! { result = holding.as_mut() => panic!("controller prematurely released: {result:?}"), result = bounded(ready) => result.unwrap() }
        let prepared = f.prepare(Some("1-2"), -1, false); assert_eq!(f.resources(), (2, 131));
        let mut reading = Box::pin(f.manager.finish_native_directory(prepared));
        tokio::select! { biased; result = reading.as_mut() => panic!("controller still held: {result:?}"), _ = tokio::task::yield_now() => {} }
        let (joined, observed_join) = oneshot::channel(); f.steps.send(Step::Finish { joined }).await.unwrap();
        let mut stopping = Box::pin(f.manager.stop(&f.line, f.revision, 100)); let began = Instant::now();
        let error = tokio::select! {
            result = stopping.as_mut() => panic!("stop cannot Join while controller held: {result:?}"),
            result = tokio::time::timeout(Duration::from_millis(200), reading.as_mut()) => result.unwrap().unwrap_err(),
        };
        assert_eq!(error, "app_account_directory_scope_invalid");
        assert!(began.elapsed() < Duration::from_millis(200), "retirement is not the ten-second directory timeout");
        drop(reading); assert_eq!(scope.native_rpc_resources(), (1, 64)); assert!(scope.is_open());
        drop(input); assert_eq!(scope.native_rpc_resources(), (1, 64));
        drop(last_input); assert_eq!(scope.native_rpc_resources(), (0, 0)); f.no_business();
        release.send(()).unwrap(); bounded(holding).await.unwrap();
        assert!(!bounded(stopping).await.unwrap().io_cleanup_required); bounded(observed_join).await.unwrap();
        assert!(!scope.is_open()); assert!(!f.temp.database.with_conn(|c| n::account_runs::ownership_pending(c, &f.line)).unwrap());
        assert_eq!(f.factory.count(), 1); bounded(f.serving).await.unwrap();
    }).await;
}

#[tokio::test]
async fn directory_account_dropped_preparation_and_shared_128_entry_128kib_refusals_preserve_scope_and_ids() {
    Box::pin(async {
        let f = Box::pin(Fixture::new(true, false, 10000, 1)).await; Box::pin(f.start(true)).await;
        let (backend, owner, scope, input) = f.original_query(); drop(input);
        backend.pause_original_polls_for_test(&owner, &scope, true).unwrap();
        bounded(Box::pin(f.manager.im_send_actor_barrier_for_test(&f.line, f.revision))).await.unwrap();
        // This submitted READ establishes a real native ID baseline after all
        // previously admitted poll commands; only scheduling is paused.
        let (baseline, view) = Box::pin(f.read(None, -1, false, relations_body(true, false, b""))).await;
        relations_view(&view, "current_user", true, false);
        let dropped = f.prepare(Some("1+2"), -1, true); assert_eq!(f.resources(), (1, 67));
        drop(dropped); assert_eq!(f.resources(), (0, 0)); assert!(scope.is_open());
        assert_eq!((MAX_MAP_ENTRIES, MAX_STREAM_BYTES), (128, 131072));
        let mut contexts = Vec::new(); for _ in 0..128 { contexts.push(f.prepare(Some("1-2"), -1, false)); }
        assert_eq!(f.resources(), (128, 128 * 67));
        assert_eq!(f.manager.prepare_native_directory(&f.line, f.revision, Some("1+2"), -1, true, true).unwrap_err(), "app_account_directory_capacity");
        assert_eq!(f.resources(), (128, 128 * 67));
        drop(contexts.pop().unwrap()); let replacement = f.prepare(None, i64::MAX, false);
        assert_eq!(f.resources(), (128, 127 * 67 + 64)); drop(replacement); drop(contexts);
        assert_eq!(f.resources(), (0, 0));
        // All native families share this very scope's cap. This existing
        // reservation API fills BYTES without constructing/copying a large
        // caller query or a fake manager scope (groups alone max out at 508).
        let full = scope.reserve_native_rpc(MAX_STREAM_BYTES).unwrap(); let last_full = full.clone();
        assert_eq!(f.resources(), (1, 131072));
        assert_eq!(f.manager.prepare_native_directory(&f.line, f.revision, Some("1-2"), -1, false, true).unwrap_err(), "app_account_directory_capacity");
        assert_eq!(f.resources(), (1, 131072)); drop(full); assert_eq!(f.resources(), (1, 131072));
        drop(last_full); assert_eq!(f.resources(), (0, 0)); assert!(scope.is_open()); f.no_business();
        let (after, view) = Box::pin(f.read(Some("1+2"), -1, true, properties_body(true, b""))).await;
        assert_eq!(after, baseline + 1, "drop/capacity/refund cannot allocate or bypass original IDs"); properties_view(&view, true);
        backend.pause_original_polls_for_test(&owner, &scope, false).unwrap();
        Box::pin(f.barrier()).await; Box::pin(f.finish()).await; assert!(!scope.is_open());
    }).await;
}

#[tokio::test]
async fn directory_account_prepared_requires_exact_sql_lease_live_fence_revision_and_same_manager_run() {
    Box::pin(async {
        for mode in 0..5 {
            let f = Box::pin(Fixture::new(true, false, 10000, 1)).await; Box::pin(f.start(true)).await;
            let prepared = f.prepare(None, -1, false); assert_eq!(f.resources(), (1, 64));
            let (revision, nonce): (i64, String) = f.temp.database.with_conn(|c| c.query_row(
                "SELECT revision,run_nonce FROM app_voice_runtime_leases WHERE line_id=?1", [&f.line], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|_| "fixture owned lease")).unwrap();
            let alien = "00000000000000000000000000000000"; assert_ne!(nonce, alien);
            let expected = match mode {
                0 => {
                    assert_eq!(f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_accounts SET revision=?1 WHERE line_id=?2 AND revision=?3",
                        rusqlite::params![revision + 1, f.line, revision]).map_err(|_| "fixture revision fence")).unwrap(), 1);
                    "app_account_revision_conflict"
                }
                1 | 2 => {
                    let sql = if mode == 1 { "UPDATE app_voice_runtime_leases SET revision=?1 WHERE line_id=?2 AND run_nonce=?3" }
                        else { "UPDATE app_voice_runtime_leases SET run_nonce=?1 WHERE line_id=?2 AND run_nonce=?3" };
                    let changed = if mode == 1 {
                        f.temp.database.with_conn(|c| c.execute(sql, rusqlite::params![revision + 1, f.line, nonce]).map_err(|_| "fixture lease revision")).unwrap()
                    } else {
                        f.temp.database.with_conn(|c| c.execute(sql, rusqlite::params![alien, f.line, nonce]).map_err(|_| "fixture lease nonce")).unwrap()
                    }; assert_eq!(changed, 1); "app_account_runtime_owner_unconfirmed"
                }
                3 => {
                    assert_eq!(f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_inbox_runs SET state='released' WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND state='live'",
                        rusqlite::params![f.line, revision, nonce]).map_err(|_| "fixture live fence")).unwrap(), 1);
                    "app_account_run_scope_invalid"
                }
                _ => "app_account_directory_scope_invalid",
            };
            if mode == 4 {
                let foreign = AccountManager::new(f.temp.database.clone());
                assert_eq!(foreign.prepare_native_directory(&f.line, f.revision, None, -1, false, true).unwrap_err(), "app_account_directory_scope_invalid");
                assert_eq!(bounded(Box::pin(foreign.finish_native_directory(prepared))).await.unwrap_err(), expected);
            } else {
                assert_eq!(bounded(Box::pin(f.manager.finish_native_directory(prepared))).await.unwrap_err(), expected);
                let current_revision = if mode == 0 { f.revision + 1 } else { f.revision };
                assert_eq!(f.manager.prepare_native_directory(&f.line, current_revision, None, -1, false, true).unwrap_err(), expected);
            }
            assert_eq!(f.resources(), (0, 0)); f.no_business();
            // Restore ONLY the exact record this local fixture just damaged,
            // solely for strict original teardown. No query/grant after restore.
            let restored = match mode {
                0 => f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_accounts SET revision=?1 WHERE line_id=?2 AND revision=?3",
                    rusqlite::params![revision, f.line, revision + 1]).map_err(|_| "fixture restore revision")).unwrap(),
                1 => f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_runtime_leases SET revision=?1 WHERE line_id=?2 AND revision=?3 AND run_nonce=?4",
                    rusqlite::params![revision, f.line, revision + 1, nonce]).map_err(|_| "fixture restore lease revision")).unwrap(),
                2 => f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_runtime_leases SET run_nonce=?1 WHERE line_id=?2 AND revision=?3 AND run_nonce=?4",
                    rusqlite::params![nonce, f.line, revision, alien]).map_err(|_| "fixture restore lease nonce")).unwrap(),
                3 => f.temp.database.with_conn(|c| c.execute("UPDATE app_voice_inbox_runs SET state='live' WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND state='released'",
                    rusqlite::params![f.line, revision, nonce]).map_err(|_| "fixture restore live fence")).unwrap(),
                _ => 1,
            }; assert_eq!(restored, 1); Box::pin(f.finish()).await;
        }
    }).await;
}

#[tokio::test]
async fn directory_account_submitted_cancel_and_timeout_ignore_late_malformed_reply_keep_sdk_and_join_without_resend() {
    Box::pin(async {
        for cancel in [true, false] {
            let f = Box::pin(Fixture::new(true, false, if cancel { 10000 } else { 100 }, 1)).await; Box::pin(f.start(true)).await;
            let (seen, release) = Box::pin(f.plan(None, -1, false, Vec::new())).await;
            let began = Instant::now(); let prepared = f.prepare(None, -1, false);
            let mut reading = Box::pin(f.manager.finish_native_directory(prepared));
            let id = tokio::select! {
                result = reading.as_mut() => panic!("not fully submitted: {result:?}"),
                result = bounded(seen) => result.unwrap(),
            }; release.send(()).unwrap();
            assert!(f.resources().0 >= 2, "input and original pending request are both retained");
            if cancel { drop(reading); }
            else {
                assert_eq!(bounded(reading).await.unwrap_err(), "app_account_directory_timeout");
                assert!(began.elapsed() >= Duration::from_millis(100));
            }
            // Existing actor command barrier processes cancellation/retirement;
            // it is not a second request, fresh deadline, or replacement scope.
            bounded(Box::pin(f.manager.im_send_actor_barrier_for_test(&f.line, f.revision))).await.unwrap();
            let (seen, observed) = oneshot::channel(); f.steps.send(Step::Late { id, seen }).await.unwrap();
            bounded(observed).await.unwrap(); assert_eq!(f.resources(), (0, 0)); f.no_business();
            let view = Box::pin(f.manager.get(&f.line, 100)).await.unwrap();
            assert_eq!(view.phase, "hosted"); assert!(view.incoming_message_ready);
            assert!(!view.incoming_call_ready && !view.outbound_authorized); assert_eq!(f.factory.count(), 1);
            Box::pin(f.finish()).await;
        }
    }).await;
}

#[tokio::test]
async fn directory_account_explicit_restart_rejects_old_prepared_snapshot_scope_context_and_joins_each_sdk_run() {
    Box::pin(async {
        let f = Box::pin(Fixture::new(true, false, 10000, 2)).await; Box::pin(f.start(true)).await;
        let (_, old_owner, old_scope, old_query) = f.original_query();
        let stopped_context = f.prepare(None, -1, false); let restarted_context = f.prepare(Some("1-2"), -1, true);
        assert_eq!(old_scope.native_rpc_resources(), (3, 195));
        Box::pin(f.stop_run()).await;
        assert!(!old_scope.is_open()); assert_eq!(old_scope.native_rpc_resources(), (3, 195), "Join/SQL compaction do not reset retained input charges");
        assert_eq!(bounded(Box::pin(f.manager.finish_native_directory(stopped_context))).await.unwrap_err(), "app_account_runtime_owner_unconfirmed");
        assert_eq!(old_scope.native_rpc_resources(), (2, 131));
        Box::pin(f.start(true)).await; assert_eq!(f.factory.count(), 2);
        let (_, current_owner, current_scope, current_query) = f.original_query();
        assert_ne!(old_owner, current_owner); assert!(!old_scope.matches(&current_scope));
        assert!(!Arc::ptr_eq(&old_query, &current_query)); drop(current_query);
        assert_eq!(bounded(Box::pin(f.manager.finish_native_directory(restarted_context))).await.unwrap_err(), "app_account_directory_scope_invalid");
        assert_eq!(old_scope.native_rpc_resources(), (1, 64)); assert_eq!(f.resources(), (0, 0));
        drop(old_query); assert_eq!(old_scope.native_rpc_resources(), (0, 0));
        Box::pin(f.barrier()).await; // old contexts must not replay on the new FD
        let (_, view) = Box::pin(f.read(Some("1+2"), 0, false, relations_body(true, false, b""))).await;
        relations_view(&view, "group", true, false); assert_eq!(f.resources(), (0, 0));
        Box::pin(f.finish()).await; assert!(!current_scope.is_open());
    }).await;
}
