//! SOURCE-only Actions tests for the original Group/Buddy directory READ FD.
//! Mounted under im_read_actor_tests::directory (use super::*).
//! Executed only by original Actions, not locally; no SDK/device/Provider
//! execution or real-network signoff. Directory DATA is not a receipt, cursor,
//! Unknown-WRITE resolution, SDK readiness or a permission to send.
use super::*;
use n::{group_directory_data as directory, rpc_stream::{MAX_MAP_ENTRIES, MAX_STREAM_BYTES}};

async fn directory_auth(
    handle: &RuntimeHandle, events: &mut mpsc::Receiver<RuntimeEvent>,
    original: &AttemptOwner, scope: &RpcConnectionScope, enabled: bool,
) {
    // Only the independent directory flag changes. No im_sync, receipt/send
    // opt-in, Info consumer, SQL fixture or replacement account is installed.
    let mut cfg = config(true);
    assert!(!cfg.im_messages.unwrap().receive_info);
    assert!(!cfg.im_messages.unwrap().directory_queries);
    cfg.im_messages.as_mut().unwrap().directory_queries = enabled;
    let setup = NativeReceiverSetup::capture(
        NativeReceiverPolicy::capture(&cfg).unwrap(), true, None,
    ).unwrap();
    let commands = handle.command_handle();
    commands.arm_native_receivers(original, scope, setup).await.unwrap();
    authenticated(handle, events, original, scope).await;
    commands.configure_native_receivers(original, scope, 7).await.unwrap();
}

fn input_bytes(properties: bool) -> usize { if properties { 27 } else { 24 } }

fn directory_query(
    scope: &RpcConnectionScope, properties: bool, deadline: Option<Instant>,
) -> Arc<ImQuery> {
    // Reserve on the ORIGINAL scope before copying even the three-byte group.
    // No separate reservation holder is returned: only actual query Arcs own it.
    let input = scope.reserve_native_rpc(input_bytes(properties)).unwrap();
    let query = if properties {
        ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()
    } else {
        ImQuery::relations(None, -1).unwrap()
    };
    let query = if let Some(end) = deadline {
        query.fix_directory_deadline(end).unwrap()
    } else { query };
    Arc::new(query.attach_directory_input(input).unwrap())
}

fn directory_enqueue(
    handle: &RuntimeHandle, original: &AttemptOwner, scope: &RpcConnectionScope,
    query: Arc<ImQuery>, gate: Arc<ImReceiptGate>, interest: Arc<ImReadInterest>,
) -> (
    oneshot::Receiver<Result<RuntimeReceipt, RuntimeError>>,
    oneshot::Receiver<Result<OwnedImReply, RuntimeError>>,
) {
    let (reply, data) = oneshot::channel();
    let receipt = handle.command_handle().enqueue(original, scope,
        Operation::ImRead(Box::new(ImReadWork { query, gate, interest, reply })),
    ).unwrap();
    (receipt, data)
}

fn directory_packet(packet: &[u8], id: i32, properties: bool) {
    let method = if properties { directory::PROPERTIES_METHOD } else { directory::RELATIONS_METHOD };
    // Parent's strict helper checks original #Group/token and exactly two fields.
    assert_eq!(query(packet, method), id);
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(&request.arguments[..5], &[0, 0, 1, 0, 0]);
    let mut r = Reader::versioned(request.arguments).unwrap();
    if properties {
        assert_eq!(r.bytes().unwrap(), b"1-2", "not an Org box or an endpoint");
        assert_eq!(r.u32().unwrap(), 1);
        assert_eq!(r.bytes().unwrap(), b"", "only the original empty prefix");
    } else {
        assert_eq!(r.bytes().unwrap(), b"fixture-user", "None selects matched SDK UID only");
        assert_eq!(r.i64().unwrap(), -1);
    }
    r.finish().unwrap();
}

