//! Actions-only：真实内存 TLS actor→账号消费器→SQLite；不是手机授信实测。
use super::*;
use super::super::{account_test_support::{config, peer, QueuedFactory, TempDatabase},
    rpc_envelope::{decode_request, decode_reply, encode_request}, rpc_offline::{GET_OFFLINE_METHOD, MORE_OFFLINE_METHOD},
    rpc_stream::{Reader, Writer, RpcProperty}, runtime_backend::tests::{Peer, next_non_poll, swap_result},
    session::{STATUS_REGISTER, STATUS_UNREGISTER}, tls_engine::tests::Certificates, transport_packets::CallKind};
use tokio::sync::oneshot;

fn offline_config(root: &str) -> AccountConfig {
    let mut config = config(root); config.profile.offline_reception = true;
    config.profile.incoming.as_mut().unwrap().receiver_kind = b"Sip.Request".to_vec(); config
}
fn message(text: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(); writer.bytes(b"Sip.Request").unwrap();
    writer.properties(&[RpcProperty { key: b"Sip.Message.From", value: b"fixture-sender" }, RpcProperty { key: b"Sip.Message.Time", value: b"1700000000000" }]).unwrap();
    writer.bytes(text).unwrap(); writer.finish().expose_to_transport().to_vec()
}
fn page(records: &[(i64, &[u8])], last: i64) -> Vec<u8> {
    let mut writer = Writer::new(); writer.boolean(true).unwrap(); writer.u32(records.len() as u32).unwrap();
    for (position, record) in records { writer.i64(*position).unwrap(); writer.bytes(record).unwrap(); }
    writer.i64(last).unwrap(); writer.bytes(b"").unwrap(); writer.finish().expose_to_transport().to_vec()
}
async fn next_get(peer: &mut Peer, expected_position: i64) -> i32 {
    loop {
        let message = peer.receive().await.unwrap(); assert_eq!(message.kind, CallKind::Request);
        let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut reader = Reader::versioned(request.arguments).unwrap();
        assert_eq!(request.metadata.iter().find(|property| property.key == b"token").unwrap().value, b"fixture-auth");
        assert!(!request.metadata.iter().any(|property| property.key == b"pwd"));
        assert_eq!(reader.bytes().unwrap(), b"fixture-user"); assert_eq!(reader.bytes().unwrap(), b"fixture-original-selector");
        if request.method == GET_OFFLINE_METHOD.as_bytes() {
            assert_eq!(reader.i64().unwrap(), expected_position); assert_eq!(reader.u32().unwrap(), 100); reader.finish().unwrap(); return request.call_id;
        }
        assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let payload = super::super::rpc_batch::read_payload(&mut reader).unwrap(); reader.finish().unwrap();
        assert!(payload.requests.is_empty()); peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await;
    }
}
async fn finish(peer: &mut Peer) {
    peer.swap(Some(STATUS_UNREGISTER), true).await;
    let message = next_non_poll(peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
}
async fn wait_messages(manager: &AccountManager, line: &str, expected: usize) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if manager.messages(line, 128).unwrap().len() == expected { return; }
            let status = manager.get(line, 100).await.unwrap(); assert!(status.last_error.is_none(), "{:?}", status.last_error);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
}

