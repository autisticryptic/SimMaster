//! Existing call history with private app-call identity CAS. A deleted/reused
//! SQLite id or another account cannot receive a late original app-call update.
use super::{account_runs::{self, RunToken}, account_store};
use crate::platform::db::{Database, insert_app_event_for_conn};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use std::fmt;
const STORAGE: &str = "app_account_call_storage_failed";
const SCOPE: &str = "app_account_call_scope_invalid";
pub(super) struct HistoryTicket { id: i64, line: String, token: String }
impl fmt::Debug for HistoryTicket { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppCallHistoryTicket(<redacted>)") } }
impl HistoryTicket { pub(super) fn id(&self) -> i64 { self.id } }
pub fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    let exists = {
        let mut query = conn.prepare("PRAGMA table_info(call_history)").map_err(|_| STORAGE)?;
        let names = query.query_map([], |row| row.get::<_, String>(1)).map_err(|_| STORAGE)?.collect::<Result<Vec<_>, _>>().map_err(|_| STORAGE)?;
        names.iter().any(|name| name == "app_call_token")
    };
    if !exists { conn.execute_batch("ALTER TABLE call_history ADD COLUMN app_call_token TEXT").map_err(|_| STORAGE)?; }
    Ok(())
}
pub(super) fn insert(db: &Database, run: &RunToken, local_id: &str, caller: &str) -> Result<HistoryTicket, &'static str> {
    if !account_store::valid_line_id(run.line_id()) || local_id.len() != 32 || !local_id.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err(SCOPE); }
    let (id, event) = db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
        account_runs::require_live(&tx, run.line_id(), run.revision(), run.nonce())?;
        if !account_runs::runtime_matches(&tx, run)? { return Err(SCOPE); }
        let metadata = account_store::metadata(&tx, run.line_id())?.ok_or(SCOPE)?;
        if metadata.revision != run.revision() { return Err(SCOPE); }
        let now = chrono::Utc::now().to_rfc3339();
        tx.execute("INSERT INTO call_history(line_id,direction,phone_number,duration,start_time,answered,app_call_token) VALUES(?1,'incoming',?2,0,?3,0,?4)",
            params![run.line_id(), caller, now, local_id]).map_err(|_| STORAGE)?;
        let id = tx.last_insert_rowid();
        let payload = serde_json::json!({"call_id":id,"direction":"incoming","phone_number":caller,"answered":false,"transport":"app_voice"}).to_string();
        let event = insert_app_event_for_conn(&tx,"call.started",Some(run.line_id()),Some("app_voice"),&payload,&now).map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)?; Ok((id,event))
    })?;
    db.publish_committed_app_event(event);
    Ok(HistoryTicket { id, line: run.line_id().to_owned(), token: local_id.to_owned() })
}
pub(super) fn connected(db: &Database, ticket: &HistoryTicket) -> Result<bool, &'static str> {
    update(db, ticket, None, true)
}
pub(super) fn ended(db: &Database, ticket: &HistoryTicket, duration: i64, answered: bool) -> Result<bool, &'static str> {
    update(db, ticket, Some(duration.max(0)), answered)
}
fn update(db: &Database, ticket: &HistoryTicket, duration: Option<i64>, answered: bool) -> Result<bool, &'static str> {
    let event = db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
        let now = chrono::Utc::now().to_rfc3339();
        let changed = if let Some(duration) = duration {
            tx.execute("UPDATE call_history SET duration=?4,end_time=?5,answered=?6 WHERE id=?1 AND line_id=?2 AND app_call_token=?3 AND end_time IS NULL",
                params![ticket.id,ticket.line,ticket.token,duration,now,answered as i32]).map_err(|_| STORAGE)?
        } else {
            tx.execute("UPDATE call_history SET answered=1 WHERE id=?1 AND line_id=?2 AND app_call_token=?3 AND end_time IS NULL AND answered=0",
                params![ticket.id,ticket.line,ticket.token]).map_err(|_| STORAGE)?
        };
        if changed == 0 { tx.commit().map_err(|_| STORAGE)?; return Ok(None); }
        let payload = serde_json::json!({"call_id":ticket.id,"duration":duration,"answered":answered,"transport":"app_voice"}).to_string();
        let event = insert_app_event_for_conn(&tx,if duration.is_some(){"call.ended"}else{"call.connected"},Some(&ticket.line),Some("app_voice"),&payload,&now).map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)?; Ok(Some(event))
    })?;
    if let Some(event) = event { db.publish_committed_app_event(event); Ok(true) } else { Ok(false) }
}
#[cfg(test)]
#[path = "account_call_history_tests.rs"]
mod tests;
