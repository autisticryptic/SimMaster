//! Actions-only original Router/TCP/TLS/Common userSend WRITE regressions.
//! No original SDK/SO, device, Provider, readiness grant or service call.
//! Peer futures are bounded and joined in place; no physical peer is detached.
use super::*;
use n::{
    account_test_support::TempDatabase,
    im_inbox::{ImSendAction, ImSendMessage, ImSendPermit, PreparedImSend},
    im_send_policy::NativeImSendConfig,
};

const SOURCE_NORMALIZED_URI: &[u8] = b"[username:fixture-user@fixture.invalid]";
const RETURNED_UID: &[u8] = b"fixture-user";
const MAIN_ACCOUNT: &[u8] = b"fixture-returned-main-account";
const TEXT: &str = "private original send";
const NONCE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn policy() -> NativeImSendConfig {
    NativeImSendConfig { send_text: true, send_info: true, auth_country_code: None }
}

// A sibling's private creator is deliberately not called. This is the same
// fresh stored account/runtime-run fixture used by received_writes::original.
fn original(temp: &TempDatabase, root: &str) -> (AttemptOwner, Arc<n::account_runs::RunToken>) {
    let line = "app-account-0123456789abcdef0123456789abcdef";
    let mut cfg = n::account_config::tests::native_fixture(root, "127.0.0.1", 1);
    cfg.credentials.sdk.expires_at_unix_seconds = None;
    cfg.profile.incoming = None;
    cfg.profile.client_ex = true;
    cfg.profile.native_receivers = Some(super::config(true));
    temp.database.with_conn(|c| n::account_store::create(c, line, &cfg, 100)).unwrap();
    let run = Arc::new(temp.database.with_conn(|c| {
        n::account_runs::register_runtime(c, line, 1, &n::account_runtime::local_nonce().unwrap())
    }).unwrap());
    let mut session = n::session::HostingSession::new(line.into());
    session.import(n::tests::imported(), 100).unwrap();
    (session.begin_login(100).unwrap(), run)
}