#[tokio::test]
async fn matched_tls_offline_pages_and_hint_ack_use_committed_position_before_the_next_request() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let database = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut endpoint, connector, dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(database.database.clone(), factory.clone());
    let line = manager.create(offline_config(&cert.root), 100).await.unwrap().account.line_id;
    let (first_consumed, continue_after_first) = oneshot::channel();
    let serving = tokio::spawn(async move {
        endpoint.authenticate().await; endpoint.swap(Some(STATUS_REGISTER), true).await;
        let first = next_get(&mut endpoint, -1).await;
        let one = message(b"fixture-one"); let two = message(b"fixture-two");
        endpoint.reply(first, &page(&[(1, &one), (2, &two)], 2)).await;
        continue_after_first.await.unwrap();
        let mut writer = Writer::versioned().unwrap(); writer.i64(4).unwrap();
        let hint = encode_request(800001, &[RpcProperty { key: b"id", value: b"#fixture-client" },
            RpcProperty { key: b"domain", value: b"fixture-domain" }, RpcProperty { key: b"__magic", value: b"fixture-hint" }],
            MORE_OFFLINE_METHOD.as_bytes(), writer.finish().expose_to_transport()).unwrap();
        endpoint.send_call(CallKind::Request, hint.expose_to_transport()).await;
        loop {
            let packet = endpoint.receive().await.unwrap();
            if packet.kind == CallKind::Reply {
                let reply = decode_reply(packet.payload.expose_to_transport()).unwrap(); assert_eq!(reply.call_id, 800001);
                assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]); break;
            }
            let request = decode_request(packet.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
            let mut reader = Reader::versioned(request.arguments).unwrap(); reader.bytes().unwrap(); reader.bytes().unwrap();
            let payload = super::super::rpc_batch::read_payload(&mut reader).unwrap(); assert!(payload.requests.is_empty());
            endpoint.reply(request.call_id, &swap_result(None, true, payload.fields)).await;
        }
        // hint=4 不能把 cursor 提前写成5；已提交位置是第一批最大2+1。
        let second = next_get(&mut endpoint, 3).await; assert!(second > first);
        let last = message(b"fixture-later"); endpoint.reply(second, &page(&[(4, &last)], 4)).await;
        finish(&mut endpoint).await;
    });
    manager.start(&line, 1, 100).await.unwrap(); wait_messages(&manager, &line, 2).await;
    first_consumed.send(()).unwrap(); wait_messages(&manager, &line, 3).await;
    let history = manager.messages(&line, 128).unwrap(); assert_eq!(history[0].message.content, "fixture-later");
    assert!(history.iter().all(|message| message.classification == "smgw_text" && message.message.pdu.is_none()));
    let original = connector.captured_scope().unwrap();
    let stopped = manager.stop(&line, 1, 100).await.unwrap(); assert!(!stopped.io_cleanup_required); assert!(!original.is_open());
    assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap(); assert_eq!(factory.build_count(), 1);
    let saved: i64 = database.database.with_conn(|conn| conn.query_row("SELECT next_position FROM app_voice_offline_cursors WHERE line_id=?1", [&line], |row| row.get(0)).map_err(|_| "fixture")).unwrap();
    assert_eq!(saved, 5);
    let reopen = AccountManager::with_factory(Arc::new(Database::new(database.path.clone()).unwrap()), factory.clone());
    let state = reopen.get(&line, 100).await.unwrap(); assert_eq!(state.phase, "stopped"); assert!(!state.incoming_message_ready); assert_eq!(factory.build_count(), 1);
}

#[test]
fn offline_install_option_is_explicit_private_roundtripped_and_cannot_create_a_receiver() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let mut original = config(&cert.root);
    assert!(!original.profile.offline_reception);
    let old = original.encode_private().unwrap(); assert!(!std::str::from_utf8(&old).unwrap().contains("offline_reception"));
    original.profile.offline_reception = true;
    let encoded = original.encode_private().unwrap(); let decoded = AccountConfig::decode_private(&encoded).unwrap();
    assert!(decoded.profile.offline_reception); assert!(decoded.runtime_profile().is_ok());
    original.profile.incoming = None; assert!(original.runtime_profile().is_err());
    assert!(!format!("{original:?}").contains("fixture"));
}

#[tokio::test]
async fn unsupported_nested_stream_is_preserved_and_cannot_trigger_a_followup_get_or_sms() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let database = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut endpoint, connector, _dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(database.database.clone(), factory);
    let line = manager.create(offline_config(&cert.root), 100).await.unwrap().account.line_id;
    let serving = tokio::spawn(async move {
        endpoint.authenticate().await; endpoint.swap(Some(STATUS_REGISTER), true).await;
        let request = next_get(&mut endpoint, -1).await;
        endpoint.reply(request, &page(&[(1, b"\x02{\"body\":\"unknown\"}")], 8)).await;
        // 允许已在途的空 poll；绝无新的取件位置、消费 ACK 或业务请求。
        while let Some(packet) = endpoint.receive().await {
            if packet.kind != CallKind::Request { continue; }
            let request = decode_request(packet.payload.expose_to_transport()).unwrap();
            assert_ne!(request.method, GET_OFFLINE_METHOD.as_bytes());
        }
    });
    manager.start(&line, 1, 100).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while connector.captured_scope_is_open() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.unwrap();
    assert!(manager.messages(&line, 10).unwrap().is_empty());
    database.database.with_conn(|conn| {
        let cursors: i64 = conn.query_row("SELECT COUNT(*) FROM app_voice_offline_cursors", [], |row| row.get(0)).map_err(|_| "fixture")?;
        let quarantined: i64 = conn.query_row("SELECT COUNT(*) FROM app_voice_offline_quarantine", [], |row| row.get(0)).map_err(|_| "fixture")?;
        assert_eq!(cursors, 0); assert_eq!(quarantined, 1); Ok(())
    }).unwrap();
    let _ = manager.stop(&line, 1, 100).await;
    serving.await.unwrap();
}
