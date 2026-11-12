//! Actions-only actual account/SQLite/receiver/cleanup/supervisor integration.
use super::*;
use super::super::super::{account_config::{AccountConfig, ReconnectPolicy}, rpc_client_ex::NOTIFY_DISCONNECT_METHOD,
    rpc_envelope::{encode_request, decode_reply}, rpc_stream::RpcProperty, smgw_text::SMGW_RECEIVER, transport_packets::CallKind};

fn config_ex(root: &str) -> AccountConfig {
    let mut config = config(root); config.profile.client_ex = true;
    config.profile.incoming.as_mut().unwrap().receiver_kind = SMGW_RECEIVER.to_vec(); config
}
async fn wait_view<F: Fn(&AccountView) -> bool>(manager: &AccountManager, line: &str, wanted: F) -> AccountView {
    tokio::time::timeout(Duration::from_secs(40), async {
        loop { let view = manager.get(line, unix_now()).await.unwrap(); if wanted(&view) { return view; }
            tokio::time::sleep(Duration::from_millis(10)).await; }
    }).await.unwrap()
}
async fn disconnect(peer: &mut super::super::super::runtime_backend::tests::Peer, id: i32, token: &[u8]) {
    let mut writer = Writer::versioned().unwrap(); writer.bytes(b"another-device-logined:fixture-private").unwrap();
    let magic = id.to_be_bytes();
    let request = encode_request(id, &[RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &magic }, RpcProperty { key: b"token", value: token }],
        NOTIFY_DISCONNECT_METHOD.as_bytes(), writer.finish().expose_to_transport()).unwrap();
    peer.send_call(CallKind::Request, request.expose_to_transport()).await;
}
async fn finish(mut peer: super::super::super::runtime_backend::tests::Peer) {
    peer.swap(Some(STATUS_UNREGISTER), true).await; let message = next_non_poll(&mut peer).await;
    let request = decode_request(message.payload.expose_to_transport()).unwrap(); assert_eq!(request.method, b"logoutSession.AccountEntry.Account");
    peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
}
#[tokio::test]
async fn matched_kick_cleans_original_run_retains_committed_sms_and_never_reconnects_into_spare_device_session() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default());
    let (mut first, connector, dropped) = peer(&cert, &factory); let (_spare, spare_connector, _) = peer(&cert, &factory);
    let manager = Arc::new(AccountManager::with_factory(temp.database.clone(), factory.clone())); manager.enable_reconnect_supervisor();
    let mut config = config_ex(&cert.root); config.cleanup_on_disconnect = true;
    config.reconnect = Some(ReconnectPolicy { max_attempts: 3, initial_delay_ms: 1000, max_delay_ms: 2000 });
    let line = manager.create(config, 100).await.unwrap().account.line_id;
    let (stored, acked) = oneshot::channel(); let (kick, kicking) = oneshot::channel();
    let observed = temp.database.clone(); let observed_line = line.clone();
    let serving = tokio::spawn(async move {
        first.authenticate().await; first.swap(Some(STATUS_REGISTER), true).await;
        first.incoming_with_fields(1, SMGW_RECEIVER, "fixture 已提交短信\n保留历史".as_bytes(), Some(b"fixture-sender"), None).await;
        let message = next_non_poll(&mut first).await; let reply = decode_reply(message.payload.expose_to_transport()).unwrap();
        assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]);
        let rows = observed.get_sms_messages_for_channel(10, 0, None, Some(&observed_line)).unwrap();
        assert_eq!(rows.len(), 1); assert_eq!(rows[0].content, "fixture 已提交短信\n保留历史");
        stored.send(()).unwrap(); kicking.await.unwrap(); disconnect(&mut first, 2, b"fixture-auth").await;
        let message = next_non_poll(&mut first).await; let reply = decode_reply(message.payload.expose_to_transport()).unwrap();
        assert_eq!(reply.call_id, 2); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]); first.drain_until_eof().await;
    });
    manager.start(&line, 1, unix_now()).await.unwrap(); acked.await.unwrap();
    let original_scope = connector.captured_scope().unwrap(); let prior = manager.messages(&line, 10).unwrap();
    assert_eq!(prior.len(), 1); let prior_id = prior[0].message.id;
    assert!(manager.get(&line, unix_now()).await.unwrap().incoming_message_ready); kick.send(()).unwrap();
    let view = wait_view(&manager, &line, |view| view.reconnect_state == Some("not_retryable") && !view.io_cleanup_required).await;
    assert_eq!(view.phase, "authentication_failed"); assert_eq!(view.last_error, Some("app_account_session_revoked"));
    assert!(!view.incoming_message_ready && !view.incoming_call_ready && !view.outbound_authorized);
    assert!(original_scope.session_revoked()); assert!(!original_scope.is_open()); assert!(dropped.load(Ordering::SeqCst));
    let after = manager.messages(&line, 10).unwrap(); assert_eq!(after.len(), 1); assert_eq!(after[0].message.id, prior_id);
    assert_eq!(after[0].message.content, "fixture 已提交短信\n保留历史");
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
    assert!(!serde_json::to_string(&view).unwrap().contains("fixture-private"));
    serving.await.unwrap(); tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(factory.build_count(), 1); assert_eq!(connector.connection_calls(), 1); assert_eq!(spare_connector.connection_calls(), 0);
    assert!(!manager.stop(&line, 1, unix_now()).await.unwrap().io_cleanup_required);
}
#[tokio::test]
async fn wrong_token_control_retains_hosted_receiver_until_explicit_unregister_logout_and_join() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default());
    let (mut peer, connector, dropped) = peer(&cert, &factory); let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(config_ex(&cert.root), 100).await.unwrap().account.line_id;
    let (done, checked) = oneshot::channel(); let (stop, stopping) = oneshot::channel();
    let serving = tokio::spawn(async move {
        peer.authenticate().await; peer.swap(Some(STATUS_REGISTER), true).await; disconnect(&mut peer, 1, b"fixture-original-pwd").await;
        let message = next_non_poll(&mut peer).await; let reply = decode_reply(message.payload.expose_to_transport()).unwrap();
        assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 0, 0, 0, 0, 0]); done.send(()).unwrap();
        stopping.await.unwrap(); finish(peer).await;
    });
    manager.start(&line, 1, unix_now()).await.unwrap(); checked.await.unwrap();
    let view = manager.get(&line, unix_now()).await.unwrap(); assert_eq!(view.phase, "hosted"); assert!(view.incoming_message_ready);
    assert!(view.last_error.is_none()); assert!(manager.messages(&line, 10).unwrap().is_empty());
    assert!(manager.inbox(&line, 10).unwrap().is_empty()); let scope = connector.captured_scope().unwrap(); assert!(!scope.session_revoked());
    stop.send(()).unwrap(); let stopped = manager.stop(&line, 1, unix_now()).await.unwrap();
    assert!(!stopped.io_cleanup_required); assert!(!scope.is_open()); assert!(dropped.load(Ordering::SeqCst));
    serving.await.unwrap(); assert_eq!(factory.build_count(), 1); assert_eq!(connector.connection_calls(), 1);
}
