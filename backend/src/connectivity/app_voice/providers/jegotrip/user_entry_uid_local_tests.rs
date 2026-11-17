//! Actions-only DATA/source/current-auth boundaries for UID-local completion.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use std::{sync::Arc,time::{Instant,Duration}};
use n::{user_entry_local::{UserEntryLocalReference,LocalUidResult},user_entry_action::UserEntryAction,im_inbox::ImReceiptGate};
fn reference(who:&AttemptOwner,parent:&RpcConnectionScope,router:&n::router_connector::RouterPolicy,bytes:&[u8])->(ArcRpcSession,RpcConnectionScope,Option<Arc<UserEntryLocalReference>>){
    let mut source=ArcRpcSession::new(who.clone());let scope=source.connection_scope();let(id,_)=source.prepare_bootstrap(b"fixture-key",&[]).unwrap();let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&[]).unwrap();w.bytes(b"AccountEntry:sarc -h 42.7 -p 98;").unwrap();w.bytes(b"").unwrap();w.bytes(bytes).unwrap();w.bytes(b"").unwrap();let result=source.on_reply(who,&scope,&reply(id,w.finish().expose_to_transport())).unwrap().unwrap();let ArcReply::Bootstrap(result)=result else{panic!("owned bootstrap")};let local=UserEntryLocalReference::from_bootstrap(&result,who,&scope,parent,router,None).unwrap();(source,scope,local)
}
fn router()->n::router_connector::RouterPolicy{let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");n::netstream_runtime::tests::policy(&cert.root,12345)}
fn action(who:&AttemptOwner,parent:&RpcConnectionScope,gate:Arc<ImReceiptGate>,bytes:&[u8])->Arc<UserEntryAction>{UserEntryAction::original(who.clone(),parent.clone(),gate,Instant::now()+Duration::from_secs(30),bytes).unwrap()}
#[test]
fn uid_local_raw_and_qualified_uid_preserve_actual_uid_without_remote_report(){
    for uri in [b"0_1".as_slice(),b"[uid:7_9]",b"[uid:7_9@foreign.invalid]"]{let(rpc,who,parent)=current_native();let policy=router();let(mut source,_,reference)=reference(&who,&parent,&policy,b"prefix/UserEntry@adapter$locator");let reference=reference.unwrap();assert!(reference.original_bootstrap_id()>0);source.close();let a=action(&who,&parent,ImReceiptGate::fresh(),uri);let local=LocalUidResult::original(a.clone(),reference,rpc.user_entry_auth().unwrap(),&policy).unwrap();let view=local.observe(&a,&policy).unwrap();assert_eq!(view.report,"local_uid");assert_eq!(view.kind,"uid");assert_eq!(local.uid(),if uri==b"0_1"{b"0_1"}else{b"7_9"});assert!(parent.is_open());}
}
#[test]
fn uid_local_requires_original_auxiliary_presence_but_does_not_select_adapter_or_locator(){
    let(_rpc,who,parent)=current_native();let policy=router();for raw in [b"UserEntry".as_slice(),b"prefix/UserEntry@adapter$locator",b"UserEntry:tls -h fixture.invalid -p 443;"]{let(_,_,local)=reference(&who,&parent,&policy,raw);assert!(local.unwrap().matches(&who,&parent,&policy));}let(_,_,absent)=reference(&who,&parent,&policy,b"");assert!(absent.is_none());
}
#[test]
fn uid_local_foreign_auth_or_owner_scope_cannot_be_used_as_current_parent(){
    let(rpc,who,parent)=current_native();let(other,_,_)=current_native();let policy=router();let(_,_,reference)=reference(&who,&parent,&policy,b"UserEntry");let a=action(&who,&parent,ImReceiptGate::fresh(),b"7_9");assert!(LocalUidResult::original(a.clone(),reference.clone().unwrap(),other.user_entry_auth().unwrap(),&policy).is_err());let alien=action(&owner(),&parent,ImReceiptGate::fresh(),b"7_9");assert!(LocalUidResult::original(alien,reference.unwrap(),rpc.user_entry_auth().unwrap(),&policy).is_err());
}
#[test]
fn uid_local_same_bytes_for_another_action_cannot_publish_the_old_observation(){
    let(rpc,who,parent)=current_native();let policy=router();let(_,_,reference)=reference(&who,&parent,&policy,b"UserEntry");let a=action(&who,&parent,ImReceiptGate::fresh(),b"7_9");let local=LocalUidResult::original(a.clone(),reference.unwrap(),rpc.user_entry_auth().unwrap(),&policy).unwrap();let other=action(&who,&parent,ImReceiptGate::fresh(),b"7_9");assert!(local.observe(&other,&policy).is_err());assert!(local.observe(&a,&router()).is_err());
}
#[test]
fn uid_local_alias_aid_can_never_be_promoted_into_the_native_uid_branch(){
    let(rpc,who,parent)=current_native();let policy=router();let(_,_,reference)=reference(&who,&parent,&policy,b"UserEntry");let a=action(&who,&parent,ImReceiptGate::fresh(),b"[phone:+12345@fixture.invalid]");assert!(LocalUidResult::original(a,reference.unwrap(),rpc.user_entry_auth().unwrap(),&policy).is_err());
}
#[test]
fn uid_local_parent_or_run_gate_retirement_rejects_retained_data_without_any_write_ack(){
    for revoke in [false,true]{let(rpc,who,parent)=current_native();let policy=router();let(_,_,reference)=reference(&who,&parent,&policy,b"UserEntry");let gate=ImReceiptGate::fresh();let a=action(&who,&parent,gate.clone(),b"7_9");let local=LocalUidResult::original(a.clone(),reference.unwrap(),rpc.user_entry_auth().unwrap(),&policy).unwrap();if revoke{parent.mark_session_revoked();}else{gate.retire();}assert!(local.observe(&a,&policy).is_err());assert!(parent.is_open());}
}
#[test]
fn uid_local_debug_and_public_view_do_not_export_raw_uid_token_selector_or_reference(){
    let(rpc,who,parent)=current_native();let policy=router();let(_,_,reference)=reference(&who,&parent,&policy,b"prefix/UserEntry@adapter$locator");let a=action(&who,&parent,ImReceiptGate::fresh(),b"[uid:7_9@fixture.invalid]");let local=LocalUidResult::original(a.clone(),reference.unwrap(),rpc.user_entry_auth().unwrap(),&policy).unwrap();let text=serde_json::to_string(&local.observe(&a,&policy).unwrap()).unwrap()+&format!("{local:?}");for secret in ["7_9","fixture","token","selector","adapter","locator","prefix"]{assert!(!text.contains(secret));}
}
#[test]
fn uid_local_keeps_the_original_sdk_rpc_id_and_pending_inventory_unchanged(){
    let(rpc,who,parent)=current_native();let policy=router();let(_,_,reference)=reference(&who,&parent,&policy,b"UserEntry");let before=(rpc.next_id,rpc.pending.len());let a=action(&who,&parent,ImReceiptGate::fresh(),b"7_9");let local=LocalUidResult::original(a.clone(),reference.unwrap(),rpc.user_entry_auth().unwrap(),&policy).unwrap();local.observe(&a,&policy).unwrap();assert_eq!((rpc.next_id,rpc.pending.len()),before);assert_eq!(rpc.phase(),ArcPhase::Authenticated);
}
