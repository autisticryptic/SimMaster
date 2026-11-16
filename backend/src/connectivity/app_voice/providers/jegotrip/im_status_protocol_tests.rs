//! Actions-only original Status.Update protocol/SQL/batch ownership regressions.
use super::*;
use n::{im_status as s,im_status_delivery::IncomingImStatus,im_inbox::CommittedImStatus};
fn args(label:&[u8],time:i64,properties:&[RpcProperty<'_>])->Vec<u8>{
    let mut body=Writer::new();body.properties(properties).unwrap();let body=body.finish();let time=time.to_string();
    let mut w=Writer::versioned().unwrap();w.bytes(s::STATUS_KIND).unwrap();w.properties(&[RpcProperty{key:b"Group.GroupId",value:label},RpcProperty{key:b"Group.UpdateTime",value:time.as_bytes()}]).unwrap();w.bytes(body.expose_to_transport()).unwrap();w.finish().expose_to_transport().to_vec()
}
fn take_status(events:Vec<OfflineEvent>)->Box<IncomingImStatus>{assert_eq!(events.len(),1);match events.into_iter().next().unwrap(){OfflineEvent::ImStatus(d)=>d,_=>panic!("only original status holder")}}
fn receive(f:&mut Fixture,rpc:i32,label:&[u8],time:i64,properties:&[RpcProperty<'_>])->Box<IncomingImStatus>{take_status(f.receive(rpc,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&args(label,time,properties),None))}
fn inbox(f:&Fixture,d:&IncomingImStatus)->ImInboxScope{ImInboxScope::original(f.run.clone(),d.notice().binding().clone(),f.gate.clone()).unwrap()}
fn commit(f:&Fixture,d:&IncomingImStatus)->CommittedImStatus{inbox(f,d).commit_status(&f.temp.database,d).unwrap()}
fn count(f:&Fixture)->i64{f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_status_observations",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
fn value(f:&Fixture,key:&[u8])->(i64,Vec<u8>){f.temp.database.with_conn(|c|c.query_row("SELECT update_time,status_value FROM app_voice_im_status_properties WHERE status_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture")).unwrap()}
#[test]
fn im_status_consumer_default_absence_nacks_without_status_message_or_cursor(){
    let mut f=Fixture::new(false);let bytes=args(b"1_2",7,&[RpcProperty{key:s::LAST_RECEIVED,value:b"9"}]);let events=f.receive(1,n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),&bytes,None);assert_eq!(control_result(&events),[0;6]);assert_eq!(count(&f),0);assert_eq!(f.count(),0);assert!(f.scope.is_open());
}
#[test]
fn im_status_consumer_original_direct_ticket_is_deferred_until_atomic_private_observation(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",4294967297,&[RpcProperty{key:s::LAST_RECEIVED,value:b"9223372036854775807"}]);assert_eq!(count(&f),0);assert!(d.matches(&f.owner,&f.scope));let proof=commit(&f,&d);assert!(proof.authorizes(d.ticket(),Instant::now()));assert_eq!(value(&f,s::LAST_RECEIVED),(4294967297,b"9223372036854775807".to_vec()));assert_eq!(result(f.backend.complete_im_status(d.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);assert_eq!(count(&f),1);
}
#[test]
fn im_status_consumer_partial_properties_at_different_times_do_not_erase_or_alias_each_other(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",8,&[RpcProperty{key:s::LAST_RECEIVED,value:b"9"}]);commit(&f,&d);let d=receive(&mut f,2,b"1_2",9,&[RpcProperty{key:s::LAST_READ,value:b"7"}]);commit(&f,&d);let d=receive(&mut f,3,b"1_2",9,&[RpcProperty{key:s::PEER_READ_NOTIFICATION,value:b"6"}]);commit(&f,&d);assert_eq!(count(&f),1);assert_eq!(value(&f,s::LAST_RECEIVED),(8,b"9".to_vec()));assert_eq!(value(&f,s::LAST_READ),(9,b"7".to_vec()));assert_eq!(value(&f,s::PEER_READ_NOTIFICATION),(9,b"6".to_vec()));
}
#[test]
fn im_status_consumer_equal_time_disjoint_keys_merge_but_changed_existing_key_rolls_back_all(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",9,&[RpcProperty{key:s::LAST_READ,value:b"7"}]);commit(&f,&d);let d=receive(&mut f,2,b"1_2",9,&[RpcProperty{key:s::LATEST,value:b"10"},RpcProperty{key:s::LAST_READ,value:b"8"}]);assert_eq!(inbox(&f,&d).commit_status(&f.temp.database,&d).err(),Some("app_account_im_status_conflict"));assert_eq!(value(&f,s::LAST_READ),(9,b"7".to_vec()));let n:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_status_properties",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(n,1);assert_eq!(result(f.backend.complete_im_status(d.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_status_consumer_old_update_is_handled_without_regressing_existing_property_or_time(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",9,&[RpcProperty{key:s::LAST_READ,value:b"7"}]);commit(&f,&d);let d=receive(&mut f,2,b"1_2",8,&[RpcProperty{key:s::LAST_READ,value:b"6"}]);let proof=commit(&f,&d);assert!(proof.authorizes(d.ticket(),Instant::now()));assert_eq!(value(&f,s::LAST_READ),(9,b"7".to_vec()));assert_eq!(result(f.backend.complete_im_status(d.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);
}
#[test]
fn im_status_consumer_duplicate_does_not_add_history_and_is_not_an_im_message_or_notice_ack(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[RpcProperty{key:s::LATEST,value:b"9"},RpcProperty{key:s::PEER_READ,value:b"8"}]);commit(&f,&d);let d=receive(&mut f,2,b"1_2",7,&[RpcProperty{key:s::PEER_READ,value:b"8"},RpcProperty{key:s::LATEST,value:b"9"}]);commit(&f,&d);assert_eq!(count(&f),1);assert_eq!(f.count(),0);let hint=f.hint(3,99,b"P2P/1_2",9);assert!(f.inbox(&hint).lookup(&f.temp.database,&hint).unwrap().is_none());assert!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),16).unwrap().is_empty());let cursor:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_receive_cursors",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(cursor,0);
}
#[test]
fn im_status_consumer_private_unknown_fields_are_bounded_data_not_executable_or_ready(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[RpcProperty{key:b"P:P_Custom-N",value:b"<script>private-status</script>"}]);let proof=commit(&f,&d);assert_eq!(value(&f,b"P:P_Custom-N").1,b"<script>private-status</script>");assert_eq!(f.count(),0);assert!(f.temp.database.get_sms_messages_for_channel(16,0,None,Some(f.run.line_id())).unwrap().is_empty());assert!(f.temp.database.get_call_history_for_line(f.run.line_id(),16,0).unwrap().is_empty());let printed=format!("{d:?} {:?} {:?} {proof:?}",d.notice(),d.ticket());for private in ["private-status","1_2","Custom","identity"]{assert!(!printed.contains(private));}
}
#[test]
fn im_status_consumer_generic_sms_path_cannot_complete_a_current_status_ticket(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[]);let ImReplySource::Direct(inner)=d.ticket().source()else{panic!("direct")};assert!(f.backend.complete_direct_message(inner,true).is_err());assert_eq!(result(f.backend.complete_im_status(d.ticket(),None).unwrap()),[0;6]);assert_eq!(count(&f),0);
}
#[test]
fn im_status_consumer_proof_cannot_redeem_other_payload_key_or_scope(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[RpcProperty{key:s::LAST_READ,value:b"7"}]);let proof=commit(&f,&d);let other=receive(&mut f,2,b"1_2",8,&[RpcProperty{key:s::LAST_READ,value:b"8"}]);assert!(!proof.authorizes(other.ticket(),Instant::now()));assert!(f.backend.complete_im_status(other.ticket(),Some(&proof)).is_err());let mut foreign=Fixture::new(true);let other=receive(&mut foreign,1,b"1_2",7,&[RpcProperty{key:s::LAST_READ,value:b"7"}]);assert!(!proof.authorizes(other.ticket(),Instant::now()));assert!(inbox(&f,&d).commit_status(&foreign.temp.database,&other).is_err());
}
#[test]
fn im_status_consumer_sql_property_failure_rolls_back_label_and_returns_no_proof(){
    let mut f=Fixture::new(true);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_status BEFORE INSERT ON app_voice_im_status_properties BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();let d=receive(&mut f,1,b"1_2",7,&[RpcProperty{key:s::LAST_READ,value:b"7"}]);assert!(inbox(&f,&d).commit_status(&f.temp.database,&d).is_err());assert_eq!(count(&f),0);assert_eq!(result(f.backend.complete_im_status(d.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_status_consumer_revision_and_live_runtime_lease_are_required_not_historical_body(){
    for revision in [false,true]{let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[]);let ctx=inbox(&f,&d);f.temp.database.with_conn(|c|c.execute(if revision{"UPDATE app_voice_accounts SET revision=2"}else{"DELETE FROM app_voice_runtime_leases"},[]).map_err(|_|"fixture")).unwrap();assert!(ctx.commit_status(&f.temp.database,&d).is_err());assert_eq!(count(&f),0);}
}
#[test]
fn im_status_consumer_parent_gate_and_original_session_revoke_disable_previously_durable_receipt(){
    for session in [false,true]{let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[]);let ctx=inbox(&f,&d);let proof=commit(&f,&d);if session{f.scope.mark_session_revoked();}else{f.gate.retire();}assert!(!proof.authorizes(d.ticket(),Instant::now()));assert!(ctx.commit_status(&f.temp.database,&d).is_err());assert_eq!(count(&f),1);}
}
#[test]
fn im_status_consumer_fixed_first_deadline_cannot_be_extended_by_a_durable_row(){
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",7,&[]);let proof=commit(&f,&d);assert!(!proof.authorizes(d.ticket(),d.ticket().deadline()));assert!(proof.authorizes(d.ticket(),Instant::now()));let old=d.ticket().captured_copy();assert_eq!(old.deadline(),d.ticket().deadline());assert!(!proof.authorizes(&old,old.deadline()));
}
#[test]
fn im_status_consumer_full_per_account_label_budget_rejects_new_label_without_evicting_originals(){
    let mut f=Fixture::new(true);for id in 1..=128{let name=format!("{id}_1");let d=receive(&mut f,id,name.as_bytes(),7,&[]);let proof=commit(&f,&d);f.backend.complete_im_status(d.ticket(),Some(&proof)).unwrap();}assert_eq!(count(&f),128);let d=receive(&mut f,129,b"999_1",7,&[]);assert_eq!(inbox(&f,&d).commit_status(&f.temp.database,&d).err(),Some("app_account_im_status_full"));assert_eq!(count(&f),128);assert_eq!(result(f.backend.complete_im_status(d.ticket(),None).unwrap()),[0;6]);
}
#[test]
fn im_status_consumer_property_budget_accumulates_across_updates_and_rolls_back_129th_key(){
    let mut f=Fixture::new(true);let keys=(0..128).map(|n|format!("Private.{n}")).collect::<Vec<_>>();let props=keys.iter().map(|k|RpcProperty{key:k.as_bytes(),value:b"v"}).collect::<Vec<_>>();let d=receive(&mut f,1,b"1_2",7,&props);commit(&f,&d);let d=receive(&mut f,2,b"1_2",8,&[RpcProperty{key:b"NewKey",value:b"v"}]);assert_eq!(inbox(&f,&d).commit_status(&f.temp.database,&d).err(),Some("app_account_im_status_full"));let time:i64=f.temp.database.with_conn(|c|c.query_row("SELECT update_time FROM app_voice_im_status_observations",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(time,7);
}
fn batch_status(f:&mut Fixture)->Box<IncomingImStatus>{
    let request=f.backend.prepare_swap(0,&[]).unwrap();let outer=decode_request(&request.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(outer.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{w.u32(word).unwrap();}w.u32(1).unwrap();w.i32(1).unwrap();w.bytes(n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();w.bytes(&args(b"1_2",7,&[RpcProperty{key:s::LAST_READ,value:b"7"}])).unwrap();w.properties(&[RpcProperty{key:b"id",value:b"MessageReceiver"}]).unwrap();w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();
    let events=f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(outer.call_id,w.finish().expose_to_transport()),1).unwrap();events.into_iter().find_map(|e|if let OfflineEvent::ImStatus(d)=e{Some(d)}else{None}).unwrap()
}
#[test]
fn im_status_consumer_batch_original_result_keeps_shared_im_write_guard_until_exact_epoch_ack(){
    let mut f=Fixture::new(true);let d=batch_status(&mut f);let ImReplySource::Batch(t)=d.ticket().source()else{panic!("batch")};assert!(f.backend.complete_message(t,true).is_err());let proof=commit(&f,&d);assert!(matches!(f.backend.complete_im_status(d.ticket(),Some(&proof)).unwrap(),Some(n::offline_backend::CallCompletion::BatchQueued)));assert_eq!(f.backend.im_write_guard().0.len(),1);
    let next=f.backend.prepare_swap(2,&[]).unwrap();let outer=decode_request(&next.packets[0].expose_to_transport()[1..]).unwrap();let mut r=Reader::versioned(outer.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();assert_eq!(payload.results[&1].result_stream,[0,1,0,0,0,0]);let ack=n::runtime_backend::tests::swap_result(None,true,payload.fields);f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(outer.call_id,&ack),3).unwrap();assert!(f.backend.im_write_guard().0.is_empty());assert_eq!(count(&f),1);
}
#[test]
fn im_status_consumer_revoked_batch_positive_cannot_replay_or_change_to_negative(){
    let mut f=Fixture::new(true);let d=batch_status(&mut f);let proof=commit(&f,&d);f.backend.complete_im_status(d.ticket(),Some(&proof)).unwrap();f.gate.retire();assert!(f.backend.prepare_swap(2,&[]).is_err());assert!(!f.scope.is_open());assert_eq!(count(&f),1);
}
#[test]
fn im_status_consumer_all_known_notification_indices_reject_noncanonical_overflow_and_absent_header(){
    for value in [b"01".as_slice(),b"-2",b"9223372036854775808",b"7\0"]{let mut body=Writer::new();body.properties(&[RpcProperty{key:s::PEER_READ_NOTIFICATION,value}]).unwrap();let body=body.finish();assert!(s::decode_owned_status(b"1_2",7,body.expose_to_transport()).is_err());}
    let mut f=Fixture::new(true);let d=receive(&mut f,1,b"1_2",-1,&[RpcProperty{key:s::LAST_READ,value:b"-1"}]);commit(&f,&d);assert_eq!(value(&f,s::LAST_READ),(-1,b"-1".to_vec()));
}
