//! Actions-only 原 Router/SARC/Common 通路；不回退 innerTLS 或改变物理信任。
use super::*;
#[tokio::test]
async fn original_common_actor_fetches_a_correlated_offline_page_without_new_scope_or_tls_inference() {
    let cert = Certificates::generate("IP:127.0.0.1"); let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
    let serving = tokio::spawn(async move {
        let mut peer = CommonPeer::open(listener, server).await;
        let packet = peer.next().await.unwrap(); let login = decode_request(&packet[1..]).unwrap();
        assert_eq!(login.method, b"loginSession.AccountEntry.Account");
        peer.send(&[reply(login.call_id, &login_result())]).await;
        loop {
            let packet = peer.next().await.unwrap(); assert_eq!(packet[0], 0);
            let request = decode_request(&packet[1..]).unwrap();
            assert_eq!(request.metadata.iter().find(|field| field.key == b"id").unwrap().value, b"AccountEntry");
            let mut reader = Reader::versioned(request.arguments).unwrap();
            assert_eq!(reader.bytes().unwrap(), b"fixture-user"); assert_eq!(reader.bytes().unwrap(), b"fixture-selected-session");
            if request.method == super::super::super::rpc_offline::GET_OFFLINE_METHOD.as_bytes() {
                assert!(!request.metadata.iter().any(|field| field.key == b"pwd"));
                assert_eq!(reader.i64().unwrap(), -1); assert_eq!(reader.u32().unwrap(), 100); reader.finish().unwrap();
                let mut nested = Writer::new(); nested.bytes(b"fixture.kind").unwrap(); nested.properties(&[]).unwrap(); nested.bytes(&[0xff, 0, 3]).unwrap();
                let nested = nested.finish(); let mut result = Writer::new(); result.boolean(true).unwrap(); result.u32(1).unwrap();
                result.i64(0x1_0000_0001).unwrap(); result.bytes(nested.expose_to_transport()).unwrap(); result.i64(0x1_0000_0001).unwrap(); result.bytes(b"").unwrap();
                peer.send(&[reply(request.call_id, result.finish().expose_to_transport())]).await; break;
            }
            assert_eq!(request.method, b"swapCalls.AccountEntry.Account");
            let payload = super::super::super::rpc_batch::read_payload(&mut reader).unwrap();
            peer.send(&[reply(request.call_id, &super::super::tests::swap_result(None, true, payload.fields))]).await;
        }
        while peer.next().await.is_some() {}
    });
    let binding = super::super::super::rpc_incoming::IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"fixture.kind" };
    let profile = RuntimeProfile::supplied_common(RuntimeProfileInputs { incoming: Some(binding), ..inputs() }, common(&cert.root, port)).unwrap()
        .with_offline_reception(true).unwrap();
    let connector = Arc::new(LegacyDenied { calls: AtomicUsize::new(0) });
    let (backend, mut events) = NativeHostingBackend::new(profile, connector.clone()); let original = owner();
    backend.login(original.clone(), request(&cert.root)).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    assert!(matches!(event.payload, BackendEventPayload::Callback(callback) if callback.callback == (Callback::SdkLogin { success: true })));
    let scope = backend.original_scope(&original).unwrap(); backend.pause_original_polls_for_test(&original, &scope, true).unwrap();
    let receipt = backend.get_offline(&original, &scope, -1).await.unwrap(); assert!(receipt.matches(&original, &scope));
    let event = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    let BackendEventPayload::OfflinePage(page) = event.payload else { panic!("expected correlated offline page"); };
    assert!(page.matches(&original, &scope)); assert_eq!(Some(page.call_id()), receipt.call_id);
    assert_eq!(page.result().unwrap().records[0].position, 0x1_0000_0001);
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0); // 没有直接 TLS 连接器后备。
    backend.retire(&original); backend.logout(original).await.unwrap(); assert!(!scope.is_open());
    tokio::time::timeout(Duration::from_secs(4), serving).await.unwrap().unwrap();
}
