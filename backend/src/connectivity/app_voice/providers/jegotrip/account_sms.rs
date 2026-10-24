//! Atomic projection of verified Smgw text into existing incoming SMS history.
//! Native/transport ids stay private; sms_id is LOCAL and never a remote UUID.
//! Unknown generic inbox data is not promoted, and no provider/channel/paid-send
//! capability is granted. External notification forwarding remains separate.
use super::{account_notifications::{self,NoticePlan,NoticeSnapshot},account_store, smgw_text::SmgwText};
use crate::platform::db::{insert_app_event_for_conn, AppEventEntry, SmsMessage};
use rusqlite::{params, Connection, Transaction};
use serde::Serialize;
use std::fmt;

const STORAGE: &str = "app_account_sms_storage_failed";
const SCOPE: &str = "app_account_sms_scope_invalid";

pub fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    let tx=Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
    let conn=&*tx;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_sms_projection(
        inbox_id INTEGER PRIMARY KEY,line_id TEXT NOT NULL,revision INTEGER NOT NULL,
        sms_id INTEGER NOT NULL UNIQUE,sent_at_millis INTEGER,
        timestamp_source TEXT NOT NULL CHECK(timestamp_source IN ('sender','received')),
        deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1)),
        envelope_retained INTEGER NOT NULL DEFAULT 1 CHECK(envelope_retained IN (0,1)),
        notification_plan_state TEXT NOT NULL DEFAULT 'legacy' CHECK(notification_plan_state IN ('legacy','queued','skipped')));
        CREATE INDEX IF NOT EXISTS idx_app_voice_sms_line ON app_voice_sms_projection(line_id,sms_id);")
        .map_err(|_| STORAGE)?;
    let mut query=conn.prepare("PRAGMA table_info(app_voice_sms_projection)").map_err(|_|STORAGE)?;
    let columns=query.query_map([],|row|row.get::<_,String>(1)).map_err(|_|STORAGE)?
        .collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;
    drop(query);
    if !columns.iter().any(|column|column=="envelope_retained") {
        conn.execute("ALTER TABLE app_voice_sms_projection ADD COLUMN envelope_retained INTEGER NOT NULL DEFAULT 1 CHECK(envelope_retained IN (0,1))",[]).map_err(|_|STORAGE)?;
    }
    if !columns.iter().any(|column|column=="notification_plan_state") {
        conn.execute("ALTER TABLE app_voice_sms_projection ADD COLUMN notification_plan_state TEXT NOT NULL DEFAULT 'legacy' CHECK(notification_plan_state IN ('legacy','queued','skipped'))",[]).map_err(|_|STORAGE)?;
    }
    // Explicit old account deletion may have already detached a surviving SMS.
    conn.execute("UPDATE app_voice_sms_projection SET envelope_retained=0 WHERE NOT EXISTS
        (SELECT 1 FROM app_voice_inbox i WHERE i.id=app_voice_sms_projection.inbox_id AND i.line_id=app_voice_sms_projection.line_id)",[]).map_err(|_|STORAGE)?;
    conn.execute_batch("DROP TRIGGER IF EXISTS app_voice_sms_deleted;
        CREATE TRIGGER app_voice_sms_deleted AFTER DELETE ON sms_messages
        WHEN OLD.transport='app_voice' BEGIN
            UPDATE app_voice_inbox SET body=X'',sender=NULL,opaque_time=NULL
                WHERE line_id=OLD.line_id AND id IN
                    (SELECT inbox_id FROM app_voice_sms_projection WHERE sms_id=OLD.id AND line_id=OLD.line_id);
            UPDATE app_voice_sms_projection SET deleted=1,sent_at_millis=NULL
                WHERE sms_id=OLD.id AND line_id=OLD.line_id;
            DELETE FROM app_voice_sms_projection WHERE sms_id=OLD.id AND line_id=OLD.line_id AND envelope_retained=0;
            UPDATE app_voice_notification_jobs SET state='cancelled',claim=NULL,title='',body=''
                WHERE sms_id=OLD.id AND line_id=OLD.line_id AND state IN ('pending','sending');
        END;
        DELETE FROM app_voice_sms_projection WHERE envelope_retained=0 AND deleted=1;")
        .map_err(|_| STORAGE)?;
    tx.commit().map_err(|_|STORAGE)
}

/// Called only during the same transaction which inserted the matched raw
/// inbox observation. No standalone insert_sms call, second commit or public
/// submit API may manufacture a projection ahead of the original receipt.
pub(super) fn promote_new(tx: &Transaction<'_>, inbox_id: i64, line: &str, revision: u64,
    message: &SmgwText<'_>, received_at: &str, notices:Option<&dyn NoticeSnapshot>) -> Result<AppEventEntry, &'static str> {
    if !account_store::valid_line_id(line) || revision == 0 || revision > i64::MAX as u64 { return Err(SCOPE); }
    let account=account_store::metadata(tx,line)?.ok_or(SCOPE)?;
    if account.revision!=revision { return Err(SCOPE); }
    let matched: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_inbox WHERE id=?1 AND line_id=?2 AND revision=?3
        AND receiver_kind=?4 AND body=?5 AND sender=?6 AND opaque_time IS ?7)",
        params![inbox_id,line,revision as i64,super::smgw_text::SMGW_RECEIVER,message.text().as_bytes(),message.sender().as_bytes(),message.raw_time()],
        |row|row.get(0)).map_err(|_| STORAGE)?;
    if !matched { return Err(SCOPE); }
    let timestamp = message.timestamp(received_at).ok_or(SCOPE)?;
    tx.execute("INSERT INTO sms_messages(direction,phone_number,content,timestamp,status,pdu,transport,line_id)
        VALUES('incoming',?1,?2,?3,'received',NULL,'app_voice',?4)",
        params![message.sender(),message.text(),timestamp,line]).map_err(|_| STORAGE)?;
    let sms_id = tx.last_insert_rowid();
    let snapshot=WipingSms(SmsMessage{id:sms_id,direction:"incoming".into(),phone_number:message.sender().into(),
        content:message.text().into(),timestamp:timestamp.clone(),status:"received".into(),pdu:None,transport:super::PROVIDER_ID.into(),line_id:Some(line.into())});
    // The configuration snapshot was captured OUTSIDE the SQLite mutex. Its
    // planner is pure: no config reload, DB, hardware, socket or notification IO.
    let plan=match notices{Some(notices)=>notices.plan(&snapshot.0)?,None=>NoticePlan::bounded(Vec::new())?};
    let notice_state=account_notifications::enqueue(tx,&snapshot.0,plan)?;
    tx.execute("INSERT INTO app_voice_sms_projection(inbox_id,line_id,revision,sms_id,sent_at_millis,timestamp_source,notification_plan_state)
        VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![inbox_id,line,revision as i64,sms_id,message.sent_at_millis(),message.timestamp_source(),notice_state]).map_err(|_| STORAGE)?;
    let payload = serde_json::json!({"sms_id":sms_id,"direction":"incoming","status":"received",
        "classification":"smgw_text","timestamp_source":message.timestamp_source(),"sent_time_known":message.sent_at_millis().is_some()}).to_string();
    insert_app_event_for_conn(tx,"sms.received",Some(line),Some(super::PROVIDER_ID),&payload,received_at).map_err(|_| STORAGE)
}

