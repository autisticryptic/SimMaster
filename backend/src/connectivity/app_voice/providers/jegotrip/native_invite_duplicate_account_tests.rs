//! Actions-only current original CallService/Inform/SQLite claim replay after
//! the first consumption window; the original live call remains mandatory.
use super::*;
#[tokio::test]
async fn native_invite_consumed_window_replays_new_direct_batch_holders_but_release_revokes_all_late_positive(){
    Box::pin(async{
        let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());
        let account=manager.create(config(&cert.root,port,500),100).await.unwrap().account;let line=account.line_id.clone();let peer_line=line.clone();let db=temp.database.clone();let(go,wait)=tokio::sync::oneshot::channel();let(done,observed)=tokio::sync::oneshot::channel();let(released,ended)=tokio::sync::oneshot::channel();
        let serving=tokio::spawn(async move{bounded(async move{
            let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();
            peer.send(&[notify(91,NATIVE_ID),invited(93)]).await;expect_receipt(&mut peer,93,true).await;let inform=control(&next(&mut peer).await,CallOperation::Inform);assert_eq!(consumed(&db,&peer_line),0);answer(&mut peer,inform,true).await;expect_receipt(&mut peer,91,true).await;assert_eq!(consumed(&db,&peer_line),1);
            tokio::time::sleep(Duration::from_millis(700)).await;
            peer.send(&[notify(94,NATIVE_ID)]).await;expect_receipt(&mut peer,94,true).await;positive_batch_duplicate(&mut peer,&db,&peer_line).await;assert_eq!(consumed(&db,&peer_line),1);done.send(()).unwrap();
            let release=control(&next(&mut peer).await,CallOperation::Release);answer(&mut peer,release,true).await;
            // The release ticket is already locally revoked before this reply;
            // an old consumed holder is never a transferable call/media grant.
            peer.send(&[notify(95,NATIVE_ID)]).await;expect_receipt(&mut peer,95,false).await;assert_eq!(consumed(&db,&peer_line),1);released.send(()).unwrap();finish_account(&mut peer).await;
        }).await});
        bounded(manager.start(&line,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&line,100).await.unwrap().incoming_call_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(observed).await.unwrap();
        let call=manager.calls(&line).await.unwrap().into_iter().next().unwrap();assert_eq!(call.phase,"ringing");assert_eq!(call.sdk_invite_notification,Some("consumed"));assert!(!call.media_active);assert_eq!(consumed(&temp.database,&line),1);assert_eq!(manager.hangup_call(&line,account.revision,&call.id,100).await.unwrap().phase,"ended");bounded(ended).await.unwrap();
        assert!(manager.messages(&line,16).unwrap().is_empty());let stopped=bounded(manager.stop(&line,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();assert!(!temp.database.with_conn(|conn|n::account_runs::ownership_pending(conn,&line)).unwrap());
    }).await;
}
