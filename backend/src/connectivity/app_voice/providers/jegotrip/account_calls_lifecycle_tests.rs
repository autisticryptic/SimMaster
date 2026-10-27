//! Actions-only per-capability permission, observer contention and cancelled
//! original supervisor cleanup. Existing native owner/lease tests stay intact.
use super::*;
#[tokio::test]
async fn disabling_incoming_permission_refuses_original_notice_without_sms_sim_or_history_grant(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;
    let(go,invited)=oneshot::channel();let(seen,checked)=oneshot::channel();let(done,stop)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invited.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,false).await;seen.send(()).unwrap();stop.await.unwrap();finish_sdk(peer).await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;let link=manager.call_operator(&account.line_id).await.unwrap();link.set_incoming_call_allowed(false);
    assert!(!manager.get(&account.line_id,100).await.unwrap().incoming_call_ready);go.send(()).unwrap();bounded(checked).await.unwrap();assert!(manager.calls(&account.line_id).await.unwrap().is_empty());
    assert!(temp.database.get_call_history_for_line(&account.line_id,20,0).unwrap().is_empty());assert_eq!(manager.reserve_outbound(&account.line_id,account.revision).err(),Some("app_account_outbound_unsupported"));
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn observer_state_contention_retries_bounded_publication_instead_of_revoking_original_call(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;
    let(go,invited)=oneshot::channel();let(inform,observed)=oneshot::channel();let(replied,reply)=oneshot::channel();let(done,stop)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invited.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;inform.send(()).unwrap();reply.await.unwrap();reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;stop.await.unwrap();finish_sdk(peer).await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();bounded(observed).await.unwrap();
    let service=manager.original_call_service(&account.line_id).unwrap();let call=lock(&service.calls).values().next().cloned().unwrap();let guard=lock(&call.state);
    let future=service.publish_state(&call,|_|42);tokio::pin!(future);
    let pending=std::future::poll_fn(|cx|Poll::Ready(Future::poll(future.as_mut(),cx).is_pending())).await;assert!(pending);assert!(call.ticket.is_live());
    drop(guard);assert_eq!(bounded(future.as_mut()).await.unwrap(),42);assert!(call.ticket.is_live());replied.send(()).unwrap();let call=first_call(&manager,&account.line_id).await;
    bounded(manager.hangup_call(&account.line_id,account.revision,&call.id,100)).await.unwrap();done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn cancelled_account_stop_retains_original_supervisor_join_and_runtime_lease_until_actual_io_release(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;peer.drain_until_eof().await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;let service=manager.original_call_service(&account.line_id).unwrap();
    // The first task is already ready; cancellation happens while joining the
    // second. Keeping both handles would repoll the consumed first JoinHandle.
    let first=tokio::spawn(async {});bounded(async{while !first.is_finished(){tokio::task::yield_now().await;}}).await;
    let socket=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();let local=socket.local_addr().unwrap();let(resume,resumed)=oneshot::channel();let join=tokio::spawn(async move{let _socket=socket;resumed.await.unwrap();});service.tasks.lock().await.extend([first,join]);
    {let future=manager.stop(&account.line_id,account.revision,100);tokio::pin!(future);
        bounded(async{loop{tokio::select!{biased;
            result=future.as_mut()=>panic!("cleanup must retain the second blocked task: {result:?}"),
            _=tokio::task::yield_now()=>{if service.tasks.try_lock().is_err(){break;}}
        }}}).await;
    }
    assert_eq!(service.tasks.lock().await.len(),1,"the first completed Join was removed before cancellation");
    assert!(!service.resources_released());assert!(temp.database.with_conn(|conn|account_runs::runtime_held(conn,&account.line_id)).unwrap());
    assert_eq!(manager.start(&account.line_id,account.revision,100).await.err(),Some("app_account_stop_before_replace"));assert!(UdpSocket::bind(local).await.is_err());
    resume.send(()).unwrap();let result=bounded(manager.stop(&account.line_id,account.revision,100)).await;
    assert!(matches!(result,Ok(_)|Err("app_account_disconnected")));assert!(service.resources_released());bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert!(!temp.database.with_conn(|conn|account_runs::ownership_pending(conn,&account.line_id)).unwrap());let reused=UdpSocket::bind(local).await.unwrap();assert_eq!(reused.local_addr().unwrap(),local);
}
#[tokio::test]
async fn true_sdk_accept_but_false_endpoint_never_marks_connected_or_answered_and_releases_original_ports(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();let endpoint=format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};",listener.local_addr().unwrap().port());let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;let(go,invite)=oneshot::channel();let(done,stop)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;let(id,_)=operation(&mut peer,CallOperation::Accept).await;reply_operation(&mut peer,id,CallOperation::Accept,true,Some(endpoint.as_bytes())).await;
        let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;stop.await.unwrap();finish_sdk(peer).await;}).await});
    let server=media.server(&rustls::version::TLS13);let media_task=tokio::spawn(async move{bounded(async move{let mut peer=MediaPeer::accept(listener,server).await;original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());peer.reply(false).await;peer.eof().await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();let call=first_call(&manager,&account.line_id).await;let offered=PlainAudioOffer::trunk(call.sdp.as_ref().unwrap().as_bytes(),true).unwrap().endpoint().unwrap();let peer=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();
    assert_eq!(bounded(manager.answer_call(&account.line_id,account.revision,&call.id,sdp(peer.local_addr().unwrap()),100)).await.err(),Some("jegotrip_media_endpoint_rejected"));
    let ended=phase(&manager,&account.line_id,&call.id,"ended").await;assert!(!ended.media_active);assert!(!temp.database.get_call_by_id(call.history_id).unwrap().unwrap().answered);
    assert!(temp.database.get_recent_app_events(Some(&account.line_id),20).unwrap().iter().all(|event|event.event_type!="call.connected"));bounded(media_task).await.unwrap();let reused=UdpSocket::bind(offered).await.unwrap();assert_eq!(reused.local_addr().unwrap(),offered);
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn original_call_api_rejects_other_manager_and_revision_then_expiry_retires_before_answer(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");let mut config=call_config(&sdk.root,&media.root);
    let expiry=super::super::super::account_runtime::unix_now()+300;config.credentials.sdk.expires_at_unix_seconds=Some(expiry);let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);
    let account=manager.create(config,100).await.unwrap().account;let(go,invite)=oneshot::channel();let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;peer.drain_until_eof().await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();let call=first_call(&manager,&account.line_id).await;let address=PlainAudioOffer::trunk(call.sdp.as_ref().unwrap().as_bytes(),true).unwrap().endpoint().unwrap();let answer=sdp("127.0.0.1:5008".parse().unwrap());
    let foreign_factory=Arc::new(QueuedFactory::default());let foreign=AccountManager::with_factory(temp.database.clone(),foreign_factory.clone());assert_eq!(foreign.answer_call(&account.line_id,account.revision,&call.id,answer.clone(),100).await.err(),Some("app_account_call_runtime_unavailable"));assert_eq!(foreign_factory.build_count(),0);
    assert_eq!(manager.answer_call(&account.line_id,account.revision+1,&call.id,answer.clone(),100).await.err(),Some("app_account_revision_conflict"));
    assert_eq!(manager.get(&account.line_id,expiry).await.unwrap().phase,"credential_expired");assert_eq!(manager.answer_call(&account.line_id,account.revision,&call.id,answer,expiry).await.err(),Some("app_account_call_runtime_unavailable"));
    let result=bounded(manager.stop(&account.line_id,account.revision,expiry)).await;assert!(matches!(result,Ok(_)|Err("app_account_lifecycle_invalid")|Err("app_account_disconnected")));bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert!(!temp.database.get_call_by_id(call.history_id).unwrap().unwrap().answered);assert!(!temp.database.with_conn(|conn|account_runs::ownership_pending(conn,&account.line_id)).unwrap());let reused=UdpSocket::bind(address).await.unwrap();assert_eq!(reused.local_addr().unwrap(),address);
}

#[derive(Clone, Copy)]
enum FinalDeliveryChange { Permission, Revision, PhysicalMedia }

/// The real original SDK/Endpoint exchange has already queued Connected, but
/// its caller has not consumed that result. Keep the operation lock so the
/// watch cannot revoke the ticket first and accidentally hide the delivery gap.
async fn queued_connected_delivery(change: FinalDeliveryChange) {
    let revision_changed = matches!(change, FinalDeliveryChange::Revision);
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
        let (id, _) = operation(&mut peer, CallOperation::Accept).await;
        reply_operation(&mut peer, id, CallOperation::Accept, true, Some(endpoint.as_bytes())).await;
        if revision_changed {
            // A concurrent database revision change retires the original SDK
            // scope. Its already-admitted bytes can precede EOF; do not invent
            // a positive remote release or force another session to clean it.
            peer.drain_until_eof().await;
        } else {
            let (id, _) = operation(&mut peer, CallOperation::Release).await;
            reply_operation(&mut peer, id, CallOperation::Release, true, None).await;
            stopped.await.unwrap(); finish_sdk(peer).await;
        }
    }).await });
    let server = media.server(&rustls::version::TLS13);
    let media_task = tokio::spawn(async move { bounded(async move {
        let mut peer = MediaPeer::accept(listener, server).await;
        original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(true).await; peer.eof().await;
    }).await });
    bounded(manager.start(&account.line_id, account.revision, 100)).await.unwrap();
    ready(&manager, &account.line_id).await; go.send(()).unwrap();
    let view = first_call(&manager, &account.line_id).await;
    let service = manager.original_call_service(&account.line_id).unwrap(); let call = service.find(&view.id).unwrap();
    let offered = PlainAudioOffer::trunk(view.sdp.as_ref().unwrap().as_bytes(), true).unwrap().endpoint().unwrap();
    let asterisk = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
    let waiter = service.accept(&view.id, sdp(asterisk.local_addr().unwrap())).await.unwrap();
    assert!(phase(&manager, &account.line_id, &view.id, "connected").await.media_active);
    let operation_guard = bounded(call.operation.lock()).await;
    // The guarded accept task sends its result before releasing this lock.
    // Thus no timeout/sleep or synthetic connected result is used here.
    assert!(call.ticket.is_live()); assert!(call.ticket.connection_scope().is_open());
    match change {
        FinalDeliveryChange::Permission => service.operator().set_incoming_call_allowed(false),
        FinalDeliveryChange::Revision => {
            temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1", [&account.line_id])
                .map_err(|_| "test_revision_injection_failed")).unwrap();
        }
        FinalDeliveryChange::PhysicalMedia => lock(&call.state).media.as_ref().unwrap().retire(),
    }
    assert!(call.ticket.is_live(), "the stale ticket alone still permits the old positive result");
    assert_eq!(bounded(waiter).await.err(), Some(SCOPE));
    assert!(!call.ticket.is_live()); drop(operation_guard);
    bounded(async { loop {
        if lock(&call.state).phase == Phase::Ended { break; }
        tokio::task::yield_now().await;
    }}).await;
    bounded(media_task).await.unwrap(); let rebound = UdpSocket::bind(offered).await.unwrap();
    assert_eq!(rebound.local_addr().unwrap(), offered);
    let row = temp.database.get_call_by_id(view.history_id).unwrap().unwrap();
    assert!(row.answered && row.end_time.is_some()); assert_eq!(row.line_id.as_deref(), Some(account.line_id.as_str()));
    if !revision_changed { done.send(()).unwrap(); }
    let revision = if revision_changed { 2 } else { account.revision };
    let result = bounded(manager.stop(&account.line_id, revision, 100)).await;
    assert!(matches!(result, Ok(_) | Err("app_account_disconnected")), "original stop verdict: {result:?}");
    bounded(sdk_task).await.unwrap(); assert!(dropped.load(Ordering::SeqCst));
    assert!(service.resources_released()); assert_eq!(factory.build_count(), 1);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &account.line_id)).unwrap());
}

#[tokio::test]
async fn queued_connected_result_rechecks_permission_before_delivery_and_joins_original_media() {
    queued_connected_delivery(FinalDeliveryChange::Permission).await;
}
#[tokio::test]
async fn queued_connected_result_rechecks_original_revision_without_rebinding_or_cached_success() {
    queued_connected_delivery(FinalDeliveryChange::Revision).await;
}
#[tokio::test]
async fn queued_connected_result_rejects_closed_physical_media_even_while_original_ticket_is_live() {
    queued_connected_delivery(FinalDeliveryChange::PhysicalMedia).await;
}