async fn sender_login(peer: &mut CommonPeer) {
    let packet = peer.next().await.unwrap();
    assert_eq!(packet[0], 0);
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(request.call_id, 1);
    assert_eq!(request.method, n::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
    assert_eq!(request.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, b"fixture-phone-token");
    assert!(!request.metadata.iter().any(|p| p.key == b"token"));
    let mut reader = Reader::versioned(request.arguments).unwrap();
    assert_eq!(reader.bytes().unwrap(), SOURCE_NORMALIZED_URI);
    assert_eq!(reader.bytes().unwrap(), b"fixture-selected-session");
    // The matched login returns two distinct pieces of DATA. Neither may
    // replace the original normalized OWN URI in outgoing account parameters.
    let mut writer = Writer::new();
    writer.boolean(true).unwrap();
    writer.bytes(RETURNED_UID).unwrap();
    writer.bytes(b"fixture-auth").unwrap();
    writer.properties(&[RpcProperty { key: b"mainAccount", value: MAIN_ACCOUNT }]).unwrap();
    writer.bytes(b"").unwrap();
    peer.send(&[reply(1, writer.finish().expose_to_transport())]).await;
}

// No ACK or router-alive WRITE may race an already cancelled original socket.
// Only exact native empty Common controls are tolerated, never an RPC/poll,
// retry, partial business frame or arbitrary packet hidden by a filter.
async fn sender_eof(peer: &mut CommonPeer) {
    while let Some(packet) = peer.next_input_only().await {
        assert!(matches!(packet.as_slice(), [2] | [14] | [4]),
            "only original empty Common control packets may precede EOF: {packet:?}");
    }
}

async fn sender_logout(peer: &mut CommonPeer, expected_id: i32) {
    let packet = peer.next().await.unwrap();
    let request=decode_request(&packet[1..]).unwrap();
    assert_eq!(packet[0],0);
    assert_eq!(request.method,n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    assert_eq!(request.call_id,expected_id,"no cancelled SEND, replay or low-level account poll consumes an ID");
    assert_eq!(request.metadata.len(),2);
    assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"AccountEntry");
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
    let mut arguments=Reader::versioned(request.arguments).unwrap();
    assert_eq!(arguments.bytes().unwrap(),RETURNED_UID);
    assert_eq!(arguments.bytes().unwrap(),b"fixture-selected-session");
    arguments.finish().unwrap();
    peer.send(&[reply(expected_id, &[0, 1, 0, 0, 0, 0])]).await;
    sender_eof(peer).await;
}

fn send_packet(packet: &[u8], expected_id: i32, expected_box: &[u8]) {
    assert_eq!(query(packet, n::im_send_data::USER_SEND_METHOD), expected_id);
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(request.method, b"userSend.Message.Message");
    // Independently consume the exact original outgoing struct, not the
    // received index/time/sender shape or the production SEND decoder.
    assert_eq!(&request.arguments[..5], &[0, 0, 1, 0, 0],
        "type0 stream, BE16 method-version count=1, BE16 version=0");
    let mut reader = Reader::versioned(request.arguments).unwrap();
    assert_eq!(reader.bytes().unwrap(), expected_box);
    assert_eq!(reader.bytes().unwrap(), b"Text");
    assert_eq!(reader.bytes().unwrap(), br#"{"text":"private original send"}"#);
    assert!(reader.properties().unwrap().is_empty(), "no invented attachment");
    let parameters = reader.properties().unwrap();
    assert_eq!(parameters.len(), 1);
    let account = parameters.iter().find(|p| p.key == b"account").unwrap().value;
    assert_eq!(account, SOURCE_NORMALIZED_URI);
    assert_ne!(account, MAIN_ACCOUNT);
    assert_ne!(account, RETURNED_UID);
    let notifications = reader.properties().unwrap();
    assert_eq!(notifications.len(), 4);
    for (key, value) in [
        (b"Notify.Text".as_slice(), TEXT.as_bytes()),
        (b"Statistics.AppId".as_slice(), b"fixture-app-id".as_slice()),
        (b"Statistics.DomainId".as_slice(), b"fixture-domain-id".as_slice()),
        (b"Statistics.CountryCode".as_slice(), b"".as_slice()),
    ] {
        assert_eq!(notifications.iter().find(|p| p.key == key).unwrap().value, value);
    }
    assert!(!notifications.iter().any(|p| p.key == b"Notify.CountryCode"));
    reader.finish().unwrap();
}

fn send_result(accepted: bool, message_id: i64, time: i64) -> Vec<u8> {
    // Native resultData is NOT versioned and has no count/result-code field:
    // bool, two signed64 values and the independently written empty footer.
    let mut writer = Writer::new();
    writer.boolean(accepted).unwrap();
    writer.i64(message_id).unwrap();
    writer.i64(time).unwrap();
    writer.bytes(b"").unwrap();
    writer.finish().expose_to_transport().to_vec()
}

async fn prepare(
    handle: &RuntimeHandle, who: &AttemptOwner, scope: &RpcConnectionScope,
    temp: &TempDatabase, run: Arc<n::account_runs::RunToken>, parent: Arc<ImReceiptGate>,
    recipient: &[u8], nonce: &str, lifetime: Duration,
) -> (Arc<ImSendAction>, Arc<ImSendPermit>) {
    let commands = handle.command_handle();
    let binding = bounded(commands.original_im_binding(who, scope)).await.unwrap();
    let identity = bounded(commands.original_im_send_identity(who, scope)).await.unwrap();
    assert_eq!(identity.uri_for(who, scope), Some(SOURCE_NORMALIZED_URI));
    let policy = policy();
    let PreparedImSend::Fresh(action) = ImSendAction::original(
        temp.database.clone(), run, parent, binding, identity, recipient,
        ImSendMessage::text(TEXT).unwrap(), nonce, Instant::now() + lifetime, &policy,
    ).unwrap() else { panic!("fresh original user SEND action") };
    assert!(action.recipient_action().is_none(), "direct UID/Org needs no auxiliary lookup");
    let permit = action.permit(&policy).unwrap();
    (action, permit)
}

fn outbox(temp: &TempDatabase) -> (i64, i64, Option<i32>) {
    temp.database.with_conn(|c| c.query_row(
        "SELECT id,state,request_id FROM app_voice_im_outbox", [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).map_err(|_| "fixture outbox")).unwrap()
}

#[tokio::test]
async fn send_io_original_uid_and_org_wire_own_uri_statistics_and_native_bool_two_i64_reply() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
        let server = cert.server_config(&TLS13);
        let temp = TempDatabase::new();
        let (who, run) = original(&temp, &cert.root);
        let peer = async move {
            let mut peer = CommonPeer::open(listener, server).await;
            sender_login(&mut peer).await;
            let packet = peer.next().await.unwrap();
            send_packet(&packet, 2, b"P2P/1_2");
            peer.send(&[reply(2, &send_result(true, 4_294_967_297, -8_589_934_593))]).await;
            let packet = peer.next().await.unwrap();
            send_packet(&packet, 3, b"Org/1-2");
            peer.send(&[reply(3, &send_result(false, -4_294_967_299, 8_589_934_601))]).await;
            sender_logout(&mut peer, 4).await;
        };
        let client = async {
            let connection = connected(&route, who.clone(), Instant::now() + Duration::from_secs(6)).await;
            let scope = connection.connection_scope();
            let (mut handle, mut events) = connection.attach().unwrap();
            auth(&handle, &mut events, &who, &scope, true).await;
            let commands = handle.command_handle();
            let (action, permit) = prepare(&handle, &who, &scope, &temp, run.clone(),
                ImReceiptGate::fresh(), b"1_2", NONCE, Duration::from_secs(3)).await;
            assert_eq!(outbox(&temp).1, 0);
            assert_eq!(outbox(&temp).2, None);
            let view = bounded(commands.send_im(&who, &scope, permit.clone())).await.unwrap();
            assert_eq!(view.state, "confirmed");
            assert_eq!(view.kind, "text");
            assert_eq!(view.message_index.as_deref(), Some("4294967297"));
            assert_eq!(view.original_time.as_deref(), Some("-8589934593"));
            assert_eq!(outbox(&temp), (view.id, 1, Some(2)));
            assert!(!permit.closed_original_io());
            drop(permit);
            drop(action);
            let (action, permit) = prepare(&handle, &who, &scope, &temp, run.clone(),
                ImReceiptGate::fresh(), b"1-2", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                Duration::from_secs(3)).await;
            let view = bounded(commands.send_im(&who, &scope, permit.clone())).await.unwrap();
            assert_eq!(view.state, "rejected", "native false is not confirmation");
            assert_eq!(view.message_index.as_deref(), Some("-4294967299"));
            assert_eq!(view.original_time.as_deref(), Some("8589934601"));
            let row: (i64, Option<i32>) = temp.database.with_conn(|c| c.query_row(
                "SELECT state,request_id FROM app_voice_im_outbox WHERE id=?1", [view.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(|_| "fixture outbox")).unwrap();
            assert_eq!(row, (2, Some(3)));
            assert!(!permit.closed_original_io());
            drop(permit);
            drop(action);
            assert!(scope.is_open());
            assert!(events.try_recv().is_err(), "SEND is not an incoming message/readiness callback");
            let closed = bounded(handle.logout(&who, &scope, &[])).await.unwrap();
            assert!(closed.io_released());
            assert_eq!(closed.reason, RuntimeError::LoggedOut);
            assert_eq!(scope.native_rpc_resources(), (0, 0));
        };
        tokio::join!(Box::pin(bounded(peer)), Box::pin(bounded(client)));
    }).await;
}

#[tokio::test]
async fn send_io_queued_command_cancel_allocates_no_send_id_and_preserves_sdk_logout_health() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
        let server = cert.server_config(&TLS13);
        let temp = TempDatabase::new();
        let (who, run) = original(&temp, &cert.root);
        let peer = async move {
            let mut peer = CommonPeer::open(listener, server).await;
            sender_login(&mut peer).await;
            sender_logout(&mut peer, 2).await;
        };
        let client = async {
            let connection = connected(&route, who.clone(), Instant::now() + Duration::from_secs(6)).await;
            let scope = connection.connection_scope();
            let (mut handle, mut events) = connection.attach().unwrap();
            auth(&handle, &mut events, &who, &scope, true).await;
            let (action, permit) = prepare(&handle, &who, &scope, &temp, run.clone(),
                ImReceiptGate::fresh(), b"1_2", NONCE, Duration::from_secs(3)).await;
            let (reply, data) = oneshot::channel();
            let receipt = handle.command_handle().enqueue(&who, &scope,
                Operation::ImSend(Box::new(ImSendWork { permit: permit.clone(), reply }))).unwrap();
            // No await between enqueue and retirement: the original actor
            // cannot dequeue this WRITE before its result owner is gone.
            drop(data);
            action.cancel();
            assert_eq!(bounded(receipt).await.unwrap().err(), Some(RuntimeError::Scope));
            let (id, state, request) = outbox(&temp);
            assert!(id > 0);
            assert_eq!((state, request), (3, None));
            assert!(permit.retired());
            assert!(!permit.closed_original_io());
            assert!(scope.is_open());
            assert!(events.try_recv().is_err());
            let closed = bounded(handle.logout(&who, &scope, &[])).await.unwrap();
            assert!(closed.io_released());
            assert_eq!(closed.reason, RuntimeError::LoggedOut);
            assert_eq!(scope.native_rpc_resources().0, 3, "closed actor cannot refund retained input/identity/arguments");
            assert!(scope.native_rpc_resources().1 > 0);
            drop(permit); drop(action);
            assert_eq!(scope.native_rpc_resources(), (0, 0));
        };
        tokio::join!(Box::pin(bounded(peer)), Box::pin(bounded(client)));
    }).await;
}

