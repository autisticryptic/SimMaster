//! Actions-only original Router/SARC/Common ClientEx; no innerTLS fallback.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::rpc_client_ex::{CHECK_ALIVE_METHOD, NOTIFY_DISCONNECT_METHOD};
#[path = "runtime_notification_rejection_tests.rs"]
mod notification_rejection;
fn client_ex_packet(method: &str, token: Option<&[u8]>) -> Vec<u8> {
    let mut writer = Writer::versioned().unwrap(); if method == NOTIFY_DISCONNECT_METHOD { writer.bytes(b"another-device-logined:fixture-private").unwrap(); }
    let mut metadata = vec![RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &[0, 0xff, 7] }];
    if let Some(value) = token { metadata.push(RpcProperty { key: b"token", value }); }
    let request = encode_request(1, &metadata, method.as_bytes(), writer.finish().expose_to_transport()).unwrap();
    let mut packet = vec![0]; packet.extend_from_slice(request.expose_to_transport()); packet
}
async fn next_control_reply(peer: &mut CommonPeer) -> Vec<u8> {
    loop {
        let packet = peer.next().await.unwrap();
        if packet[0] == 1 { let result = decode_reply(&packet[1..]).unwrap(); assert_eq!(result.call_id, 1); return result.result_stream.to_vec(); }
        assert_eq!(packet[0], 0); let request = decode_request(&packet[1..]).unwrap();
        assert_eq!(request.method, b"swapCalls.AccountEntry.Account");
        assert_eq!(request.metadata.iter().find(|field| field.key == b"id").unwrap().value, b"AccountEntry");
        assert_eq!(request.metadata.iter().find(|field| field.key == b"token").unwrap().value, b"fixture-auth");
        let mut reader = Reader::versioned(request.arguments).unwrap(); assert_eq!(reader.bytes().unwrap(), b"fixture-user");
        assert_eq!(reader.bytes().unwrap(), b"fixture-selected-session");
        let payload = super::super::super::rpc_batch::read_payload(&mut reader).unwrap();
        peer.send(&[reply(request.call_id, &super::super::tests::swap_result(None, true, payload.fields))]).await;
    }
}
fn profile_ex(root: &str, port: u16) -> RuntimeProfile {
    let binding = super::super::super::rpc_incoming::IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"fixture.kind" };
    RuntimeProfile::supplied_common(RuntimeProfileInputs { incoming: Some(binding), ..inputs() }, common(root, port)).unwrap().with_client_ex(true).unwrap()
}
#[tokio::test]
async fn native_common_login_coalesced_check_alive_is_false_without_account_alive_or_legacy_connector() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13); let (done, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        assert_eq!(login.method, b"loginSession.AccountEntry.Account");
        peer.send(&[reply(login.call_id, &login_result()), client_ex_packet(CHECK_ALIVE_METHOD, None)]).await;
        assert_eq!(next_control_reply(&mut peer).await, vec![0; 10]); done.send(()).unwrap(); while peer.next().await.is_some() {}
    });
    let connector = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) }); let (backend, mut events) = NativeHostingBackend::new(profile_ex(&cert.root, port), connector.clone());
    let original = owner(); backend.login(original.clone(), request(&cert.root)).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    assert!(matches!(event.payload, BackendEventPayload::Callback(callback) if callback.callback == (Callback::SdkLogin { success: true })));
    checked.await.unwrap(); let scope = backend.original_scope(&original).unwrap(); assert!(!scope.session_revoked()); assert!(scope.is_open());
    assert!(events.try_recv().is_err()); assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    backend.retire(&original); backend.logout(original).await.unwrap(); assert!(!scope.is_open());
    tokio::time::timeout(Duration::from_secs(4), serving).await.unwrap().unwrap();
}
#[tokio::test]
async fn native_common_matched_token_kick_keeps_original_scope_and_never_opens_a_fallback_session() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        assert_eq!(login.method, b"loginSession.AccountEntry.Account");
        peer.send(&[reply(login.call_id, &login_result()), client_ex_packet(NOTIFY_DISCONNECT_METHOD, Some(b"fixture-auth"))]).await;
        assert_eq!(next_control_reply(&mut peer).await, [0, 1, 0, 0, 0, 0]); while peer.next().await.is_some() {}
    });
    let connector = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) }); let (backend, mut events) = NativeHostingBackend::new(profile_ex(&cert.root, port), connector.clone());
    let original = owner(); let started = backend.login(original.clone(), request(&cert.root)).await;
    assert!(started == Ok(()) || started == Err(BackendFailure::SessionRevoked));
    let terminal = tokio::time::timeout(Duration::from_secs(4), async {
        loop { let event = events.recv().await.unwrap(); if matches!(event.payload, BackendEventPayload::Terminal(_)) { break event; } }
    }).await.unwrap();
    assert!(matches!(terminal.payload, BackendEventPayload::Terminal(BackendFailure::SessionRevoked)));
    let scope = backend.original_scope(&original).unwrap(); assert!(scope.session_revoked()); assert!(!scope.is_open());
    assert_eq!(backend.update_status(original.clone(), STATUS_REGISTER, gateway()).await, Err(BackendFailure::SessionRevoked));
    backend.logout(original).await.unwrap(); assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    tokio::time::timeout(Duration::from_secs(4), serving).await.unwrap().unwrap();
}
