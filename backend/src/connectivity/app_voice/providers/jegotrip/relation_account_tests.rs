//! Actions-only actual account -> original Router/TLS/Common relation WRITE.
//! Fresh local database and loopback peer only; no SO/device/real business API.
#[path="relation_intent_account_tests.rs"]
mod admission;
use super::*;
use n::{group_directory_data::Relation,group_relation_updates::Removal,
    relation_update_policy::NativeRelationUpdateConfig as Policy,account_runtime::PreparedRelationUpdate};
use std::time::Instant;
const NONCE:&str="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEXT:&str="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NOTE:&[u8]=b"private relation remark";
fn configuration(root:&str,port:u16,enabled:bool)->AccountConfig{
    let mut cfg=config(root,port,false);cfg.profile.im_sync=None;
    cfg.profile.relation_updates=enabled.then_some(Policy{additions:true,updates:true,removals:true});
    assert!(cfg.profile.im_send.is_none()&&cfg.profile.im_receipts.is_none()&&cfg.profile.im_user_read.is_none()&&cfg.profile.im_read_status.is_none());
    assert!(!cfg.profile.native_receivers.as_ref().unwrap().im_messages.as_ref().unwrap().directory_queries);cfg
}
fn capture(manager:&AccountManager,line:&str,revision:u64,group:Option<&[u8]>,nonce:&str,label:&[u8],consent:bool,write_consent:bool)->Result<PreparedRelationUpdate,&'static str>{
    manager.prepare_relation_update(line,revision,group,&[Relation{label,flags:1,first:NOTE,second:b"",properties:vec![]}],&[],&[Removal{label:b"9_9",flag:false}],nonce,consent,write_consent)
}
async fn change(manager:&AccountManager,line:&str,revision:u64,group:Option<&[u8]>,nonce:&str,label:&[u8])->Result<n::relation_journal::RelationWriteView,&'static str>{
    let prepared=capture(manager,line,revision,group,nonce,label,true,true)?;manager.finish_relation_update(prepared).await
}
fn rows(db:&crate::platform::db::Database,line:&str)->i64{db.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_relation_writes WHERE line_id=?1",[line],|r|r.get(0)).map_err(|_|"fixture relation count")).unwrap()}
fn row(db:&crate::platform::db::Database,line:&str)->(i64,i64,Option<i32>,Option<i64>,Option<i64>){db.with_conn(|c|c.query_row("SELECT id,state,request_id,base_time,update_time FROM app_voice_relation_writes WHERE line_id=?1",[line],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).map_err(|_|"fixture relation row")).unwrap()}
async fn phase(manager:&AccountManager,line:&str,wanted:&str){bounded(async{while manager.get(line,100).await.unwrap().phase!=wanted{tokio::task::yield_now().await;}}).await;}
fn released(db:&crate::platform::db::Database,line:&str){assert!(!db.with_conn(|c|n::account_runs::ownership_pending(c,line)).unwrap());let leases:i64=db.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases WHERE line_id=?1",[line],|r|r.get(0)).map_err(|_|"fixture leases")).unwrap();assert_eq!(leases,0);}
fn no_other_business(manager:&AccountManager,db:&crate::platform::db::Database,line:&str){
    for table in ["app_voice_im_messages","app_voice_im_outbox","app_voice_im_read_receipts","app_voice_im_user_reads","app_voice_im_read_status_actions"]{
        let count:i64=db.with_conn(|c|c.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE line_id=?1"),[line],|r|r.get(0)).map_err(|_|"fixture other business")).unwrap();assert_eq!(count,0,"relation permission never becomes IM/READ/SEND authority");
    }
    assert!(manager.messages(line,16).unwrap().is_empty());assert!(manager.im_messages(line,16).unwrap().is_empty());assert!(db.get_call_history_for_line(line,16,0).unwrap().is_empty());
}
async fn call(peer:&mut CommonPeer,group:bool)->i32{
    let p=query(peer,n::group_relation_updates::SET_RELATIONS_METHOD).await;assert_eq!(p[0],0);let request=decode_request(&p[1..]).unwrap();
    assert_eq!(request.method,b"setRelations.GroupServer.Group");assert_eq!(&request.arguments[..5],&[0,0,1,0,0]);let mut r=Reader::versioned(request.arguments).unwrap();
    assert_eq!(r.bytes().unwrap(),if group{b"1-2".as_slice()}else{b"fixture-user".as_slice()},"buddy uses current returned SDK UID, never imported account or mainAccount");
    assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"1_2");assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),NOTE);assert_eq!(r.bytes().unwrap(),b"");assert!(r.properties().unwrap().is_empty());
    assert_eq!(r.u32().unwrap(),0);assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"9_9");assert!(!r.boolean().unwrap());r.finish().unwrap();request.call_id
}
fn result(id:i32,accepted:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.i64(i64::MIN).unwrap();w.i64(i64::MAX).unwrap();w.bytes(b"").unwrap();reply(id,w.finish().expose_to_transport())}
async fn replied(group:bool,accepted:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let db=temp.database.clone();let peer_line=line.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{
        let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;let id=call(&mut peer,group).await;let before=row(&db,&peer_line);assert!(before.0>0);assert_eq!((before.1,before.2,before.3,before.4),(0,Some(id),None,None));
        peer.send(&[result(id,accepted)]).await;finishing.await.unwrap();finish_account(&mut peer).await;
    }).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    assert_eq!(manager.get(&line,100).await.unwrap().relation_updates_supported,Some(true));assert!(manager.relation_write_history(&line,128).unwrap().is_empty());
    let group_id=group.then_some(b"1-2".as_slice());for(consent,write)in[(false,false),(false,true),(true,false)]{assert_eq!(capture(&manager,&line,account.revision,group_id,NONCE,b"1_2",consent,write).err(),Some("app_account_relation_consent_required"));}
    assert_eq!(capture(&manager,&line,account.revision+1,group_id,NONCE,b"1_2",true,true).err(),Some("app_account_revision_conflict"));assert_eq!(rows(&temp.database,&line),0);assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));
    let view=bounded(change(&manager,&line,account.revision,group_id,NONCE,b"1_2")).await.unwrap();assert_eq!(view.state,if accepted{"confirmed"}else{"rejected"});assert_eq!(view.target_kind,if group{"group"}else{"buddy"});assert_eq!((view.additions,view.updates,view.removals),(1,0,1));assert_eq!(view.base_time.as_deref(),Some("-9223372036854775808"));assert_eq!(view.update_time.as_deref(),Some("9223372036854775807"));
    let before=row(&temp.database,&line);assert_eq!(before.0,view.id);assert_eq!((before.1,before.3,before.4),(if accepted{1}else{2},Some(i64::MIN),Some(i64::MAX)));
    for _ in 0..2{let history=bounded(change(&manager,&line,account.revision,group_id,NONCE,b"1_2")).await.unwrap();assert_eq!(history.id,view.id);assert_eq!(history.nonce_tag,view.nonce_tag);assert_eq!(row(&temp.database,&line),before);}
    assert_eq!(bounded(change(&manager,&line,account.revision,group_id,NONCE,b"different-label")).await.err(),Some("app_account_relation_nonce_conflict"));assert_eq!(row(&temp.database,&line),before);assert_eq!(rows(&temp.database,&line),1);
    let public=serde_json::to_string(&manager.relation_write_history(&line,128).unwrap()).unwrap();for secret in [NONCE,"fixture-user","fixture-auth","1_2","9_9","private relation","identity_key","arguments_hash","scope_epoch","user_nonce"]{assert!(!public.contains(secret));}
    let events=temp.database.get_recent_app_events(Some(&line),32).unwrap();assert_eq!(events.iter().filter(|e|e.event_type=="app_voice.relations_updated").count(),1);assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));no_other_business(&manager,&temp.database,&line);
    finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);assert_eq!(row(&temp.database,&line),before);
}
#[tokio::test]async fn relation_account_original_buddy_write_is_independent_and_commits_one_signed_result_event(){Box::pin(replied(false,true)).await;}
#[tokio::test]async fn relation_account_original_group_identifier_is_not_uid_p2p_send_or_directory_authority(){Box::pin(replied(true,true)).await;}
#[tokio::test]async fn relation_account_original_false_result_is_rejected_and_same_nonce_is_history_only(){Box::pin(replied(false,false)).await;}
#[tokio::test]async fn relation_account_legacy_default_and_native_im_installation_do_not_authorize_a_relation_write(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,false),100).await.unwrap().account;let line=account.line_id.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;}).await}));
    assert_eq!(capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).err(),Some("app_account_relation_scope_invalid"));bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    assert_eq!(manager.get(&line,100).await.unwrap().relation_updates_supported,Some(false));assert_eq!(capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).err(),Some("app_account_relation_not_installed"));assert_eq!(rows(&temp.database,&line),0);assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));no_other_business(&manager,&temp.database,&line);
    finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
}).await;}
async fn waiting(retire:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let mut cfg=configuration(&cert.root,port,true);cfg.profile.rpc_timeout_ms=2000;
    let account=manager.create(cfg,100).await.unwrap().account;let line=account.line_id.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;}).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    let(ready,observed)=tokio::sync::oneshot::channel();let(release,releasing)=tokio::sync::oneshot::channel();let mut holding=Box::pin(manager.relation_controller_pause_for_test(&line,ready,releasing));
    tokio::select!{value=holding.as_mut()=>panic!("original controller is not released: {value:?}"),value=bounded(observed)=>value.unwrap()}
    let started=Instant::now();let prepared=capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).unwrap();assert_eq!(manager.relation_resources_for_test(&line).unwrap().0,1);assert!(manager.relation_resources_for_test(&line).unwrap().1>0);assert_eq!(rows(&temp.database,&line),0,"reserved input is not a SQL intent or current UID/WRITE grant");
    let mut changing=Box::pin(manager.finish_relation_update(prepared));
    if retire{manager.relation_retire_parent_for_test(&line).unwrap();}
    assert_eq!(bounded(changing.as_mut()).await.err(),Some(if retire{"app_account_relation_scope_invalid"}else{"app_account_relation_timeout"}));drop(changing);
    if !retire{assert!(started.elapsed()>=Duration::from_secs(2));assert!(started.elapsed()<Duration::from_secs(3),"controller wait does not renew the original two-second action budget");}
    assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));assert_eq!(rows(&temp.database,&line),0);
    release.send(()).unwrap();bounded(holding).await.unwrap();assert_eq!(manager.get(&line,100).await.unwrap().phase,"hosted");no_other_business(&manager,&temp.database,&line);
    finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
}
#[tokio::test]async fn relation_account_controller_wait_is_precharged_and_fixed_deadline_admits_no_intent(){Box::pin(waiting(false)).await;}
#[tokio::test]async fn relation_account_parent_retirement_during_controller_wait_admits_no_intent_or_group_io(){Box::pin(waiting(true)).await;}
async fn no_intent(mode:u8){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;}).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    if mode==2{temp.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET private_config=X'00' WHERE line_id=?1",[&line]).map_err(|_|"fixture config change")).unwrap();assert_eq!(capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).err(),Some("app_account_relation_scope_invalid"));}
    else{
        let prepared=capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).unwrap();assert_eq!(manager.relation_resources_for_test(&line).unwrap().0,1);
        if mode==0{drop(prepared);}else{temp.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET private_config=X'00' WHERE line_id=?1",[&line]).map_err(|_|"fixture config change")).unwrap();assert_eq!(bounded(manager.finish_relation_update(prepared)).await.err(),Some("app_account_relation_scope_invalid"));}
    }
    assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));assert_eq!(rows(&temp.database,&line),0);assert!(manager.relation_write_history(&line,128).unwrap().is_empty());no_other_business(&manager,&temp.database,&line);
    finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
}
#[tokio::test]async fn relation_account_dropped_prepared_input_releases_charge_without_intent_or_frame(){Box::pin(no_intent(0)).await;}
#[tokio::test]async fn relation_account_same_revision_config_change_after_capture_is_not_new_write_authority(){Box::pin(no_intent(1)).await;}
#[tokio::test]async fn relation_account_changed_blob_before_admission_cannot_replace_original_runtime_configuration(){Box::pin(no_intent(2)).await;}
#[tokio::test]async fn relation_account_http_scratch_is_same_scope_accounting_not_identity_nonce_or_write_permission(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(configuration(&cert.root,port,true),100).await.unwrap().account;let line=account.line_id.clone();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{let mut peer=CommonPeer::open(listener,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;}).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    assert_eq!(manager.reserve_relation_http_scratch(&line,account.revision,usize::MAX,true,true).err(),Some("app_account_relation_capacity"));assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));
    let scratch=manager.reserve_relation_http_scratch(&line,account.revision,256,true,true).unwrap();assert_eq!(manager.relation_resources_for_test(&line).unwrap().0,1);
    let prepared=capture(&manager,&line,account.revision,None,NONCE,b"1_2",true,true).unwrap();assert!(scratch.matches(&prepared));assert_eq!(manager.relation_resources_for_test(&line).unwrap().0,2);assert_eq!(rows(&temp.database,&line),0);
    drop(prepared);assert_eq!(manager.relation_resources_for_test(&line).unwrap().0,1);drop(scratch);assert_eq!(manager.relation_resources_for_test(&line).unwrap(),(0,0));no_other_business(&manager,&temp.database,&line);
    finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);
}).await;}
#[tokio::test]async fn relation_account_unknown_write_survives_stop_restart_and_blocks_new_nonce_other_group(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let mut cfg=configuration(&cert.root,port,true);cfg.profile.rpc_timeout_ms=2000;
    let account=manager.create(cfg,100).await.unwrap().account;let line=account.line_id.clone();let db=temp.database.clone();let peer_line=line.clone();let(seen,observed)=tokio::sync::oneshot::channel();let(finish,finishing)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(Box::pin(async move{bounded(async move{
        let native_listener=listener.into_std().unwrap();let second=TcpListener::from_std(native_listener.try_clone().unwrap()).unwrap();let first=TcpListener::from_std(native_listener).unwrap();
        let mut peer=CommonPeer::open(first,server.clone()).await;login_host(&mut peer).await;let id=call(&mut peer,false).await;let before=row(&db,&peer_line);assert_eq!((before.1,before.2),(0,Some(id)));seen.send(()).unwrap();
        while let Some(p)=peer.next().await{poll(&mut peer,&p).await;}assert_eq!(row(&db,&peer_line),before,"original unknown intent survives closed IO");
        let mut peer=CommonPeer::open(second,server).await;login_host(&mut peer).await;finishing.await.unwrap();finish_account(&mut peer).await;assert_eq!(row(&db,&peer_line),before);
    }).await}));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    let mut changing=Box::pin(change(&manager,&line,account.revision,None,NONCE,b"1_2"));tokio::select!{value=changing.as_mut()=>panic!("original peer has no result: {value:?}"),value=bounded(observed)=>value.unwrap()}
    let error=bounded(changing).await.err().unwrap();assert!(["app_account_relation_uncertain","app_account_relation_timeout"].contains(&error),"only the original bounded failure category, never success or not_sent");
    phase(&manager,&line,"disconnected").await;assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);released(&temp.database,&line);let before=row(&temp.database,&line);assert_eq!((before.1,before.3,before.4),(0,None,None));
    bounded(manager.start(&line,account.revision,100)).await.unwrap();phase(&manager,&line,"hosted").await;
    let historical=bounded(change(&manager,&line,account.revision,None,NONCE,b"1_2")).await.unwrap();assert_eq!(historical.id,before.0);assert_eq!(historical.state,"uncertain");
    assert_eq!(bounded(change(&manager,&line,account.revision,Some(b"7-8"),NEXT,b"1_2")).await.err(),Some("app_account_relation_uncertain"));assert_eq!(rows(&temp.database,&line),1);assert_eq!(row(&temp.database,&line),before);no_other_business(&manager,&temp.database,&line);
    finish.send(()).unwrap();assert!(!bounded(manager.stop(&line,account.revision,100)).await.unwrap().io_cleanup_required);bounded(serving).await.unwrap();released(&temp.database,&line);assert_eq!(row(&temp.database,&line),before);
}).await;}
