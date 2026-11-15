//! Actions-only SQLite receipt CAS; actual SDK/Inform/indication ordering is
//! covered separately by the real native_invite_account_tests peer suite.
use super::*;
use super::super::super::native_invite_receipts as receipt;
use rusqlite::params;
fn count(temp:&TempDatabase)->i64{temp.database.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_invite_notification_receipts",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap()}
#[test]
fn invite_receipt_full64_id_and_fingerprint_commit_idempotently_without_answer_sms_or_secret_projection(){
    let(temp,run)=fixture();let history=insert(&temp.database,&run,&local_nonce().unwrap(),"fixture-caller").unwrap();let id=0xf123_4567_89ab_cdef;let fingerprint=[7;32];let result=receipt::consume(&temp.database,&run,&history,id,&fingerprint).unwrap();assert!(result.matches(id,&fingerprint));assert!(!result.matches(id-1,&fingerprint));assert_eq!(format!("{result:?}"),"InviteConsumptionReceipt(<redacted>)");receipt::consume(&temp.database,&run,&history,id,&fingerprint).unwrap();assert_eq!(count(&temp),1);
    let saved:Vec<u8>=temp.database.with_conn(|conn|conn.query_row("SELECT notification_id FROM app_voice_invite_notification_receipts",[],|row|row.get(0)).map_err(|_|STORAGE)).unwrap();assert_eq!(saved,id.to_be_bytes());assert!(!temp.database.get_call_by_id(history.id()).unwrap().unwrap().answered);assert!(temp.database.get_sms_messages_for_channel(10,0,None,Some(run.line_id())).unwrap().is_empty());
}
#[test]
fn invite_receipt_same_id_cannot_replace_fingerprint_or_current_original_call_history(){
    let(temp,run)=fixture();let first=insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();let second=insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();receipt::consume(&temp.database,&run,&first,1,&[7;32]).unwrap();assert!(receipt::consume(&temp.database,&run,&first,1,&[8;32]).is_err());assert!(receipt::consume(&temp.database,&run,&second,1,&[7;32]).is_err());assert_eq!(count(&temp),1);
}
#[test]
fn invite_receipt_deleted_reused_id_and_another_line_cannot_inherit_old_history_token(){
    let(temp,run)=fixture();let history=insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();temp.database.delete_call_for_line(run.line_id(),history.id()).unwrap();assert!(receipt::consume(&temp.database,&run,&history,1,&[7;32]).is_err());temp.database.with_conn(|conn|conn.execute("INSERT INTO call_history(id,line_id,direction,phone_number,duration,start_time,answered,app_call_token) VALUES(?1,?2,'incoming','replacement',0,'fixture',0,?3)",params![history.id(),run.line_id(),"bb".repeat(16)]).map_err(|_|STORAGE)).unwrap();assert!(receipt::consume(&temp.database,&run,&history,1,&[7;32]).is_err());assert_eq!(count(&temp),0);
}
#[test]
fn invite_receipt_revision_change_rejects_before_consumption_or_current_claim(){
    let(temp,run)=fixture();let history=insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();temp.database.with_conn(|conn|conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1",[run.line_id()]).map_err(|_|STORAGE)).unwrap();assert!(receipt::consume(&temp.database,&run,&history,1,&[7;32]).is_err());assert_eq!(count(&temp),0);
}
#[test]
fn invite_receipt_storage_failure_rolls_back_without_relabelling_history_as_answered(){
    let(temp,run)=fixture();let history=insert(&temp.database,&run,&local_nonce().unwrap(),"").unwrap();temp.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER fail_invite_receipt BEFORE INSERT ON app_voice_invite_notification_receipts BEGIN SELECT RAISE(ABORT,'fixture'); END").map_err(|_|STORAGE)).unwrap();assert!(receipt::consume(&temp.database,&run,&history,1,&[7;32]).is_err());assert_eq!(count(&temp),0);let call=temp.database.get_call_by_id(history.id()).unwrap().unwrap();assert!(!call.answered);assert!(call.end_time.is_none());
}
