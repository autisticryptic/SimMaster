//! Actions-only real Router/TLS/Channel/Common adapter delivery. No device/API service.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::native_receiver::{NativeReceiverConfig, DirectDomainConfig, DomainMode};
#[path = "native_receiver_account_tests.rs"]
mod account;
#[path = "native_call_runtime_tests.rs"]
mod default_calls;
fn config() -> NativeReceiverConfig { NativeReceiverConfig { direct_domains: None } }
fn native_profile(root: &str, port: u16, filter: &NativeReceiverConfig) -> RuntimeProfile {
    RuntimeProfile::supplied_common(inputs(), common(root, port)).unwrap().with_native_receivers(Some(filter)).unwrap().with_client_ex(true).unwrap()
}
fn sms_args() -> Vec<u8> { let mut w = Writer::versioned().unwrap(); w.bytes(b"Sip.Request").unwrap();
    w.properties(&[RpcProperty { key: b"Sip.Message.From", value: b"native-fixture-sender" }]).unwrap(); w.bytes("native 短信".as_bytes()).unwrap(); w.finish().expose_to_transport().to_vec() }
fn direct(id: i32, method: &[u8], arguments: &[u8], object: &[u8], domain: Option<&[u8]>) -> Vec<u8> {
    let magic = id.to_be_bytes(); let mut meta = vec![RpcProperty { key: b"id", value: object }, RpcProperty { key: b"__magic", value: &magic }];
    if let Some(value) = domain { meta.push(RpcProperty { key: b"domain", value }); }
    let rpc = encode_request(id, &meta, method, arguments).unwrap(); let mut p = vec![0]; p.extend_from_slice(rpc.expose_to_transport()); p
}
async fn next_reply(peer: &mut CommonPeer, expected: i32) -> Vec<u8> {
    next_reply_for_selector(peer, expected, b"fixture-selected-session").await
}
async fn next_reply_for_selector(peer: &mut CommonPeer, expected: i32, selector: &[u8]) -> Vec<u8> {
    loop {
        let packet = peer.next().await.unwrap();
        if packet[0] == 1 { let decoded = decode_reply(&packet[1..]).unwrap(); assert_eq!(decoded.call_id, expected); return decoded.result_stream.to_vec(); }
        let request = decode_request(&packet[1..]).unwrap(); assert_eq!(request.method, n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut r = Reader::versioned(request.arguments).unwrap(); assert_eq!(r.bytes().unwrap(), b"fixture-user"); assert_eq!(r.bytes().unwrap(), selector);
        let payload = n::rpc_batch::read_payload(&mut r).unwrap();
        peer.send(&[reply(request.call_id, &super::super::tests::swap_result(None, true, payload.fields))]).await;
    }
}
async fn event(events: &mut tokio::sync::mpsc::Receiver<BackendEvent>) -> BackendEvent { tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap() }
#[tokio::test]
async fn native_default_direct_message_coalesced_with_login_uses_real_common_and_retains_consumer_ticket() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port();
    let server = cert.server_config(&TLS13); let (acked, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        peer.send(&[reply(login.call_id, &login_result()), direct(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(), b"MessageReceiver", None)]).await;
        assert_eq!(next_reply(&mut peer, 1).await, [0, 1, 0, 0, 0, 0]); acked.send(()).unwrap(); while peer.next().await.is_some() {}
    });
    let legacy = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) }); let (backend, mut events) = NativeHostingBackend::new(native_profile(&cert.root, port, &config()), legacy.clone());
    let original = owner(); backend.login(original.clone(), request(&cert.root)).await.unwrap();
    let login = event(&mut events).await; assert!(matches!(login.payload, BackendEventPayload::Callback(callback) if callback.callback == (Callback::SdkLogin { success: true })));
    let scope = backend.original_scope(&original).unwrap(); let message = event(&mut events).await;
    let BackendEventPayload::DirectMessage(message) = message.payload else { panic!("native SMS delivery"); };
    assert_eq!(message.message().body, "native 短信".as_bytes()); assert!(message.reply_ticket().matches(&original, &scope));
    assert!(backend.original_incoming_binding(&original, &scope).is_none(), "native mode is not a fabricated legacy binding");
    assert!(backend.original_native_binding(&original, &scope).is_some()); assert!(!backend.original_call_receiver_ready(&original, &scope));
    assert!(backend.complete_direct(&original, &scope, message.reply_ticket(), true).await.unwrap().matches(&original, &scope)); checked.await.unwrap();
    backend.retire(&original); backend.logout(original).await.unwrap(); serving.await.unwrap(); assert_eq!(legacy.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn authenticated_native_batch_without_domain_keeps_direct_allowlist_separate_and_exact_result_sequence() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port();
    let server = cert.server_config(&TLS13); let (acked, checked) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap(); peer.send(&[reply(login.call_id, &login_result())]).await;
        let packet = peer.next().await.unwrap(); let swap = decode_request(&packet[1..]).unwrap(); let mut r = Reader::versioned(swap.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap();
        let payload = n::rpc_batch::read_payload(&mut r).unwrap(); let mut w = Writer::new(); w.boolean(true).unwrap(); for word in [9, 1, 1, payload.fields[0], payload.fields[1], payload.fields[2]] { w.u32(word).unwrap(); }
        w.u32(1).unwrap(); w.i32(1).unwrap(); w.bytes(n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap(); w.bytes(&sms_args()).unwrap();
        w.properties(&[RpcProperty { key: b"id", value: b"MessageReceiver" }]).unwrap(); w.u32(0).unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); peer.send(&[reply(swap.call_id, w.finish().expose_to_transport())]).await;
        loop { let packet = peer.next().await.unwrap(); let swap = decode_request(&packet[1..]).unwrap(); let mut r = Reader::versioned(swap.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap(); let payload = n::rpc_batch::read_payload(&mut r).unwrap();
            let ack = payload.results.get(&1); if let Some(ack) = ack { assert_eq!(ack.result_stream, [0, 1, 0, 0, 0, 0]); }
            peer.send(&[reply(swap.call_id, &super::super::tests::swap_result(None, true, payload.fields))]).await; if ack.is_some() { break; }
        }
        acked.send(()).unwrap(); while peer.next().await.is_some() {}
    });
    let filter = NativeReceiverConfig { direct_domains: Some(DirectDomainConfig { mode: DomainMode::AllowList, values: vec![b"only-direct-domain".to_vec()] }) };
    let (backend, mut events) = NativeHostingBackend::new(native_profile(&cert.root, port, &filter), Arc::new(LegacyDenied { calls: AtomicUsize::new(0) }));
    let original = owner(); backend.login(original.clone(), request(&cert.root)).await.unwrap(); assert!(matches!(event(&mut events).await.payload, BackendEventPayload::Callback(_)));
    let scope = backend.original_scope(&original).unwrap(); let message = event(&mut events).await;
    let BackendEventPayload::BatchMessage(message) = message.payload else { panic!("batch native SMS"); };
    assert!(message.ticket().matches(&original, &scope)); assert_eq!(message.message().body, "native 短信".as_bytes());
    backend.complete_batch(&original, &scope, message.ticket(), true).await.unwrap(); checked.await.unwrap(); backend.retire(&original); backend.logout(original).await.unwrap(); serving.await.unwrap();
}
#[tokio::test]
async fn native_direct_allowlist_rejection_closes_original_peer_without_sms_or_tls_fallback() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let (go, wait) = tokio::sync::oneshot::channel(); let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap(); peer.send(&[reply(login.call_id, &login_result())]).await;
        wait.await.unwrap(); peer.send(&[direct(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(), b"MessageReceiver", None)]).await;
        while let Some(packet) = peer.next().await { assert_ne!(packet[0], 1, "no fabricated accepted SMS reply"); }
    });
    let filter = NativeReceiverConfig { direct_domains: Some(DirectDomainConfig { mode: DomainMode::AllowList, values: vec![b"required".to_vec()] }) };
    let legacy = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) }); let (backend, mut events) = NativeHostingBackend::new(native_profile(&cert.root, port, &filter), legacy.clone());
    let original = owner(); backend.login(original.clone(), request(&cert.root)).await.unwrap(); event(&mut events).await; go.send(()).unwrap();
    assert!(matches!(event(&mut events).await.payload, BackendEventPayload::Terminal(_)));
    let scope = backend.original_scope(&original).unwrap(); assert!(!scope.is_open()); assert!(!scope.session_revoked());
    backend.logout(original).await.unwrap(); serving.await.unwrap(); assert_eq!(legacy.calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn original_native_call_notice_has_control_ticket_before_late_configuration_but_no_media_grant() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap(); let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let (acked, checked) = tokio::sync::oneshot::channel(); let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await; let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        let mut w = Writer::versioned().unwrap(); w.u64(0x1_0000_1234).unwrap(); w.bytes(b"native-peer").unwrap(); w.properties(&[]).unwrap();
        peer.send(&[reply(login.call_id, &login_result()), direct(1, n::rpc_calls::INVITED_METHOD.as_bytes(), w.finish().expose_to_transport(), b"CallClientEx", None)]).await;
        assert_eq!(next_reply(&mut peer, 1).await, [0, 1, 0, 0, 0, 0]); acked.send(()).unwrap(); while peer.next().await.is_some() {}
    });
    let p = native_profile(&cert.root, port, &config()).with_call_control(&[RpcProperty { key: b"id", value: b"#original-control" }, RpcProperty { key: b"domain", value: b"original-control-domain" }]).unwrap();
    let (backend, mut events) = NativeHostingBackend::new(p, Arc::new(LegacyDenied { calls: AtomicUsize::new(0) })); let original = owner();
    backend.login(original.clone(), request(&cert.root)).await.unwrap(); event(&mut events).await;
    let call = event(&mut events).await; let BackendEventPayload::Call(call) = call.payload else { panic!("native call notice"); };
    let scope = backend.original_scope(&original).unwrap(); assert!(call.control_ticket().is_some()); assert_eq!(call.notice().native_id(), n::rpc_calls::NativeCallId::from_bits(0x1_0000_1234));
    assert!(call.ticket().matches(&original, &scope)); backend.complete_call(&original, &scope, call.ticket(), true, &[]).await.unwrap(); checked.await.unwrap();
    backend.retire(&original); backend.logout(original).await.unwrap(); serving.await.unwrap();
}
