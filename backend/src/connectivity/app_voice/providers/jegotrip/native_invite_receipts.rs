//! Durable original Invite consumption association, separate from channel
//! delivery, call answer/media readiness and the original RPC reply write.
use super::{account_call_history::HistoryTicket, account_runs::{self, RunToken}, account_store};
use crate::platform::db::Database;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
const STORAGE: &str = "app_account_invite_notification_storage_failed";
const SCOPE: &str = "app_account_invite_notification_scope_invalid";
const MAX_RECEIPTS: i64 = 2048;

pub(super) struct InviteConsumptionReceipt { notification_id: u64, fingerprint: [u8;32] }
impl InviteConsumptionReceipt {
    pub(super) fn matches(&self, notification_id: u64, fingerprint: &[u8;32]) -> bool { self.notification_id == notification_id && &self.fingerprint == fingerprint }
}
impl std::fmt::Debug for InviteConsumptionReceipt { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("InviteConsumptionReceipt(<redacted>)") } }

pub(super) fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_invite_notification_receipts (
        line_id TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
        run_nonce TEXT NOT NULL, notification_id BLOB NOT NULL CHECK(typeof(notification_id)='blob' AND length(notification_id)=8),
        fingerprint BLOB NOT NULL CHECK(typeof(fingerprint)='blob' AND length(fingerprint)=32),
        call_history_id INTEGER NOT NULL, call_token TEXT NOT NULL,
        consumed_at TEXT NOT NULL,
        PRIMARY KEY(line_id,run_nonce,notification_id),
        FOREIGN KEY(call_history_id) REFERENCES call_history(id) ON DELETE CASCADE
    ); CREATE INDEX IF NOT EXISTS idx_app_voice_invite_receipts_line ON app_voice_invite_notification_receipts(line_id);")
        .map_err(|_| STORAGE)
}

/// Called only with an already-published original call, after its matched
/// existence Inform and actual local incoming indication. The transaction
/// requires the same live run/revision/history token; an ID reused after
/// deletion or another run cannot inherit this receipt. No raw user message,
/// caller/URL/token/SDK identity or channel delivery data is stored here.
pub(super) fn consume(db: &Database, run: &RunToken, history: &HistoryTicket,
    notification_id: u64, fingerprint: &[u8;32]) -> Result<InviteConsumptionReceipt, &'static str> {
    if notification_id == 0 || history.line() != run.line_id() { return Err(SCOPE); }
    let id = notification_id.to_be_bytes();
    db.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
        account_runs::require_live(&tx, run.line_id(), run.revision(), run.nonce())?;
        if !account_runs::runtime_matches(&tx, run)? || account_store::metadata(&tx, run.line_id())?.is_none_or(|row| row.revision != run.revision()) {
            return Err(SCOPE);
        }
        let current: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM call_history WHERE id=?1 AND line_id=?2 AND app_call_token=?3 AND direction='incoming' AND end_time IS NULL)",
            params![history.id(),history.line(),history.token()], |row| row.get(0)).map_err(|_| STORAGE)?;
        if !current { return Err(SCOPE); }
        let prior: Option<(i64,Vec<u8>,i64,String)> = tx.query_row(
            "SELECT revision,fingerprint,call_history_id,call_token FROM app_voice_invite_notification_receipts WHERE line_id=?1 AND run_nonce=?2 AND notification_id=?3",
            params![run.line_id(),run.nonce(),id.as_slice()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(|_| STORAGE)?;
        if let Some((revision, prior, call_id, token)) = prior {
            if revision != run.revision() as i64 || prior.as_slice() != fingerprint || call_id != history.id() || token != history.token() { return Err(SCOPE); }
            tx.commit().map_err(|_| STORAGE)?; return Ok(InviteConsumptionReceipt { notification_id, fingerprint: *fingerprint });
        }
        let retained: i64 = tx.query_row("SELECT COUNT(*) FROM app_voice_invite_notification_receipts WHERE line_id=?1", [run.line_id()], |row| row.get(0)).map_err(|_| STORAGE)?;
        if retained >= MAX_RECEIPTS { return Err("app_account_invite_notification_backpressure"); }
        tx.execute("INSERT INTO app_voice_invite_notification_receipts(line_id,revision,run_nonce,notification_id,fingerprint,call_history_id,call_token,consumed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![run.line_id(),run.revision() as i64,run.nonce(),id.as_slice(),fingerprint.as_slice(),history.id(),history.token(),chrono::Utc::now().to_rfc3339()])
            .map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)?;
        Ok(InviteConsumptionReceipt { notification_id, fingerprint: *fingerprint })
    })
}
