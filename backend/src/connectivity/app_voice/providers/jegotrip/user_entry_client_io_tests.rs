//! Actions-only regression SOURCE; intentionally not compiled/executed locally.
//! Mount beside user_entry_common_tests as runtime_backend::common_tests::client_user_entry.
//! Real ORIGINAL bootstrap + SDK Router/TLS/Common peers, exactly TWO physical
//! connections. No auxiliary resolver, direct main aidToUid RPC, real service,
//! native SDK/device, WRITE permission, fabricated callback or Unknown ACK.
//! Keep the existing standard-stack fixtures and deadlines; boxing only owns
//! the original futures. Certificates::generate is SOURCE for Actions only.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{im_inbox::ImReceiptGate, user_entry_action::{UserEntryAction, UserEntryObservation}};
use tokio::sync::oneshot;

const FACTORY: &[u8] = b"  prefix/UserEntry  ";
const RECIPIENT: &str = "[phone:+12345@account-domain.invalid]";
const READ_DEADLINE: Duration = Duration::from_millis(100);
const JOIN_BOUND: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Reported, Negative, Unreported, ForeignAid, Ambiguous, VersionError,
    BadFooter, FullTimeout, ConsumerDropMixed, StopPending, UidLocal,
    AccountRead, PreparedDeadline, AccountSqlFence, ControllerDeadline,
}
impl Case {
    fn managed(self) -> bool {
        matches!(self, Self::AccountRead | Self::PreparedDeadline | Self::AccountSqlFence | Self::ControllerDeadline)
    }
    fn expects_read(self) -> bool {
        !matches!(self, Self::UidLocal | Self::PreparedDeadline | Self::ControllerDeadline)
    }
}

fn bootstrap_client() -> Vec<u8> {
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    w.properties(&[
        RpcProperty { key: b"DomainName", value: b"account-domain.invalid" },
        RpcProperty { key: b"DomainId", value: b"fixture-domain" },
        RpcProperty { key: b"AppId", value: b"fixture-app" },
        RpcProperty { key: b"CC", value: b"fixture-cc" },
    ]).unwrap();
    w.bytes(DESTINATION.as_bytes()).unwrap();
    w.bytes(b"").unwrap(); // AccountAlive stays absent.
    w.bytes(FACTORY).unwrap(); // Original auxiliary STRING, NOT a property/id guess.
    w.bytes(b"").unwrap();
    w.finish().expose_to_transport().to_vec()
}

/// Strict original bool + map<UID, pair<AID-kind, AID-value>> + whole footer.
/// In particular, a negative reply still carries plausible DATA, not a grant.
fn answer(case: Case) -> Vec<u8> {
    let mut w = Writer::new();
    w.boolean(case != Case::Negative).unwrap();
    let count = if case == Case::Unreported { 0 } else if case == Case::Ambiguous { 2 } else { 1 };
    w.u32(count).unwrap();
    if count != 0 {
        w.bytes(b"7_9").unwrap();
        w.bytes(b"phone").unwrap();
        w.bytes(if case == Case::ForeignAid { b"+67890" } else { b"+12345" }).unwrap();
        if count == 2 {
            w.bytes(b"8_9").unwrap();
            w.bytes(b"phone").unwrap();
            w.bytes(b"+12345").unwrap();
        }
    }
    w.bytes(b"").unwrap();
    let mut bytes = w.finish().expose_to_transport().to_vec();
    if case == Case::BadFooter {
        // A truncated optional footer property map invalidates the WHOLE result.
        // Do not corrupt the outer envelope/Common group: the SDK stays healthy.
        bytes.push(0xff);
    }
    bytes
}

fn swap_answer(fields: [u32; 6], results: &[(i32, u32, &[u8])], ack: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    let words = if ack { [9, 1, 1, fields[0], fields[1], fields[2]] } else { [9, 1, 1, 0, 0, 0] };
    for word in words { w.u32(word).unwrap(); }
    w.u32(0).unwrap(); // No invented incoming request/consumer completion.
    w.u32(results.len() as u32).unwrap();
    for &(sequence, code, bytes) in results {
        w.i32(sequence).unwrap(); w.u32(code).unwrap(); w.bytes(bytes).unwrap();
    }
    w.properties(&[]).unwrap(); w.bytes(b"").unwrap();
    w.finish().expose_to_transport().to_vec()
}