fn relations_result() -> Vec<u8> {
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    w.boolean(false).unwrap(); // Opaque reported DATA, not a grant.
    for (label, flags, first, second) in [
        (b"opaque-first".as_slice(), 0xf123_4567, b"not-a-route\0".as_slice(), b"not-a-uid".as_slice()),
        (b"opaque-second".as_slice(), 0x8000_0000, b"Org/1-2".as_slice(), b"tcp -h foreign -p 1".as_slice()),
    ] {
        w.u32(1).unwrap();
        w.bytes(label).unwrap();
        w.u32(flags).unwrap();
        w.bytes(first).unwrap();
        w.bytes(second).unwrap();
        w.properties(&[RpcProperty { key: b"WRITE", value: b"opaque-not-permission" }]).unwrap();
    }
    w.u32(2).unwrap();
    w.bytes(b"opaque-name-a").unwrap();
    w.bytes(b"opaque-name-b").unwrap();
    w.i64(-8_589_934_593).unwrap();
    w.bytes(b"").unwrap();
    w.properties(&[RpcProperty { key: b"token", value: b"opaque-footer-not-auth" }]).unwrap();
    w.finish().expose_to_transport().to_vec()
}

fn properties_result() -> Vec<u8> {
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    w.properties(&[
        RpcProperty { key: b"P:P_MR", value: b"9223372036854775807" },
        RpcProperty { key: b"WRITE", value: b"opaque-not-permission" },
    ]).unwrap();
    w.bytes(b"opaque-refusal").unwrap(); // Footer reason makes accepted false.
    w.properties(&[RpcProperty { key: b"grant", value: b"opaque-footer" }]).unwrap();
    w.finish().expose_to_transport().to_vec()
}

