//! Actions-only real AccountManager before-await exclusion and owner retirement.
//! Local native peer only; stored preparation data never grants SDK/Group IO.
use super::*;
fn preparations(db:&crate::platform::db::Database,line:&str)->Vec<(i64,Option<i64>)>{db.with_conn(|c|{
    let mut s=c.prepare("SELECT state,write_id FROM app_voice_relation_intents WHERE line_id=?1 ORDER BY id").map_err(|_|"fixture")?;
    let rows=s.query_map([line],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture")?;rows.collect::<Result<Vec<_>,_>>().map_err(|_|"fixture")
}).unwrap()}
#[tokio::test]async fn relation_account_preparation_is_committed_synchronously_before_controller_identity_or_other_nonce(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;}).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    let(ready,observed)=tokio::sync::oneshot::channel();let(release,releasing)=tokio::sync::oneshot::channel();let mut holding=Box::pin(manager.relation_controller_pause_for_test(&line,ready,releasing));
    tokio::select!{r=holding.as_mut()=>panic!("controller still held: {r:?}"),r=bounded(observed)=>r.unwrap()}
    let prepared=capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).unwrap();
    // No await before these assertions: the immutable reservation already owns
    // the durable nonce fence even though no controller/auth/RPC work started.
    assert_eq!(preparations(&temp.database,&line),[(0,None)]);assert_eq!(rows(&temp.database,&line),0);assert_eq!(manager.relation_resources_for_test(&line).unwrap().0,1);
    assert_eq!(capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).err(),Some("app_account_relation_preparing"));assert_eq!(capture(&manager,&line,account.revision,None,NONCE,b"other",true,true).err(),Some("app_account_relation_nonce_conflict"));assert_eq!(capture(&manager,&line,account.revision,Some(b"7-8"),NEXT,b"1_2",true,true).err(),Some("app_account_relation_preparing"));
    drop(prepared);assert_eq!(preparations(&temp.database,&line),[(2,None)]);assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));assert_eq!(capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).err(),Some("app_account_relation_nonce_retired"));
    let next=capture(&manager,&line,account.revision,None,NEXT,b"1_2",true,true).unwrap();drop(next);assert_eq!(preparations(&temp.database,&line),[(2,None),(2,None)]);assert_eq!(rows(&temp.database,&line),0);
    release.send(()).unwrap();bounded(holding).await.unwrap();finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
}).await;}
#[tokio::test]async fn relation_account_async_wait_drop_only_retires_its_owned_preparation_without_write_or_relogin(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;}).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    let(ready,observed)=tokio::sync::oneshot::channel();let(release,releasing)=tokio::sync::oneshot::channel();let mut holding=Box::pin(manager.relation_controller_pause_for_test(&line,ready,releasing));
    tokio::select!{r=holding.as_mut()=>panic!("controller still held: {r:?}"),r=bounded(observed)=>r.unwrap()}
    let prepared=capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).unwrap();let mut waiting=Box::pin(manager.finish_relation_update(prepared));
    assert!(tokio::time::timeout(Duration::from_millis(35),waiting.as_mut()).await.is_err());assert_eq!(preparations(&temp.database,&line),[(0,None)]);assert_eq!(rows(&temp.database,&line),0);drop(waiting);
    assert_eq!(preparations(&temp.database,&line),[(2,None)]);assert_eq!(rows(&temp.database,&line),0);assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));
    release.send(()).unwrap();bounded(holding).await.unwrap();assert_eq!(manager.get(&line,100).await.unwrap().phase,"hosted");finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
}).await;}
#[tokio::test]async fn relation_account_unknown_preparation_is_not_adopted_by_explicit_stop_new_sdk_or_empty_write_history(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let(first,first_done)=tokio::sync::oneshot::channel();let(second,second_done)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{
        let native_listener=listener.into_std().unwrap();let next=TcpListener::from_std(native_listener.try_clone().unwrap()).unwrap();let initial=TcpListener::from_std(native_listener).unwrap();
        let mut peer=CommonPeer::open(initial,server.clone()).await;login_host(&mut peer).await;first_done.await.unwrap();finish_account(&mut peer).await;
        let mut peer=CommonPeer::open(next,server).await;login_host(&mut peer).await;second_done.await.unwrap();finish_account(&mut peer).await;
    }).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    let old=capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).unwrap();drop(old);
    // Simulate durable data with no retained owner, not a revived runtime or
    // permission. The current run does not own this distinct old claim.
    temp.database.with_conn(|c|c.execute("UPDATE app_voice_relation_intents SET state=0,run_nonce=?2,claim_nonce=?3 WHERE line_id=?1",rusqlite::params![line,"c".repeat(32),"d".repeat(32)]).map_err(|_|"fixture unowned record")).unwrap();
    assert!(manager.relation_write_history(&line,128).unwrap().is_empty());first.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);released(&temp.database,&line);
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    assert_eq!(capture(&manager,&line,account.revision,None,NEXT,b"1_2",true,true).err(),Some("app_account_relation_preparing"));assert_eq!(preparations(&temp.database,&line),[(0,None)]);assert_eq!(rows(&temp.database,&line),0);assert_eq!(manager.relation_intent_history(&line,128).unwrap()[0].state,"preparing");
    second.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);assert_eq!(preparations(&temp.database,&line),[(0,None)]);
}).await;}