fn stored_config(root: &str, port: u16, case: Case) -> n::account_config::AccountConfig {
    let mut config = bootstrap_account_config(root, port);
    // Only shorten the existing advertised RPC budget to the old 100-ms test
    // deadline. No handshake, startup, IO, stack or aggregate limit is enlarged.
    if matches!(case,Case::PreparedDeadline|Case::ControllerDeadline) { config.profile.rpc_timeout_ms = 100; }
    config
}

/// Expected selector/password/application/device are captured from the actual
/// fixture SDK request, not guessed from the returned token or user identity.
fn expected_login(root: &str, port: u16, case: Case) -> (LoginCapture, Vec<u8>) {
    let captured = if case.managed() {
        let config = stored_config(root, port, case);
        let profile = config.runtime_profile().unwrap();
        let mut session = n::session::HostingSession::new("fixture-client-wire-expectation".into());
        session.import(config.credentials, 100).unwrap();
        let owner = session.begin_login(100).unwrap();
        LoginCapture::capture(session.login_request(&owner).unwrap(), &profile).unwrap()
    } else {
        let profile = RuntimeProfile::supplied_common_bootstrap(
            bootstrap_inputs(), bootstrap_common(root, port), bootstrap_route()).unwrap();
        LoginCapture::capture(SdkLoginRequest { server_address: BOOTSTRAP_SDK, ..request(root) }, &profile).unwrap()
    };
    let uri = session_uri(&captured, b"account-domain.invalid").unwrap().as_bytes().to_vec();
    (captured, uri)
}