async fn directory_logout(peer: &mut CommonPeer, expected_id: i32) {
    let packet = peer.next().await.unwrap();
    assert_eq!(packet[0], 0);
    let request = decode_request(&packet[1..]).unwrap();
    assert_eq!(request.method, n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    assert_eq!(request.call_id, expected_id, "no retired READ, resend, WRITE or account poll");
    assert_eq!(request.metadata.len(), 2);
    assert_eq!(request.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"AccountEntry");
    assert_eq!(request.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
    let mut r = Reader::versioned(request.arguments).unwrap();
    assert_eq!(r.bytes().unwrap(), b"fixture-user");
    assert_eq!(r.bytes().unwrap(), b"fixture-selected-session");
    r.finish().unwrap();
    peer.send(&[reply(expected_id, &[0, 1, 0, 0, 0, 0])]).await;
    assert!(peer.next().await.is_none(), "strict original FD EOF, no replay/filter");
}

fn no_directory_events(events: &mut mpsc::Receiver<RuntimeEvent>) {
    assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "directory result must not emit message, receipt or ready callbacks");
}

#[tokio::test]
async fn directory_io_original_self_relations_properties_wire_and_last_holder_charges() {
    Box::pin(async {
        assert_eq!((MAX_MAP_ENTRIES, MAX_STREAM_BYTES), (128, 131072));
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
        let server = cert.server_config(&TLS13);
        let serving = tokio::spawn(Box::pin(bounded(async move {
            let mut peer = CommonPeer::open(listener, server).await;
            peer_login(&mut peer).await;
            let packet = peer.next().await.unwrap();
            directory_packet(&packet, 2, false);
            peer.send(&[reply(2, &relations_result())]).await;
            let packet = peer.next().await.unwrap();
            directory_packet(&packet, 3, true);
            peer.send(&[reply(3, &properties_result())]).await;
            directory_logout(&mut peer, 4).await;
        })));
        let original = owner();
        let conn = connected(&route, original.clone(), Instant::now() + Duration::from_secs(6)).await;
        let scope = conn.connection_scope();
        let (mut handle, mut events) = conn.attach().unwrap();
        directory_auth(&handle, &mut events, &original, &scope, true).await;
        assert_eq!(scope.native_rpc_resources(), (0, 0));
        let commands = handle.command_handle();
        let binding = bounded(commands.original_im_binding(&original, &scope)).await.unwrap();
        assert!(binding.directory_queries());
        assert!(!binding.receives_info());
        let gate = ImReceiptGate::fresh();
        let relations_end = Instant::now() + Duration::from_secs(1);
        let relations = directory_query(&scope, false, Some(relations_end));
        let last_relations = relations.clone();
        let data = bounded(commands.read_im(&original, &scope, relations.clone(), gate.clone())).await.unwrap();
        assert!(data.matches(&original, &scope));
        assert_eq!(data.request().call_id(), 2);
        assert_eq!(data.request().deadline(), relations_end, "same fixed query/request deadline");
        assert!(Arc::ptr_eq(data.request().query_arc(), &relations));
        assert!(Arc::ptr_eq(&data.request().gate(), &gate));
        assert!(Arc::ptr_eq(data.request().binding(), &binding));
        {
            let ImReplyData::Relations(value) = data.result().unwrap() else { panic!("original relations DATA") };
            assert!(value.accepted);
            assert!(!value.reported);
            assert_eq!(directory::record_count(&value).unwrap(), 2);
            assert_eq!(value.first.len(), 1);
            assert_eq!(value.second.len(), 1);
            assert_eq!(value.first[0].label, b"opaque-first");
            assert_eq!(value.first[0].flags, 0xf123_4567);
            assert_eq!(value.first[0].first, b"not-a-route\0");
            assert_eq!(value.first[0].second, b"not-a-uid");
            assert_eq!(value.second[0].label, b"opaque-second");
            assert_eq!(value.second[0].flags, 0x8000_0000);
            assert_eq!(value.second[0].first, b"Org/1-2");
            assert_eq!(value.second[0].second, b"tcp -h foreign -p 1");
            for record in [&value.first[0], &value.second[0]] {
                assert_eq!(record.properties.len(), 1);
                assert_eq!(record.properties[0].key, b"WRITE");
                assert_eq!(record.properties[0].value, b"opaque-not-permission");
            }
            assert_eq!(value.names, [b"opaque-name-a".as_slice(), b"opaque-name-b".as_slice()]);
            assert_eq!(value.time, -8_589_934_593);
            assert!(value.footer.reason.is_empty());
            assert_eq!(value.footer.properties.len(), 1);
            assert_eq!(value.footer.properties[0].key, b"token");
            assert_eq!(value.footer.properties[0].value, b"opaque-footer-not-auth");
        }
        let held_relations = Arc::new(data);
        let last_reply_relations = held_relations.clone();
        let request_relations = held_relations.request().clone();
        let last_request_relations = request_relations.clone();
        let relations_charge = scope.native_rpc_resources().1 - 24;
        assert_eq!(scope.native_rpc_resources().0, 2);
        assert!(relations_charge > relations_result().len());

        let properties_end = Instant::now() + Duration::from_secs(1);
        let properties = directory_query(&scope, true, Some(properties_end));
        let last_properties = properties.clone();
        let data = bounded(commands.read_im(&original, &scope, properties.clone(), gate.clone())).await.unwrap();
        assert!(data.matches(&original, &scope));
        assert_eq!(data.request().call_id(), 3);
        assert_eq!(data.request().deadline(), properties_end);
        assert!(Arc::ptr_eq(data.request().query_arc(), &properties));
        {
            let ImReplyData::GroupProperties(value) = data.result().unwrap() else { panic!("original properties DATA") };
            assert!(!value.accepted, "true plus refusal footer is not a positive ACK");
            assert_eq!(value.properties.len(), 2);
            // Original map ordering is key length first: WRITE(5) precedes P:P_MR(6).
            assert_eq!(value.properties[0].key, b"WRITE");
            assert_eq!(value.properties[0].value, b"opaque-not-permission");
            assert_eq!(value.properties[1].key, b"P:P_MR");
            assert_eq!(value.properties[1].value, b"9223372036854775807");
            assert_eq!(value.footer.reason, b"opaque-refusal");
            assert_eq!(value.footer.properties.len(), 1);
            assert_eq!(value.footer.properties[0].key, b"grant");
            assert_eq!(value.footer.properties[0].value, b"opaque-footer");
        }
        let held_properties = Arc::new(data);
        let last_reply_properties = held_properties.clone();
        let request_properties = held_properties.request().clone();
        let last_request_properties = request_properties.clone();
        let charged = scope.native_rpc_resources();
        assert_eq!(charged.0, 4, "two query inputs and two owned native request/replies");
        assert!(charged.1 > 51 + relations_result().len() + properties_result().len());
        assert_eq!(bounded(commands.read_im(&original, &scope,
            Arc::new(ImQuery::set_received(ImBox::parse(b"Org/1-2").unwrap(), 7).unwrap()),
            gate.clone())).await.err(), Some(RuntimeError::InvalidConfiguration));
        no_directory_events(&mut events);
        let joined = bounded(handle.logout(&original, &scope, &[])).await.unwrap();
        assert!(joined.io_released());
        assert_eq!(joined.reason, RuntimeError::LoggedOut);
        assert!(handle.task.is_none(), "original IO Join consumed, not detached");
        bounded(serving).await.unwrap();
        assert!(!scope.is_open());
        assert!(!held_relations.matches(&original, &scope));
        assert!(held_relations.result().is_err());
        assert!(held_properties.result().is_err());
        assert!(bounded(events.recv()).await.is_none());
        assert_eq!(scope.native_rpc_resources(), charged, "logout and both actual Joins cannot reset budgets");
        gate.retire();
        drop(held_relations); drop(held_properties);
        assert_eq!(scope.native_rpc_resources(), charged);
        drop(last_reply_relations); drop(last_reply_properties);
        assert_eq!(scope.native_rpc_resources(), charged, "actual request Arcs still retain reply charges");
        drop(request_relations); drop(request_properties);
        assert_eq!(scope.native_rpc_resources(), charged);
        drop(last_request_relations);
        assert_eq!(scope.native_rpc_resources(), (3, charged.1 - relations_charge));
        drop(last_request_properties);
        assert_eq!(scope.native_rpc_resources(), (2, 51));
        drop(relations); drop(properties);
        assert_eq!(scope.native_rpc_resources(), (2, 51));
        drop(last_relations);
        assert_eq!(scope.native_rpc_resources(), (1, 27));
        drop(last_properties);
        assert_eq!(scope.native_rpc_resources(), (0, 0));
    }).await;
}

#[tokio::test]
async fn directory_io_queued_retirement_cancel_or_disabled_policy_precedes_id_and_packets() {
    Box::pin(async {
        let defaults: NativeImConfig = serde_json::from_str(r#"{"notification_timeout_ms":3000}"#).unwrap();
        assert!(!defaults.directory_queries && !defaults.receive_info);
        for mode in 0..3 {
            let cert = Certificates::generate("IP:127.0.0.1");
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
            let server = cert.server_config(&TLS13);
            let serving = tokio::spawn(Box::pin(bounded(async move {
                let mut peer = CommonPeer::open(listener, server).await;
                peer_login(&mut peer).await;
                directory_logout(&mut peer, 2).await;
            })));
            let original = owner();
            let conn = connected(&route, original.clone(), Instant::now() + Duration::from_secs(6)).await;
            let scope = conn.connection_scope();
            let (mut handle, mut events) = conn.attach().unwrap();
            directory_auth(&handle, &mut events, &original, &scope, mode != 0).await;
            let query = directory_query(&scope, mode == 1, None);
            let last_query = query.clone();
            let bytes = input_bytes(mode == 1);
            let gate = ImReceiptGate::fresh();
            let interest = ImReadInterest::original(&scope);
            let (receipt, data) = directory_enqueue(&handle, &original, &scope,
                query.clone(), gate.clone(), interest.clone());
            // Default current-thread runtime: no suspension between admission
            // and queued retirement/cancellation; actor cannot begin the frame.
            if mode == 1 { gate.retire(); }
            if mode == 2 { interest.cancel(); }
            let expected = if mode == 0 { RuntimeError::InvalidConfiguration } else { RuntimeError::Cancelled };
            assert_eq!(bounded(receipt).await.unwrap().err(), Some(expected));
            assert_eq!(bounded(data).await.unwrap().err(), Some(expected));
            assert!(!interest.is_writing());
            if mode != 0 { assert!(interest.is_retired()); }
            assert_eq!(scope.native_rpc_resources(), (1, bytes), "no native pending/reply admission");
            assert!(scope.is_open());
            assert!(!scope.session_revoked());
            no_directory_events(&mut events);
            let joined = bounded(handle.logout(&original, &scope, &[])).await.unwrap();
            assert!(joined.io_released());
            assert_eq!(joined.reason, RuntimeError::LoggedOut);
            assert!(handle.task.is_none());
            bounded(serving).await.unwrap();
            assert_eq!(scope.native_rpc_resources(), (1, bytes));
            drop(query);
            assert_eq!(scope.native_rpc_resources(), (1, bytes), "only the actual last external query owns input");
            drop(last_query);
            assert_eq!(scope.native_rpc_resources(), (0, 0));
        }
    }).await;
}

#[tokio::test]
async fn directory_io_writing_cancel_at_original_gate_closes_same_fd_and_consumes_join() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
        let server = cert.server_config(&TLS13);
        let serving = tokio::spawn(Box::pin(bounded(async move {
            let mut peer = CommonPeer::open(listener, server).await;
            peer_login(&mut peer).await;
            assert!(peer.next_input_only().await.is_none(), "no paused directory bytes, later frame or replay");
        })));
        let original = owner();
        let mut conn = connected(&route, original.clone(), Instant::now() + Duration::from_secs(6)).await;
        let blocked = Arc::new(AtomicBool::new(false));
        let pauses = n::common_runtime::tests::gate_im_read_write(conn.common.as_mut().unwrap(), blocked.clone());
        let scope = conn.connection_scope();
        let (mut handle, mut events) = conn.attach().unwrap();
        directory_auth(&handle, &mut events, &original, &scope, true).await;
        blocked.store(true, Ordering::SeqCst);
        let query = directory_query(&scope, true, None);
        let last_query = query.clone();
        let interest = ImReadInterest::original(&scope);
        let (mut receipt, data) = directory_enqueue(&handle, &original, &scope,
            query.clone(), ImReceiptGate::fresh(), interest.clone());
        // Mandatory original gate assertion, not an enlarged fixture timeout.
        assert!(tokio::time::timeout(Duration::from_millis(35), &mut receipt).await.is_err());
        assert!(interest.is_writing());
        assert!(pauses.load(Ordering::SeqCst) > 0);
        assert!(scope.is_open());
        assert_eq!(scope.native_rpc_resources().0, 2);
        assert!(scope.native_rpc_resources().1 > 27);
        interest.cancel();
        assert!(interest.is_retired());
        bounded(scope.closed()).await;
        assert_eq!(bounded(receipt).await.unwrap().err(), Some(RuntimeError::Cancelled));
        // During writing, the actor drops the original work's DATA sender;
        // do not invent a success or a fabricated DATA error variant.
        assert!(bounded(data).await.is_err());
        let joined = bounded(handle.close()).await;
        assert!(joined.io_released());
        assert_eq!(joined.reason, RuntimeError::Cancelled);
        assert!(handle.task.is_none());
        bounded(serving).await.unwrap();
        assert!(!scope.is_open());
        assert!(!scope.session_revoked());
        assert!(bounded(events.recv()).await.is_none());
        assert_eq!(scope.native_rpc_resources(), (1, 27));
        // Even unblocking after both Joins cannot produce a later write.
        blocked.store(false, Ordering::SeqCst);
        assert_eq!(handle.command_handle().read_im(&original, &scope,
            query.clone(), ImReceiptGate::fresh()).await.err(), Some(RuntimeError::Scope));
        drop(query);
        assert_eq!(scope.native_rpc_resources(), (1, 27));
        drop(last_query);
        assert_eq!(scope.native_rpc_resources(), (0, 0));
    }).await;
}

