//! Actions-only original authenticated reply -> atomic IM cursor/records tests.
//! The fake peer supplies DATA to the original pending ID; no raw bool or row
//! creates an IO/notification/write capability. Existing fixtures remain intact.
use super::*;
use n::{im_inbox::{CursorSession,OnlinePosition},rpc_im::{ImQuery,OwnedImReply},im_status};
fn status(rows:&[(&[u8],Option<i64>,Option<i64>)],time:i64,yes:bool)->Vec<u8>{
    let mut w=Writer::new();w.boolean(yes).unwrap();w.u32(rows.len()as u32).unwrap();for(label,base,target)in rows{w.bytes(label).unwrap();let base=base.map(|v|v.to_string());let target=target.map(|v|v.to_string());let mut p=Vec::new();if let Some(b)=&base{p.push(RpcProperty{key:im_status::PEER_READ,value:b.as_bytes()});}if let Some(t)=&target{p.push(RpcProperty{key:im_status::LATEST,value:t.as_bytes()});}w.properties(&p).unwrap();}w.i64(time).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
fn received(rows:&[(i64,&[u8],&[u8])],yes:bool)->Vec<u8>{
    let mut w=Writer::new();w.boolean(yes).unwrap();w.u32(rows.len()as u32).unwrap();for(index,kind,text)in rows{w.i64(*index).unwrap();w.i64(-123).unwrap();w.bytes(b"private-sender-uid").unwrap();w.bytes(kind).unwrap();w.bytes(text).unwrap();w.properties(&[RpcProperty{key:b"../private-attachment",value:b"private-attachment-bytes"}]).unwrap();w.properties(&[RpcProperty{key:b"account",value:b"private-native-account"}]).unwrap();}w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
fn roundtrip(f:&mut Fixture,query:Arc<ImQuery>,bytes:&[u8])->OwnedImReply{
    let p=f.backend.prepare_im_read(query,f.gate.clone(),Instant::now()+Duration::from_secs(3)).unwrap();let events=f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,bytes),0).unwrap();assert!(events.is_empty());let mut result=f.backend.take_im_replies();assert_eq!(result.len(),1);result.pop().unwrap()
}
fn start(f:&mut Fixture,rows:&[(&[u8],Option<i64>,Option<i64>)])->CursorSession{
    let reply=roundtrip(f,Arc::new(ImQuery::refresh(&[],-1).unwrap()),&status(rows,1,true));let mut cursor=CursorSession::from_refresh(f.run.clone(),f.gate.clone(),&reply).unwrap();assert!(cursor.observe_refresh(&f.temp.database,&reply).unwrap());cursor
}
fn positions(f:&Fixture,box_id:&[u8])->(i64,i64,i64){f.temp.database.with_conn(|conn|conn.query_row("SELECT baseline,consumed,target FROM app_voice_im_receive_cursors WHERE line_id=?1 AND box_id=?2",rusqlite::params![f.run.line_id(),box_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"fixture")).unwrap()}
fn events(f:&Fixture)->usize{f.temp.database.get_recent_app_events(Some(f.run.line_id()),128).unwrap().iter().filter(|e|e.event_type=="im.received").count()}
#[test]
fn im_cursor_original_initial_status_creates_baseline_observation_not_history_or_write(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(6),Some(7))]);assert!(cursor.initial());assert_eq!(positions(&f,b"P2P/1_2"),(6,6,7));assert_eq!(f.count(),0);assert_eq!(events(&f),0);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();assert!(matches!(plan.query().as_ref(),ImQuery::Receive{start:7,count:1,..}));
}
#[test]
fn im_cursor_absent_peer_read_is_not_zero_and_notification_index_cannot_seed_any_cursor(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",None,Some(7))]);assert!(!cursor.initial());assert!(cursor.next_gap(&f.temp.database).unwrap().is_none());let notice=f.hint(1,99,b"P2P/1_2",i64::MAX);assert!(cursor.online_position(&f.temp.database,&notice).is_err());let n:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_receive_cursors",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(n,0);
}
#[test]
fn im_cursor_sparse_nonempty_recv_commits_all_records_and_advances_only_last_actual_index(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(20))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[(9,b"Text",br#"{"text":"nine"}"#),(2,b"Text",br#"{"text":"two"}"#)],true));let committed=cursor.commit_gap(&f.temp.database,&plan,&response).unwrap();assert_eq!(committed.end(),9);assert!(committed.is_current(&f.temp.database));assert_eq!(committed.proofs().len(),2);assert_eq!(f.count(),2);assert_eq!(events(&f),2);assert_eq!(positions(&f,b"P2P/1_2"),(0,9,20));assert!(matches!(cursor.next_gap(&f.temp.database).unwrap().unwrap().query().as_ref(),ImQuery::Receive{start:10,count:10,..}));
}
#[test]
fn im_cursor_positive_empty_reply_uses_only_its_original_requested_span_and_cannot_ack_missing_message(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(25))]);for expected in [10,20,25]{let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[],true));let committed=cursor.commit_gap(&f.temp.database,&plan,&response).unwrap();assert_eq!(committed.end(),expected);assert!(committed.proofs().is_empty());assert!(committed.is_current(&f.temp.database));}
    assert!(cursor.next_gap(&f.temp.database).unwrap().is_none());assert_eq!(f.count(),0);assert_eq!(events(&f),0);let notice=f.hint(1,99,b"P2P/1_2",25);assert!(f.inbox(&notice).lookup(&f.temp.database,&notice).unwrap().is_none());
}
#[test]
fn im_cursor_negative_receive_never_advances_or_becomes_empty_success(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(7))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[],false));assert_eq!(cursor.commit_gap(&f.temp.database,&plan,&response).err(),Some("app_account_im_receive_rejected"));assert_eq!(positions(&f,b"P2P/1_2"),(0,0,7));assert_eq!(f.count(),0);
}
#[test]
fn im_cursor_unknown_returned_type_rolls_back_whole_span_instead_of_silent_advance(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(2))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[(1,b"Text",br#"{"text":"one"}"#),(2,b"Info.Unknown",b"opaque")],true));assert_eq!(cursor.commit_gap(&f.temp.database,&plan,&response).err(),Some("app_account_im_type_unsupported"));assert_eq!(f.count(),0);assert_eq!(events(&f),0);assert_eq!(positions(&f,b"P2P/1_2"),(0,0,2));
}
#[test]
fn im_cursor_malformed_second_text_never_leaves_first_message_event_or_advanced_cursor(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(2))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[(1,b"Text",br#"{"text":"one"}"#),(2,b"Text",b"not json")],true));assert!(cursor.commit_gap(&f.temp.database,&plan,&response).is_err());assert_eq!(f.count(),0);assert_eq!(events(&f),0);assert_eq!(positions(&f,b"P2P/1_2"),(0,0,2));
}
#[test]
fn im_cursor_sql_event_failure_rolls_back_records_outcome_and_cursor_in_one_transaction(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(2))]);f.temp.database.with_conn(|c|c.execute_batch("CREATE TRIGGER reject_cursor_record BEFORE INSERT ON app_events WHEN NEW.event_type='im.received' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|"fixture")).unwrap();let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[(1,b"Text",br#"{"text":"one"}"#),(2,b"Text",br#"{"text":"two"}"#)],true));assert!(cursor.commit_gap(&f.temp.database,&plan,&response).is_err());assert_eq!(f.count(),0);assert_eq!(positions(&f,b"P2P/1_2"),(0,0,2));assert_eq!(events(&f),0);
}
#[test]
fn im_cursor_changed_existing_record_refuses_entire_gap_and_preserves_original_history(){
    let mut f=Fixture::new(true);let existing=f.message(1,b"P2P/1_2",2,r#"{"text":"saved"}"#);f.inbox(&existing).commit(&f.temp.database,&existing).unwrap();let cursor=start(&mut f,&[(b"1_2",Some(0),Some(2))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[(1,b"Text",br#"{"text":"one"}"#),(2,b"Text",br#"{"text":"changed"}"#)],true));assert_eq!(cursor.commit_gap(&f.temp.database,&plan,&response).err(),Some("app_account_im_message_conflict"));assert_eq!(f.count(),1);assert_eq!(events(&f),1);assert_eq!(positions(&f,b"P2P/1_2"),(0,0,2));
}
#[test]
fn im_cursor_original_plan_query_arc_is_not_replaceable_by_equivalent_caller_data(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(1))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let copied=Arc::new(ImQuery::receive(n::im_data::ImBox::parse(b"P2P/1_2").unwrap(),1,1).unwrap());let response=roundtrip(&mut f,copied,&received(&[],true));assert!(cursor.commit_gap(&f.temp.database,&plan,&response).is_err());assert_eq!(positions(&f,b"P2P/1_2"),(0,0,1));
}
#[test]
fn im_cursor_foreign_original_login_reply_cannot_advance_same_named_line_or_box(){
    let mut f=Fixture::new(true);let mut other=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(1))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut other,plan.query(),&received(&[],true));assert!(cursor.commit_gap(&f.temp.database,&plan,&response).is_err());assert_eq!(positions(&f,b"P2P/1_2"),(0,0,1));
}
#[test]
fn im_cursor_retired_gate_scope_and_runtime_lease_each_prevent_new_durable_cursor_advance(){
    for mode in 0..3{let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(1))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[],true));match mode{0=>f.gate.retire(),1=>f.scope.mark_session_revoked(),_=>{f.temp.database.with_conn(|c|c.execute("DELETE FROM app_voice_runtime_leases",[]).map_err(|_|"fixture")).unwrap();}}assert!(cursor.commit_gap(&f.temp.database,&plan,&response).is_err());assert_eq!(positions(&f,b"P2P/1_2"),(0,0,1));}
}
#[test]
fn im_cursor_new_peer_read_observation_never_skips_unconsumed_local_positions(){
    let mut f=Fixture::new(true);let mut cursor=start(&mut f,&[(b"1_2",Some(0),Some(2))]);let reply=roundtrip(&mut f,Arc::new(ImQuery::refresh(&[],cursor.since()).unwrap()),&status(&[(b"1_2",Some(99),Some(100))],2,true));assert!(cursor.observe_refresh(&f.temp.database,&reply).unwrap());assert_eq!(positions(&f,b"P2P/1_2"),(0,0,100));assert!(matches!(cursor.next_gap(&f.temp.database).unwrap().unwrap().query().as_ref(),ImQuery::Receive{start:1,count:10,..}));
}
#[test]
fn im_cursor_target_growth_preserves_captured_original_small_gap_and_cursor_cas(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(2))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let message=f.message(1,b"P2P/1_2",20,r#"{"text":"later"}"#);assert!(matches!(cursor.online_position(&f.temp.database,&message).unwrap(),OnlinePosition::Pending));let reply=roundtrip(&mut f,plan.query(),&received(&[],true));cursor.commit_gap(&f.temp.database,&plan,&reply).unwrap();assert_eq!(positions(&f,b"P2P/1_2"),(0,2,20));assert!(matches!(cursor.next_gap(&f.temp.database).unwrap().unwrap().query().as_ref(),ImQuery::Receive{start:3,count:10,..}));
}
#[test]
fn im_cursor_post_initial_real_online_message_seeds_box_but_cannot_skip_an_existing_gap(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[]);let first=f.message(1,b"Org/1-2",7,r#"{"text":"first"}"#);assert!(matches!(cursor.online_position(&f.temp.database,&first).unwrap(),OnlinePosition::Next));assert!(cursor.commit_online(&f.temp.database,&first).unwrap().is_some());assert_eq!(positions(&f,b"Org/1-2"),(6,7,7));let later=f.message(2,b"Org/1-2",9,r#"{"text":"later"}"#);assert!(matches!(cursor.online_position(&f.temp.database,&later).unwrap(),OnlinePosition::Pending));assert!(cursor.commit_online(&f.temp.database,&later).is_err());assert_eq!(positions(&f,b"Org/1-2"),(6,7,9));assert_eq!(f.count(),1);
}
#[test]
fn im_cursor_deleted_online_replay_uses_private_receipt_without_resurrecting_text_or_event(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[]);let first=f.message(1,b"P2P/1_2",7,r#"{"text":"deleted"}"#);cursor.online_position(&f.temp.database,&first).unwrap();cursor.commit_online(&f.temp.database,&first).unwrap();let id=f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap()[0].id;f.temp.database.with_conn(|c|n::im_inbox::delete(c,f.run.line_id(),id)).unwrap();let duplicate=f.message(2,b"P2P/1_2",7,r#"{"text":"deleted"}"#);assert!(matches!(cursor.online_position(&f.temp.database,&duplicate).unwrap(),OnlinePosition::Replay));assert!(cursor.commit_online(&f.temp.database,&duplicate).unwrap().is_some());assert!(f.temp.database.with_conn(|c|n::im_inbox::list(c,f.run.line_id(),16)).unwrap().is_empty());assert_eq!(events(&f),1);
}
#[test]
fn im_cursor_server_empty_history_is_not_an_existing_message_receipt_for_late_online_data(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(1))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[],true));cursor.commit_gap(&f.temp.database,&plan,&response).unwrap();let late=f.message(1,b"P2P/1_2",1,r#"{"text":"late"}"#);assert!(matches!(cursor.online_position(&f.temp.database,&late).unwrap(),OnlinePosition::MissingOlder));assert!(cursor.commit_online(&f.temp.database,&late).unwrap().is_none());assert_eq!(f.count(),0);assert_eq!(events(&f),0);
}
#[test]
fn im_cursor_signed_maximum_end_is_checked_without_wrapping_count_or_next_request(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(i64::MAX-1),Some(i64::MAX))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();assert!(matches!(plan.query().as_ref(),ImQuery::Receive{start:i64::MAX,count:1,..}));let response=roundtrip(&mut f,plan.query(),&received(&[],true));cursor.commit_gap(&f.temp.database,&plan,&response).unwrap();assert!(cursor.next_gap(&f.temp.database).unwrap().is_none());assert_eq!(positions(&f,b"P2P/1_2"),(i64::MAX-1,i64::MAX,i64::MAX));
}
#[test]
fn im_cursor_debug_and_plan_outcome_remain_private_and_no_raw_im_becomes_sms_or_call(){
    let mut f=Fixture::new(true);let cursor=start(&mut f,&[(b"1_2",Some(0),Some(1))]);let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();let response=roundtrip(&mut f,plan.query(),&received(&[],true));let committed=cursor.commit_gap(&f.temp.database,&plan,&response).unwrap();assert_eq!(format!("{cursor:?}"),"ImCursorSession(<redacted>)");assert_eq!(format!("{plan:?}"),"OriginalImGapPlan(<redacted>)");assert_eq!(format!("{committed:?}"),"CommittedImGap(<redacted>)");assert!(f.temp.database.get_sms_messages_for_channel(16,0,None,Some(f.run.line_id())).unwrap().is_empty());assert!(f.temp.database.get_call_history_for_line(f.run.line_id(),16,0).unwrap().is_empty());
}
