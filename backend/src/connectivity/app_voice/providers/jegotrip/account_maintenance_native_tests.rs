// Actions-only original actor+consumer joins and durable retention fences.
use super::*;
use super::super::super::{account_config::AccountConfig,transport_packets::CallKind};
fn smgw(root:&str)->AccountConfig {let mut input=config(root);input.profile.incoming.as_mut().unwrap().receiver_kind=b"Sip.Request".to_vec();input}
fn count(db:&Database,table:&str)->i64 {db.with_conn(|conn|conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|row|row.get(0)).map_err(|_|"test_query_failed")).unwrap()}
async fn serving(mut peer:super::super::super::runtime_backend::tests::Peer) {
    peer.authenticate().await;peer.swap(Some(STATUS_REGISTER),true).await;
    peer.incoming_with_fields(91,b"Sip.Request",b"fixture-sms",Some(b"fixture-sender"),None).await;
    let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);
    peer.swap(Some(STATUS_UNREGISTER),true).await;let request=next_non_poll(&mut peer).await;
    let request=decode_request(request.payload.expose_to_transport()).unwrap();peer.reply(request.call_id,&[0,1,0,0,0,0]).await;
    assert!(peer.receive().await.is_none());
}
async fn have_messages(manager:&AccountManager,line:&str,total:usize) {
    tokio::time::timeout(Duration::from_secs(4),async {loop{if manager.messages(line,128).unwrap().len()==total {break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
}
#[tokio::test]
async fn actual_stop_joins_both_original_tasks_then_compacts_without_erasing_sms_history() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());
    let (first,_,first_dropped)=peer(&cert,&factory);let (second,_,second_dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory.clone());let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    for (index,peer,dropped) in [(1,first,first_dropped),(2,second,second_dropped)] {
        let worker=tokio::spawn(serving(peer));manager.start(&line,1,100).await.unwrap();have_messages(&manager,&line,index).await;
        assert_eq!(count(&temp.database,"app_voice_inbox"),1);assert_eq!(count(&temp.database,"app_voice_inbox_runs"),1);
        let stopped=manager.stop(&line,1,100).await.unwrap();assert!(!stopped.io_cleanup_required);assert!(dropped.load(Ordering::SeqCst));
        worker.await.unwrap();assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"app_voice_inbox_runs"),0);
        assert_eq!(manager.messages(&line,128).unwrap().len(),index);assert_eq!(count(&temp.database,"app_voice_sms_projection"),index as i64);
    }
    assert_eq!(factory.build_count(),2);
}
#[tokio::test]
async fn failed_gc_after_native_release_keeps_cleanup_required_and_retries_no_rpc() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (peer,connector,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    let worker=tokio::spawn(serving(peer));manager.start(&line,1,100).await.unwrap();have_messages(&manager,&line,1).await;
    temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER refuse_run_gc BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;").map_err(|_|"test_trigger_failed")).unwrap();
    assert_eq!(manager.stop(&line,1,100).await.err(),Some("app_account_run_storage_failed"));worker.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert!(manager.get(&line,100).await.unwrap().io_cleanup_required);assert_eq!(manager.start(&line,1,100).await.err(),Some("app_account_stop_before_replace"));
    assert_eq!(count(&temp.database,"app_voice_inbox"),1);assert_eq!(count(&temp.database,"app_voice_inbox_runs"),1);
    temp.database.with_conn(|conn|conn.execute_batch("DROP TRIGGER refuse_run_gc").map_err(|_|"test_trigger_failed")).unwrap();
    assert!(!manager.stop(&line,1,100).await.unwrap().io_cleanup_required);assert_eq!(connector.connection_calls(),1);
    assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(manager.messages(&line,128).unwrap().len(),1);
}
#[tokio::test]
async fn process_shutdown_cancels_pending_start_and_joins_original_run_without_rebinding() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());
    let held=Arc::new(HeldConnector{entered:tokio::sync::Notify::new(),scope:Mutex::new(None)});factory.add(held.clone());
    let manager=Arc::new(AccountManager::with_factory(temp.database.clone(),factory.clone()));let line=manager.create(config(&cert.root),100).await.unwrap().account.line_id;
    let owner=manager.clone();let id=line.clone();let start=tokio::spawn(async move{owner.start(&id,1,100).await});
    tokio::time::timeout(Duration::from_secs(3),held.entered.notified()).await.unwrap();
    let original=lock(&held.scope).clone().unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(3),manager.shutdown_all()).await.unwrap(),0);
    assert_eq!(start.await.unwrap().err(),Some("app_account_shutting_down"));assert!(!original.is_open());
    assert_eq!(manager.start(&line,1,100).await.err(),Some("app_account_shutting_down"));
    assert_eq!(count(&temp.database,"app_voice_inbox_runs"),0);assert_eq!(factory.build_count(),1);
}

#[tokio::test]
async fn process_shutdown_gracefully_releases_hosted_original_run_and_keeps_sms_history() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let(peer,_,dropped)=peer(&cert,&factory);
    let manager=AccountManager::with_factory(temp.database.clone(),factory);let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    let worker=tokio::spawn(serving(peer));manager.start(&line,1,100).await.unwrap();have_messages(&manager,&line,1).await;
    assert_eq!(manager.shutdown_all().await,0);worker.await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"sms_messages"),1);
    assert_eq!(manager.shutdown_all().await,0);
}

#[tokio::test]
async fn cancelling_between_actor_release_and_consumer_join_keeps_original_fence_and_join() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let (peer,_,dropped)=peer(&cert,&factory);
    let manager=Arc::new(AccountManager::with_factory(temp.database.clone(),factory));let line=manager.create(smgw(&cert.root),100).await.unwrap().account.line_id;
    let worker=tokio::spawn(serving(peer));manager.start(&line,1,100).await.unwrap();have_messages(&manager,&line,1).await;
    let slot=lock(&manager.slots).get(&line).unwrap().clone();let run=lock(&slot.run).clone().unwrap();
    let held=run.task.lock().await;
    let owner=manager.clone();let id=line.clone();let stop=tokio::spawn(async move{owner.stop(&id,1,100).await});
    tokio::time::timeout(Duration::from_secs(4),async{loop {
        let released=run.controller.try_lock().map(|controller|controller.resources_released()).unwrap_or(false);
        if released&&dropped.load(Ordering::SeqCst){break;}tokio::task::yield_now().await;
    }}).await.unwrap();
    stop.abort();assert!(stop.await.unwrap_err().is_cancelled());worker.await.unwrap();
    assert!(!run.finalized.load(Ordering::SeqCst));assert_eq!(count(&temp.database,"app_voice_inbox"),1);
    assert!(temp.database.with_conn(|conn|account_runs::require_live(conn,&line,1,run.run_token.nonce())).is_ok());
    assert_eq!(manager.replace(&line,1,smgw(&cert.root),100).await.err(),Some("app_account_stop_before_replace"));
    drop(held);assert!(!manager.stop(&line,1,100).await.unwrap().io_cleanup_required);
    assert_eq!(count(&temp.database,"app_voice_inbox"),0);assert_eq!(count(&temp.database,"sms_messages"),1);
}