#[tokio::test]
async fn directory_io_submitted_cancel_or_parent_retirement_ignores_malformed_late_reply() {
    Box::pin(async {
        for parent in [false, true] {
            let cert = Certificates::generate("IP:127.0.0.1");
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
            let server = cert.server_config(&TLS13);
            let (seen, request_seen) = oneshot::channel();
            let (go, wait) = oneshot::channel();
            let serving = tokio::spawn(Box::pin(bounded(async move {
                let mut peer = CommonPeer::open(listener, server).await;
                peer_login(&mut peer).await;
                let packet = peer.next().await.unwrap();
                directory_packet(&packet, 2, parent);
                seen.send(()).unwrap();
                wait.await.unwrap();
                // Valid exact envelope, deliberately invalid directory body.
                // Retirement must precede decoding either directory family.
                peer.send(&[reply(2, &[0])]).await;
                directory_logout(&mut peer, 3).await;
            })));
            let original = owner();
            let conn = connected(&route, original.clone(), Instant::now() + Duration::from_secs(6)).await;
            let scope = conn.connection_scope();
            let (mut handle, mut events) = conn.attach().unwrap();
            directory_auth(&handle, &mut events, &original, &scope, true).await;
            let query = directory_query(&scope, parent, None);
            let last_query = query.clone();
            let gate = ImReceiptGate::fresh();
            let interest = ImReadInterest::original(&scope);
            let (receipt, data) = directory_enqueue(&handle, &original, &scope,
                query.clone(), gate.clone(), interest.clone());
            let submitted = bounded(receipt).await.unwrap().unwrap();
            assert!(submitted.matches(&original, &scope));
            assert_eq!(submitted.call_id, Some(2));
            bounded(request_seen).await.unwrap();
            assert!(!interest.is_writing());
            assert!(!interest.is_retired());
            assert_eq!(scope.native_rpc_resources().0, 2);
            assert!(scope.native_rpc_resources().1 > input_bytes(parent));
            if parent { gate.retire(); } else { interest.cancel(); }
            assert_eq!(bounded(data).await.unwrap().err(), Some(RuntimeError::Cancelled));
            assert!(interest.is_retired());
            assert!(scope.is_open());
            assert!(!scope.session_revoked());
            assert_eq!(scope.native_rpc_resources(), (1, input_bytes(parent)));
            no_directory_events(&mut events);
            go.send(()).unwrap(); // Actual retirement completed before late IO.
            let joined = bounded(handle.logout(&original, &scope, &[])).await.unwrap();
            assert!(joined.io_released());
            assert_eq!(joined.reason, RuntimeError::LoggedOut);
            assert!(handle.task.is_none());
            bounded(serving).await.unwrap();
            assert!(bounded(events.recv()).await.is_none());
            assert_eq!(scope.native_rpc_resources(), (1, input_bytes(parent)));
            drop(query);
            assert_eq!(scope.native_rpc_resources(), (1, input_bytes(parent)));
            drop(last_query);
            assert_eq!(scope.native_rpc_resources(), (0, 0));
        }
    }).await;
}

