//! Actions-only whole-call retirement, including ringing without a media actor.
use super::*;

#[derive(Clone, Copy)]
enum RingRetirement { Disconnect, Expiry, Revision }

async fn ringing_retirement(reason: RingRetirement) {
    let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default());
    let sdk = Certificates::generate("DNS:fixture.invalid"); let media = Certificates::generate("IP:127.0.0.1");
    let (mut peer, _, dropped) = sdk_peer(&sdk, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let mut config = call_config(&sdk.root, &media.root);
    let expiry = super::super::super::account_runtime::unix_now() + 300;
    if matches!(reason, RingRetirement::Expiry) { config.credentials.sdk.expires_at_unix_seconds = Some(expiry); }
    let account = manager.create(config, 100).await.unwrap().account;
    let (go, invite) = oneshot::channel(); let (end, ended) = oneshot::channel();
    let sdk_task = tokio::spawn(async move { bounded(async move {
        peer.authenticate().await; peer.swap(Some(4), true).await; invite.await.unwrap();
        notice(&mut peer, CallNoticeKind::Invited, 101, &sdp("127.0.0.1:5006".parse().unwrap()), b"fixture-domain").await;
        receipt(&mut peer, 101, true).await;
        let (id, _) = operation(&mut peer, CallOperation::Inform).await;
        reply_operation(&mut peer, id, CallOperation::Inform, true, None).await;
        ended.await.unwrap();
        if matches!(reason, RingRetirement::Disconnect) { drop(peer); }
        else { peer.drain_until_eof().await; }
    }).await });
    bounded(manager.start(&account.line_id, account.revision, 100)).await.unwrap(); ready(&manager, &account.line_id).await;
    let service = manager.original_call_service(&account.line_id).unwrap(); let mut events = service.operator().subscribe_events();
    go.send(()).unwrap(); let view = first_call(&manager, &account.line_id).await; let call = service.find(&view.id).unwrap();
    let watch_id = call.watch.lock().await.as_ref().expect("ringing owns a watcher").id();
    let local = PlainAudioOffer::trunk(view.sdp.as_ref().unwrap().as_bytes(), true).unwrap().endpoint().unwrap();
    let revision = if matches!(reason, RingRetirement::Revision) { 2 } else { account.revision };
    match reason {
        RingRetirement::Disconnect => {},
        RingRetirement::Expiry => { assert_eq!(manager.get(&account.line_id, expiry).await.unwrap().phase, "credential_expired"); },
        RingRetirement::Revision => {
            temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1", [&account.line_id])
                .map_err(|_| "test_revision_injection_failed")).unwrap();
        }
    }
    end.send(()).unwrap();
    bounded(async { loop { if lock(&call.state).phase == Phase::Ended { break; } tokio::task::yield_now().await; } }).await;
    bounded(sdk_task).await.unwrap(); assert!(dropped.load(Ordering::SeqCst));
    assert!(!call.ticket.is_live()); assert!(!call.view().media_active);
    assert_eq!(call.watch.lock().await.as_ref().unwrap().id(), watch_id);
    let row = temp.database.get_call_by_id(view.history_id).unwrap().unwrap();
    assert!(!row.answered && row.end_time.is_some()); assert_eq!(row.line_id.as_deref(), Some(account.line_id.as_str()));
    let mut ends = 0;
    while let Ok(event) = events.try_recv() { if matches!(event, OperatorEvent::Ended { call_id } if call_id == view.id) { ends += 1; } }
    assert_eq!(ends, 1, "original Ended is published without a manual account stop");
    let rebound = UdpSocket::bind(local).await.unwrap(); assert_eq!(rebound.local_addr().unwrap(), local);
    // Default remains explicit account cleanup; a finished call does not forge
    // the controller+consumer run-release proof or start a replacement session.
    assert!(temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
    let stopped = bounded(manager.stop(&account.line_id, revision, expiry)).await;
    assert!(matches!(stopped, Ok(_) | Err("app_account_disconnected") | Err("app_account_lifecycle_invalid")));
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
    assert_eq!(factory.build_count(), 1);
}

#[tokio::test]
async fn ringing_original_connection_loss_finishes_history_and_ended_without_manual_stop() {
    ringing_retirement(RingRetirement::Disconnect).await;
}
#[tokio::test]
async fn ringing_credential_expiry_retires_original_call_without_waiting_for_media() {
    ringing_retirement(RingRetirement::Expiry).await;
}
#[tokio::test]
async fn ringing_revision_change_finishes_only_original_history_and_releases_its_socket() {
    ringing_retirement(RingRetirement::Revision).await;
}