async fn actual_prefix(parent_retirement: bool, deadline: bool) {
    let cert = Certificates::generate("IP:127.0.0.1");
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
    let server = cert.server_config(&TLS13);
    let temp = TempDatabase::new();
    let (who, run) = original(&temp, &cert.root);
    let peer = async move {
        let mut peer = CommonPeer::open(listener, server).await;
        sender_login(&mut peer).await;
        // The SAME main fd's single accepted ciphertext byte cannot form a
        // complete outgoing SEND or a replay. Read-only EOF cannot ACK it.
        sender_eof(&mut peer).await;
    };
    let client = async {
        let mut connection = connected(&route, who.clone(), Instant::now() + Duration::from_secs(6)).await;
        let prefix = n::router_runtime::tests::auxiliary_io::AuxiliaryIoTest::original_unarmed(1);
        n::common_runtime::tests::gate_auxiliary_prefix(connection.common.as_mut().unwrap(), prefix.clone());
        let scope = connection.connection_scope();
        assert!(prefix.scope().matches(&scope), "wrapper owns the same main Group fd/scope");
        let (mut handle, mut events) = connection.attach().unwrap();
        auth(&handle, &mut events, &who, &scope, true).await;
        let parent = ImReceiptGate::fresh();
        let (action, permit) = prepare(&handle, &who, &scope, &temp, run.clone(), parent.clone(),
            b"1_2", NONCE, Duration::from_secs(if deadline { 1 } else { 3 })).await;
        prefix.arm();
        let started = Instant::now();
        let commands = handle.command_handle();
        let mut sending = Box::pin(commands.send_im(&who, &scope, permit.clone()));
        tokio::select! {
            result = sending.as_mut() => panic!("partial original SEND cannot complete: {result:?}"),
            _ = bounded(prefix.paused()) => {}
        }
        assert_eq!(prefix.accepted.load(Ordering::SeqCst), 1);
        assert!(prefix.pauses.load(Ordering::SeqCst) > 0);
        assert!(!prefix.dropped.load(Ordering::SeqCst));
        let before = outbox(&temp);
        assert!(before.0 > 0);
        assert_eq!((before.1, before.2), (0, Some(2)));
        assert!(permit.current(2));
        let reason = if deadline {
            assert_eq!(bounded(sending).await.err(), Some(RuntimeError::Timeout));
            assert!(Instant::now() >= action.deadline());
            assert!(started.elapsed() < Duration::from_secs(2),
                "original one-second action deadline precedes IO(2s)/RPC(3s), not a renewed budget");
            RuntimeError::Timeout
        } else if parent_retirement {
            parent.retire();
            assert_eq!(bounded(sending).await.err(), Some(RuntimeError::Cancelled));
            RuntimeError::Cancelled
        } else {
            drop(sending); // Actual commands.send_im result-wait Drop cancels its permit.
            RuntimeError::Cancelled
        };
        bounded(scope.closed()).await;
        let closed = bounded(handle.close()).await;
        assert!(closed.io_released());
        assert_eq!(closed.reason, reason);
        assert!(permit.closed_original_io());
        assert!(!permit.current(2));
        assert!(prefix.dropped.load(Ordering::SeqCst), "actual fd Drop precedes current IO Join receipt");
        assert_eq!(prefix.accepted.load(Ordering::SeqCst), 1, "no late ciphertext byte");
        assert_eq!(outbox(&temp), before, "possible partial WRITE remains unknown, never refunded");
        assert_eq!(scope.native_rpc_resources().0, 3, "original external holders outlive the actor's IO Join");
        assert!(scope.native_rpc_resources().1 > 0);
        drop(permit); drop(action);
        assert_eq!(scope.native_rpc_resources(), (0, 0));
    };
    tokio::join!(Box::pin(bounded(peer)), Box::pin(bounded(client)));
}