#[tokio::test]
async fn directory_io_fixed_preparation_deadline_clips_native_wait_without_reset_or_sdk_close() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
        let server = cert.server_config(&TLS13);
        let (seen, request_seen) = oneshot::channel();
        let (go, wait) = oneshot::channel();
        let serving = tokio::spawn(Box::pin(bounded(async move {
            let mut peer = CommonPeer::open(listener, server).await;
            peer_login(&mut peer).await;
            let packet = peer.next().await.unwrap();
            directory_packet(&packet, 2, false);
            seen.send(()).unwrap();
            wait.await.unwrap();
            peer.send(&[reply(2, &[0])]).await;
            directory_logout(&mut peer, 3).await;
        })));
        let original = owner();
        let conn = connected(&route, original.clone(), Instant::now() + Duration::from_secs(6)).await;
        let scope = conn.connection_scope();
        let (mut handle, mut events) = conn.attach().unwrap();
        directory_auth(&handle, &mut events, &original, &scope, true).await;
        assert_eq!(rpc_policy().rpc_timeout, Duration::from_secs(3));
        let prepared_at = Instant::now();
        let fixed = prepared_at + Duration::from_millis(100);
        let query = directory_query(&scope, false, Some(fixed));
        let last_query = query.clone();
        assert_eq!(query.clipped_deadline(prepared_at + Duration::from_secs(3)), fixed);
        assert_eq!(query.clipped_deadline(fixed - Duration::from_millis(25)), fixed - Duration::from_millis(25));
        assert!(ImQuery::relations(None, -1).unwrap().fix_directory_deadline(fixed).unwrap()
            .fix_directory_deadline(prepared_at + Duration::from_secs(3)).is_err(), "fix is one-shot, never an extension");
        let interest = ImReadInterest::original(&scope);
        let (receipt, data) = directory_enqueue(&handle, &original, &scope,
            query.clone(), ImReceiptGate::fresh(), interest.clone());
        let submitted = bounded(receipt).await.unwrap().unwrap();
        assert!(submitted.matches(&original, &scope));
        assert_eq!(submitted.call_id, Some(2), "actual original READ was submitted before its fixed end");
        bounded(request_seen).await.unwrap();
        assert!(!interest.is_writing());
        assert_eq!(bounded(data).await.unwrap().err(), Some(RuntimeError::Timeout));
        assert!(Instant::now() >= fixed);
        assert!(prepared_at.elapsed() < Duration::from_secs(1), "100ms preparation bound, not renewed native 3s");
        assert_eq!(query.clipped_deadline(Instant::now() + Duration::from_secs(3)), fixed);
        assert!(interest.is_retired());
        assert!(scope.is_open(), "timeout retires ONLY the READ pending ID");
        assert!(!scope.session_revoked());
        assert_eq!(scope.native_rpc_resources(), (1, 24));
        no_directory_events(&mut events);
        go.send(()).unwrap();
        let joined = bounded(handle.logout(&original, &scope, &[])).await.unwrap();
        assert!(joined.io_released());
        assert_eq!(joined.reason, RuntimeError::LoggedOut);
        assert!(handle.task.is_none());
        bounded(serving).await.unwrap();
        assert!(bounded(events.recv()).await.is_none());
        assert_eq!(scope.native_rpc_resources(), (1, 24));
        drop(query);
        assert_eq!(scope.native_rpc_resources(), (1, 24));
        drop(last_query);
        assert_eq!(scope.native_rpc_resources(), (0, 0));
    }).await;
}

