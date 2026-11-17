//! Actions-only original readonly Group reconciliation DATA/SQL/ownership.
//! Neither a reported user index nor these fixtures redeem a native WRITE.
use super::*;
use n::im_inbox::{ImReconciliationAction, ImReadObservation};
fn observation_action(f: &Fixture, receipt_id: i64) -> Arc<ImReconciliationAction> {
    ImReconciliationAction::original(f.temp.database.clone(), f.run.clone(), f.owner.clone(), f.scope.clone(), f.gate.clone(), receipt_id, Instant::now()+Duration::from_secs(30)).unwrap()
}
fn recorded(index: i64) -> (Fixture, i64) {
    let mut f=Fixture::new(true);let id=message_id(&mut f,91,index);let a=action(&f,id,&"a".repeat(32));let _permit=checked(&mut f,&a,Some(index),true).unwrap();
    let receipt=f.temp.database.with_conn(|c|c.query_row("SELECT id FROM app_voice_im_user_reads",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();(f,receipt)
}
fn sample(read: Option<i64>, accepted: bool, present: bool) -> Vec<u8> {
    let mut w=Writer::new();w.boolean(accepted).unwrap();w.u32(u32::from(present)).unwrap();
    if present { w.bytes(b"1_2").unwrap();let value=read.map(|n|n.to_string());let mut props=vec![RpcProperty{key:n::im_status::LATEST,value:b"9223372036854775807"},RpcProperty{key:n::im_status::LAST_RECEIVED,value:b"8"}];if let Some(value)=&value{props.push(RpcProperty{key:n::im_status::LAST_READ,value:value.as_bytes()});}w.properties(&props).unwrap(); }
    w.i64(4294967297).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
fn observe(f: &mut Fixture, receipt: i64, read: Option<i64>, accepted: bool, present: bool) -> ImReadObservation {
    let action=observation_action(f,receipt);let context=action.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(context.query(),action.gate(),action.deadline()).unwrap();
    f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&sample(read,accepted,present)),1).unwrap();context.observe(&f.backend.take_im_replies().pop().unwrap()).unwrap()
}
#[test]
fn im_reconciliation_current_report_does_not_clear_unknown_or_allow_another_user_nonce(){
    let(mut f,id)=recorded(7);let value=observe(&mut f,id,Some(9),true,true);assert_eq!(value.local_state,"uncertain");assert_eq!(value.report,"reported");assert_eq!(value.reported_last_read.as_deref(),Some("9"));assert_eq!(state(&f),(0,None,7));let message=f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().pop().unwrap().id;let next=action(&f,message,&"b".repeat(32));assert_eq!(next.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_uncertain"));
}
#[test]
fn im_reconciliation_missing_negative_or_lower_report_never_means_original_write_not_sent(){
    for read in [None,Some(-1),Some(6)]{let(mut f,id)=recorded(7);let value=observe(&mut f,id,read,true,true);assert_eq!(value.reported_last_read,read.map(|n|n.to_string()));assert_eq!(value.local_state,"uncertain");assert_eq!(state(&f),(0,None,7));}
}
#[test]
fn im_reconciliation_rejected_and_unreported_are_finite_distinct_observations_not_ack(){
    for(accepted,present,report)in[(false,true,"rejected"),(true,false,"unreported")]{let(mut f,id)=recorded(7);let value=observe(&mut f,id,Some(9),accepted,present);assert_eq!(value.report,report);assert_eq!(value.reported_last_read,None);assert_eq!(value.reported_latest,None);assert_eq!(value.server_update_time,None);assert_eq!(state(&f),(0,None,7));}
}
#[test]
fn im_reconciliation_preserves_full_signed64_positions_and_current_group_timestamp(){
    let(mut f,id)=recorded(i64::MAX);let value=observe(&mut f,id,Some(i64::MAX),true,true);assert_eq!(value.target_index,"9223372036854775807");assert_eq!(value.reported_last_read.as_deref(),Some("9223372036854775807"));assert_eq!(value.server_update_time.as_deref(),Some("4294967297"));assert_eq!(state(&f),(0,None,i64::MAX));
}
#[test]
fn im_reconciliation_same_bytes_from_an_unrelated_query_cannot_fill_current_observation(){
    let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(Arc::new(ImQuery::refresh(&[], -1).unwrap()),a.gate(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&sample(Some(9),true,true)),1).unwrap();assert!(context.observe(&f.backend.take_im_replies().pop().unwrap()).is_err());assert_eq!(state(&f),(0,None,7));
}
#[test]
fn im_reconciliation_original_query_with_another_gate_cannot_redeem_current_action(){
    let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(context.query(),ImReceiptGate::fresh(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&sample(Some(9),true,true)),1).unwrap();assert!(context.observe(&f.backend.take_im_replies().pop().unwrap()).is_err());
}
#[test]
fn im_reconciliation_foreign_native_binding_and_owner_scope_cannot_query_a_local_receipt(){
    let(f,id)=recorded(7);let other=Fixture::new(true);let a=observation_action(&f,id);assert!(a.bind_current(other.backend.captured_im_binding().unwrap()).is_err());assert_eq!(state(&f),(0,None,7));
}
#[test]
fn im_reconciliation_invalid_id_or_already_expired_action_creates_no_scope_or_user_nonce(){
    let(f,id)=recorded(7);for receipt in [0,-1]{assert!(ImReconciliationAction::original(f.temp.database.clone(),f.run.clone(),f.owner.clone(),f.scope.clone(),f.gate.clone(),receipt,Instant::now()+Duration::from_secs(1)).is_err());}assert!(ImReconciliationAction::original(f.temp.database.clone(),f.run.clone(),f.owner.clone(),f.scope.clone(),f.gate.clone(),id,Instant::now()).is_err());assert_eq!(state(&f),(0,None,7));
}
#[test]
fn im_reconciliation_retired_parent_or_action_rejects_late_result_without_user_write(){
    for parent in [false,true]{let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(context.query(),a.gate(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&sample(Some(9),true,true)),1).unwrap();let r=f.backend.take_im_replies().pop().unwrap();if parent{f.gate.retire();}else{a.retire();}assert!(context.observe(&r).is_err());assert_eq!(state(&f),(0,None,7));assert!(f.scope.is_open());}
}
#[test]
fn im_reconciliation_deleted_tombstone_can_be_observed_without_text_or_event_resurrection(){
    let(mut f,id)=recorded(7);let message=f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().pop().unwrap().id;f.temp.database.with_conn(|c|n::im_inbox::delete(c,f.run.line_id(),message)).unwrap();let value=observe(&mut f,id,Some(9),true,true);assert_eq!(value.local_state,"uncertain");assert!(f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().is_empty());assert_eq!(f.count(),1);
}
#[test]
fn im_reconciliation_changed_target_record_after_query_cannot_rebind_an_observation(){
    let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(context.query(),a.gate(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&sample(Some(9),true,true)),1).unwrap();let r=f.backend.take_im_replies().pop().unwrap();f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_im_user_reads SET target_index=8",[]).map_err(|_|"fixture")).unwrap();assert!(context.observe(&r).is_err());
}
#[test]
fn im_reconciliation_revision_or_message_fingerprint_change_prevents_a_positive_report(){
    for revision in [false,true]{let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(context.query(),a.gate(),a.deadline()).unwrap();f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&sample(Some(9),true,true)),1).unwrap();let r=f.backend.take_im_replies().pop().unwrap();f.temp.database.with_conn(|c|if revision{c.execute("UPDATE app_voice_accounts SET revision=2",[]).map_err(|_|"fixture")}else{c.execute("UPDATE app_voice_im_messages SET fingerprint=zeroblob(32)",[]).map_err(|_|"fixture")}).unwrap();assert!(context.observe(&r).is_err());assert_eq!(state(&f),(0,None,7));}
}
#[test]
fn im_reconciliation_original_native_query_is_single_label_uid_and_never_set_read_or_status_write(){
    let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let p=f.backend.prepare_im_read(context.query(),a.gate(),a.deadline()).unwrap();let raw=decode_request(&p.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(raw.method,n::im_status::REFRESH_METHOD.as_bytes());let mut r=Reader::versioned(raw.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"1_2");assert_eq!(r.u32().unwrap(),5);for _ in 0..5{r.bytes().unwrap();}assert_eq!(r.i64().unwrap(),-1);r.finish().unwrap();assert_eq!(state(&f),(0,None,7));
}
#[test]
fn im_reconciliation_queries_have_zero_sql_changes_and_emit_no_new_receipt_or_message_event(){
    let(mut f,id)=recorded(7);let before:i64=f.temp.database.with_conn(|c|c.query_row("SELECT total_changes()",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();let _=observe(&mut f,id,Some(9),true,true);let after:i64=f.temp.database.with_conn(|c|c.query_row("SELECT total_changes()",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(before,after);assert_eq!(f.count(),1);assert_eq!(state(&f),(0,None,7));
}
#[test]
fn im_reconciliation_debug_and_projection_never_export_original_context_credentials_or_write_id(){
    let(mut f,id)=recorded(7);let a=observation_action(&f,id);let context=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap();let value=observe(&mut f,id,Some(9),true,true);let text=serde_json::to_string(&value).unwrap()+&format!("{a:?} {context:?}");for secret in ["1_2","fixture-user","fixture-auth","user_nonce","request_id","scope_epoch","identity_key","box_id","fingerprint"]{assert!(!text.contains(secret));}assert_eq!(state(&f),(0,None,7));
}
