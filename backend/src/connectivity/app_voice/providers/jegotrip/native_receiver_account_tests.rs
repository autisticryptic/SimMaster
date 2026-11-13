//! Actions-only real account persistence/ACK/cleanup using native receiver mode.
use super::*;
use n::{account_config::AccountConfig, account_runtime::AccountManager, account_test_support::{config_json, TempDatabase}};
fn account_config(root: &str, port: u16, offline: bool) -> AccountConfig {
    let mut json = config_json(root);
    json["credentials"]["sdk"]["server_address"] = serde_json::json!(ENTRY);
    json["profile"]["route"] = serde_json::json!({"router_common":{
        "reference":format!("Router:tls -h 127.0.0.1 -p {port} -ri 12001;"),
        "anchors":[{"router_id":12001,"host":"127.0.0.1","port":port,"authority":"127.0.0.1","root_der_base64":root,"enable_sni":false}],
        "access":[],"endpoint_index":0,"resolved_host_index":0,"startup_timeout_ms":5000,"attempt_timeout_ms":8000,"compress_above":null}});
    json["profile"]["authority"] = serde_json::json!(""); json["profile"]["enable_sni"] = serde_json::json!(false);
    json["profile"]["incoming"] = serde_json::Value::Null; json["profile"]["native_receivers"] = serde_json::json!({});
    json["profile"]["gateway_metadata"] = serde_json::json!([]); json["profile"]["client_ex"] = serde_json::json!(true);
    json["profile"]["offline_reception"] = serde_json::json!(offline); serde_json::from_value(json).unwrap()
}
async fn until<F: Fn() -> bool>(wanted: F) { tokio::time::timeout(Duration::from_secs(6), async { while !wanted() { tokio::time::sleep(Duration::from_millis(5)).await; } }).await.unwrap(); }
async fn finish_account(peer: &mut CommonPeer) {
    let (_, flag) = hosting_call(peer).await; assert_eq!(flag, b"3");
    loop { let packet = peer.next().await.unwrap(); let request = decode_request(&packet[1..]).unwrap();
        if request.method == n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes() {
            peer.send(&[reply(request.call_id, &[0, 1, 0, 0, 0, 0])]).await; break;
        }
        assert_eq!(request.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut r = Reader::versioned(request.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap(); let payload = n::rpc_batch::read_payload(&mut r).unwrap();
        peer.send(&[reply(request.call_id, &super::super::super::tests::swap_result(None, true, payload.fields))]).await;
    }
    while peer.next().await.is_some() {}
}
async fn early_message_before_hosting(accepted: bool) {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone()); let line = manager.create(account_config(&cert.root, port, false), 100).await.unwrap().account.line_id;
    let database = temp.database.clone(); let observed_line = line.clone(); let (done, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        peer.send(&[reply(login.call_id, &login_result()), direct(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(), b"MessageReceiver", None)]).await;
        loop {
            let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0, "the unconfirmed message cannot be ACKed"); let swap = decode_request(&packet[1..]).unwrap();
            assert_eq!(swap.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes()); let mut r = Reader::versioned(swap.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap(); let payload = n::rpc_batch::read_payload(&mut r).unwrap();
            if payload.requests.is_empty() { peer.send(&[reply(swap.call_id, &super::super::super::tests::swap_result(None, true, payload.fields))]).await; continue; }
            tokio::time::sleep(Duration::from_millis(80)).await;
            assert!(database.get_sms_messages_for_channel(10, 0, None, Some(&observed_line)).unwrap().is_empty());
            let seq = *payload.requests.keys().next().unwrap();
            peer.send(&[reply(swap.call_id, &super::super::super::tests::swap_result(Some(seq), accepted, payload.fields))]).await; break;
        }
        if accepted {
            assert_eq!(next_reply(&mut peer, 1).await, [0, 1, 0, 0, 0, 0]);
            assert_eq!(database.get_sms_messages_for_channel(10, 0, None, Some(&observed_line)).unwrap().len(), 1);
            done.send(()).unwrap(); finish_account(&mut peer).await;
        } else {
            while let Some(packet) = peer.next().await { assert_ne!(packet[0], 1, "rejected hosting must not ACK the early message"); }
            done.send(()).unwrap();
        }
    });
    manager.start(&line, 1, 100).await.unwrap(); tokio::time::timeout(Duration::from_secs(6), checked).await.unwrap().unwrap();
    assert_eq!(manager.messages(&line, 10).unwrap().len(), usize::from(accepted));
    let view = manager.get(&line, 100).await.unwrap(); assert_eq!(view.incoming_message_ready, accepted);
    assert!(!manager.stop(&line, 1, 100).await.unwrap().io_cleanup_required); serving.await.unwrap();
}
#[tokio::test]
async fn early_native_sms_waits_for_original_hosting_ack_without_new_deadline_or_persistence() { early_message_before_hosting(true).await; }
#[tokio::test]
async fn rejected_hosting_drops_early_native_sms_without_ack_or_history() { early_message_before_hosting(false).await; }

