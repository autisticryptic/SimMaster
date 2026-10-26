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
    let socket=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();let local=socket.local_addr().unwrap();let(resume,resumed)=oneshot::channel();let join=tokio::spawn(async move{let _socket=socket;resumed.await.unwrap();});service.tasks.lock().await.push(join);
    {let future=manager.stop(&account.line_id,account.revision,100);tokio::pin!(future);let pending=std::future::poll_fn(|cx|Poll::Ready(Future::poll(future.as_mut(),cx).is_pending())).await;assert!(pending);}
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
