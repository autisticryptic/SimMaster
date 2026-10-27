//! Actions-only original call task admission and delayed batch-receipt tests.
//! The batch scheduling barrier pauses only automatic poll submissions; native
//! TLS, callback correlation, receipt queuing and call RPCs execute unchanged.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as native;
use native::{account_call_test_support::NATIVE_ID, rpc_stream::Writer, runtime_backend::tests::swap_result};

async fn cancelled_before_admission(answer: bool) {
    let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default());
    let sdk = Certificates::generate("DNS:fixture.invalid"); let media = Certificates::generate("IP:127.0.0.1");
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};", listener.local_addr().unwrap().port());
    let (mut peer, _, dropped) = sdk_peer(&sdk, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let account = manager.create(call_config(&sdk.root, &media.root), 100).await.unwrap().account;
    let (go, invited) = oneshot::channel(); let (done, stopped) = oneshot::channel();
    let sdk_task = tokio::spawn(async move { bounded(async move {
        peer.authenticate().await; peer.swap(Some(4), true).await; invited.await.unwrap();
        notice(&mut peer, CallNoticeKind::Invited, 101, &sdp("127.0.0.1:5006".parse().unwrap()), b"fixture-domain").await;
        receipt(&mut peer, 101, true).await;
        let (id, _) = operation(&mut peer, CallOperation::Inform).await;
        reply_operation(&mut peer, id, CallOperation::Inform, true, None).await;
        if answer {
            let (id, _) = operation(&mut peer, CallOperation::Accept).await;
            reply_operation(&mut peer, id, CallOperation::Accept, true, Some(endpoint.as_bytes())).await;
        }
        let (id, _) = operation(&mut peer, CallOperation::Release).await;
        reply_operation(&mut peer, id, CallOperation::Release, true, None).await;
        stopped.await.unwrap(); finish_sdk(peer).await;
    }).await });
    let server = media.server(&rustls::version::TLS13);
    let media_task = if answer { Some(tokio::spawn(async move { bounded(async move {
        let mut peer = MediaPeer::accept(listener, server).await;
        original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(true).await; peer.eof().await;
    }).await })) } else { drop(listener); None };
    bounded(manager.start(&account.line_id, account.revision, 100)).await.unwrap();
    ready(&manager, &account.line_id).await; go.send(()).unwrap();
    let view = first_call(&manager, &account.line_id).await;
    let service = manager.original_call_service(&account.line_id).unwrap(); let call = service.find(&view.id).unwrap();
    let offered = PlainAudioOffer::trunk(view.sdp.as_ref().unwrap().as_bytes(), true).unwrap().endpoint().unwrap();
    let asterisk = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap(); let body = sdp(asterisk.local_addr().unwrap());
    let admission = service.tasks.lock().await;
    if answer {
        let future = service.accept(&view.id, body.clone()); tokio::pin!(future);
        assert!(std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
    } else {
        let future = service.hangup(&view.id); tokio::pin!(future);
        assert!(std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await);
    }
    // No task/waiter was admitted. Cancelling the pending API may not strand a
    // queued flag, revoke the original call, or lose its bound audio socket.
    assert!(call.ticket.is_live()); assert!(!call.cancelled.load(Ordering::SeqCst));
    { let state = lock(&call.state);
        assert!(state.phase == Phase::Ringing && state.socket.is_some());
        assert!(!state.accept_queued && !state.end_queued);
    }
    assert!(!temp.database.get_call_by_id(view.history_id).unwrap().unwrap().answered);
    drop(admission);
    if answer {
        let accepted = bounded(manager.answer_call(&account.line_id, account.revision, &view.id, body, 100)).await.unwrap();
        assert_eq!(accepted.phase, "connected"); assert!(accepted.media_active);
    }
    let ended = bounded(manager.hangup_call(&account.line_id, account.revision, &view.id, 100)).await.unwrap();
    assert_eq!(ended.phase, "ended"); assert!(!ended.media_active);
    if let Some(task) = media_task { bounded(task).await.unwrap(); }
    let rebound = UdpSocket::bind(offered).await.unwrap(); assert_eq!(rebound.local_addr().unwrap(), offered);
    let history = temp.database.get_call_by_id(view.history_id).unwrap().unwrap();
    assert_eq!(history.answered, answer); assert!(history.end_time.is_some());
    done.send(()).unwrap(); bounded(manager.stop(&account.line_id, account.revision, 100)).await.unwrap();
    bounded(sdk_task).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
}

#[tokio::test]
async fn cancelled_answer_before_task_admission_remains_retryable_without_stranded_flags() {
    cancelled_before_admission(true).await;
}
#[tokio::test]
async fn cancelled_hangup_before_task_admission_preserves_original_call_until_retry() {
    cancelled_before_admission(false).await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchOutcome { Answer, Rejected, Deadline, Cancelled }

async fn delayed_batch_invitation(outcome: BatchOutcome) {
    let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default());
    let sdk = Certificates::generate("DNS:fixture.invalid"); let media = Certificates::generate("IP:127.0.0.1");
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};", listener.local_addr().unwrap().port());
    let (mut peer, _, dropped) = sdk_peer(&sdk, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let mut config = call_config(&sdk.root, &media.root); config.profile.rpc_timeout_ms = 1000;
    let account = manager.create(config, 100).await.unwrap().account;
    let (captured, poll_captured) = oneshot::channel(); let (go, invited) = oneshot::channel();
    let (done, stopped) = oneshot::channel();
    let sdk_task = tokio::spawn(async move { bounded(async move {
        peer.authenticate().await; peer.swap(Some(4), true).await;
        let packet = peer.receive().await.unwrap();
        let request = decode_request(packet.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, native::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut reader = Reader::versioned(request.arguments).unwrap(); reader.bytes().unwrap(); reader.bytes().unwrap();
        let fields = native::rpc_batch::read_payload(&mut reader).unwrap().fields; let id = request.call_id;
        captured.send(()).unwrap(); invited.await.unwrap();
        let mut arguments = Writer::versioned().unwrap(); arguments.u64(NATIVE_ID).unwrap(); arguments.bytes(b"fixture-caller").unwrap();
        let offer = sdp("127.0.0.1:5006".parse().unwrap());
        arguments.properties(&[RpcProperty { key: b"callServerEx.oid", value: b"#fixture-call-server" },
            RpcProperty { key: b"RseEx.SDP", value: &offer }]).unwrap();
        let mut reply = Writer::new(); reply.boolean(true).unwrap();
        for word in [9, 1, 1, fields[0], fields[1], fields[2]] { reply.u32(word).unwrap(); }
        reply.u32(1).unwrap(); reply.i32(1).unwrap(); reply.bytes(native::rpc_calls::INVITED_METHOD.as_bytes()).unwrap();
        reply.bytes(arguments.finish().expose_to_transport()).unwrap();
        reply.properties(&[RpcProperty { key: b"id", value: b"#fixture-call-client" },
            RpcProperty { key: b"domain", value: b"fixture-domain" }, RpcProperty { key: b"__magic", value: &[0, 0xff, 0x43] }]).unwrap();
        reply.u32(0).unwrap(); reply.properties(&[]).unwrap(); reply.bytes(b"").unwrap();
        peer.reply(id, reply.finish().expose_to_transport()).await;
        if matches!(outcome, BatchOutcome::Answer | BatchOutcome::Rejected) {
            loop {
                let packet = peer.receive().await.unwrap(); let request = decode_request(packet.payload.expose_to_transport()).unwrap();
                assert_eq!(request.method, native::rpc_batch::SWAP_CALLS_METHOD.as_bytes(), "Inform must not overtake the original batch receipt");
                let mut reader = Reader::versioned(request.arguments).unwrap(); reader.bytes().unwrap(); reader.bytes().unwrap();
                let payload = native::rpc_batch::read_payload(&mut reader).unwrap(); let complete = payload.results.contains_key(&1);
                if complete { assert_eq!(payload.results[&1].result_stream,
                    encode_notice_result(CallNoticeKind::Invited, true, &[]).unwrap().expose_to_transport()); }
                peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await;
                if complete { break; }
            }
            let (id, _) = operation(&mut peer, CallOperation::Inform).await;
            reply_operation(&mut peer, id, CallOperation::Inform, outcome == BatchOutcome::Answer, None).await;
            if outcome == BatchOutcome::Answer {
                let (id, _) = operation(&mut peer, CallOperation::Accept).await;
                reply_operation(&mut peer, id, CallOperation::Accept, true, Some(endpoint.as_bytes())).await;
            }
        }
        // Deadline/cancel sends no Inform. Rejected/accepted Inform is never
        // replayed: the next wire call must be one original Release.
        let (id, _) = operation(&mut peer, CallOperation::Release).await;
        reply_operation(&mut peer, id, CallOperation::Release, true, None).await;
        stopped.await.unwrap(); finish_sdk(peer).await;
    }).await });
    let down = rtp(55, 0x31); let up = rtp(56, 0x42); let native_down = down.clone(); let native_up = up.clone();
    let (audio_done, audio_seen) = oneshot::channel(); let server = media.server(&rustls::version::TLS13);
    let media_task = if outcome == BatchOutcome::Answer { Some(tokio::spawn(async move { bounded(async move {
        let mut peer = MediaPeer::accept(listener, server).await;
        original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport()); peer.reply(true).await;
        let (packets, _) = tcp_mpath::encode_payload(0, 54, 1200, 0, &native_down).unwrap();
        peer.send_raw(packets[0].expose_to_transport()).await;
        let raw = peer.next_raw().await.unwrap().unwrap(); let mut receiver = tcp_mpath::Receiver::default();
        let Some(MpathInput::Payload { channel: 0, payload, .. }) = receiver.on_packet(&raw, Instant::now()).unwrap() else { panic!("original batch-call uplink"); };
        assert_eq!(payload.expose_to_transport(), native_up); audio_done.send(()).unwrap(); peer.eof().await;
    }).await })) } else { drop(listener); None };
    bounded(manager.start(&account.line_id, account.revision, 100)).await.unwrap(); ready(&manager, &account.line_id).await;
    bounded(poll_captured).await.unwrap(); let service = manager.original_call_service(&account.line_id).unwrap();
    let (owner, scope) = { let bound = lock(&service.bound); let original = bound.as_ref().unwrap(); (original.owner.clone(), original.scope.clone()) };
    service.backend.pause_original_polls_for_test(&owner, &scope, true).unwrap(); go.send(()).unwrap();
    let call = bounded(async { loop {
        if let Some(call) = lock(&service.calls).values().next().cloned() { break call; }
        tokio::task::yield_now().await;
    }}).await;
    // Deliberately span at least one retry while no swap can carry the queued
    // receipt. This is still an original Checking call, not a false rejection.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(lock(&call.state).phase == Phase::Checking); assert!(call.ticket.is_live());
    assert!(!temp.database.get_call_by_id(call.history.id()).unwrap().unwrap().answered);
    if matches!(outcome, BatchOutcome::Answer | BatchOutcome::Rejected) {
        service.backend.pause_original_polls_for_test(&owner, &scope, false).unwrap();
    }
    if outcome == BatchOutcome::Answer {
        let view = first_call(&manager, &account.line_id).await;
        let offered = PlainAudioOffer::trunk(view.sdp.as_ref().unwrap().as_bytes(), true).unwrap().endpoint().unwrap();
        let asterisk = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let connected = bounded(manager.answer_call(&account.line_id, account.revision, &view.id, sdp(asterisk.local_addr().unwrap()), 100)).await.unwrap();
        assert_eq!(connected.phase, "connected"); assert!(connected.media_active);
        let mut bytes = [0; 4096]; let (count, source) = bounded(asterisk.recv_from(&mut bytes)).await.unwrap();
        assert_eq!(source, offered); assert_eq!(&bytes[..count], down);
        asterisk.send_to(&up, offered).await.unwrap(); bounded(audio_seen).await.unwrap();
        bounded(manager.hangup_call(&account.line_id, account.revision, &view.id, 100)).await.unwrap();
        bounded(media_task.unwrap()).await.unwrap();
        let rebound = UdpSocket::bind(offered).await.unwrap(); assert_eq!(rebound.local_addr().unwrap(), offered);
    } else if outcome == BatchOutcome::Cancelled {
        bounded(manager.hangup_call(&account.line_id, account.revision, &call.id, 100)).await.unwrap();
    }
    let ended = phase(&manager, &account.line_id, &call.id, "ended").await;
    assert!(!ended.media_active);
    let expected = match outcome { BatchOutcome::Answer => None, BatchOutcome::Rejected => Some("app_account_call_rejected"),
        BatchOutcome::Deadline => Some("app_account_call_timeout"), BatchOutcome::Cancelled => Some(SCOPE) };
    assert_eq!(ended.last_error, expected); assert!(scope.is_open());
    let history = temp.database.get_call_by_id(call.history.id()).unwrap().unwrap();
    assert_eq!(history.answered, outcome == BatchOutcome::Answer); assert!(history.end_time.is_some());
    service.backend.pause_original_polls_for_test(&owner, &scope, false).unwrap();
    done.send(()).unwrap(); bounded(manager.stop(&account.line_id, account.revision, 100)).await.unwrap();
    bounded(sdk_task).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert!(service.resources_released());
    assert_eq!(factory.build_count(), 1);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
}

#[tokio::test]
async fn delayed_batch_receipt_precedes_one_inform_answer_audio_and_original_hangup() {
    delayed_batch_invitation(BatchOutcome::Answer).await;
}
#[tokio::test]
async fn delayed_batch_receipt_does_not_retry_a_submitted_rejected_inform() {
    delayed_batch_invitation(BatchOutcome::Rejected).await;
}
#[tokio::test]
async fn delayed_batch_receipt_expires_one_fixed_budget_without_inform_or_scope_rebinding() {
    delayed_batch_invitation(BatchOutcome::Deadline).await;
}
#[tokio::test]
async fn cancelled_batch_receipt_wait_releases_original_call_without_inform_or_media() {
    delayed_batch_invitation(BatchOutcome::Cancelled).await;
}
