//! Actions-only real memory TLS/ARC controls and sticky revocation failures.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::rpc_client_ex::{CHECK_ALIVE_METHOD, NOTIFY_CALLS_METHOD, NOTIFY_DISCONNECT_METHOD};

fn control_args(method: &str, reason: &[u8]) -> Vec<u8> {
    let mut writer = Writer::versioned().unwrap();
    if method == NOTIFY_CALLS_METHOD { writer.u64(0xfeed_1234_5678_9abc).unwrap(); writer.properties(&[]).unwrap(); }
    if method == NOTIFY_DISCONNECT_METHOD { writer.bytes(reason).unwrap(); }
    writer.finish().expose_to_transport().to_vec()
}
fn control_request(id: i32, method: &str, token: Option<&[u8]>, reason: &[u8]) -> super::super::super::rpc_stream::RpcBytes {
    let magic = id.to_be_bytes();
    let mut properties = vec![RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &magic }];
    if let Some(value) = token { properties.push(RpcProperty { key: b"token", value }); }
    super::super::super::rpc_envelope::encode_request(id, &properties, method.as_bytes(), &control_args(method, reason)).unwrap()
}
async fn send_control(peer: &mut Peer, id: i32, method: &str, token: Option<&[u8]>, reason: &[u8]) {
    let request = control_request(id, method, token, reason); peer.send_call(CallKind::Request, request.expose_to_transport()).await;
}
fn fixture_ex(cert: &Certificates) -> (Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>, Peer, Arc<AtomicBool>, Arc<AtomicBool>, Arc<DuplexConnector>) {
    let (unused, old, connector, peer, dropped, blocked) = fixture(cert, false); drop(unused); drop(old);
    let (backend, events) = NativeHostingBackend::new(profile().with_client_ex(true).unwrap(), connector.clone());
    (backend, events, peer, dropped, blocked, connector)
}
async fn terminal_event(events: &mut mpsc::Receiver<BackendEvent>) -> BackendEvent {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop { let event = events.recv().await.unwrap(); if matches!(event.payload, BackendEventPayload::Terminal(_)) { return event; } }
    }).await.unwrap()
}
async fn coalesced_login_and_kick(peer: &mut Peer) {
    let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, b"loginSession.AccountEntry.Account");
    let mut writer = Writer::new(); writer.boolean(true).unwrap(); writer.bytes(b"fixture-user").unwrap(); writer.bytes(b"fixture-auth").unwrap();
    writer.properties(&[]).unwrap(); writer.bytes(b"").unwrap();
    let login = encode_reply(request.call_id, 0, writer.finish().expose_to_transport()).unwrap();
    let kick = control_request(1, NOTIFY_DISCONNECT_METHOD, Some(b"fixture-auth"), b"same-device-login:fixture-private");
    for (kind, bytes) in [(CallKind::Reply, login.expose_to_transport()), (CallKind::Request, kick.expose_to_transport())] {
        for packet in encode_call_packets(kind, bytes, TLS_FRAGMENT_THRESHOLD).unwrap() {
            std::io::Write::write_all(&mut peer.server.writer(), packet.expose_to_transport()).unwrap();
        }
    }
    let mut cipher = Vec::new(); while peer.server.wants_write() { peer.server.write_tls(&mut cipher).unwrap(); }
    peer.send_cipher(&cipher).await;
}

