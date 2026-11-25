//! Actions-only original receiver -> owned event -> SQLite -> exact RPC reply.
//! No external device, original SO loading, fake public ACK or relation WRITE.
use super::*;
use n::{group_event_delivery::IncomingGroupEvent,group_event_inbox,group_events::NativeRelationEventsConfig,relation_journal::RelationConfiguration};
fn current(enabled:bool)->Fixture{current_timeout(enabled,30000)}
fn current_timeout(enabled:bool,timeout:u64)->Fixture{
 let temp=TempDatabase::new();let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let line="app-account-0123456789abcdef0123456789abcdef";
 let cfg=NativeReceiverConfig{direct_domains:None,invite_notifications:None,im_messages:Some(NativeImConfig{notification_timeout_ms:timeout,receive_info:false,directory_queries:false})};
 let mut c=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",1);c.profile.incoming=None;c.profile.client_ex=true;c.profile.native_receivers=Some(cfg.clone());c.profile.relation_events=enabled.then_some(NativeRelationEventsConfig{group_updates:true});c.credentials.sdk.expires_at_unix_seconds=None;
 temp.database.with_conn(|conn|account_store::create(conn,line,&c,100)).unwrap();let run=Arc::new(temp.database.with_conn(|conn|account_runs::register_runtime(conn,line,1,&local_nonce().unwrap())).unwrap());
 let mut session=HostingSession::new(line.into());session.import(imported(),100).unwrap();let owner=session.begin_login(100).unwrap();let mut backend=OfflineBackend::new(owner.clone());backend.claim_common_transport().unwrap();backend.bind_common_identity(b"AccountEntry").unwrap();let scope=backend.connection_scope();backend.arm_native_receivers(NativeReceiverSetup::capture(NativeReceiverPolicy::capture(&cfg).unwrap(),true,None).unwrap()).unwrap();if enabled{backend.arm_relation_events(NativeRelationEventsConfig{group_updates:true}).unwrap();}
 let login=backend.prepare_login(login(),&[]).unwrap();backend.on_plaintext_packet(&owner,&scope,&reply_packet(login.call_id,&login_result()),0).unwrap();backend.configure_native_receivers(7).unwrap();
 Fixture{owner,scope,backend,temp,run,gate:ImReceiptGate::fresh()}
}
fn payload(target:Option<&[u8]>,base:i64,update:i64,label:&[u8])->Vec<u8>{
 let mut body=Writer::new();body.u32(2).unwrap();for(label,flags)in[(label,1),(b"private-removal".as_slice(),20)]{body.bytes(label).unwrap();body.u32(flags).unwrap();body.bytes(b"private-remark").unwrap();body.bytes(b"").unwrap();body.properties(&[]).unwrap();}
 let base=base.to_string();let update=update.to_string();let mut h=vec![RpcProperty{key:b"Group.LastUpdateTime",value:base.as_bytes()},RpcProperty{key:b"Group.UpdateTime",value:update.as_bytes()}];if let Some(target)=target{h.push(RpcProperty{key:b"Group.GroupId",value:target});}
 let mut args=Writer::versioned().unwrap();args.bytes(b"Group.Update").unwrap();args.properties(&h).unwrap();args.bytes(body.finish().expose_to_transport()).unwrap();args.finish().expose_to_transport().to_vec()
}
fn event(f:&mut Fixture,id:i32,target:Option<&[u8]>,base:i64,update:i64,label:&[u8])->Box<IncomingGroupEvent>{
 let mut e=f.receive(id,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&payload(target,base,update,label),None);assert_eq!(e.len(),1);let OfflineEvent::GroupEvent(e)=e.pop().unwrap()else{panic!("only original deferred relation event")};e
}
fn config(f:&Fixture)->Arc<RelationConfiguration>{Arc::new(f.temp.database.with_conn(|c|RelationConfiguration::capture(c,f.run.line_id(),f.run.revision(),Some(&f.scope))).unwrap())}
fn commit(f:&Fixture,e:&IncomingGroupEvent)->group_event_inbox::CommittedGroupEvent{group_event_inbox::commit(&f.temp.database,f.run.clone(),e,f.gate.clone(),&config(f)).unwrap()}
fn history(f:&Fixture)->Vec<group_event_inbox::GroupEventView>{f.temp.database.with_conn(|c|group_event_inbox::history(c,f.run.line_id(),128)).unwrap()}
#[path="group_event_lifetime_tests.rs"]
mod lifetimes;
#[test]fn group_event_policy_absent_native_im_read_or_write_installation_does_not_admit_relation_event(){
 let mut f=current(false);let e=f.receive(1,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&payload(Some(b"1-2"),0,1,b"x"),None);assert_eq!(control_result(&e),[0;6]);assert!(history(&f).is_empty());assert_eq!(f.count(),0);assert!(f.scope.is_open());
}
#[test]fn group_event_original_current_binding_defers_positive_reply_until_own_durable_observation(){
 let mut f=current(true);let e=event(&mut f,1,Some(b"1-2"),i64::MIN,i64::MAX,b"private-label");assert!(history(&f).is_empty());assert_eq!(f.count(),0);let proof=commit(&f,&e);assert!(proof.authorizes(e.ticket(),Instant::now()));let rows=history(&f);assert_eq!(rows.len(),1);assert_eq!(rows[0].base_time,i64::MIN.to_string());assert_eq!(rows[0].update_time,i64::MAX.to_string());assert_eq!((rows[0].updates,rows[0].removals),(1,1));assert_eq!(rows[0].continuity,"unseeded");assert!(!rows[0].full_roster_verified);assert_eq!(result(f.backend.complete_group_event(e.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);
}
#[test]fn group_event_current_uid_fallback_cannot_select_another_user_or_group(){
 let mut f=current(true);let e=event(&mut f,1,None,0,1,b"private-label");assert_eq!(e.notice().identifier(),b"fixture-user");let proof=commit(&f,&e);assert!(proof.authorizes(e.ticket(),Instant::now()));assert_eq!(history(&f)[0].target_kind,"buddy");assert_eq!(f.count(),0);
}
#[test]fn group_event_foreign_owner_scope_and_commit_proof_cannot_authorize_original_group_reply(){
 let mut f=current(true);let mut other=current(true);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");let foreign=event(&mut other,1,Some(b"1-2"),0,1,b"a");let proof=commit(&other,&foreign);assert!(!e.matches(&other.owner,&other.scope));assert!(!proof.authorizes(e.ticket(),Instant::now()));assert!(f.backend.complete_group_event(e.ticket(),Some(&proof)).unwrap().is_none());assert!(history(&f).is_empty());
}
#[test]fn group_event_duplicate_data_with_new_original_rpc_is_idempotent_but_different_same_time_conflicts(){
 let mut f=current(true);let first=event(&mut f,1,Some(b"1-2"),0,1,b"a");let p=commit(&f,&first);f.backend.complete_group_event(first.ticket(),Some(&p)).unwrap();let duplicate=event(&mut f,2,Some(b"1-2"),0,1,b"a");commit(&f,&duplicate);assert_eq!(history(&f).len(),1);let changed=event(&mut f,3,Some(b"1-2"),0,1,b"b");assert_eq!(group_event_inbox::commit(&f.temp.database,f.run.clone(),&changed,f.gate.clone(),&config(&f)).err(),Some("app_account_relation_event_conflict"));assert_eq!(history(&f).len(),1);assert_eq!(result(f.backend.complete_group_event(changed.ticket(),None).unwrap()),[0;6]);assert!(f.scope.is_open());
}
#[test]fn group_event_partial_chain_gap_and_stale_times_are_observations_not_complete_roster_or_write_resolution(){
 let mut f=current(true);for(id,base,update,continuity)in[(1,0,1,"unseeded"),(2,1,2,"partial_chain"),(3,4,5,"gap"),(4,-1,0,"stale"),(5,2,3,"gap")]{let e=event(&mut f,id,Some(b"1-2"),base,update,b"a");commit(&f,&e);assert_eq!(history(&f)[0].continuity,continuity);assert!(!history(&f)[0].full_roster_verified);}
 let heads:(i64,i64,i64)=f.temp.database.with_conn(|c|c.query_row("SELECT observed_time,target_time,gap FROM app_voice_relation_event_heads",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"fixture")).unwrap();assert_eq!(heads,(2,5,1));
 let writes:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_relation_writes",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(writes,0);assert_eq!(f.count(),0);
}
#[test]fn group_event_sql_event_failure_rolls_back_head_and_record_before_any_positive_reply(){
 let mut f=current(true);let mut broadcast=f.temp.database.subscribe_app_events();let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fail_group_event BEFORE INSERT ON app_events WHEN NEW.event_type='app_voice.relation_event_observed' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();assert!(group_event_inbox::commit(&f.temp.database,f.run.clone(),&e,f.gate.clone(),&config(&f)).is_err());assert!(history(&f).is_empty());let heads:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_relation_event_heads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(heads,0);assert!(broadcast.try_recv().is_err());assert_eq!(result(f.backend.complete_group_event(e.ticket(),None).unwrap()),[0;6]);
}
#[test]fn group_event_committed_event_publishes_once_after_transaction_with_no_private_body_or_grant(){
 let mut f=current(true);let mut broadcast=f.temp.database.subscribe_app_events();let e=event(&mut f,1,Some(b"1-2"),0,1,b"private-label");commit(&f,&e);let data=broadcast.try_recv().unwrap();assert_eq!(data.event_type,"app_voice.relation_event_observed");assert_eq!(history(&f).len(),1);assert!(broadcast.try_recv().is_err());for secret in ["private-label","private-remark","private-removal","fixture-user","1-2","token","identity_key","request_id","nonce"]{assert!(!data.payload_json.contains(secret));}assert!(serde_json::from_str::<serde_json::Value>(&data.payload_json).unwrap()["full_roster_verified"]==false);
}
#[test]fn group_event_configuration_parent_gate_revoke_and_original_close_cannot_redeem_old_ack(){
 let mut f=current(true);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");let p=commit(&f,&e);f.gate.retire();assert!(!p.authorizes(e.ticket(),Instant::now()));assert!(f.backend.complete_group_event(e.ticket(),Some(&p)).unwrap().is_none());f.backend.close();assert!(!e.matches(&f.owner,&f.scope));assert_eq!(history(&f).len(),1);
}
#[test]fn group_event_actual_waiter_notice_and_sql_proof_keep_budget_until_last_drop_not_io_close(){
 let mut f=current(true);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");let p=commit(&f,&e);let retained=p.captured_copy();f.backend.close();drop(p);drop(e);assert!(f.scope.native_rpc_resources().0>0);drop(retained);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]fn group_event_config_changed_without_revision_is_refused_before_sql_and_does_not_reset_unknown(){
 let mut f=current(true);let configuration=config(&f);let e=event(&mut f,1,Some(b"1-2"),0,1,b"a");f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET private_config=X'00'",[]).map_err(|_|"fixture")).unwrap();assert!(group_event_inbox::commit(&f.temp.database,f.run.clone(),&e,f.gate.clone(),&configuration).is_err());assert!(history(&f).is_empty());assert!(f.scope.is_open());
}