struct WipingSms(SmsMessage);
impl Drop for WipingSms { fn drop(&mut self) {use zeroize::Zeroize;self.0.content.zeroize();self.0.phone_number.zeroize();} }

/// Authorized read surface contains only classified messages, never arbitrary
/// raw inbox or imported deployment/credential fields. Time source is explicit.
#[derive(Serialize)]
pub struct AccountSmsMessage {
    #[serde(flatten)]
    pub message: SmsMessage,
    pub classification: &'static str,
    pub timestamp_source: &'static str,
    pub sent_at_unix_millis: Option<i64>,
}
impl fmt::Debug for AccountSmsMessage {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("AccountSmsMessage(<redacted>)") }
}
pub(super) fn list(conn:&Connection,line:&str,limit:usize)->Result<Vec<AccountSmsMessage>, &'static str> {
    if !account_store::valid_line_id(line) { return Err(SCOPE); }
    let mut query=conn.prepare("SELECT s.id,s.direction,s.phone_number,s.content,s.timestamp,s.status,s.pdu,s.transport,s.line_id,
        p.timestamp_source,p.sent_at_millis FROM app_voice_sms_projection p JOIN sms_messages s ON s.id=p.sms_id
        AND s.line_id=p.line_id AND s.transport='app_voice' WHERE p.line_id=?1 AND p.deleted=0 ORDER BY s.id DESC LIMIT ?2")
        .map_err(|_|STORAGE)?;
    let rows=query.query_map(params![line,limit.clamp(1,128) as i64],|row|Ok((SmsMessage {
        id:row.get(0)?,direction:row.get(1)?,phone_number:row.get(2)?,content:row.get(3)?,timestamp:row.get(4)?,
        status:row.get(5)?,pdu:row.get(6)?,transport:row.get(7)?,line_id:row.get(8)?,
    },row.get::<_,String>(9)?,row.get::<_,Option<i64>>(10)?))).map_err(|_|STORAGE)?;
    let mut output=Vec::new();
    for row in rows {
        let (message,source,sent_at_unix_millis)=row.map_err(|_|STORAGE)?;
        let timestamp_source=match (source.as_str(),sent_at_unix_millis) {
            ("sender",Some(_))=>"sender",("received",None)=>"received",_=>return Err(STORAGE),
        };
        if message.direction!="incoming" || message.pdu.is_some() { return Err(STORAGE); }
        output.push(AccountSmsMessage {message,classification:"smgw_text",timestamp_source,sent_at_unix_millis});
    }
    Ok(output)
}

#[cfg(test)]
#[path="account_sms_tests.rs"]
mod tests;
