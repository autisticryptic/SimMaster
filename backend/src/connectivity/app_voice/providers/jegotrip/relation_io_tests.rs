//! Actions-only original Router/TCP/TLS/Common relation WRITE and physical Join.
//! No original SO/SDK, device, Provider API, live target or detached protocol peer.
use super::*;
#[path="group_management_io_tests.rs"]
mod management;
#[path="group_create_io_tests.rs"]
mod creation;
use n::{account_test_support::TempDatabase,group_directory_data::Relation,
    group_relation_updates::Removal,relation_journal::{RelationAction,RelationMessage,RelationPermit,PreparedRelation},
    relation_update_policy::NativeRelationUpdateConfig as Policy};
const NONCE:&str="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NOTE:&[u8]=b"private original relation note";
fn policy()->Policy{Policy{additions:true,updates:true,removals:true}}
fn additions()->[Relation<'static>;1]{[Relation{label:b"1_2",flags:1,first:NOTE,second:b"",properties:vec![RpcProperty{key:b"opaque",value:b"private-property"}]}]}
fn updates()->[Relation<'static>;1]{[Relation{label:b"3_4",flags:11,first:b"private update",second:b"private second",properties:vec![]}]}
fn removals()->[Removal<'static>;1]{[Removal{label:b"5_6",flag:false}]}
fn original(temp:&TempDatabase,root:&str)->(AttemptOwner,Arc<n::account_runs::RunToken>){
    let line="app-account-0123456789abcdef0123456789abcdef";
    let mut cfg=n::account_config::tests::native_fixture(root,"127.0.0.1",1);
    cfg.credentials.sdk.expires_at_unix_seconds=None;cfg.profile.incoming=None;cfg.profile.client_ex=true;
    cfg.profile.native_receivers=Some(super::config(true));cfg.profile.relation_updates=Some(policy());
    temp.database.with_conn(|c|n::account_store::create(c,line,&cfg,100)).unwrap();
    let run=Arc::new(temp.database.with_conn(|c|n::account_runs::register_runtime(c,line,1,&n::account_runtime::local_nonce().unwrap())).unwrap());
    let mut session=n::session::HostingSession::new(line.into());session.import(n::tests::imported(),100).unwrap();
    (session.begin_login(100).unwrap(),run)
}
async fn prepare(handle:&RuntimeHandle,who:&AttemptOwner,scope:&RpcConnectionScope,temp:&TempDatabase,run:Arc<n::account_runs::RunToken>,parent:Arc<ImReceiptGate>,group:Option<&[u8]>,timeout:Duration)->(Arc<RelationAction>,Arc<RelationPermit>){
    let commands=handle.command_handle();let binding=bounded(commands.original_im_binding(who,scope)).await.unwrap();
    let identity=bounded(commands.original_relation_identity(who,scope)).await.unwrap();
    let message=RelationMessage::capture(scope,group,&additions(),&updates(),&removals(),NONCE,policy()).unwrap();
    let PreparedRelation::Fresh(action)=RelationAction::original(temp.database.clone(),run,binding,identity,parent,message,Instant::now()+timeout,policy()).unwrap()else{panic!("one original fresh relation intent")};
    let permit=action.permit(policy()).unwrap();(action,permit)
}
fn row(temp:&TempDatabase)->(i64,Option<i32>,Option<i64>,Option<i64>){
    temp.database.with_conn(|c|c.query_row("SELECT state,request_id,base_time,update_time FROM app_voice_relation_writes",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|_|"fixture relation row")).unwrap()
}
fn packet(bytes:&[u8],expected:i32,identifier:&[u8]){
    assert_eq!(query(bytes,n::group_relation_updates::SET_RELATIONS_METHOD),expected);
    let request=decode_request(&bytes[1..]).unwrap();assert_eq!(&request.arguments[..5],&[0,0,1,0,0]);
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),identifier);
    assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"1_2");assert_eq!(r.u32().unwrap(),1);
    assert_eq!(r.bytes().unwrap(),NOTE);assert_eq!(r.bytes().unwrap(),b"");let properties=r.properties().unwrap();
    assert_eq!(properties.len(),1);assert_eq!(properties[0].key,b"opaque");assert_eq!(properties[0].value,b"private-property");
    assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"3_4");assert_eq!(r.u32().unwrap(),11);
    assert_eq!(r.bytes().unwrap(),b"private update");assert_eq!(r.bytes().unwrap(),b"private second");assert!(r.properties().unwrap().is_empty());
    assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"5_6");assert!(!r.boolean().unwrap(),"original removal defaults false, never an inferred grant");r.finish().unwrap();
}
fn result(accepted:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.i64(i64::MIN).unwrap();w.i64(i64::MAX).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
async fn eof(peer:&mut CommonPeer){
    while let Some(p)=peer.next_input_only().await{assert!(matches!(p.as_slice(),[2]|[14]|[4]),"only exact empty original Common controls, no relation replay or late business frame");}
}
async fn logout(peer:&mut CommonPeer,id:i32){
    let bytes=peer.next().await.unwrap();assert_eq!(bytes[0],0);let r=decode_request(&bytes[1..]).unwrap();
    assert_eq!(r.method,n::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());assert_eq!(r.call_id,id,"no cancelled intent or history observation allocates an extra RPC ID");
    assert_eq!(r.metadata.iter().find(|v|v.key==b"token").unwrap().value,b"fixture-auth");
    peer.send(&[reply(id,&[0,1,0,0,0,0])]).await;eof(peer).await;
}
fn no_event(temp:&TempDatabase){assert!(temp.database.get_recent_app_events(Some("app-account-0123456789abcdef0123456789abcdef"),16).unwrap().is_empty());}
fn held(scope:&RpcConnectionScope){assert_eq!(scope.native_rpc_resources().0,3,"input, current private identity and immutable arguments remain held after actual IO Join");assert!(scope.native_rpc_resources().1>0);}
async fn confirmed(group:bool,accepted:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let route=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);
    let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);let db=temp.database.clone();
    let identifier=if group{b"1-2".as_slice()}else{b"fixture-user".as_slice()};
    let peer=async move{
        let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let p=peer.next().await.unwrap();packet(&p,2,identifier);
        let before:(i64,Option<i32>)=db.with_conn(|c|c.query_row("SELECT state,request_id FROM app_voice_relation_writes",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture")).unwrap();assert_eq!(before,(0,Some(2)),"durable possible WRITE precedes physical peer result");
        peer.send(&[reply(2,&result(accepted))]).await;logout(&mut peer,3).await;
    };
    let client=async{
        let conn=connected(&route,who.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();
        auth(&handle,&mut events,&who,&scope,true).await;
        let group_id=group.then_some(b"1-2".as_slice());let parent=ImReceiptGate::fresh();
        let(action,permit)=prepare(&handle,&who,&scope,&temp,run.clone(),parent.clone(),group_id,Duration::from_secs(3)).await;
        let view=bounded(handle.command_handle().update_relations(&who,&scope,permit.clone())).await.unwrap();
        assert_eq!(view.state,if accepted{"confirmed"}else{"rejected"});assert_eq!(view.target_kind,if group{"group"}else{"buddy"});
        assert_eq!((view.additions,view.updates,view.removals),(1,1,1));assert_eq!(view.base_time.as_deref(),Some("-9223372036854775808"));assert_eq!(view.update_time.as_deref(),Some("9223372036854775807"));
        assert_eq!(row(&temp),(if accepted{1}else{2},Some(2),Some(i64::MIN),Some(i64::MAX)));assert!(scope.is_open());assert!(events.try_recv().is_err(),"no fabricated runtime/SDK/message readiness callback");
        let stored=temp.database.get_recent_app_events(Some(run.line_id()),16).unwrap();assert_eq!(stored.len(),1);assert_eq!(stored[0].event_type,"app_voice.relations_updated");
        let message=RelationMessage::capture(&scope,group_id,&additions(),&updates(),&removals(),NONCE,policy()).unwrap();
        let commands=handle.command_handle();let binding=bounded(commands.original_im_binding(&who,&scope)).await.unwrap();let identity=bounded(commands.original_relation_identity(&who,&scope)).await.unwrap();
        let PreparedRelation::Replay(history)=RelationAction::original(temp.database.clone(),run.clone(),binding,identity,parent,message,Instant::now()+Duration::from_secs(3),policy()).unwrap()else{panic!("same nonce is stored DATA, no new permit")};assert_eq!(history.id,view.id);assert_eq!(history.state,view.state);
        let public=serde_json::to_string(&history).unwrap();for hidden in ["fixture-user","1_2","3_4","5_6","private-property",NONCE,"fixture-auth","identity_key","arguments_hash","intent_hash"]{assert!(!public.contains(hidden));}
        let closed=bounded(handle.logout(&who,&scope,&[])).await.unwrap();assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::LoggedOut);held(&scope);
        drop(permit);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
    };
    tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)));
}
#[tokio::test]async fn relation_io_original_buddy_uid_and_private_signed_result_commit_once_then_logout_join(){Box::pin(confirmed(false,true)).await;}
#[tokio::test]async fn relation_io_original_explicit_group_maps_and_false_removal_are_not_a_p2p_or_uid_conversion(){Box::pin(confirmed(true,true)).await;}
#[tokio::test]async fn relation_io_original_false_result_is_terminal_rejected_without_retry_or_new_id(){Box::pin(confirmed(false,false)).await;}
async fn queued(parent_retirement:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let route=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);
    let peer=async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;logout(&mut peer,2).await;};
    let client=async{
        let conn=connected(&route,who.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&who,&scope,true).await;
        let parent=ImReceiptGate::fresh();let(action,permit)=prepare(&handle,&who,&scope,&temp,run.clone(),parent.clone(),None,Duration::from_secs(3)).await;
        let(reply,data)=oneshot::channel();let receipt=handle.command_handle().enqueue(&who,&scope,Operation::RelationUpdate(Box::new(RelationWork{permit:permit.clone(),reply}))).unwrap();
        // No await: current actor cannot dequeue/allocate before this retirement.
        drop(data);if parent_retirement{parent.retire();}else{action.cancel();}
        assert_eq!(bounded(receipt).await.unwrap().err(),Some(RuntimeError::Scope));assert_eq!(row(&temp),(3,None,None,None));assert!(permit.retired());assert!(!permit.closed_original_io());assert!(scope.is_open());no_event(&temp);
        let closed=bounded(handle.logout(&who,&scope,&[])).await.unwrap();assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::LoggedOut);held(&scope);drop(permit);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
    };tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)));
}
#[tokio::test]async fn relation_io_queued_result_drop_is_not_sent_without_rpc_id_or_sdk_disconnect(){Box::pin(queued(false)).await;}
#[tokio::test]async fn relation_io_parent_retirement_before_first_poll_is_not_sent_and_preserves_logout_join(){Box::pin(queued(true)).await;}
async fn partial(parent_retirement:bool,deadline:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let route=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);
    let peer=async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;eof(&mut peer).await;};
    let client=async{
        let mut conn=connected(&route,who.clone(),Instant::now()+Duration::from_secs(6)).await;
        let prefix=n::router_runtime::tests::auxiliary_io::AuxiliaryIoTest::original_unarmed(1);
        n::common_runtime::tests::gate_auxiliary_prefix(conn.common.as_mut().unwrap(),prefix.clone());let scope=conn.connection_scope();assert!(prefix.scope().matches(&scope));
        let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&who,&scope,true).await;let parent=ImReceiptGate::fresh();
        let(action,permit)=prepare(&handle,&who,&scope,&temp,run.clone(),parent.clone(),None,Duration::from_secs(if deadline{1}else{3})).await;prefix.arm();let started=Instant::now();
        let commands=handle.command_handle();let mut writing=Box::pin(commands.update_relations(&who,&scope,permit.clone()));
        tokio::select!{result=writing.as_mut()=>panic!("partial original WRITE cannot complete: {result:?}"),_=bounded(prefix.paused())=>{}}
        assert_eq!(prefix.accepted.load(Ordering::SeqCst),1);assert!(prefix.pauses.load(Ordering::SeqCst)>0);assert!(!prefix.dropped.load(Ordering::SeqCst));let before=row(&temp);assert_eq!(before,(0,Some(2),None,None));assert!(permit.current(2));
        let reason=if deadline{assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Timeout));assert!(Instant::now()>=action.deadline());assert!(started.elapsed()<Duration::from_secs(2),"original one-second action deadline is not renewed to IO or RPC timeout");RuntimeError::Timeout}
            else if parent_retirement{parent.retire();assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Cancelled));RuntimeError::Cancelled}
            else{drop(writing);RuntimeError::Cancelled};
        bounded(scope.closed()).await;let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,reason);assert!(permit.closed_original_io());assert!(!permit.current(2));
        assert!(prefix.dropped.load(Ordering::SeqCst),"same main fd Drop precedes IO Join");assert_eq!(prefix.accepted.load(Ordering::SeqCst),1,"no late ciphertext byte or retried relation frame");assert_eq!(row(&temp),before);no_event(&temp);held(&scope);drop(permit);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));assert_eq!(row(&temp),before);
    };tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)));
}
#[tokio::test]async fn relation_io_partial_ciphertext_wait_drop_keeps_unknown_and_joins_same_fd(){Box::pin(partial(false,false)).await;}
#[tokio::test]async fn relation_io_parent_retirement_during_partial_write_blocks_late_bytes_and_keeps_unknown(){Box::pin(partial(true,false)).await;}
#[tokio::test]async fn relation_io_original_action_deadline_bounds_partial_write_without_renewing_actor_budget(){Box::pin(partial(false,true)).await;}
#[derive(Clone,Copy)]enum End{WaitDrop,Parent,SqlFailure,Deadline}
async fn submitted(end:End){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let route=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);
    let(seen,observed)=oneshot::channel();let(release,releasing)=oneshot::channel();
    let peer=async move{
        let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let p=peer.next().await.unwrap();packet(&p,2,b"fixture-user");seen.send(()).unwrap();
        if releasing.await.is_ok(){peer.send(&[reply(2,&result(true))]).await;}eof(&mut peer).await;
    };
    let client=async{
        let mut conn=connected(&route,who.clone(),Instant::now()+Duration::from_secs(6)).await;
        let physical=n::router_runtime::tests::auxiliary_io::AuxiliaryIoTest::original_unarmed(1);
        n::common_runtime::tests::gate_auxiliary_prefix(conn.common.as_mut().unwrap(),physical.clone());let scope=conn.connection_scope();assert!(physical.scope().matches(&scope));
        let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&who,&scope,true).await;let parent=ImReceiptGate::fresh();
        let(action,permit)=prepare(&handle,&who,&scope,&temp,run.clone(),parent.clone(),None,Duration::from_secs(if matches!(end,End::Deadline){1}else{3})).await;
        let commands=handle.command_handle();let mut writing=Box::pin(commands.update_relations(&who,&scope,permit.clone()));
        tokio::select!{result=writing.as_mut()=>panic!("physical peer has not replied: {result:?}"),value=bounded(observed)=>value.unwrap()}
        // An original command completion can only follow actual send_packets,
        // successful submitted() and private pending installation. Peer receipt
        // alone is not used as the submission-state proof.
        tokio::select!{result=writing.as_mut()=>panic!("original result remains absent: {result:?}"),binding=bounded(commands.original_im_binding(&who,&scope))=>assert!(Arc::ptr_eq(&binding.unwrap(),permit.binding()))}
        let before=row(&temp);assert_eq!(before,(0,Some(2),None,None));assert!(permit.current(2));assert!(!physical.dropped.load(Ordering::SeqCst));
        let reason=match end{
            End::SqlFailure=>{
                temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fail_relation_event BEFORE INSERT ON app_events WHEN NEW.event_type='app_voice.relations_updated' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture SQL failure")).unwrap();
                release.send(()).unwrap();assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Protocol));RuntimeError::Protocol
            },
            End::Parent=>{drop(release);parent.retire();assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Cancelled));RuntimeError::Cancelled},
            End::WaitDrop=>{drop(release);drop(writing);RuntimeError::Cancelled},
            End::Deadline=>{drop(release);assert_eq!(bounded(writing).await.err(),Some(RuntimeError::Timeout));assert!(Instant::now()>=action.deadline());RuntimeError::Timeout},
        };
        bounded(scope.closed()).await;let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,reason);assert!(permit.closed_original_io());assert!(physical.dropped.load(Ordering::SeqCst));assert_eq!(row(&temp),before,"submitted WRITE is neither not_sent nor refunded by cancellation/result-storage failure");no_event(&temp);held(&scope);drop(permit);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));assert_eq!(row(&temp),before);
    };tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)));
}
#[tokio::test]async fn relation_io_full_submission_wait_drop_closes_original_scope_and_keeps_unknown(){Box::pin(submitted(End::WaitDrop)).await;}
#[tokio::test]async fn relation_io_full_submission_parent_retirement_keeps_unknown_and_cancelled_join(){Box::pin(submitted(End::Parent)).await;}
#[tokio::test]async fn relation_io_private_ack_event_sql_failure_rolls_back_result_and_joins_original_io(){Box::pin(submitted(End::SqlFailure)).await;}
#[tokio::test]async fn relation_io_full_submission_original_deadline_keeps_unknown_without_late_reply_or_retry(){Box::pin(submitted(End::Deadline)).await;}
async fn invalid_reply(code:u32){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let route=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let(who,run)=original(&temp,&cert.root);
    let peer=async move{
        let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;let p=peer.next().await.unwrap();packet(&p,2,b"fixture-user");
        let rpc=encode_reply(2,code,&[0,1]).unwrap();let mut p=vec![1];p.extend_from_slice(rpc.expose_to_transport());peer.send(&[p]).await;eof(&mut peer).await;
    };
    let client=async{
        let conn=connected(&route,who.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&who,&scope,true).await;
        let(action,permit)=prepare(&handle,&who,&scope,&temp,run.clone(),ImReceiptGate::fresh(),None,Duration::from_secs(3)).await;
        assert!(bounded(handle.command_handle().update_relations(&who,&scope,permit.clone())).await.is_err());let closed=bounded(handle.close()).await;assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::Protocol);assert_eq!(row(&temp),(0,Some(2),None,None));no_event(&temp);held(&scope);drop(permit);drop(action);assert_eq!(scope.native_rpc_resources(),(0,0));
    };tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)));
}
#[tokio::test]async fn relation_io_malformed_signed_time_reply_is_not_terminal_success_or_replayed(){Box::pin(invalid_reply(0)).await;}
#[tokio::test]async fn relation_io_remote_version_error_preserves_unknown_and_original_refusal_not_automatic_retry(){Box::pin(invalid_reply(65537)).await;}
#[tokio::test]async fn relation_io_uninstalled_or_foreign_original_identity_does_not_allocate_or_send_group_request(){Box::pin(async{
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let route=router_policy(&cert.root,listener.local_addr().unwrap().port());let server=cert.server_config(&TLS13);
    let peer=async move{let mut peer=CommonPeer::open(listener,server).await;peer_login(&mut peer).await;logout(&mut peer,2).await;};
    let client=async{
        let who=owner();let conn=connected(&route,who.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=conn.connection_scope();let(mut handle,mut events)=conn.attach().unwrap();auth(&handle,&mut events,&who,&scope,false).await;
        let commands=handle.command_handle();assert_eq!(bounded(commands.original_relation_identity(&who,&scope)).await.err(),Some(RuntimeError::Scope));assert_eq!(bounded(commands.original_relation_identity(&owner(),&scope)).await.err(),Some(RuntimeError::Scope));assert_eq!(bounded(commands.original_relation_identity(&who,&RpcConnectionScope::fresh())).await.err(),Some(RuntimeError::Scope));assert_eq!(scope.native_rpc_resources(),(0,0));assert!(scope.is_open());assert!(bounded(handle.logout(&who,&scope,&[])).await.unwrap().io_released());
    };tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)));
}).await;}
