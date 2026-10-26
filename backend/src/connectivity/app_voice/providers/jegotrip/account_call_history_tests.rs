//! Actions-only SQLite original app-call history identity/transaction tests.
use super::*;
use super::super::{account_store,account_test_support::{TempDatabase,config},account_runtime::local_nonce,tls_engine::tests::Certificates};
fn fixture()->(TempDatabase,RunToken){let temp=TempDatabase::new();let cert=Certificates::generate("DNS:fixture.invalid");
    let line=format!("app-account-{}",local_nonce().unwrap());temp.database.with_conn(|conn|account_store::create(conn,&line,&config(&cert.root),100)).unwrap();
    let run=temp.database.with_conn(|conn|account_runs::register_runtime(conn,&line,1,&local_nonce().unwrap())).unwrap();(temp,run)}
#[test]
fn original_call_history_and_started_event_commit_together_and_answer_end_keep_line(){
    let(temp,run)=fixture();let ticket=insert(&temp.database,&run,&local_nonce().unwrap(),"fixture-caller").unwrap();
    let row=temp.database.get_call_by_id(ticket.id()).unwrap().unwrap();assert_eq!(row.line_id.as_deref(),Some(run.line_id()));assert!(!row.answered);
    assert_eq!(temp.database.get_recent_app_events(Some(run.line_id()),10).unwrap()[0].event_type,"call.started");
    assert!(connected(&temp.database,&ticket).unwrap());assert!(ended(&temp.database,&ticket,10,true).unwrap());
    let row=temp.database.get_call_by_id(ticket.id()).unwrap().unwrap();assert!(row.answered);assert_eq!(row.duration,10);assert!(row.end_time.is_some());
    assert!(!ended(&temp.database,&ticket,20,true).unwrap());assert!(!connected(&temp.database,&ticket).unwrap());
    assert!(!serde_json::to_string(&row).unwrap().contains("app_call_token"));
}
#[test]
fn deleted_and_reused_sqlite_id_never_receives_old_call_answer_end_or_event(){
    let(temp,run)=fixture();let ticket=insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();
    temp.database.delete_call_for_line(run.line_id(),ticket.id()).unwrap();
    temp.database.with_conn(|conn|conn.execute("INSERT INTO call_history(id,line_id,direction,phone_number,duration,start_time,answered) VALUES(?1,'other-line','incoming','other',0,'fixture',0)",[ticket.id()]).map_err(|_|STORAGE)).unwrap();
    assert!(!connected(&temp.database,&ticket).unwrap());assert!(!ended(&temp.database,&ticket,99,true).unwrap());
    let row=temp.database.get_call_by_id(ticket.id()).unwrap().unwrap();assert_eq!(row.line_id.as_deref(),Some("other-line"));assert!(!row.answered);assert!(row.end_time.is_none());
    assert!(temp.database.get_recent_app_events(Some(run.line_id()),20).unwrap().iter().all(|event|event.event_type!="call.connected"&&event.event_type!="call.ended"));
}
#[test]
fn failing_started_event_rolls_back_history_instead_of_acknowledging_raw_metadata_only(){
    let(temp,run)=fixture();temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_app_call BEFORE INSERT ON app_events WHEN NEW.event_type='call.started' BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|STORAGE)).unwrap();
    assert_eq!(insert(&temp.database,&run,&local_nonce().unwrap(),"").err(),Some(STORAGE));
    assert!(temp.database.get_call_history_for_line(run.line_id(),20,0).unwrap().is_empty());assert!(temp.database.get_recent_app_events(Some(run.line_id()),20).unwrap().is_empty());
}
#[test]
fn old_runtime_revision_or_nonexclusive_run_cannot_create_current_call_history(){
    let(temp,run)=fixture();temp.database.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[run.line_id()]).map_err(|_|STORAGE)).unwrap();
    assert_eq!(insert(&temp.database,&run,&local_nonce().unwrap(),"").err(),Some(SCOPE));
    assert!(temp.database.get_call_history_for_line(run.line_id(),20,0).unwrap().is_empty());
}
