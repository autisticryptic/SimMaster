//! Actions-only independently assembled mark-read DATA and original SQL/RPC
//! permissions. No native SDK execution, device action or user read from status.
use super::*;
use n::{im_inbox::{UserReadAction,BoundUserRead,UserReadPermit},im_user_read_data as d,im_status::StatusLabel,rpc_im::ImQuery};
fn message_id(f:&mut Fixture,rpc:i32,index:i64)->i64{let msg=f.message(rpc,b"P2P/1_2",index,r#"{"text":"user read text"}"#);let proof=f.inbox(&msg).commit(&f.temp.database,&msg).unwrap().unwrap();f.backend.complete_im(msg.ticket(),Some(&proof)).unwrap();f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().into_iter().find(|r|r.message_index==index.to_string()).unwrap().id}
fn action(f:&Fixture,id:i64,nonce:&str)->Arc<UserReadAction>{UserReadAction::original(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.owner.clone(),f.scope.clone(),id,nonce,Instant::now()+Duration::from_secs(30)).unwrap()}
fn status(latest:Option<i64>,accepted:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.u32(1).unwrap();w.bytes(b"1_2").unwrap();let value=latest.map(|n|n.to_string());let props=value.as_ref().map(|n|vec![RpcProperty{key:n::im_status::LATEST,value:n.as_bytes()}]).unwrap_or_default();w.properties(&props).unwrap();w.i64(1).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn checked(f:&mut Fixture,a:&Arc<UserReadAction>,latest:Option<i64>,accepted:bool)->Result<Arc<UserReadPermit>,&'static str>{
    let binding=f.backend.captured_im_binding().unwrap();let BoundUserRead::Context(c)=a.bind_current(binding)?else{panic!("fresh context")};let p=f.backend.prepare_im_read(c.query(),a.gate(),Instant::now()+Duration::from_secs(30)).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&status(latest,accepted)),1).unwrap();let r=f.backend.take_im_replies().pop().unwrap();c.admit(&r)
}
fn state(f:&Fixture)->(i64,Option<i32>,i64){f.temp.database.with_conn(|c|c.query_row("SELECT state,request_id,target_index FROM app_voice_im_user_reads",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"fixture")).unwrap()}
fn native_reply(f:&mut Fixture,id:i32,yes:bool,code:u32)->n::im_user_read_rpc::OwnedUserReadReply{let rpc=encode_reply(id,code,if yes{&[0,1,0,0,0,0]}else{&[0;6]}).unwrap();let mut p=vec![1];p.extend_from_slice(rpc.expose_to_transport());f.backend.on_plaintext_packet(&f.owner,&f.scope,&p,3).unwrap();let mut r=f.backend.take_user_read_replies();assert_eq!(r.len(),1);r.pop().unwrap()}
#[test]
fn user_read_data_exact_method_one_version_entry_label_uid_property_string_index_epoch_ms(){
    let out=d::encode_mark_read(StatusLabel::parse(b"1_2").unwrap(),b"current-sdk-uid",i64::MAX,4294967297).unwrap();let mut e=vec![0,0,1,0,0];for s in [b"1_2".as_slice(),b"current-sdk-uid",b"P:P_UDMR-N",b"9223372036854775807"]{e.extend_from_slice(&(s.len()as u32).to_be_bytes());e.extend_from_slice(s);}e.extend_from_slice(&4294967297i64.to_be_bytes());assert_eq!(out.expose_to_transport(),e);assert_eq!(d::MARK_READ_METHOD,"setStatus.GroupServer.Group");assert_ne!(d::MARK_READ_METHOD,n::im_data::SET_READ_METHOD);
}
#[test]
fn user_read_data_rejects_guessed_group_negative_index_time_and_missing_current_sdk_uid(){
    assert!(d::encode_mark_read(StatusLabel::parse(b"1-2").unwrap(),b"uid",7,1).is_err());for(i,t,u)in[(-1,1,b"uid".as_slice()),(7,-1,b"uid"),(7,1,b""),(7,1,b"uid\0")]{assert!(d::encode_mark_read(StatusLabel::parse(b"1_2").unwrap(),u,i,t).is_err());}
}
#[test]
fn user_read_result_bool_footer_truncation_trailing_and_version_failure_never_become_user_intent(){
    assert!(d::decode_mark_read_result(0,&[0,1,0,0,0,0]).unwrap().accepted);assert!(!d::decode_mark_read_result(0,&[0;6]).unwrap().accepted);for end in 0..6{assert!(d::decode_mark_read_result(0,&[0,1,0,0,0,0][..end]).is_err());}assert!(d::decode_mark_read_result(65537,&[0,1,0,0,0,0]).is_err());assert!(d::decode_mark_read_result(0,&[0,1,0,0,0,0,9]).is_err());
}
#[test]
fn user_read_action_needs_canonical_nonce_current_scope_and_local_message_not_raw_target(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);for nonce in ["", "bad", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"]{assert!(UserReadAction::original(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.owner.clone(),f.scope.clone(),id,nonce,Instant::now()+Duration::from_secs(1)).is_err());}assert!(UserReadAction::original(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.owner.clone(),f.scope.clone(),0,&"a".repeat(32),Instant::now()+Duration::from_secs(1)).is_err());
}
#[test]
fn user_read_current_native_binding_and_purpose_target_reply_precede_any_marker_or_write(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();assert_eq!(state(&f),(0,None,7));let write=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();let rpc=decode_request(&write.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(rpc.method,b"setStatus.GroupServer.Group");assert_eq!(rpc.metadata.len(),2);assert_eq!(rpc.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");assert_eq!(rpc.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");let mut r=Reader::versioned(rpc.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"1_2");assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"P:P_UDMR-N");assert_eq!(r.bytes().unwrap(),b"7");assert!(r.i64().unwrap()>0);r.finish().unwrap();assert_eq!(state(&f),(0,Some(write.call_id),7));
}
#[test]
fn user_read_missing_or_rejected_or_lower_current_target_keeps_no_journal(){
    for(latest,yes)in[(None,true),(Some(6),true),(Some(9),false)]{let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));assert!(checked(&mut f,&a,latest,yes).is_err());let n:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(n,0);}
}
#[test]
fn user_read_unrelated_query_same_status_bytes_cannot_admit_fixed_original_context(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let BoundUserRead::Context(c)=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!()};let p=f.backend.prepare_im_read(Arc::new(ImQuery::refresh(&[],-1).unwrap()),a.gate(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&status(Some(9),true)),1).unwrap();assert!(c.admit(&f.backend.take_im_replies().pop().unwrap()).is_err());
}
#[test]
fn user_read_foreign_owner_scope_cannot_bind_current_identical_local_message(){
    let mut f=Fixture::new(true);let other=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));assert!(a.bind_current(other.backend.captured_im_binding().unwrap()).is_err());
}
#[test]
fn user_read_deleted_text_before_admission_is_not_recreated_or_marked_from_a_tombstone(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);f.temp.database.with_conn(|c|n::im_inbox::delete(c,f.run.line_id(),id)).unwrap();let a=action(&f,id,&"a".repeat(32));assert!(a.bind_current(f.backend.captured_im_binding().unwrap()).is_err());assert!(f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().is_empty());
}
#[test]
fn user_read_delete_after_target_check_before_write_is_known_no_io_and_no_resurrection(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();f.temp.database.with_conn(|c|n::im_inbox::delete(c,f.run.line_id(),id)).unwrap();assert!(f.backend.prepare_user_read_write(p.clone(),a.deadline()).is_err());p.cancel();assert_eq!(state(&f),(3,None,7));assert!(f.scope.is_open());
}
#[test]
fn user_read_queued_cancel_records_known_no_io_without_closing_other_sdk_operations(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let _p=checked(&mut f,&a,Some(9),true).unwrap();a.cancel();assert_eq!(state(&f),(3,None,7));assert!(f.scope.is_open());
}
#[test]
fn user_read_possible_partial_and_submitted_cancel_keep_unknown_and_close_same_scope(){
    for full in [false,true]{let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(w.call_id));if full{assert!(p.submitted());}a.cancel();assert_eq!(state(&f),(0,Some(w.call_id),7));assert!(!f.scope.is_open());}
}
#[test]
fn user_read_only_matched_original_private_reply_resolves_once_not_received_or_raw_bool(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(w.call_id));assert!(p.submitted());let r=native_reply(&mut f,w.call_id,true,0);let view=p.resolve(&r).unwrap();assert_eq!(view.message_id,id);assert_eq!(view.state,"confirmed");assert!(p.resolve(&r).is_err());assert_eq!(state(&f),(1,Some(w.call_id),7));assert_eq!(f.count(),1);let count:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_read_receipts",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(count,0);
}
#[test]
fn user_read_native_false_and_version_result_are_known_rejections_and_never_replayed(){
    for code in [0,65537]{let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(w.call_id));assert!(p.submitted());let r=native_reply(&mut f,w.call_id,false,code);assert_eq!(p.resolve(&r).unwrap().state,"rejected");let b=action(&f,id,&"a".repeat(32));let BoundUserRead::Replay(view)=b.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!()};assert_eq!(view.state,"rejected");assert!(f.scope.is_open());}
}
#[test]
fn user_read_uncertain_marker_blocks_new_nonce_before_another_native_target_request(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let _p=checked(&mut f,&a,Some(9),true).unwrap();let b=action(&f,id,&"b".repeat(32));assert_eq!(b.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_uncertain"));assert_eq!(state(&f),(0,None,7));
}
#[test]
fn user_read_nonce_cannot_rebind_another_local_message_or_native_index(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let second=message_id(&mut f,2,8);let a=action(&f,id,&"a".repeat(32));let _p=checked(&mut f,&a,Some(9),true).unwrap();let b=action(&f,second,&"a".repeat(32));assert_eq!(b.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_nonce_conflict"));
}
#[test]
fn user_read_known_no_io_allows_another_explicit_nonce_but_not_automatic_resend(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let _p=checked(&mut f,&a,Some(9),true).unwrap();a.cancel();let b=action(&f,id,&"b".repeat(32));let _p=checked(&mut f,&b,Some(9),true).unwrap();let count:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(count,2);
}
#[test]
fn user_read_purpose_reply_or_parent_retirement_cannot_extend_original_action_deadline(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let parent=f.gate.clone();let p=checked(&mut f,&a,Some(9),true).unwrap();parent.retire();assert!(f.backend.prepare_user_read_write(p.clone(),a.deadline()).is_err());p.cancel();assert_eq!(state(&f),(3,None,7));
}
#[test]
fn user_read_sql_admission_failure_leaves_no_marker_or_user_write_id(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_user_read BEFORE INSERT ON app_voice_im_user_reads BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();let a=action(&f,id,&"a".repeat(32));assert!(checked(&mut f,&a,Some(9),true).is_err());let count:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(count,0);
}
#[test]
fn user_read_scope_revision_change_and_changed_message_fingerprint_prevent_positive_resolution(){
    for revision in [false,true]{let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(w.call_id));assert!(p.submitted());let r=native_reply(&mut f,w.call_id,true,0);f.temp.database.with_conn(|c|if revision{c.execute("UPDATE app_voice_accounts SET revision=2",[]).map_err(|_|"fixture")}else{c.execute("UPDATE app_voice_im_messages SET fingerprint=zeroblob(32)",[]).map_err(|_|"fixture")}).unwrap();assert!(p.resolve(&r).is_err());assert_eq!(state(&f),(0,Some(w.call_id),7));}
}
#[test]
fn user_read_debug_and_metadata_do_not_export_box_uid_token_nonce_or_sdk_scope(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let text=format!("{a:?} {p:?}");for hidden in ["1_2","fixture","nonce","token"]{assert!(!text.contains(hidden));}let rows=f.temp.database.with_conn(|c|n::im_inbox::user_read_history(c,f.run.line_id(),16)).unwrap();let text=serde_json::to_string(&rows).unwrap();for hidden in ["box_id","1_2","identity_key","scope_epoch","user_nonce","run_nonce","request_id"]{assert!(!text.contains(hidden));}
}