#[tokio::test]
async fn directory_io_matched_session_revocation_suppresses_data_ready_write_and_joins_original_fd() {
    Box::pin(async {
        let cert = Certificates::generate("IP:127.0.0.1");
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = router_policy(&cert.root, listener.local_addr().unwrap().port());
        let server = cert.server_config(&TLS13);
        let (seen, request_seen) = oneshot::channel();
        let (go, wait) = oneshot::channel();
        let serving = tokio::spawn(Box::pin(bounded(async move {
            let mut peer = CommonPeer::open(listener, server).await;
            peer_login(&mut peer).await;
            let packet = peer.next().await.unwrap();
            directory_packet(&packet, 2, false);
            seen.send(()).unwrap();
            wait.await.unwrap();
            let mut w = Writer::versioned().unwrap();
            w.bytes(b"server-kick-off:opaque-directory-test").unwrap();
            let kick = encode_request(1, &[
                RpcProperty { key: b"id", value: b"MessageReceiver" },
                RpcProperty { key: b"domain", value: b"fixture-domain" },
                RpcProperty { key: b"__magic", value: b"original-directory-kick" },
                RpcProperty { key: b"token", value: b"fixture-auth" },
            ], n::rpc_client_ex::NOTIFY_DISCONNECT_METHOD.as_bytes(), w.finish().expose_to_transport()).unwrap();
            let mut packet = vec![0];
            packet.extend_from_slice(kick.expose_to_transport());
            // The matched physical kick latches first; even valid positive
            // directory DATA behind it cannot redeem the revoked original ID.
            peer.send(&[packet, reply(2, &relations_result())]).await;
            // Input-only decoding does not ACK an already closing socket.
            let packet = peer.next_input_only().await.unwrap();
            assert_eq!(packet[0], 1, "only original terminal control reply, no directory WRITE/logout/reconnect");
            let result = decode_reply(&packet[1..]).unwrap();
            assert_eq!(result.call_id, 1);
            assert_eq!(result.result_code, 0);
            assert_eq!(result.result_stream, &[0, 1, 0, 0, 0, 0]);
            assert!(peer.next_input_only().await.is_none(), "no post-revocation business bytes or replay");
        })));
        let original = owner();
        let conn = connected(&route, original.clone(), Instant::now() + Duration::from_secs(6)).await;
        let scope = conn.connection_scope();
        let (mut handle, mut events) = conn.attach().unwrap();
        directory_auth(&handle, &mut events, &original, &scope, true).await;
        let commands = handle.command_handle();
        let binding = bounded(commands.original_im_binding(&original, &scope)).await.unwrap();
        let query = directory_query(&scope, false, None);
        let last_query = query.clone();
        let interest = ImReadInterest::original(&scope);
        let (receipt, data) = directory_enqueue(&handle, &original, &scope,
            query.clone(), ImReceiptGate::fresh(), interest.clone());
        let submitted = bounded(receipt).await.unwrap().unwrap();
        assert!(submitted.matches(&original, &scope));
        assert_eq!(submitted.call_id, Some(2));
        bounded(request_seen).await.unwrap();
        assert!(!interest.is_writing());
        assert!(!interest.is_retired());
        assert_eq!(scope.native_rpc_resources().0, 2);
        no_directory_events(&mut events);
        go.send(()).unwrap();
        bounded(scope.closed()).await;
        assert!(scope.session_revoked(), "sticky reason belongs to the original physical scope");
        assert!(!scope.is_open());
        assert!(submitted.submitted_on(&original, &scope), "historical READ submission only");
        assert!(!submitted.matches(&original, &scope), "submission never becomes ready after kick");
        assert!(!binding.matches(&original, &scope));
        // No DATA sender can survive original actor teardown to publish true.
        assert!(bounded(data).await.is_err());
        assert_eq!(commands.read_im(&original, &scope, query.clone(), ImReceiptGate::fresh()).await.err(), Some(RuntimeError::Scope));
        assert_eq!(commands.read_im(&original, &scope,
            Arc::new(ImQuery::set_received(ImBox::parse(b"Org/1-2").unwrap(), 7).unwrap()),
            ImReceiptGate::fresh()).await.err(), Some(RuntimeError::Scope));
        assert_eq!(commands.poll(&original, &scope, &[]).await.err(), Some(RuntimeError::Scope));
        assert_eq!(commands.logout(&original, &scope, &[]).await.err(), Some(RuntimeError::Scope));
        let joined = bounded(handle.close()).await;
        assert!(joined.io_released());
        assert_eq!(joined.reason, RuntimeError::SessionRevoked, "not retryable close/timeout/protocol or a new scope");
        assert!(handle.task.is_none());
        bounded(serving).await.unwrap();
        let event = bounded(events.recv()).await.unwrap();
        assert_eq!(event.terminal_failure(&original, &scope), Some(n::session::BackendFailure::SessionRevoked));
        assert!(event.events_for(&original, &scope).is_none(), "no positive event access after close");
        let callback = event.into_terminal_callback(&original, &scope).unwrap();
        assert_eq!(callback.owner, original);
        assert_eq!(callback.callback, Callback::Failure(n::session::BackendFailure::SessionRevoked));
        assert!(bounded(events.recv()).await.is_none(), "only the actual original terminal failure");
        assert_eq!(scope.native_rpc_resources(), (1, 24));
        drop(query);
        assert_eq!(scope.native_rpc_resources(), (1, 24));
        drop(last_query);
        assert_eq!(scope.native_rpc_resources(), (0, 0));
    }).await;
}