#[tokio::test]
async fn exact_memory_controls_answer_without_fake_alive_or_positive_user_events() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, mut peer, dropped, _, connector) = fixture_ex(&cert);
    let (go, start) = oneshot::channel(); let (done, checked) = oneshot::channel();
    let serving = tokio::spawn(async move {
        peer.authenticate().await; start.await.unwrap();
        for (id, method, token, expected) in [(1, CHECK_ALIVE_METHOD, None, vec![0; 10]),
            (2, NOTIFY_CALLS_METHOD, None, vec![0, 1, 0, 0, 0, 0]),
            (3, NOTIFY_DISCONNECT_METHOD, Some(b"fixture-phone".as_slice()), vec![0, 0, 0, 0, 0, 0])] {
            send_control(&mut peer, id, method, token, b"server-kick-off").await;
            let message = next_non_poll(&mut peer).await; assert_eq!(message.kind, CallKind::Reply);
            let reply = super::super::super::rpc_envelope::decode_reply(message.payload.expose_to_transport()).unwrap();
            assert_eq!(reply.call_id, id); assert_eq!(reply.result_code, 0); assert_eq!(reply.result_stream, expected);
        }
        done.send(()).unwrap(); peer.drain_until_eof().await;
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    go.send(()).unwrap(); checked.await.unwrap(); assert!(scope.is_open()); assert!(!scope.session_revoked());
    assert!(events.try_recv().is_err(), "controls do not manufacture SDK/hosting/call/message events");
    backend.retire(&original); backend.logout(original).await.unwrap(); serving.await.unwrap();
    assert!(dropped.load(Ordering::SeqCst)); assert_eq!(connector.connection_calls(), 1);
}
#[tokio::test]
async fn matched_memory_kick_is_nonretryable_and_original_io_is_joined_without_new_login() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, mut peer, dropped, _, connector) = fixture_ex(&cert);
    let (go, start) = oneshot::channel();
    let serving = tokio::spawn(async move {
        peer.authenticate().await; start.await.unwrap();
        send_control(&mut peer, 1, NOTIFY_DISCONNECT_METHOD, Some(b"fixture-auth"), b"another-device-logined:fixture-private").await;
        let message = next_non_poll(&mut peer).await; let reply = super::super::super::rpc_envelope::decode_reply(message.payload.expose_to_transport()).unwrap();
        assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]); peer.drain_until_eof().await;
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await; go.send(()).unwrap();
    let event = terminal_event(&mut events).await; assert!(event.matches(&original, &scope));
    assert!(matches!(event.payload, BackendEventPayload::Terminal(BackendFailure::SessionRevoked)));
    assert!(scope.session_revoked()); assert!(!scope.is_open());
    assert_eq!(backend.update_status(original.clone(), STATUS_REGISTER, gateway()).await, Err(BackendFailure::SessionRevoked));
    backend.logout(original).await.unwrap(); serving.await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(connector.connection_calls(), 1);
}
#[tokio::test]
async fn failed_control_reply_write_cannot_hide_the_matched_kick_as_retryable_io() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, mut peer, dropped, blocked, connector) = fixture_ex(&cert);
    let (go, start) = oneshot::channel();
    let serving = tokio::spawn(async move { peer.authenticate().await; start.await.unwrap();
        send_control(&mut peer, 1, NOTIFY_DISCONNECT_METHOD, Some(b"fixture-auth"), b"server-kick-off:fixture-private").await;
        peer.drain_until_eof().await;
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    blocked.store(true, Ordering::SeqCst); go.send(()).unwrap();
    let event = terminal_event(&mut events).await;
    assert!(matches!(event.payload, BackendEventPayload::Terminal(BackendFailure::SessionRevoked)));
    assert!(scope.session_revoked()); assert!(!scope.is_open()); backend.logout(original).await.unwrap();
    serving.await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(connector.connection_calls(), 1);
}
#[tokio::test]
async fn cancellation_while_reply_is_blocked_preserves_original_revocation_and_join() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, mut peer, dropped, blocked, _) = fixture_ex(&cert);
    let (go, start) = oneshot::channel();
    let serving = tokio::spawn(async move { peer.authenticate().await; start.await.unwrap();
        send_control(&mut peer, 1, NOTIFY_DISCONNECT_METHOD, Some(b"fixture-auth"), b"same-device-login").await;
        peer.drain_until_eof().await;
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    blocked.store(true, Ordering::SeqCst); go.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async { while !scope.session_revoked() { tokio::task::yield_now().await; } }).await.unwrap();
    backend.retire(&original); let event = terminal_event(&mut events).await;
    assert!(matches!(event.payload, BackendEventPayload::Terminal(BackendFailure::SessionRevoked)));
    backend.logout(original).await.unwrap(); serving.await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert!(scope.session_revoked());
}
#[tokio::test]
async fn coalesced_login_and_matched_kick_never_publish_a_late_sdk_success_or_open_another_session() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, mut peer, dropped, _, connector) = fixture_ex(&cert);
    let serving = tokio::spawn(async move {
        coalesced_login_and_kick(&mut peer).await;
        let message = next_non_poll(&mut peer).await; assert_eq!(message.kind, CallKind::Reply);
        let reply = super::super::super::rpc_envelope::decode_reply(message.payload.expose_to_transport()).unwrap();
        assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]); peer.drain_until_eof().await;
    });
    let original = owner(&cert.root); let started = backend.login(original.clone(), request(&cert.root)).await;
    assert!(started == Ok(()) || started == Err(BackendFailure::SessionRevoked));
    let first = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    assert!(matches!(first.payload, BackendEventPayload::Terminal(BackendFailure::SessionRevoked)), "no SDK success escapes the revoked coalesced group");
    let scope = backend.original_scope(&original).unwrap(); assert!(scope.session_revoked()); assert!(!scope.is_open());
    backend.logout(original).await.unwrap(); serving.await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(connector.connection_calls(), 1);
}
#[tokio::test]
async fn automatic_control_replies_do_not_ack_an_unconsumed_original_message() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, mut peer, dropped, _, _) = fixture_ex(&cert);
    let (go, start) = oneshot::channel(); let (done, checked) = oneshot::channel();
    let serving = tokio::spawn(async move {
        peer.authenticate().await; start.await.unwrap(); peer.incoming(1).await;
        for (id, method) in [(2, CHECK_ALIVE_METHOD), (3, NOTIFY_CALLS_METHOD)] {
            send_control(&mut peer, id, method, None, b"").await;
            let message = next_non_poll(&mut peer).await;
            let reply = super::super::super::rpc_envelope::decode_reply(message.payload.expose_to_transport()).unwrap();
            assert_eq!(reply.call_id, id, "the pending message cannot be acknowledged by a control response");
        }
        done.send(()).unwrap(); let message = next_non_poll(&mut peer).await;
        let reply = super::super::super::rpc_envelope::decode_reply(message.payload.expose_to_transport()).unwrap();
        assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]); peer.drain_until_eof().await;
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await; go.send(()).unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    let BackendEventPayload::DirectMessage(message) = event.payload else { panic!("pending original message expected"); };
    checked.await.unwrap(); assert!(message.reply_ticket().is_open());
    let receipt = backend.complete_direct(&original, &scope, message.reply_ticket(), true).await.unwrap();
    assert!(receipt.matches(&original, &scope)); assert!(receipt.direct_replay_frontier().is_some());
    backend.retire(&original); backend.logout(original).await.unwrap(); serving.await.unwrap(); assert!(dropped.load(Ordering::SeqCst));
}
#[test]
fn client_ex_builder_needs_existing_exact_binding_and_does_not_change_legacy_or_call_readiness() {
    assert!(!profile().client_ex); assert!(profile().with_client_ex(true).unwrap().client_ex);
    let client = [RpcProperty { key: b"ClientId", value: b"fixture-exact-client-id" }];
    let inputs = inputs(&client);
    assert!(RuntimeProfile::supplied(RuntimeProfileInputs { incoming: None, ..inputs }).unwrap().with_client_ex(true).is_err());
    let enabled = profile().with_client_ex(true).unwrap(); assert!(enabled.call_incoming.is_none()); assert!(enabled.call_control.is_none());
    assert!(!format!("{enabled:?}").contains("fixture"));
}