async fn sdk_peer(
    listener: TcpListener, config: Arc<tokio_rustls::rustls::ServerConfig>, case: Case,
    captured: LoginCapture, uri: Vec<u8>, seen: oneshot::Sender<(i32, i32)>,
    resume: oneshot::Receiver<()>, reads: Arc<AtomicUsize>,
) {
    let mut peer = CommonPeer::open(listener, config).await;
    let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0);
    let call = decode_request(&packet[1..]).unwrap();
    assert_eq!(call.method, b"loginSession.AccountEntry.Account");
    assert_eq!(call.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"DestEntry");
    assert_eq!(call.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, captured.password.as_bytes());
    assert!(!call.metadata.iter().any(|p| p.key == b"token"));
    let mut args = Reader::versioned(call.arguments).unwrap();
    assert_eq!(args.bytes().unwrap(), uri);
    assert_eq!(args.bytes().unwrap(), captured.selector.as_bytes());
    assert_ne!(captured.selector.as_bytes(), b"fixture-auth");
    let properties = args.properties().unwrap(); args.finish().unwrap();
    for (key, value) in [
        (b"__domain".as_slice(), b"fixture-domain".as_slice()),
        (b"__app", b"fixture-app"), (b"DeviceInfo.CC", b"fixture-cc"),
        (b"__appKey", captured.appkey.as_bytes()),
    ] {
        assert_eq!(properties.iter().find(|p| p.key == key).unwrap().value, value);
    }
    if let Some(device) = &captured.device {
        assert_eq!(properties.iter().find(|p| p.key == b"DeviceId").unwrap().value, device.as_bytes());
    }
    peer.send(&[reply(call.call_id, &if case.managed() { login_result_for_account() } else { login_result() })]).await;

    let mut registered = false;
    let mut unregistered = false;
    let mut read_seen = false;
    let mut seen = Some(seen);
    let mut resume = Some(resume);
    loop {
        let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0);
        let call = decode_request(&packet[1..]).unwrap();
        // Every authenticated OUTER request uses the original SDK session.
        assert_eq!(call.metadata.len(), 2, "original account id and token ONLY");
        assert_eq!(call.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"DestEntry");
        assert_eq!(call.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
        assert!(!call.metadata.iter().any(|p| p.key == b"pwd"));
        let mut args = Reader::versioned(call.arguments).unwrap();
        assert_eq!(args.bytes().unwrap(), b"fixture-user"); // Actual login reply UID, including managed case.
        assert_eq!(args.bytes().unwrap(), captured.selector.as_bytes());
        if call.method == b"logoutSession.AccountEntry.Account" {
            args.finish().unwrap();
            assert!(registered && unregistered, "logout must follow original hosting 4 then 3");
            assert_eq!(read_seen, case.expects_read());
            peer.send(&[reply(call.call_id, b"\0\x01\0\0\0\0")]).await;
            peer.no_more_common().await;
            return;
        }
        // Direct aidToUid on main is deliberately NOT accepted by this fixture.
        assert_eq!(call.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let payload = n::rpc_batch::read_payload(&mut args).unwrap(); args.finish().unwrap();
        assert!(payload.results.is_empty(), "READ must not synthesize consumer/WRITE results");
        let already_unregistered = unregistered;
        let mut read_sequence = None;
        let mut status_sequence = None;
        let mut retained_register = false;
        for (&sequence, nested) in &payload.requests {
            if nested.method == n::user_entry_data::AID_TO_UID_METHOD.as_bytes() {
                assert!(case.expects_read());
                assert!(!read_seen, "retired/completed READ was replayed in a later epoch");
                assert!(registered && !unregistered);
                assert_eq!(sequence, 2, "Client READ must share hosting's original nested tracker");
                assert_eq!(nested.metadata.len(), 2, "inner ONLY original Agent CC and whole factory id");
                assert_eq!(nested.metadata.iter().find(|p| p.key == b"DeviceInfo.CC").unwrap().value, b"fixture-cc");
                let id = nested.metadata.iter().find(|p| p.key == b"id").unwrap().value;
                assert_eq!(id, FACTORY); assert_ne!(id, b"prefix/UserEntry");
                assert!(!nested.metadata.iter().any(|p| matches!(p.key, b"token" | b"pwd" | b"accountid")));
                let mut fields = Reader::versioned(nested.arguments).unwrap();
                assert_eq!(fields.u32().unwrap(), 1);
                assert_eq!(fields.bytes().unwrap(), b"phone");
                assert_eq!(fields.bytes().unwrap(), b"+12345"); fields.finish().unwrap();
                assert!(read_sequence.replace(sequence).is_none());
            } else {
                assert_eq!(nested.method, n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());
                assert_eq!(nested.metadata.iter().find(|p| p.key == b"id").unwrap().value,
                    n::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes());
                let mut fields = Reader::versioned(nested.arguments).unwrap();
                assert_eq!(fields.bytes().unwrap(), uri);
                let properties = fields.properties().unwrap(); fields.finish().unwrap();
                let flag = properties.iter().find(|p| p.key == b"SipFlag").unwrap().value;
                assert_eq!(properties.iter().find(|p| p.key == b"SipPwd").unwrap().value, b"fixture-SipPwd");
                if flag == b"4" {
                    assert_eq!(sequence, 1);
                    if registered {
                        // Deliberately retained ORIGINAL WRITE epoch, not a new registration.
                        assert_eq!(case, Case::ConsumerDropMixed); retained_register = true;
                    } else { registered = true; status_sequence = Some(sequence); }
                } else {
                    assert_eq!(flag, b"3"); assert!(registered);
                    // Original unregister may remain retained until its actual
                    // ACK is processed; do not mistake that WRITE for READ replay.
                    assert_eq!(sequence, if case.expects_read() { 3 } else { 2 },
                        "UID/prepared-expired branch must not consume a nested Query ID");
                    unregistered = true; status_sequence = Some(sequence);
                }
            }
        }
        if let Some(sequence) = read_sequence {
            reads.fetch_add(1, Ordering::SeqCst); read_seen = true;
            match case {
                Case::FullTimeout | Case::StopPending => {
                    // NO outer ACK, nested answer, replay or fake timeout callback.
                }
                Case::ConsumerDropMixed => {
                    assert!(retained_register, "test must exercise a genuinely mixed READ/WRITE frame");
                    // Keep the WRITE epoch unACKed while completing this outer
                    // RPC only. Consumer retirement must purge READ retry copies.
                    peer.send(&[reply(call.call_id, &swap_answer(payload.fields, &[], false))]).await;
                }
                Case::AccountSqlFence => {
                    seen.take().unwrap().send((sequence, call.call_id)).unwrap();
                    resume.take().unwrap().await.unwrap();
                    let bytes = answer(Case::Reported);
                    peer.send(&[reply(call.call_id, &swap_answer(payload.fields, &[(sequence, 0, &bytes)], true))]).await;
                    continue;
                }
                _ => {
                    let (code, bytes) = if case == Case::VersionError { (0x10000, vec![0xff]) } else { (0, answer(case)) };
                    peer.send(&[reply(call.call_id, &swap_answer(payload.fields, &[(sequence, code, &bytes)], true))]).await;
                }
            }
            seen.take().unwrap().send((sequence, call.call_id)).unwrap();
        } else {
            if unregistered && !already_unregistered && case == Case::ConsumerDropMixed {
                assert!(retained_register, "READ retirement must not erase retained registration WRITE");
                assert_eq!(payload.requests.len(), 2, "original registration and unregister ONLY");
            }
            let results = status_sequence.map(|s| vec![(s, 0, b"\0\x01\0\0\0\0".as_slice())]).unwrap_or_default();
            let ack = case != Case::ConsumerDropMixed || unregistered;
            peer.send(&[reply(call.call_id, &swap_answer(payload.fields, &results, ack))]).await;
        }
    }
}

