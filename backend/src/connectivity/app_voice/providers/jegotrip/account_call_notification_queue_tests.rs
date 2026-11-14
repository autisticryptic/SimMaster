//! Actions-only call-job SQLite ownership and uncertainty tests. No channel I/O.
use super::super::super::{account_call_notifications::*,account_call_history as history,account_notifications::{NoticePlan,NoticeTarget},account_runs::{self,RunToken},account_store,
    account_test_support::{TempDatabase,config},account_runtime::local_nonce,tls_engine::tests::Certificates};
use rusqlite::params;
fn fixture()->(TempDatabase,RunToken,history::HistoryTicket){
    let db=TempDatabase::new();let cert=Certificates::generate("DNS:fixture.invalid");let line=format!("app-account-{}",local_nonce().unwrap());
    db.database.with_conn(|c|account_store::create(c,&line,&config(&cert.root),100)).unwrap();
    let run=db.database.with_conn(|c|account_runs::register_runtime(c,&line,1,&local_nonce().unwrap())).unwrap();
    let ticket=history::insert(&db.database,&run,&local_nonce().unwrap(),"fixture-caller").unwrap();(db,run,ticket)
}
fn plan()->NoticePlan{NoticePlan::bounded(vec![NoticeTarget{rule_id:"rule-original".into(),channel_id:"channel-original".into(),fingerprint:"ab".repeat(32),title:"fixture-call-title".into(),body:"fixture-call-body".into()}]).unwrap()}
fn queue(db:&TempDatabase,run:&RunToken,ticket:&history::HistoryTicket){let record=history::notification_record(&db.database,ticket).unwrap();enqueue(&db.database,run,ticket,&record,plan()).unwrap();}
#[test]
fn call_notice_jobs_are_not_sms_rows_and_metadata_never_exposes_destination_or_native_identity(){
    let(db,run,ticket)=fixture();let sms=db.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM sms_messages",[],|r|r.get::<_,i64>(0)).map_err(|_|"test")).unwrap();queue(&db,&run,&ticket);
    let state=db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap();assert_eq!(state.len(),1);assert_eq!(state[0].call_history_id,ticket.id());assert_eq!(state[0].state,"pending");
    let text=serde_json::to_string(&state).unwrap();for private in ["fixture",run.nonce(),ticket.token(),"rule_id","channel_id","phone"]{assert!(!text.contains(private));}
    assert_eq!(db.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM sms_messages",[],|r|r.get::<_,i64>(0)).map_err(|_|"test")).unwrap(),sms);
}
#[test]
fn call_notice_exact_original_history_token_and_run_revision_gate_queue_and_claim(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);
    db.database.with_conn(|c|c.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[run.line_id()]).map_err(|_|"test")).unwrap();
    assert!(claim_next(&db.database,&run,&ticket).unwrap().is_none());assert!(enqueue(&db.database,&run,&ticket,&history::notification_record(&db.database,&ticket).unwrap(),plan()).is_err());
}
#[test]
fn call_notice_duplicate_admission_keeps_original_rendered_bytes_and_never_creates_a_second_target(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);let record=history::notification_record(&db.database,&ticket).unwrap();
    let changed=NoticePlan::bounded(vec![NoticeTarget{rule_id:"rule-original".into(),channel_id:"channel-original".into(),fingerprint:"ff".repeat(32),title:"must-not-replan".into(),body:"must-not-replan".into()}]).unwrap();enqueue(&db.database,&run,&ticket,&record,changed).unwrap();
    let work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();assert_eq!(work.title.as_str(),"fixture-call-title");assert_eq!(work.fingerprint,"ab".repeat(32));assert!(claim_next(&db.database,&run,&ticket).unwrap().is_none());
}
#[test]
fn call_notice_deleted_reused_history_id_cannot_claim_or_mark_a_different_call_delivered(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);let work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();
    db.database.delete_call_for_line(run.line_id(),ticket.id()).unwrap();
    db.database.with_conn(|c|c.execute("INSERT INTO call_history(id,line_id,direction,phone_number,duration,start_time,answered,app_call_token) VALUES(?1,?2,'incoming','other',0,'fixture',0,?3)",params![ticket.id(),run.line_id(),local_nonce().unwrap()]).map_err(|_|"test")).unwrap();
    assert!(!admitted(&db.database,&run,&ticket,&work).unwrap());uncertain(&db.database,&work);
    let state=db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap();assert_eq!(state[0].state,"uncertain");assert!(!db.database.get_call_by_id(ticket.id()).unwrap().unwrap().answered);
}
#[test]
fn call_notice_answer_or_end_cancels_pending_but_preserves_sending_as_uncertain(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);let work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();history::connected(&db.database,&ticket).unwrap();assert!(!admitted(&db.database,&run,&ticket,&work).unwrap());cancel(&db.database,&ticket);
    let state=db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap();assert_eq!(state[0].state,"uncertain");
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);history::ended(&db.database,&ticket,0,false).unwrap();cancel(&db.database,&ticket);assert_eq!(db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap()[0].state,"cancelled");
}
#[test]
fn call_notice_recovery_never_replays_sending_or_an_orphaned_ringing_plan(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);let _work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();db.database.with_conn(recover).unwrap();
    assert_eq!(db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap()[0].state,"uncertain");assert!(claim_next(&db.database,&run,&ticket).unwrap().is_none());
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);db.database.with_conn(recover).unwrap();assert_eq!(db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap()[0].state,"cancelled");
}
#[test]
fn call_notice_unknown_dispatch_is_terminal_and_scrubs_payload_without_retry(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);let work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();finish(&db.database,&work,CallNoticeOutcome::Uncertain).unwrap();
    assert!(claim_next(&db.database,&run,&ticket).unwrap().is_none());let result:(String,String,String)=db.database.with_conn(|c|c.query_row("SELECT state,title,body FROM app_voice_call_notification_jobs WHERE id=?1",[work.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"test")).unwrap();assert_eq!(result,("uncertain".into(),"".into(),"".into()));
}
#[test]
fn call_notice_pre_send_deferral_refunds_attempt_and_keeps_one_fixed_expiry(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);let work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();let expiry=db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap()[0].expires_at;
    finish(&db.database,&work,CallNoticeOutcome::Deferred(std::time::Duration::from_secs(1))).unwrap();let row=db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap();assert_eq!(row[0].attempts,0);assert_eq!(row[0].expires_at,expiry);assert_eq!(row[0].state,"pending");
    expire(&db.database,&ticket);assert_eq!(db.database.with_conn(|c|metadata(c,run.line_id(),16)).unwrap()[0].state,"expired");
}
#[test]
fn call_notice_destination_channel_is_serialized_with_sending_sms_under_four_worker_cap(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);
    db.database.with_conn(|c|c.execute("INSERT INTO app_voice_notification_jobs(sms_id,line_id,rule_id,channel_id,fingerprint,title,body,state,not_before,expires_at,created_at) VALUES(999,?1,'other','channel-original',?2,'','', 'sending',0,9999999999,0)",params![run.line_id(),"ab".repeat(32)]).map_err(|_|"test")).unwrap();
    assert!(claim_next(&db.database,&run,&ticket).unwrap().is_none());
    db.database.with_conn(|c|c.execute("UPDATE app_voice_notification_jobs SET state='delivered'",[]).map_err(|_|"test")).unwrap();let work=claim_next(&db.database,&run,&ticket).unwrap().unwrap();assert!(db.database.with_conn(|c|channel_sending(c,"channel-original")).unwrap());finish(&db.database,&work,CallNoticeOutcome::Delivered).unwrap();
}
#[test]
fn call_notice_malformed_database_payload_is_bounded_before_materialization(){
    let(db,run,ticket)=fixture();queue(&db,&run,&ticket);db.database.with_conn(|c|c.execute("UPDATE app_voice_call_notification_jobs SET title=zeroblob(1000000)",[]).map_err(|_|"test")).unwrap();assert!(claim_next(&db.database,&run,&ticket).unwrap().is_none());
    db.database.with_conn(|c|c.execute("UPDATE app_voice_call_notification_jobs SET state=?1",["x".repeat(1000000)]).map_err(|_|"test")).unwrap_err();
}