#[tokio::test]
async fn native_account_direct_and_batch_sms_commit_before_their_own_ack_without_fabricated_domain() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port();
    let server = cert.server_config(&TLS13); let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
    let config = account_config(&cert.root, port, false); assert!(config.profile.incoming.is_none());
    let created = manager.create(config, 100).await.unwrap(); assert!(!created.incoming_message_ready); let line = created.account.line_id;
    let database = temp.database.clone(); let server_line = line.clone(); let (acked, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        assert_eq!(login.method, n::rpc_login::LOGIN_SESSION_METHOD.as_bytes()); peer.send(&[reply(login.call_id, &login_result())]).await;
        let (_, flag) = hosting_call(&mut peer).await; assert_eq!(flag, b"4");
        peer.send(&[direct(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(), b"MessageReceiver", None)]).await;
        assert_eq!(next_reply(&mut peer, 1).await, [0, 1, 0, 0, 0, 0]);
        assert_eq!(database.get_sms_messages_for_channel(10, 0, None, Some(&server_line)).unwrap().len(), 1);
        let packet = peer.next().await.unwrap(); let swap = decode_request(&packet[1..]).unwrap(); let mut r = Reader::versioned(swap.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap(); let payload = n::rpc_batch::read_payload(&mut r).unwrap();
        let mut w = Writer::new(); w.boolean(true).unwrap(); for word in [9, 1, 1, payload.fields[0], payload.fields[1], payload.fields[2]] { w.u32(word).unwrap(); }
        w.u32(1).unwrap(); w.i32(1).unwrap(); w.bytes(n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap(); w.bytes(&sms_args()).unwrap(); w.properties(&[RpcProperty { key: b"id", value: b"MessageReceiver" }]).unwrap();
        w.u32(0).unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); peer.send(&[reply(swap.call_id, w.finish().expose_to_transport())]).await;
        loop { let packet = peer.next().await.unwrap(); let swap = decode_request(&packet[1..]).unwrap(); let mut r = Reader::versioned(swap.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap(); let payload = n::rpc_batch::read_payload(&mut r).unwrap();
            let ack = payload.results.get(&1); if let Some(ack) = ack { assert_eq!(ack.result_stream, [0, 1, 0, 0, 0, 0]); assert_eq!(database.get_sms_messages_for_channel(10, 0, None, Some(&server_line)).unwrap().len(), 2); }
            peer.send(&[reply(swap.call_id, &super::super::super::tests::swap_result(None, true, payload.fields))]).await; if ack.is_some() { break; }
        }
        acked.send(()).unwrap(); finish_account(&mut peer).await;
    });
    manager.start(&line, 1, 100).await.unwrap(); tokio::time::timeout(Duration::from_secs(6), checked).await.unwrap().unwrap();
    let view = manager.get(&line, 100).await.unwrap(); assert_eq!(view.phase, "hosted"); assert!(view.incoming_message_ready); assert!(!view.incoming_call_ready && !view.outbound_authorized);
    let rows = manager.messages(&line, 10).unwrap(); assert_eq!(rows.len(), 2); assert!(rows.iter().all(|row| row.message.content == "native 短信"));
    let saved: i64 = temp.database.with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM app_voice_inbox WHERE line_id=?1 AND receiver_object=?2 AND receiver_kind=?3 AND receiver_domain IS NULL", rusqlite::params![line, b"MessageReceiver".as_slice(), b"Sip.Request".as_slice()], |row| row.get(0)).map_err(|_| "test_query")).unwrap();
    assert_eq!(saved, 2, "native namespace keeps NULL absence, never a forged legacy domain");
    let stopped = manager.stop(&line, 1, 100).await.unwrap(); assert!(!stopped.io_cleanup_required); serving.await.unwrap();
    assert_eq!(manager.messages(&line, 10).unwrap().len(), 2); assert_eq!(manager.get(&line, 100).await.unwrap().phase, "stopped");
}
#[tokio::test]
async fn native_offline_account_persists_cursor_and_sms_on_same_namespace_before_next_page() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone()); let line = manager.create(account_config(&cert.root, port, true), 100).await.unwrap().account.line_id;
    let observed = temp.database.clone(); let server_line = line.clone(); let (done, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap(); peer.send(&[reply(login.call_id, &login_result())]).await;
        let (_, flag) = hosting_call(&mut peer).await; assert_eq!(flag, b"4");
        for page in 0..2 { loop {
            let packet = peer.next().await.unwrap(); let request = decode_request(&packet[1..]).unwrap();
            assert_eq!(request.metadata.iter().find(|field| field.key == b"token").unwrap().value, b"fixture-auth");
            let mut r = Reader::versioned(request.arguments).unwrap(); assert_eq!(r.bytes().unwrap(), b"fixture-user"); assert_eq!(r.bytes().unwrap(), b"fixture-original-selector");
            if request.method == n::rpc_offline::GET_OFFLINE_METHOD.as_bytes() {
                assert_eq!(r.i64().unwrap(), if page == 0 { -1 } else { 8 }); assert_eq!(r.u32().unwrap(), 100); r.finish().unwrap();
                if page == 1 { assert_eq!(observed.get_sms_messages_for_channel(10, 0, None, Some(&server_line)).unwrap().len(), 1); }
                let mut nested = Writer::new(); nested.bytes(b"Sip.Request").unwrap(); nested.properties(&[RpcProperty { key: b"Sip.Message.From", value: b"offline-sender" }]).unwrap(); nested.bytes(format!("offline-{page}").as_bytes()).unwrap();
                let mut response = Writer::new(); response.boolean(true).unwrap(); response.u32(1).unwrap(); response.i64(7 + page).unwrap(); response.bytes(nested.finish().expose_to_transport()).unwrap(); response.i64(8).unwrap(); response.bytes(b"").unwrap();
                peer.send(&[reply(request.call_id, response.finish().expose_to_transport())]).await; break;
            }
            assert_eq!(request.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes()); let body = n::rpc_batch::read_payload(&mut r).unwrap(); peer.send(&[reply(request.call_id, &super::super::super::tests::swap_result(None, true, body.fields))]).await;
        } }
        done.send(()).unwrap(); finish_account(&mut peer).await;
    });
    manager.start(&line, 1, 100).await.unwrap(); checked.await.unwrap(); until(|| manager.messages(&line, 10).unwrap().len() == 2).await;
    let cursor: i64 = temp.database.with_conn(|conn| conn.query_row("SELECT next_position FROM app_voice_offline_cursors WHERE line_id=?1", [&line], |row| row.get(0)).map_err(|_| "test_cursor")).unwrap(); assert_eq!(cursor, 9);
    let rows = manager.messages(&line, 10).unwrap(); assert!(rows.iter().all(|row| row.timestamp_source == "received"));
    assert!(manager.get(&line, 100).await.unwrap().incoming_message_ready);
    assert!(!manager.stop(&line, 1, 100).await.unwrap().io_cleanup_required); serving.await.unwrap();
    assert_eq!(manager.messages(&line, 10).unwrap().len(), 2);
}