struct WireFixture {
    root: String, port: u16, seen: oneshot::Receiver<(i32, i32)>,
    resume: oneshot::Sender<()>, done: oneshot::Sender<()>,
    connections: Arc<AtomicUsize>, reads: Arc<AtomicUsize>,
    source: tokio::task::JoinHandle<()>, sdk: tokio::task::JoinHandle<()>,
    relay: tokio::task::JoinHandle<()>,
}
impl WireFixture {
    async fn open(case: Case) -> Self {
        let cert = Certificates::generate("IP:127.0.0.1");
        let front = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = front.local_addr().unwrap().port();
        let boot = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let main = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let ports = [boot.local_addr().unwrap().port(), main.local_addr().unwrap().port()];
        let config = cert.server_config(&TLS13); let source_config = config.clone();
        let (seen, submitted) = oneshot::channel(); let (resume, resumed) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        let connections = Arc::new(AtomicUsize::new(0)); let counted = connections.clone();
        let reads = Arc::new(AtomicUsize::new(0));
        let relay = tokio::spawn(async move {
            let mut joins = Vec::new();
            for target in ports {
                let (mut inbound, _) = front.accept().await.unwrap();
                counted.fetch_add(1, Ordering::SeqCst);
                let mut outbound = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, target)).await.unwrap();
                joins.push(tokio::spawn(async move {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }));
            }
            // Keep rejecting a third physical dial THROUGH the READ and logout,
            // not just sampling a connection counter before delayed work runs.
            tokio::select! {
                result = front.accept() => panic!("Client/UID READ attempted an auxiliary connection: {}", result.is_ok()),
                result = finished => result.unwrap(),
            }
            assert!(tokio::time::timeout(Duration::from_millis(35), front.accept()).await.is_err());
            for join in joins { join.await.unwrap(); }
        });
        let source = tokio::spawn(async move {
            let mut peer = CommonPeer::open(boot, source_config).await;
            let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0);
            let call = decode_request(&packet[1..]).unwrap();
            assert_eq!(call.method, b"getAppConfigs.AccountEntry.Account");
            assert_eq!(call.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"BootEntry");
            peer.send(&[reply(call.call_id, &bootstrap_client())]).await;
            peer.no_more_common().await;
        });
        let (captured, uri) = expected_login(&cert.root, port, case);
        let sdk = tokio::spawn(Box::pin(sdk_peer(main, config, case, captured, uri, seen, resumed, reads.clone())));
        Self { root: cert.root.clone(), port, seen: submitted, resume, done, connections, reads, source, sdk, relay }
    }
    async fn finish(self, expected_reads: usize) {
        self.done.send(()).unwrap();
        tokio::time::timeout(JOIN_BOUND, async {
            self.source.await.unwrap(); self.sdk.await.unwrap(); self.relay.await.unwrap();
        }).await.unwrap();
        assert_eq!(self.connections.load(Ordering::SeqCst), 2);
        assert_eq!(self.reads.load(Ordering::SeqCst), expected_reads);
    }
}

fn observation(view: &UserEntryObservation, kind: &str, report: &str) {
    assert_eq!(view.kind, kind); assert_eq!(view.report, report);
    assert!(chrono::DateTime::parse_from_rfc3339(&view.observed_at).is_ok());
    let json = serde_json::to_value(view).unwrap();
    let object = json.as_object().unwrap(); assert_eq!(object.len(), 3);
    for field in ["kind", "report", "observed_at"] { assert!(object.contains_key(field)); }
    let body = json.to_string();
    for data in ["12345", "7_9", "fixture", "UserEntry", "prefix/", "auth", "token", "password", "selected_session"] {
        assert!(!body.contains(data), "public observation leaked private resolver DATA");
    }
    assert_eq!(format!("{view:?}"), "UserEntryObservation(<redacted>)");
}

async fn wait_callback(events: &mut mpsc::Receiver<BackendEvent>, owner: &AttemptOwner,
    scope: &RpcConnectionScope, expected: Callback) {
    let event = tokio::time::timeout(JOIN_BOUND, events.recv()).await.unwrap().unwrap();
    assert!(event.matches(owner, scope));
    assert!(matches!(event.payload, BackendEventPayload::Callback(c) if c.callback == expected));
}

