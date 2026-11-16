//! Actions-only full original user WRITE submission, cancellation, SQL failure
//! and actual owned IO joins. No readonly wait is used to retire a WRITE.
use super::*;
#[derive(Clone,Copy)]
enum Ending{WaitDrop,ParentRetired,ReplySqlFailed}
fn state(temp:&TempDatabase)->(i64,Option<i32>){temp.database.with_conn(|c|c.query_row("SELECT state,request_id FROM app_voice_im_user_reads",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture")).unwrap()}
async fn full_submission(ending:Ending){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let policy=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);
    let(go,ready)=oneshot::channel();let(observed,seen)=oneshot::channel();let(release,reply_ready)=oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        let mut peer=CommonPeer::open(listener,server).await;peer_ready(&mut peer,ready).await;
        let packet=peer.next().await.unwrap();let id=query(&packet,n::im_user_read_data::MARK_READ_METHOD);observed.send(id).unwrap();
        if reply_ready.await.is_ok(){peer.send(&[reply(id,&[0,1,0,0,0,0])]).await;}
        assert!(peer.next().await.is_none(),"unknown original user WRITE is never replayed or followed by another frame");
    }).await});
    let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);let connection=connected(&policy,who.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();auth(&handle,&mut events,&who,&scope,true).await;go.send(()).unwrap();
    let parent=ImReceiptGate::fresh();let(_action,permit)=setup(&handle,&mut events,&who,&scope,&temp,run,parent.clone()).await;let commands=handle.command_handle();let mut writing=Box::pin(commands.user_read_write(&who,&scope,permit.clone()));
    let id=tokio::select!{r=writing.as_mut()=>panic!("original peer has not replied: {r:?}"),r=seen=>r.unwrap()};
    let submitted=async{while !permit.write_was_submitted(){tokio::task::yield_now().await;}};
    tokio::select!{r=writing.as_mut()=>panic!("original private result still absent: {r:?}"),_=bounded(submitted)=>{}};
    assert_eq!(state(&temp),(0,Some(id)));assert!(scope.is_open());
    let reason=match ending{
        Ending::WaitDrop=>{drop(release);drop(writing);RuntimeError::Cancelled},
        Ending::ParentRetired=>{drop(release);parent.retire();assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Cancelled));RuntimeError::Cancelled},
        Ending::ReplySqlFailed=>{
            temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_original_user_reply BEFORE UPDATE OF state ON app_voice_im_user_reads WHEN NEW.state IN(1,2) BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();
            release.send(()).unwrap();assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Protocol));RuntimeError::Protocol
        },
    };
    bounded(scope.closed()).await;let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,reason);assert_eq!(state(&temp),(0,Some(id)));bounded(serving).await.unwrap();
}
#[tokio::test]
async fn user_read_io_full_submission_wait_drop_preserves_unknown_and_joins_the_same_original_io(){Box::pin(full_submission(Ending::WaitDrop)).await;}
#[tokio::test]
async fn user_read_io_full_submission_parent_retirement_keeps_unknown_with_actual_original_joins(){Box::pin(full_submission(Ending::ParentRetired)).await;}
#[tokio::test]
async fn user_read_io_matched_private_reply_sql_failure_keeps_unknown_and_stops_original_io_without_replay(){Box::pin(full_submission(Ending::ReplySqlFailed)).await;}