#[tokio::test]
async fn one_original_watcher_survives_ringing_to_connected_and_joins_failed_media_once() {
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
        let (id, _) = operation(&mut peer, CallOperation::Inform).await; reply_operation(&mut peer, id, CallOperation::Inform, true, None).await;
        let (id, _) = operation(&mut peer, CallOperation::Accept).await; reply_operation(&mut peer, id, CallOperation::Accept, true, Some(endpoint.as_bytes())).await;
        let (id, _) = operation(&mut peer, CallOperation::Release).await; reply_operation(&mut peer, id, CallOperation::Release, true, None).await;
        stopped.await.unwrap(); finish_sdk(peer).await;
    }).await });
    let (fail, failed) = oneshot::channel(); let server = media.server(&rustls::version::TLS13);
    let media_task = tokio::spawn(async move { bounded(async move {
        let mut peer = MediaPeer::accept(listener, server).await; original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(true).await; failed.await.unwrap(); drop(peer);
    }).await });
    bounded(manager.start(&account.line_id, account.revision, 100)).await.unwrap(); ready(&manager, &account.line_id).await;
    go.send(()).unwrap(); let view = first_call(&manager, &account.line_id).await;
    let service = manager.original_call_service(&account.line_id).unwrap(); let call = service.find(&view.id).unwrap();
    let watch_id = call.watch.lock().await.as_ref().unwrap().id(); let mut events = service.operator().subscribe_events();
    let local = PlainAudioOffer::trunk(view.sdp.as_ref().unwrap().as_bytes(), true).unwrap().endpoint().unwrap();
    let asterisk = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
    assert!(bounded(manager.answer_call(&account.line_id, account.revision, &view.id, sdp(asterisk.local_addr().unwrap()), 100)).await.unwrap().media_active);
    assert_eq!(call.watch.lock().await.as_ref().unwrap().id(), watch_id, "media may not replace/detach the original watcher");
    fail.send(()).unwrap(); bounded(media_task).await.unwrap(); phase(&manager, &account.line_id, &view.id, "ended").await;
    assert!(call.ticket.connection_scope().is_open()); assert!(manager.get(&account.line_id, 100).await.unwrap().incoming_call_ready);
    let row = temp.database.get_call_by_id(view.history_id).unwrap().unwrap(); assert!(row.answered && row.end_time.is_some());
    let mut ends = 0; while let Ok(event) = events.try_recv() { if matches!(event, OperatorEvent::Ended { call_id } if call_id == view.id) { ends += 1; } } assert_eq!(ends, 1);
    let rebound = UdpSocket::bind(local).await.unwrap(); assert_eq!(rebound.local_addr().unwrap(), local);
    done.send(()).unwrap(); bounded(manager.stop(&account.line_id, account.revision, 100)).await.unwrap();
    bounded(sdk_task).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
}

#[tokio::test]
async fn failed_call_history_end_retains_original_run_until_same_owner_cleanup_retry() {
    let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default());
    let sdk = Certificates::generate("DNS:fixture.invalid"); let media = Certificates::generate("IP:127.0.0.1");
    let (mut peer, _, dropped) = sdk_peer(&sdk, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let mut config = call_config(&sdk.root, &media.root); config.cleanup_on_disconnect = true;
    let account = manager.create(config, 100).await.unwrap().account;
    let (go, invited) = oneshot::channel(); let (end, ended) = oneshot::channel();
    let sdk_task = tokio::spawn(async move { bounded(async move {
        peer.authenticate().await; peer.swap(Some(4), true).await; invited.await.unwrap();
        notice(&mut peer, CallNoticeKind::Invited, 101, &sdp("127.0.0.1:5006".parse().unwrap()), b"fixture-domain").await;
        receipt(&mut peer, 101, true).await;
        let (id, _) = operation(&mut peer, CallOperation::Inform).await; reply_operation(&mut peer, id, CallOperation::Inform, true, None).await;
        ended.await.unwrap(); drop(peer);
    }).await });
    bounded(manager.start(&account.line_id, account.revision, 100)).await.unwrap(); ready(&manager, &account.line_id).await;
    go.send(()).unwrap(); let view = first_call(&manager, &account.line_id).await;
    let service = manager.original_call_service(&account.line_id).unwrap(); let call = service.find(&view.id).unwrap();
    temp.database.with_conn(|conn| conn.execute_batch("CREATE TRIGGER fail_call_end BEFORE INSERT ON app_events WHEN NEW.event_type='call.ended' BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;")
        .map_err(|_| "test_trigger_failed")).unwrap();
    end.send(()).unwrap(); bounded(sdk_task).await.unwrap();
    let observed = bounded(async { loop {
        let view = manager.get(&account.line_id, 100).await.unwrap();
        if view.cleanup_error.is_some() { break view; } tokio::task::yield_now().await;
    }}).await;
    assert_eq!(observed.cleanup_error, Some("app_account_call_storage_failed")); assert!(observed.io_cleanup_required);
    assert!(!service.resources_released()); assert!(lock(&call.state).phase == Phase::Ending);
    assert!(temp.database.get_call_by_id(view.history_id).unwrap().unwrap().end_time.is_none());
    assert!(temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
    assert_eq!(manager.start(&account.line_id, account.revision, 100).await.err(), Some("app_account_stop_before_replace"));
    temp.database.with_conn(|conn| conn.execute_batch("DROP TRIGGER fail_call_end").map_err(|_| "test_trigger_failed")).unwrap();
    let stopped = bounded(manager.stop(&account.line_id, account.revision, 100)).await.unwrap(); assert!(!stopped.io_cleanup_required);
    assert!(temp.database.get_call_by_id(view.history_id).unwrap().unwrap().end_time.is_some());
    assert!(service.resources_released()); assert!(dropped.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
}