async fn native(case: Case) {
    let mut wire = WireFixture::open(case).await;
    let profile = RuntimeProfile::supplied_common_bootstrap(
        bootstrap_inputs(), bootstrap_common(&wire.root, wire.port), bootstrap_route()).unwrap();
    let connector = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) });
    let (backend, mut events) = NativeHostingBackend::new(profile, connector.clone());
    let owner = owner();
    backend.login(owner.clone(), SdkLoginRequest { server_address: BOOTSTRAP_SDK, ..request(&wire.root) }).await.unwrap();
    let parent = backend.original_scope(&owner).unwrap();
    backend.pause_original_polls_for_test(&owner, &parent, true).unwrap();
    wait_callback(&mut events, &owner, &parent, Callback::SdkLogin { success: true }).await;
    backend.update_status(owner.clone(), STATUS_REGISTER, gateway()).await.unwrap();
    // Result, not submission/outer ACK, establishes hosting. Also ensures the
    // timeout case starts with a READ-only frame and no pending hosting WRITE.
    wait_callback(&mut events, &owner, &parent, Callback::HostingAck { success: true }).await;
    let attempt = backend.original(&owner).unwrap();
    assert!(attempt.bootstrap_handle.lock().await.is_none());
    let source = attempt.bootstrap_scope.as_ref().unwrap(); assert!(!source.is_open());
    let target = attempt.user_entries.client_target().unwrap().unwrap();
    let router = &backend.profile.common.as_ref().unwrap().router;
    assert_eq!(target.factory_input_for(&owner, &parent, router), Some(FACTORY));
    assert!(target.original_bootstrap_id() > 0);
    assert_eq!(format!("{target:?}"), "NewClientTarget(<redacted>)");
    assert!(lock(&attempt.user_entry).is_err(), "endpoint-bearing resolver must not be selected");
    assert!(backend.user_entry_supported(&owner));
    let ready = (backend.original_message_receiver_ready(&owner, &parent), backend.original_call_receiver_ready(&owner, &parent));
    let resources = parent.native_rpc_resources();
    assert_eq!(attempt.user_entries.active(), 0); assert_eq!(attempt.user_entries.retained_bytes(), 0);
    let mut stopped = false;
    if case == Case::UidLocal {
        for recipient in [b"0_1".as_slice(), b"[uid:7_9]", b"[uid:7_9@foreign.invalid]"] {
            let action = UserEntryAction::original(owner.clone(), parent.clone(), ImReceiptGate::fresh(),
                Instant::now() + Duration::from_secs(3), recipient).unwrap();
            let view = backend.resolve_user_entry(action).await.unwrap();
            observation(&view, "uid", "local_uid");
            assert_eq!(wire.reads.load(Ordering::SeqCst), 0);
        }
        // Accounting probe ONLY, after the local work; never dial this test lease.
        // The first auxiliary ID is still 1 and peer unhosting must be sequence 2.
        let lease = attempt.user_entries.acquire(&owner, &parent, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(lease.id(), 1); drop(lease);
    } else {
        let deadline = Instant::now() + if case == Case::FullTimeout { READ_DEADLINE } else { Duration::from_secs(3) };
        let action = UserEntryAction::original(owner.clone(), parent.clone(), ImReceiptGate::fresh(),
            deadline, RECIPIENT.as_bytes()).unwrap();
        let mut lookup = Box::pin(backend.resolve_user_entry(action.clone()));
        if matches!(case, Case::ConsumerDropMixed | Case::StopPending) {
            tokio::select! {
                result = &mut lookup => panic!("outer ACK/submission cannot finish READ: {result:?}"),
                result = &mut wire.seen => { assert_eq!(result.unwrap().0, 2); }
            }
            assert_eq!(attempt.user_entries.active(), 0, "Client READ must not acquire AuxBook lease");
            assert_eq!(attempt.user_entries.retained_bytes(), 0);
            let held = parent.native_rpc_resources();
            assert!(held.0 > resources.0 && held.1 > resources.1,
                "pending Client READ must charge the ORIGINAL SDK shared budget");
            assert!(held.1 <= 128 * 1024, "no expansion of original aggregate byte bound");
            if case == Case::ConsumerDropMixed {
                drop(lookup);
                // Interest retirement is not action/account consent retirement.
                assert!(action.live()); assert!(action.gate().live()); assert!(parent.is_open());
            } else {
                let stopping = backend.update_status(owner.clone(), STATUS_UNREGISTER, gateway());
                let (result, stop) = tokio::join!(lookup, stopping);
                assert_eq!(result.err(), Some("app_account_user_entry_scope_invalid")); stop.unwrap();
                stopped = true;
                assert!(action.live(), "stop retires READ admission independently of its action");
                assert_eq!(backend.resolve_user_entry(action.clone()).await.err(), Some("app_account_user_entry_scope_invalid"));
            }
        } else {
            let result = lookup.await;
            match case {
                Case::Reported => observation(&result.unwrap(), "phone", "reported"),
                Case::Negative | Case::VersionError => observation(&result.unwrap(), "phone", "rejected"),
                Case::Unreported => observation(&result.unwrap(), "phone", "unreported"),
                Case::ForeignAid | Case::BadFooter => assert_eq!(result.err(), Some("app_account_user_entry_reply_invalid")),
                Case::Ambiguous => assert_eq!(result.err(), Some("app_account_user_entry_ambiguous")),
                Case::FullTimeout => {
                    assert_eq!(result.err(), Some("app_account_user_entry_timeout"));
                    assert!(Instant::now() >= deadline);
                    assert!(Instant::now().duration_since(deadline) < Duration::from_secs(2));
                    assert_eq!(tokio::time::timeout(JOIN_BOUND, &mut wire.seen).await.unwrap().unwrap().0,
                        2, "timeout must follow actual fully submitted READ");
                }
                _ => unreachable!(),
            }
        }
    }
    assert!(parent.is_open(), "READ error/drop/timeout must preserve healthy original SDK");
    { let state = lock(&attempt.state); assert!(state.configured && !state.retired); }
    assert_eq!(ready, (backend.original_message_receiver_ready(&owner, &parent), backend.original_call_receiver_ready(&owner, &parent)));
    assert_eq!(attempt.user_entries.active(), 0); assert_eq!(attempt.user_entries.retained_bytes(), 0);
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    if !stopped { backend.update_status(owner.clone(), STATUS_UNREGISTER, gateway()).await.unwrap(); }
    // The peer checks no retired READ in this next epoch, and for mixed work
    // proves the original unACKed registration survives alongside hosting 3.
    tokio::time::timeout(JOIN_BOUND, async {
        loop { if parent.native_rpc_resources() == resources { break; } tokio::task::yield_now().await; }
    }).await.unwrap();
    backend.logout(owner.clone()).await.unwrap();
    assert!(!parent.is_open()); assert!(lock(&attempt.state).released);
    assert!(attempt.handle.lock().await.is_none()); assert!(attempt.bootstrap_handle.lock().await.is_none());
    assert!(attempt.alive_handle.lock().await.is_none());
    assert_eq!(parent.native_rpc_resources(), resources);
    wire.finish(usize::from(case.expects_read())).await;
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
}