#[tokio::test]
async fn send_io_actual_one_byte_cancel_keeps_unknown_and_joins_same_main_fd_drop() {
    Box::pin(actual_prefix(false, false)).await;
}
#[tokio::test]
async fn send_io_parent_retirement_after_one_original_byte_has_no_late_frame_or_refund() {
    Box::pin(actual_prefix(true, false)).await;
}
#[tokio::test]
async fn send_io_original_action_deadline_bounds_actual_partial_write_before_actor_budget() {
    Box::pin(actual_prefix(false, true)).await;
}

async fn full_submission(sql_failure: bool, parent_retirement: bool) {
    let cert = Certificates::generate("IP:127.0.0.1");
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
    let server = cert.server_config(&TLS13);
    let temp = TempDatabase::new();
    let (who, run) = original(&temp, &cert.root);
    let (seen, request_seen) = oneshot::channel();
    let (release, reply_ready) = oneshot::channel();
    let peer = async move {
        let mut peer = CommonPeer::open(listener, server).await;
        sender_login(&mut peer).await;
        let packet = peer.next().await.unwrap();
        send_packet(&packet, 2, b"P2P/1_2");
        seen.send(()).unwrap();
        if reply_ready.await.is_ok() {
            peer.send(&[reply(2, &send_result(true, 4_294_967_297, -8_589_934_593))]).await;
        }
        sender_eof(&mut peer).await;
    };
    let client = async {
        let mut connection = connected(&route, who.clone(), Instant::now() + Duration::from_secs(6)).await;
        // Leave this wrapper unarmed: full WRITE is real/unrestricted, while
        // dropped still observes destruction of the SAME original main fd.
        let actual_drop = n::router_runtime::tests::auxiliary_io::AuxiliaryIoTest::original_unarmed(1);
        n::common_runtime::tests::gate_auxiliary_prefix(connection.common.as_mut().unwrap(), actual_drop.clone());
        let scope = connection.connection_scope();
        assert!(actual_drop.scope().matches(&scope));
        let (mut handle, mut events) = connection.attach().unwrap();
        auth(&handle, &mut events, &who, &scope, true).await;
        let parent = ImReceiptGate::fresh();
        let (action, permit) = prepare(&handle, &who, &scope, &temp, run.clone(), parent.clone(),
            b"1_2", NONCE, Duration::from_secs(3)).await;
        let commands = handle.command_handle();
        let mut sending = Box::pin(commands.send_im(&who, &scope, permit.clone()));
        tokio::select! {
            result = sending.as_mut() => panic!("original peer has not replied: {result:?}"),
            result = bounded(request_seen) => result.unwrap()
        }
        // Physical peer receipt/Channel ACK alone is NOT the actor's completed
        // submission. Keep polling the actual send future until a subsequent
        // actor-command completion: it cannot execute until send_packets has
        // returned, submitted() succeeded and the private pending wait exists.
        // This needs no mutation of phase or nonexistent test-only accessor.
        tokio::select! {
            result = sending.as_mut() => panic!("original result remains absent: {result:?}"),
            binding = bounded(commands.original_im_binding(&who, &scope)) => {
                let binding = binding.unwrap();
                assert!(Arc::ptr_eq(&binding, permit.binding()));
            }
        }
        assert!(permit.current(2));
        assert!(!actual_drop.dropped.load(Ordering::SeqCst));
        let before = outbox(&temp);
        assert!(before.0 > 0);
        assert_eq!((before.1, before.2), (0, Some(2)));
        let reason = if sql_failure {
            temp.database.with_conn(|c| c.execute_batch(
                "CREATE TRIGGER reject_original_send_result BEFORE UPDATE OF state ON app_voice_im_outbox \
                 WHEN NEW.state IN(1,2) BEGIN SELECT RAISE(ABORT,'fixture'); END",
            ).map_err(|_| "fixture SQL failure")).unwrap();
            release.send(()).unwrap();
            assert_eq!(bounded(sending).await.err(), Some(RuntimeError::Protocol));
            RuntimeError::Protocol
        } else {
            drop(release); // No late reply WRITE may race cancellation/EOF.
            if parent_retirement {
                parent.retire();
                assert_eq!(bounded(sending).await.err(), Some(RuntimeError::Cancelled));
            } else {
                drop(sending); // Real ImSendPermitWait Drop, not read-interest retirement.
            }
            RuntimeError::Cancelled
        };
        bounded(scope.closed()).await;
        let closed = bounded(handle.close()).await;
        assert!(closed.io_released());
        assert_eq!(closed.reason, reason);
        assert!(permit.closed_original_io());
        assert!(!permit.current(2));
        assert!(actual_drop.dropped.load(Ordering::SeqCst), "actual original fd released before IO Join");
        assert_eq!(outbox(&temp), before, "full WRITE/failed matched SQL result remains unknown");
        assert_eq!(scope.native_rpc_resources().0, 3, "Join/retirement cannot refund original held arguments");
        assert!(scope.native_rpc_resources().1 > 0);
        drop(permit);
        drop(action);
        assert_eq!(scope.native_rpc_resources(), (0, 0));
        assert_eq!(outbox(&temp), before, "owner retirement cannot refund/replay uncertain SEND");
    };
    tokio::join!(Box::pin(bounded(peer)), Box::pin(bounded(client)));
}

#[tokio::test]
async fn send_io_full_write_wait_drop_keeps_unknown_and_current_io_join_reason_cancelled() {
    Box::pin(full_submission(false, false)).await;
}
#[tokio::test]
async fn send_io_full_parent_retirement_keeps_unknown_and_original_cancelled_join() {
    Box::pin(full_submission(false, true)).await;
}
#[tokio::test]
async fn send_io_matched_native_reply_sql_failure_is_protocol_and_keeps_unknown() {
    Box::pin(full_submission(true, false)).await;
}
