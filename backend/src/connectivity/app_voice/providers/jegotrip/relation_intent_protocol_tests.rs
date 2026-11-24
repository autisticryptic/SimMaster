//! Actions-only before-await intent/SDK binding/Unknown-WRITE/lastDrop data tests.
//! These stored preparation reservations cannot authenticate or emit a frame.
use super::*;
use n::relation_journal::{RelationIntent,IntentAdmission,RelationIntentWait,RelationConfiguration};
fn admission(f:&Fixture,nonce:&str,group:Option<&[u8]>,label:&[u8])->Result<IntentAdmission,&'static str>{
    let message=Arc::new(RelationMessage::capture(&f.scope,group,&[relation(label)],&[],&[],nonce,policy())?);
    let configuration=Arc::new(f.temp.database.with_conn(|c|RelationConfiguration::capture(c,f.run.line_id(),f.run.revision(),Some(&f.scope)))?);
    RelationIntent::capture(f.temp.database.clone(),f.run.clone(),f.owner.clone(),f.scope.clone(),message,configuration,f.gate.clone(),Instant::now()+Duration::from_secs(30))
}
fn intent(f:&Fixture)->Arc<RelationIntent>{let IntentAdmission::Fresh(i)=admission(f,&"a".repeat(32),None,b"1_2").unwrap()else{panic!("fresh before-await preparation")};i}
fn early(f:&Fixture)->(i64,Option<i64>){f.temp.database.with_conn(|c|c.query_row("SELECT state,write_id FROM app_voice_relation_intents ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture early record")).unwrap()}
fn writes(f:&Fixture)->i64{f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_relation_writes",[],|r|r.get(0)).map_err(|_|"fixture writes")).unwrap()}
fn bound(f:&Fixture,i:&Arc<RelationIntent>)->Arc<RelationAction>{let PreparedRelation::Fresh(a)=i.bind(f.backend.captured_im_binding().unwrap(),f.backend.captured_relation_identity().unwrap(),policy()).unwrap()else{panic!("real sealed identity binding")};a}
#[test]fn relation_early_intent_is_durable_without_uid_rpc_id_write_marker_or_sdk_permission(){
    let f=Fixture::new(true);let _i=intent(&f);assert_eq!(early(&f),(0,None));assert_eq!(writes(&f),0);assert_eq!(f.scope.native_rpc_resources().0,1);assert!(f.scope.is_open());assert!(f.temp.database.with_conn(|c|n::relation_journal::history(c,f.run.line_id(),128)).unwrap().is_empty());
}
#[test]fn relation_early_same_nonce_is_not_a_second_preparation_or_native_permit(){
    let f=Fixture::new(true);let _i=intent(&f);assert_eq!(admission(&f,&"a".repeat(32),None,b"1_2").err(),Some("app_account_relation_preparing"));assert_eq!(early(&f),(0,None));assert_eq!(writes(&f),0);assert_eq!(f.scope.native_rpc_resources().0,1);
}
#[test]fn relation_early_changed_body_same_nonce_conflicts_before_identity_or_actor_queue(){
    let f=Fixture::new(true);let _i=intent(&f);assert_eq!(admission(&f,&"a".repeat(32),None,b"9_9").err(),Some("app_account_relation_nonce_conflict"));assert_eq!(writes(&f),0);assert!(f.scope.is_open());
}
#[test]fn relation_early_new_nonce_other_group_cannot_overtake_old_preparation(){
    let f=Fixture::new(true);let _i=intent(&f);assert_eq!(admission(&f,&"b".repeat(32),Some(b"7-8"),b"1_2").err(),Some("app_account_relation_preparing"));assert_eq!(early(&f),(0,None));assert_eq!(writes(&f),0);
}
#[test]fn relation_early_result_owner_drop_marks_only_preparing_not_sent_and_never_closes_sdk(){
    let f=Fixture::new(true);let i=intent(&f);let waiting=RelationIntentWait::original(i.clone());drop(waiting);assert_eq!(early(&f),(2,None));assert_eq!(writes(&f),0);assert!(f.scope.is_open());assert!(i.bind(f.backend.captured_im_binding().unwrap(),f.backend.captured_relation_identity().unwrap(),policy()).is_err());
    assert_eq!(admission(&f,&"a".repeat(32),None,b"1_2").err(),Some("app_account_relation_nonce_retired"));let next=admission(&f,&"b".repeat(32),None,b"1_2").unwrap();assert!(matches!(next,IntentAdmission::Fresh(_)));drop(next);drop(i);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]fn relation_early_record_with_old_run_or_missing_owner_is_not_recovered_by_a_read_or_new_nonce(){
    let f=Fixture::new(true);let i=intent(&f);i.cancel();drop(i);
    f.temp.database.with_conn(|c|c.execute("UPDATE app_voice_relation_intents SET state=0,run_nonce=?1,claim_nonce=?2",rusqlite::params!["c".repeat(32),"d".repeat(32)]).map_err(|_|"fixture old unowned preparation")).unwrap();
    assert_eq!(admission(&f,&"b".repeat(32),None,b"1_2").err(),Some("app_account_relation_preparing"));let records=f.temp.database.with_conn(|c|n::relation_journal::intent_history(c,f.run.line_id(),128)).unwrap();assert_eq!(records[0].state,"preparing");assert_eq!(admission(&f,&"b".repeat(32),None,b"1_2").err(),Some("app_account_relation_preparing"));assert_eq!(early(&f),(0,None));
}
#[test]fn relation_early_foreign_input_scope_cannot_be_charged_as_current_run_or_bound_identity(){
    let f=Fixture::new(true);let other=Fixture::new(true);let message=Arc::new(RelationMessage::capture(&other.scope,None,&[relation(b"1_2")],&[],&[],&"a".repeat(32),policy()).unwrap());let config=Arc::new(f.temp.database.with_conn(|c|RelationConfiguration::capture(c,f.run.line_id(),f.run.revision(),Some(&f.scope))).unwrap());
    assert_eq!(RelationIntent::capture(f.temp.database.clone(),f.run.clone(),f.owner.clone(),f.scope.clone(),message,config,f.gate.clone(),Instant::now()+Duration::from_secs(1)).err(),Some("app_account_relation_scope_invalid"));assert_eq!(writes(&f),0);
    let i=intent(&f);assert!(i.bind(other.backend.captured_im_binding().unwrap(),other.backend.captured_relation_identity().unwrap(),policy()).is_err());assert_eq!(early(&f),(0,None));assert_eq!(writes(&f),0);
}
#[test]fn relation_early_binding_is_atomic_with_actual_write_marker_and_keeps_original_sdk_uid(){
    let mut f=Fixture::new(true);let i=intent(&f);let a=bound(&f,&i);assert_eq!(writes(&f),1);let row_id=f.temp.database.with_conn(|c|c.query_row("SELECT id FROM app_voice_relation_writes",[],|r|r.get::<_,i64>(0)).map_err(|_|"fixture")).unwrap();assert_eq!(early(&f),(1,Some(row_id)));assert_eq!(state(&f),(0,None,None,None));
    let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert_eq!(packet.call_id,2);let request=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(Reader::versioned(request.arguments).unwrap().bytes().unwrap(),b"fixture-user");p.cancel();assert_eq!(state(&f),(3,Some(2),None,None));assert!(f.scope.is_open());
}
#[test]fn relation_early_bound_partial_write_cancel_keeps_unknown_and_does_not_retire_as_unwritten(){
    let mut f=Fixture::new(true);let i=intent(&f);let a=bound(&f,&i);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));let waiting=RelationIntentWait::original(i.clone());drop(waiting);assert!(!f.scope.is_open());assert!(p.closed_original_io());assert_eq!(state(&f),(0,Some(2),None,None));assert_eq!(early(&f).0,1);
}
#[test]fn relation_early_known_result_same_nonce_is_history_only_and_cannot_allocate_another_request(){
    let mut f=Fixture::new(true);let i=intent(&f);let a=bound(&f,&i);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let result=reply(&mut f,packet.call_id,true);let data=p.resolve(&result).unwrap();
    let IntentAdmission::Replay(id)=admission(&f,&"a".repeat(32),None,b"1_2").unwrap()else{panic!("metadata only")};assert_eq!(id,data.id);assert_eq!(writes(&f),1);assert_eq!(state(&f),(1,Some(2),Some(i64::MIN),Some(i64::MAX)));assert_eq!(admission(&f,&"a".repeat(32),None,b"different").err(),Some("app_account_relation_nonce_conflict"));
    let next=admission(&f,&"b".repeat(32),None,b"1_2").unwrap();assert!(matches!(next,IntentAdmission::Fresh(_)));drop(next);assert_eq!(writes(&f),1);
}
#[test]fn relation_early_sql_failure_rolls_back_without_native_write_id_or_budget_leak(){
    let mut f=Fixture::new(true);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER refuse_early_relation BEFORE INSERT ON app_voice_relation_intents BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();
    assert_eq!(admission(&f,&"a".repeat(32),None,b"1_2").err(),Some("app_account_relation_storage_failed"));assert_eq!(writes(&f),0);assert_eq!(f.scope.native_rpc_resources(),(0,0));let packet=f.backend.prepare_im_read(Arc::new(n::rpc_im::ImQuery::refresh(&[],-1).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(packet.call_id,2);
}
#[test]fn relation_early_legacy_write_is_not_adopted_or_cleared_without_original_body_proof(){
    let f=Fixture::new(true);let _a=action(&f);assert_eq!(admission(&f,&"a".repeat(32),None,b"1_2").err(),Some("app_account_relation_legacy_intent"));assert_eq!(admission(&f,&"b".repeat(32),None,b"1_2").err(),Some("app_account_relation_uncertain"));assert_eq!(state(&f),(0,None,None,None));assert!(f.scope.is_open());
}
#[test]fn relation_early_snapshot_is_bounded_redacted_data_not_uid_token_nonce_or_write_permission(){
    let f=Fixture::new(true);let i=intent(&f);let records=f.temp.database.with_conn(|c|n::relation_journal::intent_history(c,f.run.line_id(),128)).unwrap();let data=serde_json::to_string(&records).unwrap();assert_eq!(records.len(),1);assert_eq!(records[0].state,"preparing");assert_eq!(records[0].write_id,None);
    for private in ["1_2","remark","fixture-user","fixture-auth","body_hash","configuration_hash","claim_nonce","run_nonce","user_nonce",&"a".repeat(32)]{assert!(!data.contains(private));}assert_eq!(format!("{i:?}"),"RelationIntent(<redacted>)");assert!(f.temp.database.with_conn(|c|n::relation_journal::intent_history(c,f.run.line_id(),0)).is_err());assert!(f.scope.is_open());
}
