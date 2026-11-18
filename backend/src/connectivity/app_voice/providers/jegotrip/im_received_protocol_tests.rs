//! Actions-only original ARC/SQLite received-write permissions. Pure protocol
//! phase transitions are supplementary; real IO/Joins have separate peer cases.
use super::*;
use n::{im_inbox::{CursorSession,GapCommitted,ReceivedAdmission,ReceivedWritePermit},im_receipt_policy::NativeImReceiptConfig,rpc_im::ImQuery};
#[path="native_rpc_budget_received_tests.rs"]
mod shared_budget;
#[path="im_read_status_protocol_tests.rs"]
mod manual_read_status;
fn status(base:i64,target:i64,time:i64)->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(1).unwrap();w.bytes(b"1_2").unwrap();let b=base.to_string();let t=target.to_string();w.properties(&[RpcProperty{key:n::im_status::PEER_READ,value:b.as_bytes()},RpcProperty{key:n::im_status::LATEST,value:t.as_bytes()}]).unwrap();w.i64(time).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn bool_result(value:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(value).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn empty()->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.u32(0).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn read(f:&mut Fixture,q:Arc<ImQuery>,body:&[u8])->n::rpc_im::OwnedImReply{let p=f.backend.prepare_im_read(q,f.gate.clone(),Instant::now()+Duration::from_secs(30)).unwrap();assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,body),1).unwrap().is_empty());let mut r=f.backend.take_im_replies();assert_eq!(r.len(),1);r.pop().unwrap()}
fn span(f:&mut Fixture,base:i64,target:i64,time:i64)->GapCommitted{let r=read(f,Arc::new(ImQuery::refresh(&[],-1).unwrap()),&status(base,target,time));let mut c=CursorSession::from_refresh(f.run.clone(),f.gate.clone(),&r).unwrap();c.observe_refresh(&f.temp.database,&r).unwrap();let plan=c.next_gap(&f.temp.database).unwrap().unwrap();let r=read(f,plan.query(),&empty());c.commit_gap(&f.temp.database,&plan,&r).unwrap()}
fn admit(f:&Fixture,s:&GapCommitted)->Arc<ReceivedWritePermit>{let ReceivedAdmission::Fresh(p)=n::im_inbox::begin_received(f.temp.database.clone(),s,&NativeImReceiptConfig{set_received:true}).unwrap()else{panic!("fresh original receipt")};p}
fn state(f:&Fixture)->(i64,Option<i32>,i64){f.temp.database.with_conn(|c|c.query_row("SELECT state,request_id,target_index FROM app_voice_im_read_receipts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"fixture")).unwrap()}
fn write(f:&mut Fixture,p:&Arc<ReceivedWritePermit>)->n::arc_protocol::OwnedCallPackets{f.backend.prepare_received_write(p.clone(),Instant::now()+Duration::from_secs(30)).unwrap()}
fn result(f:&mut Fixture,id:i32,value:bool,code:u32)->n::rpc_im_received::OwnedReceivedReply{let bytes=bool_result(value);let rpc=encode_reply(id,code,&bytes).unwrap();let mut packet=vec![1];packet.extend_from_slice(rpc.expose_to_transport());assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&packet,3).unwrap().is_empty());let mut replies=f.backend.take_received_replies();assert_eq!(replies.len(),1);replies.pop().unwrap()}
#[test]
fn received_write_policy_is_strict_separate_from_read_and_default_profiles_omit_it(){
    for data in [r#"{}"#,r#"{"set_received":false}"#,r#"{"set_received":true,"token":"forged"}"#]{assert!(serde_json::from_str::<NativeImReceiptConfig>(data).is_err()||serde_json::from_str::<NativeImReceiptConfig>(data).unwrap().validate().is_err());}
    let f=Fixture::new(true);let stored=f.temp.database.with_conn(|c|account_store::load(c,f.run.line_id())).unwrap().unwrap();assert!(stored.config.profile.im_receipts.is_none());assert!(!std::str::from_utf8(&stored.config.encode_private().unwrap()).unwrap().contains("im_receipts"));
}
#[test]
fn received_write_read_success_or_notification_is_not_a_cursor_write_permission(){
    let mut f=Fixture::new(true);let r=read(&mut f,Arc::new(ImQuery::refresh(&[],-1).unwrap()),&status(0,1,1));assert!(r.result().is_ok());let h=f.hint(1,99,b"P2P/1_2",1);assert!(f.inbox(&h).lookup(&f.temp.database,&h).unwrap().is_none());let rows:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_read_receipts",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(rows,0);
}
#[test]
fn received_write_marker_precedes_rpc_allocation_and_binds_only_exact_original_method_token_box_index(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);assert_eq!(state(&f),(0,None,7));let packet=write(&mut f,&p);assert_eq!(packet.call_id,4);let r=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(r.method,b"setRead.Message.Message");assert_eq!(r.metadata.len(),2);assert_eq!(r.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");assert_eq!(r.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");let mut args=Reader::versioned(r.arguments).unwrap();assert_eq!(args.bytes().unwrap(),b"P2P/1_2");assert_eq!(args.i64().unwrap(),7);args.finish().unwrap();assert_eq!(state(&f),(0,Some(packet.call_id),7));
}
#[test]
fn received_write_read_api_still_denies_setreceived_before_any_new_id(){
    let mut f=Fixture::new(true);let q=Arc::new(ImQuery::set_received(n::im_data::ImBox::parse(b"P2P/1_2").unwrap(),7).unwrap());assert!(f.backend.prepare_im_read(q,f.gate.clone(),Instant::now()+Duration::from_secs(1)).is_err());let r=f.backend.prepare_im_read(Arc::new(ImQuery::refresh(&[],-1).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(r.call_id,2);
}
#[test]
fn received_write_policy_refusal_and_stale_span_cannot_leave_a_marker_or_rpc(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,1,1);assert!(n::im_inbox::begin_received(f.temp.database.clone(),&s,&NativeImReceiptConfig{set_received:false}).is_err());let _later=span(&mut f,0,2,2);assert!(!s.is_current(&f.temp.database));assert!(n::im_inbox::begin_received(f.temp.database.clone(),&s,&NativeImReceiptConfig{set_received:true}).is_err());let rows:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_read_receipts",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(rows,0);
}
#[test]
fn received_write_second_nonce_and_later_target_are_blocked_while_original_outcome_is_uncertain(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let _p=admit(&f,&s);assert_eq!(n::im_inbox::begin_received(f.temp.database.clone(),&s,&NativeImReceiptConfig{set_received:true}).err(),Some("app_account_im_receipt_uncertain"));let next=span(&mut f,0,8,2);assert_eq!(n::im_inbox::begin_received(f.temp.database.clone(),&next,&NativeImReceiptConfig{set_received:true}).err(),Some("app_account_im_receipt_uncertain"));assert_eq!(state(&f),(0,None,7));
}
#[test]
fn received_write_queued_wait_drop_records_known_no_io_without_closing_sdk_scope(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);drop(n::im_inbox::ReceivedWait::original(p.clone()));assert_eq!(state(&f),(3,None,7));assert!(f.scope.is_open());assert!(!p.begin_write(4));assert!(f.backend.prepare_received_write(p,Instant::now()+Duration::from_secs(1)).is_err());
}
#[test]
fn received_write_possible_partial_phase_cancel_keeps_unknown_and_closes_original_scope(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));p.cancel();assert_eq!(state(&f),(0,Some(packet.call_id),7));assert!(!f.scope.is_open());assert!(!p.submitted());
}
#[test]
fn received_write_submitted_cancel_is_not_readonly_interest_retirement_or_permission_to_replay(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());drop(n::im_inbox::ReceivedWait::original(p.clone()));assert!(!f.scope.is_open());assert_eq!(state(&f),(0,Some(packet.call_id),7));assert!(!p.matches(&f.owner,&f.scope));
}
#[test]
fn received_write_only_matched_private_original_reply_resolves_journal_once_without_message_or_media_grant(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=result(&mut f,packet.call_id,true,0);assert!(r.belongs_to(&p));p.resolve_original(&r).unwrap();assert_eq!(state(&f),(1,Some(packet.call_id),7));assert!(p.resolve_original(&r).is_err());p.cancel();assert!(f.scope.is_open());assert_eq!(f.count(),0);assert!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),16).unwrap().is_empty());assert!(matches!(n::im_inbox::begin_received(f.temp.database.clone(),&s,&NativeImReceiptConfig{set_received:true}).unwrap(),ReceivedAdmission::AlreadyConfirmed));
}
#[test]
fn received_write_false_and_version_error_are_known_rejections_not_automatic_retries(){
    for code in [0,65537]{let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=result(&mut f,packet.call_id,false,code);assert!(!r.accepted().unwrap());p.resolve_original(&r).unwrap();assert_eq!(state(&f),(2,Some(packet.call_id),7));assert!(f.backend.prepare_received_write(p,Instant::now()+Duration::from_secs(1)).is_err());assert_eq!(n::im_inbox::begin_received(f.temp.database.clone(),&s,&NativeImReceiptConfig{set_received:true}).err(),Some("app_account_im_receipt_not_replayed"));assert!(f.scope.is_open());}
}
#[test]
fn received_write_equivalent_other_scope_cannot_use_original_permit_or_resolve_same_request_id(){
    let mut f=Fixture::new(true);let mut other=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);assert!(other.backend.prepare_received_write(p.clone(),Instant::now()+Duration::from_secs(1)).is_err());let read=other.backend.prepare_im_read(Arc::new(ImQuery::refresh(&[],-1).unwrap()),other.gate.clone(),Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(read.call_id,2);assert_eq!(state(&f),(0,None,7));
}
#[test]
fn received_write_same_permit_can_arm_and_begin_io_only_once(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(f.backend.prepare_received_write(p.clone(),Instant::now()+Duration::from_secs(1)).is_err());assert!(p.begin_write(packet.call_id));assert!(!p.begin_write(packet.call_id));assert!(p.submitted());assert!(!p.submitted());assert_eq!(state(&f),(0,Some(packet.call_id),7));
}
#[test]
fn received_write_expired_or_retired_admission_has_no_rpc_and_durable_marker_stays_no_io(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);assert!(f.backend.prepare_received_write(p.clone(),Instant::now()).is_err());drop(n::im_inbox::ReceivedWait::original(p));assert_eq!(state(&f),(3,None,7));assert!(f.scope.is_open());
}
#[test]
fn received_write_sql_marker_failure_cannot_produce_permission_or_allocate_a_write_id(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON app_voice_im_read_receipts BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();assert!(n::im_inbox::begin_received(f.temp.database.clone(),&s,&NativeImReceiptConfig{set_received:true}).is_err());let r=f.backend.prepare_im_read(Arc::new(ImQuery::refresh(&[],-1).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(r.call_id,4);
}
#[test]
fn received_write_revision_change_after_submission_keeps_unknown_not_accepted_on_new_configuration(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=result(&mut f,packet.call_id,true,0);f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET revision=2",[]).map_err(|_|"fixture")).unwrap();assert!(p.resolve_original(&r).is_err());assert_eq!(state(&f),(0,Some(packet.call_id),7));p.cancel();assert!(!f.scope.is_open());
}
#[test]
fn received_write_unknown_and_duplicate_reply_ids_cannot_create_another_owned_write_result(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(packet.call_id+90,&bool_result(true)),1).unwrap().is_empty());assert!(f.backend.take_received_replies().is_empty());let r=result(&mut f,packet.call_id,true,0);assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(packet.call_id,&bool_result(true)),4).unwrap().is_empty());assert!(f.backend.take_received_replies().is_empty());p.resolve_original(&r).unwrap();
}
#[test]
fn received_write_malformed_positive_payload_never_resolves_marker_or_creates_owned_reply(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(packet.call_id,&[0,1]),1).is_err());assert!(f.backend.take_received_replies().is_empty());assert_eq!(state(&f),(0,Some(packet.call_id),7));assert!(!f.scope.is_open());
}
#[test]
fn received_write_journal_resolve_failure_retains_original_unknown_outcome_and_scope_retirement(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=result(&mut f,packet.call_id,true,0);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_receipt_result BEFORE UPDATE OF state ON app_voice_im_read_receipts BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();assert!(p.resolve_original(&r).is_err());assert_eq!(state(&f),(0,Some(packet.call_id),7));p.cancel();assert!(!f.scope.is_open());
}
#[test]
fn received_write_signed64_maximum_target_is_preserved_and_debug_never_exports_identity_nonce_token(){
    let mut f=Fixture::new(true);let s=span(&mut f,i64::MAX-1,i64::MAX,1);let p=admit(&f,&s);let packet=write(&mut f,&p);let r=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();let mut a=Reader::versioned(r.arguments).unwrap();assert_eq!(a.bytes().unwrap(),b"P2P/1_2");assert_eq!(a.i64().unwrap(),i64::MAX);let printed=format!("{p:?} {s:?}");for private in ["fixture","P2P/1_2","nonce","token"]{assert!(!printed.contains(private));}assert_eq!(state(&f).2,i64::MAX);
}
