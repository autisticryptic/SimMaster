//! Actions-only real Router/TLS/Common proof: a known negative control reply
//! must not close the original connection or acknowledge its pending message.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::rpc_client_ex::NOTIFY_CALLS_METHOD;
fn direct_packet(id: i32, method: &[u8], arguments: &[u8]) -> Vec<u8> {
    let magic = id.to_be_bytes();
    let encoded = encode_request(id, &[RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &magic }], method, arguments).unwrap();
    let mut bytes = vec![0]; bytes.extend_from_slice(encoded.expose_to_transport()); bytes
}
fn notification(command: &[u8]) -> Vec<u8> {
    let (first, second): (&[u8], &[u8]) = if command == b"Message" { (b"Notify.MsgIdx", b"Notify.Box") } else { (b"Notify.CallId", b"Notify.Caller") };
    let mut writer = Writer::versioned().unwrap(); writer.u64(0xfedc_ba98_7654_3210).unwrap();
    writer.properties(&[RpcProperty { key: b"Notify.Command", value: command }, RpcProperty { key: first, value: b"12345" }, RpcProperty { key: second, value: b"fixture-private-target" }]).unwrap();
    direct_packet(2, NOTIFY_CALLS_METHOD.as_bytes(), writer.finish().expose_to_transport())
}
fn ordinary_message() -> Vec<u8> {
    let mut writer = Writer::versioned().unwrap(); writer.bytes(b"fixture.kind").unwrap();
    writer.properties(&[RpcProperty { key: b"Sip.Message.From", value: b"fixture-sender" }]).unwrap(); writer.bytes(b"fixture-normal-message").unwrap();
    direct_packet(1, b"recvOnlineMessage.MessageReceiver.Client", writer.finish().expose_to_transport())
}
async fn next_result(peer: &mut CommonPeer, wanted: i32) -> Vec<u8> {
    loop {
        let packet = peer.next().await.unwrap();
        if packet[0] == 1 { let decoded = decode_reply(&packet[1..]).unwrap(); assert_eq!(decoded.call_id, wanted, "control must not acknowledge the pending message"); assert_eq!(decoded.result_code, 0); return decoded.result_stream.to_vec(); }
        assert_eq!(packet[0], 0); let request = decode_request(&packet[1..]).unwrap(); assert_eq!(request.method, b"swapCalls.AccountEntry.Account");
        assert_eq!(request.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"AccountEntry");
        assert_eq!(request.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
        let mut reader = Reader::versioned(request.arguments).unwrap(); assert_eq!(reader.bytes().unwrap(), b"fixture-user"); assert_eq!(reader.bytes().unwrap(), b"fixture-selected-session");
        let payload = crate::connectivity::app_voice::providers::jegotrip::rpc_batch::read_payload(&mut reader).unwrap(); reader.finish().unwrap();
        peer.send(&[reply(request.call_id, &crate::connectivity::app_voice::providers::jegotrip::runtime_backend::tests::swap_result(None, true, payload.fields))]).await;
    }
}
async fn rejected_hook_preserves_original_receiver(command: &'static [u8]) {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let (go, start) = tokio::sync::oneshot::channel(); let (done, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        assert_eq!(login.method, b"loginSession.AccountEntry.Account"); peer.send(&[reply(login.call_id, &login_result())]).await; start.await.unwrap();
        peer.send(&[ordinary_message(), notification(command)]).await;
        assert_eq!(next_result(&mut peer, 2).await, [0; 6]); done.send(()).unwrap();
        assert_eq!(next_result(&mut peer, 1).await, [0, 1, 0, 0, 0, 0]); while peer.next().await.is_some() {}
    });
    let connector = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) });
    let (backend, mut events) = NativeHostingBackend::new(profile_ex(&cert.root, port), connector.clone());
    let original = owner(); backend.login(original.clone(), request(&cert.root)).await.unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    assert!(matches!(ready.payload, BackendEventPayload::Callback(callback) if callback.callback == (Callback::SdkLogin { success: true })));
    let scope = backend.original_scope(&original).unwrap(); go.send(()).unwrap();
    let incoming = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    let BackendEventPayload::DirectMessage(message) = incoming.payload else { panic!("original message, not a manufactured hook event"); };
    assert_eq!(message.message().body, b"fixture-normal-message");
    tokio::time::timeout(Duration::from_secs(4), checked).await.unwrap().unwrap();
    assert!(scope.is_open()); assert!(!scope.session_revoked()); assert!(message.reply_ticket().is_open()); assert!(events.try_recv().is_err());
    let receipt = backend.complete_direct(&original, &scope, message.reply_ticket(), true).await.unwrap(); assert!(receipt.matches(&original, &scope));
    backend.retire(&original); backend.logout(original).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), serving).await.unwrap().unwrap(); assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn original_common_message_command_nack_keeps_session_and_pending_message_ack_separate() { rejected_hook_preserves_original_receiver(b"Message").await; }
#[tokio::test]
async fn original_common_invite_command_nack_keeps_session_without_fake_call_or_fallback() { rejected_hook_preserves_original_receiver(b"Invite").await; }