fn sql_counts(database: &crate::platform::db::Database) -> Vec<i64> {
    database.with_conn(|conn| {
        let mut counts = Vec::new();
        for table in ["sms_messages", "app_voice_inbox", "app_voice_im_messages", "app_voice_im_user_reads",
            "app_voice_im_read_receipts", "app_voice_im_read_status_actions", "app_voice_im_outbox"] {
            counts.push(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).map_err(|_| "fixture-count")?);
        }
        Ok(counts)
    }).unwrap()
}
fn lease_revision(database: &crate::platform::db::Database, line: &str, from: i64, to: i64) {
    // Fault injection into the SYNTHETIC local SQL lease only. Never inspect or
    // export a run nonce; restore the exact revision before ordinary cleanup.
    database.with_conn(|conn| {
        let changed = conn.execute("UPDATE app_voice_runtime_leases SET revision=?3 WHERE line_id=?1 AND revision=?2",
            rusqlite::params![line, from, to]).map_err(|_| "fixture-lease")?;
        assert_eq!(changed, 1); Ok(())
    }).unwrap();
}

async fn managed(case: Case) {
    let mut wire = WireFixture::open(case).await;
    let temp = n::account_test_support::TempDatabase::new();
    let factory = Arc::new(n::account_test_support::QueuedFactory::default());
    let connector = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) }); factory.add(connector.clone());
    let manager = Arc::new(n::account_runtime::AccountManager::with_factory(temp.database.clone(), factory.clone()));
    let config = stored_config(&wire.root, wire.port, case);
    let original_timeout = config.runtime_profile().unwrap().rpc_timeout;
    let created = manager.create(config, 100).await.unwrap(); let line = created.account.line_id.clone();
    manager.start(&line, 1, 100).await.unwrap();
    let before = tokio::time::timeout(Duration::from_secs(10), async {
        loop { let view = manager.get(&line, 100).await.unwrap();
            if view.phase == "hosted" { break view; } tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert_eq!(before.user_entry_supported, Some(true));
    let counts = sql_counts(&temp.database);
    let mut stale_after_stop = None;
    match case {
        Case::AccountRead => {
            assert_eq!(manager.prepare_user_entry(&line, 1, RECIPIENT, false).err(), Some("app_account_user_entry_consent_required"));
            assert_eq!(manager.prepare_user_entry(&line, 2, RECIPIENT, true).err(), Some("app_account_revision_conflict"));
            assert_eq!(manager.prepare_user_entry(&line, 1, "not-a-recipient", true).err(), Some("app_account_user_entry_recipient_invalid"));
            assert_eq!(manager.lookup_user_entry(&line, 1, "[phone:+12345@foreign.invalid]", true).await.err(), Some("app_account_user_entry_realm_mismatch"));
            assert_eq!(wire.reads.load(Ordering::SeqCst), 0, "guards must run before network admission");
            let prepared = manager.prepare_user_entry(&line, 1, RECIPIENT, true).unwrap();
            assert_eq!(format!("{prepared:?}"), "PreparedUserEntryLookup(<redacted>)");
            // Age the original preparation without private controller/run access.
            tokio::time::sleep(Duration::from_millis(20)).await;
            let view = manager.finish_user_entry(prepared).await.unwrap(); observation(&view, "phone", "reported");
        }
        Case::PreparedDeadline => {
            assert_eq!(original_timeout, READ_DEADLINE);
            let prepared = manager.prepare_user_entry(&line, 1, RECIPIENT, true).unwrap();
            let prepared_at = Instant::now();
            assert_eq!(wire.reads.load(Ordering::SeqCst), 0);
            tokio::time::sleep(original_timeout + Duration::from_millis(20)).await;
            assert_eq!(manager.finish_user_entry(prepared).await.err(), Some("app_account_user_entry_timeout"));
            assert!(prepared_at.elapsed() < Duration::from_secs(2));
            assert_eq!(wire.reads.load(Ordering::SeqCst), 0, "finish must not renew preparation's absolute deadline");
        }
        Case::ControllerDeadline=>{
            assert_eq!(original_timeout,READ_DEADLINE);
            let(entered,ready)=oneshot::channel();let(release,released)=oneshot::channel();let blocked=manager.clone();let fixed=line.clone();
            let holder=tokio::spawn(async move{blocked.test_user_entry_controller_barrier(&fixed,entered,released).await;});
            ready.await.unwrap();let elapsed=Instant::now();
            assert_eq!(tokio::time::timeout(Duration::from_secs(2),manager.lookup_user_entry(&line,1,RECIPIENT,true)).await.unwrap().err(),Some("app_account_user_entry_timeout"));
            assert!(elapsed.elapsed()>=READ_DEADLINE&&elapsed.elapsed()<Duration::from_secs(2));assert_eq!(wire.reads.load(Ordering::SeqCst),0);
            release.send(()).unwrap();holder.await.unwrap();
        }
        Case::AccountSqlFence => {
            let prepared = manager.prepare_user_entry(&line, 1, RECIPIENT, true).unwrap();
            lease_revision(&temp.database, &line, 1, 2);
            assert_eq!(manager.finish_user_entry(prepared).await.err(), Some("app_account_runtime_owner_unconfirmed"));
            lease_revision(&temp.database, &line, 2, 1);
            assert_eq!(wire.reads.load(Ordering::SeqCst), 0);
            let prepared = manager.prepare_user_entry(&line, 1, RECIPIENT, true).unwrap();
            let mut lookup = Box::pin(manager.finish_user_entry(prepared));
            tokio::select! {
                result = &mut lookup => panic!("READ returned before the original peer answer: {result:?}"),
                result = &mut wire.seen => assert_eq!(result.unwrap().0, 2),
            }
            let(entered,ready)=oneshot::channel();let(release,released)=oneshot::channel();let held=manager.clone();let fixed=line.clone();
            let holder=tokio::spawn(async move{held.test_user_entry_controller_barrier(&fixed,entered,released).await;});ready.await.unwrap();
            lease_revision(&temp.database, &line, 1, 2);
            // Answer is valid under the same SDK; only the ORIGINAL SQL/run
            // fence changed. The manager must check again AFTER the IO await.
            // Move only this signal, leaving all peer/relay Joins with wire.
            let (unused, _) = oneshot::channel();
            std::mem::replace(&mut wire.resume, unused).send(()).unwrap();
            assert_eq!(lookup.await.err(), Some("app_account_runtime_owner_unconfirmed"));
            lease_revision(&temp.database, &line, 2, 1);
            release.send(()).unwrap();holder.await.unwrap();
            stale_after_stop = Some(manager.prepare_user_entry(&line, 1, RECIPIENT, true).unwrap());
        }
        _ => unreachable!(),
    }
    let after = manager.get(&line, 100).await.unwrap(); assert_eq!(after.phase, "hosted");
    assert_eq!(before.incoming_message_ready, after.incoming_message_ready);
    assert_eq!(before.incoming_call_ready, after.incoming_call_ready);
    assert_eq!(before.outbound_authorized, after.outbound_authorized);
    assert_eq!(before.user_entry_supported, after.user_entry_supported);
    assert_eq!(counts, sql_counts(&temp.database));
    assert_eq!(factory.build_count(), 1); assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    let stopped = manager.stop(&line, 1, 100).await.unwrap();
    assert!(!stopped.io_cleanup_required);
    assert!(!stopped.incoming_message_ready && !stopped.incoming_call_ready && !stopped.outbound_authorized);
    if let Some(prepared) = stale_after_stop {
        assert_eq!(manager.finish_user_entry(prepared).await.err(), Some("app_account_user_entry_scope_invalid"));
    }
    assert_eq!(manager.shutdown_all().await, 0);
    wire.finish(usize::from(case.expects_read())).await;
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0); assert_eq!(factory.build_count(), 1);
}

#[tokio::test]
async fn client_user_entry_reported_whole_factory_cc_outer_sdk_auth_and_two_joins() { Box::pin(native(Case::Reported)).await; }
#[tokio::test]
async fn client_user_entry_negative_plausible_uid_is_rejected_data_not_ready_or_write_grant() { Box::pin(native(Case::Negative)).await; }
#[tokio::test]
async fn client_user_entry_positive_empty_map_is_unreported_without_uid_guess_or_retry() { Box::pin(native(Case::Unreported)).await; }
#[tokio::test]
async fn client_user_entry_foreign_aid_is_exact_reply_invalid_and_sdk_stays_healthy() { Box::pin(native(Case::ForeignAid)).await; }
#[tokio::test]
async fn client_user_entry_ambiguous_uid_map_is_exact_ambiguous_without_arbitrary_selection() { Box::pin(native(Case::Ambiguous)).await; }
#[tokio::test]
async fn client_user_entry_nested_version_error_is_rejected_without_retry() { Box::pin(native(Case::VersionError)).await; }
#[tokio::test]
async fn client_user_entry_bad_nested_footer_suppresses_uid_without_closing_sdk() { Box::pin(native(Case::BadFooter)).await; }
#[tokio::test]
async fn client_user_entry_full_read_timeout_without_outer_ack_preserves_unregister_logout() { Box::pin(native(Case::FullTimeout)).await; }
#[tokio::test]
async fn client_user_entry_consumer_drop_purges_read_copies_but_preserves_mixed_write_epoch() { Box::pin(native(Case::ConsumerDropMixed)).await; }
#[tokio::test]
async fn client_user_entry_stop_retires_pending_read_without_replay_then_all_original_joins() { Box::pin(native(Case::StopPending)).await; }
#[tokio::test]
async fn client_user_entry_uid_local_baseline_has_no_query_id_read_or_third_connection() { Box::pin(native(Case::UidLocal)).await; }
#[tokio::test]
async fn client_user_entry_account_consent_revision_realm_and_read_leave_sql_and_ready_unchanged() { Box::pin(managed(Case::AccountRead)).await; }
#[tokio::test]
async fn client_user_entry_prepared_account_ages_past_original_100ms_deadline_without_read_io() { Box::pin(managed(Case::PreparedDeadline)).await; }
#[tokio::test]
async fn client_user_entry_prepared_account_checks_sql_before_after_io_and_run_after_stop() { Box::pin(managed(Case::AccountSqlFence)).await; }
#[tokio::test]
async fn client_user_entry_controller_contention_consumes_original_100ms_deadline_before_any_read_io(){Box::pin(managed(Case::ControllerDeadline)).await;}

#[path="client_user_entry_runtime_race_tests.rs"]
mod races;
