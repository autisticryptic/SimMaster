//! Actions-only original SDK/SQLite/RPC relation WRITEs, never a live peer.
#[path="relation_intent_protocol_tests.rs"]
mod admission;
#[path="group_management_tests.rs"]
mod management;
#[path="group_create_tests.rs"]
mod creation;
use super::*;
use n::{group_directory_data::Relation,group_relation_updates::Removal,relation_update_policy::NativeRelationUpdateConfig as Policy,
    relation_journal::{RelationAction,RelationMessage,RelationPermit,PreparedRelation}};
fn policy()->Policy{Policy{additions:true,updates:true,removals:true}}
fn relation(label:&[u8])->Relation<'_>{Relation{label,flags:1,first:b"remark",second:b"",properties:vec![]}}
fn prepared(f:&Fixture,nonce:&str,group:Option<&[u8]>,label:&[u8])->PreparedRelation{
    let message=RelationMessage::capture(&f.scope,group,&[relation(label)],&[],&[],nonce,policy()).unwrap();
    RelationAction::original(f.temp.database.clone(),f.run.clone(),f.backend.captured_im_binding().unwrap(),f.backend.captured_relation_identity().unwrap(),f.gate.clone(),message,Instant::now()+Duration::from_secs(30),policy()).unwrap()
}
fn action(f:&Fixture)->Arc<RelationAction>{let PreparedRelation::Fresh(a)=prepared(f,&"a".repeat(32),None,b"1_2")else{panic!("one fresh relation action")};a}
fn state(f:&Fixture)->(i64,Option<i32>,Option<i64>,Option<i64>){f.temp.database.with_conn(|c|c.query_row("SELECT state,request_id,base_time,update_time FROM app_voice_relation_writes ORDER BY id LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|_|"fixture")).unwrap()}
fn write(f:&mut Fixture,permit:&Arc<RelationPermit>)->n::arc_protocol::OwnedCallPackets{f.backend.prepare_relation_update(permit.clone(),Instant::now()+Duration::from_secs(30)).unwrap()}
fn reply(f:&mut Fixture,id:i32,accepted:bool)->n::rpc_relation_update::OwnedRelationReply{
    let mut w=Writer::new();w.boolean(accepted).unwrap();w.i64(i64::MIN).unwrap();w.i64(i64::MAX).unwrap();w.bytes(b"").unwrap();
    assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(id,w.finish().expose_to_transport()),1).unwrap().is_empty());let mut replies=f.backend.take_relation_replies();assert_eq!(replies.len(),1);replies.pop().unwrap()
}
#[test]
fn relation_action_marker_precedes_rpc_id_and_buddy_uid_comes_only_from_current_sdk(){
    let mut f=Fixture::new(true);let a=action(&f);assert_eq!(state(&f),(0,None,None,None));let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert_eq!(packet.call_id,2);
    let request=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();assert_eq!(request.method,b"setRelations.GroupServer.Group");assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
    let mut args=Reader::versioned(request.arguments).unwrap();assert_eq!(args.bytes().unwrap(),b"fixture-user");assert_eq!(args.u32().unwrap(),1);assert_eq!(args.bytes().unwrap(),b"1_2");assert_eq!(args.u32().unwrap(),1);assert_eq!(args.bytes().unwrap(),b"remark");assert_eq!(args.bytes().unwrap(),b"");assert!(args.properties().unwrap().is_empty());assert_eq!(args.u32().unwrap(),0);assert_eq!(args.u32().unwrap(),0);args.finish().unwrap();assert_eq!(state(&f),(0,Some(2),None,None));p.cancel();assert!(f.scope.is_open());
}
#[test]
fn relation_action_explicit_group_id_is_not_current_uid_or_p2p_box(){
    let mut f=Fixture::new(true);let PreparedRelation::Fresh(a)=prepared(&f,&"a".repeat(32),Some(b"1-2"),b"1_2")else{panic!()};let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);let request=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();let mut args=Reader::versioned(request.arguments).unwrap();assert_eq!(args.bytes().unwrap(),b"1-2");p.cancel();assert_eq!(state(&f),(3,Some(2),None,None));assert!(f.scope.is_open());
}
#[test]
fn relation_action_same_nonce_uncertain_is_history_only_with_no_new_id_or_token(){
    let f=Fixture::new(true);let _a=action(&f);let PreparedRelation::Replay(v)=prepared(&f,&"a".repeat(32),None,b"1_2")else{panic!("metadata replay only")};assert_eq!(v.state,"uncertain");assert_eq!(state(&f),(0,None,None,None));
}
#[test]
fn relation_action_changed_body_same_nonce_conflicts_before_new_rpc(){
    let f=Fixture::new(true);let _a=action(&f);let m=RelationMessage::capture(&f.scope,None,&[relation(b"9_9")],&[],&[],&"a".repeat(32),policy()).unwrap();
    assert_eq!(RelationAction::original(f.temp.database.clone(),f.run.clone(),f.backend.captured_im_binding().unwrap(),f.backend.captured_relation_identity().unwrap(),f.gate.clone(),m,Instant::now()+Duration::from_secs(1),policy()).err(),Some("app_account_relation_nonce_conflict"));assert_eq!(state(&f),(0,None,None,None));
}
#[test]
fn relation_action_unknown_blocks_a_new_nonce_even_for_an_explicit_other_group(){
    let f=Fixture::new(true);let _a=action(&f);let m=RelationMessage::capture(&f.scope,Some(b"7-8"),&[],&[],&[Removal{label:b"9_9",flag:false}],&"b".repeat(32),policy()).unwrap();
    assert_eq!(RelationAction::original(f.temp.database.clone(),f.run.clone(),f.backend.captured_im_binding().unwrap(),f.backend.captured_relation_identity().unwrap(),f.gate.clone(),m,Instant::now()+Duration::from_secs(1),policy()).err(),Some("app_account_relation_uncertain"));assert_eq!(state(&f),(0,None,None,None));
}
#[test]
fn relation_action_queued_cancel_is_not_sent_without_closing_sdk(){
    let f=Fixture::new(true);let a=action(&f);a.cancel();assert_eq!(state(&f),(3,None,None,None));assert!(f.scope.is_open());assert!(a.permit(policy()).is_err());
}
#[test]
fn relation_permit_queued_cancel_after_allocation_cannot_emit_any_bytes(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);p.cancel();assert_eq!(state(&f),(3,Some(packet.call_id),None,None));assert!(!p.begin_write(packet.call_id));assert!(!p.closed_original_io());assert!(f.scope.is_open());
}
#[test]
fn relation_permit_possible_or_fully_submitted_cancel_keeps_unknown_and_closes_original(){
    for full in [false,true]{let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));if full{assert!(p.submitted());}p.cancel();assert_eq!(state(&f),(0,Some(packet.call_id),None,None));assert!(p.closed_original_io());assert!(!f.scope.is_open());}
}
#[test]
fn relation_private_ack_confirms_exact_signed_times_and_commits_one_finite_event(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let result=reply(&mut f,packet.call_id,true);let view=p.resolve(&result).unwrap();assert_eq!(view.state,"confirmed");assert_eq!(view.base_time.as_deref(),Some("-9223372036854775808"));assert_eq!(view.update_time.as_deref(),Some("9223372036854775807"));assert_eq!(state(&f),(1,Some(2),Some(i64::MIN),Some(i64::MAX)));
    let events=f.temp.database.get_recent_app_events(Some(f.run.line_id()),16).unwrap();assert_eq!(events.len(),1);assert_eq!(events[0].event_type,"app_voice.relations_updated");assert!(p.resolve(&result).is_err());p.cancel();assert!(f.scope.is_open());assert_eq!(f.count(),0);
}
#[test]
fn relation_false_reply_is_terminal_rejected_not_an_automatic_retry(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=reply(&mut f,packet.call_id,false);assert_eq!(p.resolve(&r).unwrap().state,"rejected");assert!(f.backend.prepare_relation_update(p.clone(),Instant::now()+Duration::from_secs(1)).is_err());assert!(f.scope.is_open());
}
#[test]
fn relation_foreign_binding_scope_cannot_arm_a_current_identity_or_steal_id(){
    let mut f=Fixture::new(true);let mut other=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();assert!(other.backend.prepare_relation_update(p.clone(),Instant::now()+Duration::from_secs(1)).is_err());assert_eq!(state(&f),(0,None,None,None));assert_eq!(write(&mut f,&p).call_id,2);p.cancel();assert!(other.scope.is_open());
}
#[test]
fn relation_event_storage_failure_rolls_back_terminal_state_and_keeps_unknown(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=reply(&mut f,packet.call_id,true);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER relation_event_failure BEFORE INSERT ON app_events WHEN NEW.event_type='app_voice.relations_updated' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();assert!(p.resolve(&r).is_err());assert_eq!(state(&f),(0,Some(2),None,None));assert!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),16).unwrap().is_empty());p.cancel();assert!(!f.scope.is_open());
}
#[test]
fn relation_changed_marker_or_configuration_after_arm_prevents_first_io_poll(){
    for config in [false,true]{let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);f.temp.database.with_conn(|c|c.execute(if config{"UPDATE app_voice_accounts SET private_config=X'00'"}else{"UPDATE app_voice_relation_writes SET arguments_hash=zeroblob(32)"},[]).map_err(|_|"fixture")).unwrap();assert!(!p.begin_write(packet.call_id));p.cancel();assert!(f.scope.is_open());}
}
#[test]
fn relation_sql_admission_failure_does_not_consume_original_positive_rpc_id(){
    let mut f=Fixture::new(true);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_relation BEFORE INSERT ON app_voice_relation_writes BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();
    let m=RelationMessage::capture(&f.scope,None,&[relation(b"1_2")],&[],&[],&"a".repeat(32),policy()).unwrap();assert!(RelationAction::original(f.temp.database.clone(),f.run.clone(),f.backend.captured_im_binding().unwrap(),f.backend.captured_relation_identity().unwrap(),f.gate.clone(),m,Instant::now()+Duration::from_secs(1),policy()).is_err());let packet=f.backend.prepare_im_read(Arc::new(n::rpc_im::ImQuery::refresh(&[],-1).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(packet.call_id,2);
}
#[test]
fn relation_actual_detached_packet_holds_budget_until_last_frame_drop_not_wrapper_drop(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let mut frame=write(&mut f,&p);let held=frame.packets.pop().unwrap();f.backend.retire_relation_update(frame.call_id);drop(frame);p.cancel();drop(p);drop(a);assert_ne!(f.scope.native_rpc_resources(),(0,0));drop(held);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]
fn relation_private_reply_has_no_public_conversion_from_code_time_boolean_or_history(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let r=reply(&mut f,packet.call_id,true);let other=Fixture::new(true);let foreign=action(&other);let foreign_p=foreign.permit(policy()).unwrap();assert!(foreign_p.resolve(&r).is_err());assert_eq!(state(&other),(0,None,None,None));p.cancel();
}
#[test]
fn relation_missing_native_receiver_cannot_export_identity_or_construct_a_permit(){
    let f=Fixture::new(false);assert!(f.backend.captured_relation_identity().is_err());assert!(f.backend.captured_im_binding().is_none());
}
#[test]
fn relation_input_policy_conflicts_invalid_nonce_or_box_are_refused_before_copy(){
    let f=Fixture::new(true);for nonce in ["", "ABCDEF", "not-a-valid-32-byte-lowercase-nonce!"]{assert!(RelationMessage::capture(&f.scope,None,&[relation(b"1_2")],&[],&[],nonce,policy()).is_err());}
    for group in [b"1_2".as_slice(),b"Org/1-2",b"#object:tcp -h fixture.invalid -p 1;"]{assert!(RelationMessage::capture(&f.scope,Some(group),&[relation(b"1_2")],&[],&[],&"a".repeat(32),policy()).is_err());}
    assert!(RelationMessage::capture(&f.scope,None,&[relation(b"x")],&[relation(b"x")],&[],&"a".repeat(32),policy()).is_err());assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]
fn relation_history_and_debug_never_export_raw_uid_label_body_nonce_token_or_config(){
    let f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let message=format!("{a:?} {p:?}");for private in ["fixture","1_2","remark","token","nonce"]{assert!(!message.contains(private));}
    let views=f.temp.database.with_conn(|c|n::relation_journal::history(c,f.run.line_id(),16)).unwrap();let body=serde_json::to_string(&views).unwrap();for private in ["fixture-user","1_2","remark","fixture-auth","user_nonce","intent_hash","arguments_hash","private_config",&"a".repeat(32)]{assert!(!body.contains(private));}
}
#[test]
fn relation_result_event_is_published_once_only_after_sql_commit_without_sdk_or_identity_projection(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packets=write(&mut f,&p);let mut events=f.temp.database.subscribe_app_events();assert!(events.try_recv().is_err());
    assert!(p.begin_write(packets.call_id));assert!(p.submitted());let result=reply(&mut f,packets.call_id,true);let data=p.resolve(&result).unwrap();let event=events.try_recv().unwrap();assert_eq!(event.event_type,"app_voice.relations_updated");assert_eq!(event.line_id.as_deref(),Some(f.run.line_id()));assert_eq!(state(&f),(1,Some(packets.call_id),Some(i64::MIN),Some(i64::MAX)));
    let body:serde_json::Value=serde_json::from_str(&event.payload_json).unwrap();assert_eq!(body["relation_write_id"],data.id);assert_eq!(body["state"],"confirmed");for hidden in ["fixture-user","fixture-auth","1_2","remark","token","ready","user_nonce","arguments_hash",&"a".repeat(32)]{assert!(!event.payload_json.contains(hidden));}
    assert!(p.resolve(&result).is_err());assert!(events.try_recv().is_err());assert!(f.scope.is_open());
}
#[test]
fn relation_failed_event_transaction_publishes_nothing_and_does_not_lift_unknown(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let packets=write(&mut f,&p);let mut events=f.temp.database.subscribe_app_events();assert!(p.begin_write(packets.call_id));assert!(p.submitted());let result=reply(&mut f,packets.call_id,true);
    f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER fail_relation_broadcast BEFORE INSERT ON app_events WHEN NEW.event_type='app_voice.relations_updated' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();assert!(p.resolve(&result).is_err());assert_eq!(state(&f),(0,Some(packets.call_id),None,None));assert!(events.try_recv().is_err());p.cancel();assert!(!f.scope.is_open());
}
#[test]
fn relation_history_corrupt_private_metadata_or_inconsistent_times_are_not_sanitized_into_permission(){
    for change in ["UPDATE app_voice_relation_writes SET updated_at='private-token'","UPDATE app_voice_relation_writes SET user_nonce='AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'","UPDATE app_voice_relation_writes SET state=1"]{
        let f=Fixture::new(true);let _a=action(&f);f.temp.database.with_conn(|c|c.execute_batch(change).map_err(|_|"fixture")).unwrap();assert_eq!(f.temp.database.with_conn(|c|n::relation_journal::history(c,f.run.line_id(),128)).err(),Some("app_account_relation_storage_failed"));assert_eq!(state(&f).1,None);assert!(f.scope.is_open());
    }
}
#[test]
fn relation_cancelled_submitted_reply_stays_unknown_and_no_new_nonce_reopens_sdk(){
    let mut f=Fixture::new(true);let a=action(&f);let p=a.permit(policy()).unwrap();let frame=write(&mut f,&p);assert!(p.begin_write(frame.call_id));assert!(p.submitted());p.cancel();assert!(f.backend.take_relation_replies().is_empty());assert_eq!(state(&f),(0,Some(2),None,None));assert!(!f.scope.is_open());
}
