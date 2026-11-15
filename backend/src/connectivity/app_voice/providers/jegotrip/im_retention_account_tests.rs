//! Actions-only original Common account, durable association pressure/restart
//! and explicit delete. No real SDK/410/phone or outbound service operation.
use super::*;
fn association_count(db:&crate::platform::db::Database)->i64{db.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_notifications",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
#[tokio::test]
async fn im_account_eighty_completed_notifications_leave_live_budget_without_forgetting_persistent_identity(){
    Box::pin(async{
        let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,3000),100).await.unwrap().account;let db=temp.database.clone();let(go,wait)=tokio::sync::oneshot::channel();let(done,observed)=tokio::sync::oneshot::channel();
        let serving=tokio::spawn(async move{bounded(async move{
            let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();peer.send(&[message(91,7,true)]).await;next_reply(&mut peer,91,true).await;
            for rpc in 92..=171{peer.send(&[hint(rpc,rpc as u64,7)]).await;next_reply(&mut peer,rpc,true).await;assert_eq!(association_count(&db),i64::from(rpc-91));assert_eq!(count(&db),1);}
            // This exact first association was removed from live memory, not
            // forgotten or given a new consumer deadline by the fresh RPC.
            peer.send(&[hint(172,92,7)]).await;next_reply(&mut peer,172,true).await;assert_eq!(association_count(&db),80);done.send(()).unwrap();finish_account(&mut peer).await;
        }).await});
        bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&account.line_id,100).await.unwrap().incoming_message_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(observed).await.unwrap();assert_eq!(manager.im_messages(&account.line_id,16).unwrap().len(),1);assert!(manager.messages(&account.line_id,16).unwrap().is_empty());assert_eq!(temp.database.get_recent_app_events(Some(&account.line_id),128).unwrap().iter().filter(|e|e.event_type=="im.received").count(),1);
        let stopped=bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();assert_eq!(association_count(&temp.database),0);assert_eq!(count(&temp.database),1);assert!(!temp.database.with_conn(|c|n::account_runs::ownership_pending(c,&account.line_id)).unwrap());
    }).await;
}
#[tokio::test]
async fn im_account_stopped_run_drops_only_its_associations_and_new_original_login_keeps_deleted_message_tombstone(){
    Box::pin(async{
        let cert=Certificates::generate("IP:127.0.0.1");let front=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=front.local_addr().unwrap().port();let first_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let second_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let targets=[first_listener.local_addr().unwrap().port(),second_listener.local_addr().unwrap().port()];let server=cert.server_config(&TLS13);
        let relay=tokio::spawn(async move{let mut tasks=Vec::new();for target in targets{let(mut source,_)=front.accept().await.unwrap();let mut dest=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,target)).await.unwrap();tasks.push(tokio::spawn(async move{tokio::io::copy_bidirectional(&mut source,&mut dest).await.unwrap();}));}for task in tasks{task.await.unwrap();}});
        let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,3000),100).await.unwrap().account;let mut first_epoch=None;
        for(round,listener)in[(0,first_listener),(1,second_listener)]{
            let server=server.clone();let(go,wait)=tokio::sync::oneshot::channel();let(done,observed)=tokio::sync::oneshot::channel();
            let serving=tokio::spawn(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();
                peer.send(&[message(91,7,true)]).await;next_reply(&mut peer,91,true).await;
                // The second run may legitimately reuse a notification ID but
                // cannot reuse a scope token or restore the deleted text row.
                if round==1{peer.send(&[message(92,8,true)]).await;next_reply(&mut peer,92,true).await;}
                peer.send(&[hint(93,99,if round==0{7}else{8})]).await;next_reply(&mut peer,93,true).await;done.send(()).unwrap();finish_account(&mut peer).await;
            }).await});
            bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();bounded(async{while !manager.get(&account.line_id,100).await.unwrap().incoming_message_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(observed).await.unwrap();
            let epoch:String=temp.database.with_conn(|c|c.query_row("SELECT scope_epoch FROM app_voice_im_notifications",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();if let Some(first)=&first_epoch{assert_ne!(first,&epoch);}else{first_epoch=Some(epoch);}
            let list=manager.im_messages(&account.line_id,16).unwrap();assert_eq!(list.len(),1);assert_eq!(list[0].message_index,if round==0{"7"}else{"8"});if round==0{assert!(manager.delete_im_message(&account.line_id,list[0].id).unwrap());}
            let stopped=bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();assert_eq!(association_count(&temp.database),0);assert_eq!(count(&temp.database),i64::from(round+1));
        }
        bounded(relay).await.unwrap();assert_eq!(temp.database.get_recent_app_events(Some(&account.line_id),64).unwrap().iter().filter(|e|e.event_type=="im.received").count(),2);assert!(manager.messages(&account.line_id,16).unwrap().is_empty());manager.delete(&account.line_id,account.revision).await.unwrap();assert_eq!(count(&temp.database),0);
    }).await;
}
