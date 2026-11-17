//! Actions-only SAME original physical READ cancellation, never WRITE retirement.
use super::*;
use n::im_inbox::ImReconciliationAction;
fn receipt(temp: &TempDatabase) -> i64 { temp.database.with_conn(|c|c.query_row("SELECT id FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap() }
fn unknown(temp: &TempDatabase) -> i64 { temp.database.with_conn(|c|c.query_row("SELECT state FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap() }
async fn scenario(partial: bool) {
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let policy=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let(go,ready)=oneshot::channel();let(sent,seen)=oneshot::channel();let(late,wait)=oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;peer_ready(&mut peer,ready).await;
        if partial { assert!(peer.next().await.is_none(),"blocked observation cannot send a Group query or user WRITE"); }
        else { let p=peer.next().await.unwrap();let id=query(&p,n::im_status::REFRESH_METHOD);sent.send(()).unwrap();wait.await.unwrap();peer.send(&[reply(id,&[0])]).await;peer_logout(&mut peer).await; }
    }).await});
    let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);let mut connection=connected(&policy,who.clone(),Instant::now()+Duration::from_secs(6)).await;let flag=Arc::new(AtomicBool::new(false));let pauses=n::common_runtime::tests::gate_im_read_write(connection.common.as_mut().unwrap(),flag.clone());let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();auth(&handle,&mut events,&who,&scope,true).await;go.send(()).unwrap();
    let parent=ImReceiptGate::fresh();let(_decision,_permit)=setup(&handle,&mut events,&who,&scope,&temp,run.clone(),parent.clone()).await;let action=ImReconciliationAction::original(temp.database.clone(),run,who.clone(),scope.clone(),parent,receipt(&temp),Instant::now()+Duration::from_secs(3)).unwrap();let binding=handle.command_handle().original_im_binding(&who,&scope).await.unwrap();let context=action.bind_current(binding).unwrap();let interest=ImReadInterest::original(&scope);let waiting=ImReadWait::original(interest.clone());let(reply,data)=oneshot::channel();if partial{flag.store(true,Ordering::SeqCst);}let mut submitted=handle.command_handle().enqueue(&who,&scope,Operation::ImRead(Box::new(ImReadWork{query:context.query(),gate:action.gate(),interest:interest.clone(),reply}))).unwrap();
    if partial { assert!(tokio::time::timeout(Duration::from_millis(35),&mut submitted).await.is_err());assert!(interest.is_writing());assert!(pauses.load(Ordering::SeqCst)>0);drop(waiting);action.retire();drop(data);bounded(scope.closed()).await;let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::Cancelled);drop(late); }
    else { let submitted=bounded(submitted).await.unwrap().unwrap();assert!(submitted.matches(&who,&scope));bounded(seen).await.unwrap();assert!(!interest.is_writing());drop(waiting);action.retire();drop(data);assert!(scope.is_open());late.send(()).unwrap();let closed=bounded(handle.logout(&who,&scope,&[])).await.unwrap();assert!(closed.io_released()); }
    assert_eq!(unknown(&temp),0);bounded(serving).await.unwrap();
}
#[tokio::test]
async fn im_reconciliation_full_read_submission_cancel_ignores_late_malformed_and_keeps_sdk_logout_join(){Box::pin(scenario(false)).await;}
#[tokio::test]
async fn im_reconciliation_possible_partial_read_cancel_closes_same_original_scope_without_user_write(){Box::pin(scenario(true)).await;}
