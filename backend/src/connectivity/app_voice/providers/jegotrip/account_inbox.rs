//! Private durable GENERIC receiver inbox, not SMS/PDU classification or media.
//!
//! The original consumer response can be admitted only after the raw bytes and
//! redacted event commit atomically. TCP/TLS/Router ACKs and batch transport ACKs
//! are different layers. Local row ids/nonces are NOT remote message identities.
//! Unknown data is never evicted to make room, promoted as SMS or marked ready.
use super::{
    account_store, arc_protocol::IncomingProtocolMessage,
    native_events::IncomingMessage, offline_backend::ScheduledIncomingMessage,
    rpc_pending::RpcConnectionScope, runtime_backend::NativeHostingBackend,
    session::AttemptOwner, transport_actor::RuntimeError,
};
use crate::platform::db::{insert_app_event_for_conn, Database};
use ring::digest::{Context, SHA256};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use std::{fmt, sync::atomic::{AtomicBool, Ordering}};
#[cfg(test)]
use zeroize::Zeroizing;

const STORAGE: &str = "app_account_inbox_storage_failed";
const SCOPE: &str = "app_account_inbox_scope_invalid";
const FULL: &str = "app_account_inbox_full";
const INVALID: &str = "app_account_inbox_message_invalid";
const MAX_FIELD_BYTES: usize = 64 * 1024;
const MAX_PER_ACCOUNT: i64 = 128;
const MAX_TOTAL: i64 = 2048;
const MAX_TOTAL_BYTES: i64 = 16 * 1024 * 1024;

pub fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_inbox(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,
        run_nonce TEXT NOT NULL,source_kind TEXT NOT NULL CHECK(source_kind IN ('direct','batch')),
        peer_epoch INTEGER NOT NULL,sequence INTEGER NOT NULL,fingerprint TEXT NOT NULL,
        body BLOB NOT NULL,sender BLOB,opaque_time BLOB,
        reply_state TEXT NOT NULL DEFAULT 'pending' CHECK(reply_state IN ('pending','direct_submitted','batch_queued')),
        created_at TEXT NOT NULL,
        UNIQUE(line_id,revision,run_nonce,source_kind,peer_epoch,sequence));
        CREATE INDEX IF NOT EXISTS idx_app_voice_inbox_line ON app_voice_inbox(line_id,id);")
        .map_err(|_| STORAGE)
}
fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 32 && nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn message_size(message: &IncomingMessage) -> Result<i64, &'static str> {
    let mut bytes = 0usize;
    for value in [Some(message.body.as_slice()), message.from.as_deref(), message.time.as_deref()].into_iter().flatten() {
        if value.len() > MAX_FIELD_BYTES { return Err(INVALID); }
        bytes = bytes.checked_add(value.len()).ok_or(INVALID)?;
    }
    Ok(bytes as i64)
}
fn fingerprint(message: &IncomingMessage) -> String {
    let mut digest = Context::new(&SHA256);
    // Explicit presence+length makes NULL different from an empty binary field.
    // This is an exact-content dedup aid, never credential/authentication proof.
    for value in [Some(message.body.as_slice()), message.from.as_deref(), message.time.as_deref()] {
        digest.update(&[u8::from(value.is_some())]);
        let bytes = value.unwrap_or_default(); digest.update(&(bytes.len() as u64).to_be_bytes()); digest.update(bytes);
    }
    digest.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
fn require_account(conn: &Connection, line: &str, revision: u64) -> Result<(), &'static str> {
    let account = account_store::metadata(conn, line)?.ok_or(SCOPE)?;
    if account.revision != revision { return Err(SCOPE); }
    Ok(())
}

/// Constructed only by trusted account lifecycle code after matching the actual
/// backend event, original resource owner and current hosted phase. It cannot be
/// supplied or serialized by HTTP callers, and never creates/relabels a scope.
pub(super) struct InboxScope {
    line_id: String, revision: u64, nonce: String, owner: AttemptOwner, scope: RpcConnectionScope,
}
impl fmt::Debug for InboxScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("InboxScope(<redacted>)") }
}
impl InboxScope {
    pub(super) fn original(line: &str, revision: u64, nonce: &str, owner: &AttemptOwner, scope: &RpcConnectionScope)
        -> Result<Self, &'static str> {
        if !account_store::valid_line_id(line) || owner.line_id() != line || revision == 0 || revision > i64::MAX as u64
            || !valid_nonce(nonce) || !scope.is_open() { return Err(SCOPE); }
        Ok(Self { line_id: line.to_owned(), revision, nonce: nonce.to_owned(), owner: owner.clone(), scope: scope.clone() })
    }
    fn commit(&self, database: &Database, kind: &'static str, epoch: u32, sequence: i32, message: &IncomingMessage)
        -> Result<CommittedInbox, &'static str> {
        if !self.scope.is_open() || sequence <= 0 { return Err(SCOPE); }
        if !matches!(kind,"direct"|"batch") || kind=="direct" && epoch!=0 || kind=="batch" && epoch==0 { return Err(INVALID); }
        let size = message_size(message)?; let fingerprint = fingerprint(message);
        // Never hold the NONBLOCKING IO poll gate across SQLite/config IO. The
        // captured observation can be committed historically if physical close
        // races it; a closed/retired scope cannot admit the subsequent reply.
        let (id, event) = database.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
            require_account(&tx, &self.line_id, self.revision)?;
            let existing: Option<(i64, bool)> = tx.query_row("SELECT id,
                fingerprint=?7 AND body=?8 AND sender IS ?9 AND opaque_time IS ?10
                FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND run_nonce=?3
                AND source_kind=?4 AND peer_epoch=?5 AND sequence=?6",
                params![&self.line_id,self.revision as i64,&self.nonce,kind,epoch as i64,sequence,&fingerprint,
                    message.body.as_slice(),message.from.as_deref(),message.time.as_deref()],
                |row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_| STORAGE)?;
            if let Some((id, matches)) = existing {
                if !matches { return Err("app_account_inbox_duplicate_conflict"); }
                tx.commit().map_err(|_| STORAGE)?; return Ok((id,None));
            }
            let (local,total,stored): (i64,i64,i64) = tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),
                COALESCE(SUM(length(body)+COALESCE(length(sender),0)+COALESCE(length(opaque_time),0)),0) FROM app_voice_inbox",
                [&self.line_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_| STORAGE)?;
            if local >= MAX_PER_ACCOUNT || total >= MAX_TOTAL || stored.checked_add(size).map_or(true,|bytes|bytes>MAX_TOTAL_BYTES) {
                return Err(FULL);
            }
            let stamp = chrono::Utc::now().to_rfc3339();
            tx.execute("INSERT INTO app_voice_inbox(line_id,revision,run_nonce,source_kind,peer_epoch,sequence,fingerprint,body,sender,opaque_time,created_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![&self.line_id,self.revision as i64,&self.nonce,kind,epoch as i64,sequence,&fingerprint,
                    message.body.as_slice(),message.from.as_deref(),message.time.as_deref(),&stamp]).map_err(|_| STORAGE)?;
            let id = tx.last_insert_rowid();
            let payload = serde_json::json!({"inbox_id":id,"classification":"unclassified"}).to_string();
            let event = insert_app_event_for_conn(&tx, "app_voice.inbox_stored", Some(&self.line_id), Some(super::PROVIDER_ID), &payload, &stamp)
                .map_err(|_| STORAGE)?;
            tx.commit().map_err(|_| STORAGE)?;
            Ok((id,Some(event)))
        })?;
        if let Some(event) = event { database.publish_committed_app_event(event); }
        Ok(CommittedInbox { id, line_id: self.line_id.clone(), revision: self.revision, nonce: self.nonce.clone(),
            kind, epoch, sequence, fingerprint })
    }
    pub(super) fn commit_direct(&self, database: &Database, message: &IncomingProtocolMessage) -> Result<CommittedInbox, &'static str> {
        if message.owner()!=&self.owner || !message.reply_ticket().matches(&self.owner,&self.scope) { return Err(SCOPE); }
        self.commit(database,"direct",0,message.reply_ticket().request_id(),message.message())
    }
    pub(super) fn commit_batch(&self, database: &Database, message: &ScheduledIncomingMessage) -> Result<CommittedInbox, &'static str> {
        if message.owner()!=&self.owner || !message.ticket().matches(&self.owner,&self.scope) { return Err(SCOPE); }
        self.commit(database,"batch",message.ticket().peer_incarnation(),message.ticket().sequence(),message.message())
    }
    pub(super) async fn accept_direct(&self, database: &Database, backend: &NativeHostingBackend, message: &IncomingProtocolMessage,
        admission: &AtomicBool) -> Result<Completion, &'static str> {
        let committed = self.commit_direct(database,message)?;
        if !admission.load(Ordering::SeqCst) { return Ok(Completion::Retired); }
        match backend.complete_direct(&self.owner,&self.scope,message.reply_ticket(),true).await {
            Ok(receipt) if receipt.submitted_on(&self.owner,&self.scope) => {
                Ok(if committed.record_reply(database,"direct_submitted").is_ok() { Completion::Recorded }
                    else { Completion::StatusUnrecorded })
            }
            Err(RuntimeError::Backpressure) => Ok(Completion::Backpressure),
            _ => Err("app_account_inbox_reply_failed"),
        }
    }
    pub(super) async fn accept_batch(&self, database: &Database, backend: &NativeHostingBackend, message: &ScheduledIncomingMessage,
        admission: &AtomicBool) -> Result<Completion, &'static str> {
        let committed = self.commit_batch(database,message)?;
        if !admission.load(Ordering::SeqCst) { return Ok(Completion::Retired); }
        match backend.complete_batch(&self.owner,&self.scope,message.ticket(),true).await {
            // This receipt queues a result in the original scheduler. It does
            // NOT prove that the next poll wrote it or the peer received it.
            Ok(receipt) if receipt.submitted_on(&self.owner,&self.scope) => {
                Ok(if committed.record_reply(database,"batch_queued").is_ok() { Completion::Recorded }
                    else { Completion::StatusUnrecorded })
            }
            Err(RuntimeError::Backpressure) => Ok(Completion::Backpressure),
            _ => Err("app_account_inbox_reply_failed"),
        }
    }
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum Completion { Recorded, Backpressure, StatusUnrecorded, Retired }

/// Non-serializable commit proof, created ONLY after the transaction commits.
pub(super) struct CommittedInbox {
    id:i64,line_id:String,revision:u64,nonce:String,kind:&'static str,epoch:u32,sequence:i32,fingerprint:String,
}
impl fmt::Debug for CommittedInbox {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("CommittedInbox(<redacted>)") }
}
impl CommittedInbox {
    fn record_reply(&self,database:&Database,state:&'static str)->Result<(), &'static str> {
        if !matches!((self.kind,state),("direct","direct_submitted")|("batch","batch_queued")) { return Err(INVALID); }
        database.with_conn(|conn| {
            let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
            require_account(&tx,&self.line_id,self.revision)?;
            let changed=tx.execute("UPDATE app_voice_inbox SET reply_state=?1 WHERE id=?2 AND line_id=?3 AND revision=?4
                AND run_nonce=?5 AND source_kind=?6 AND peer_epoch=?7 AND sequence=?8 AND fingerprint=?9",
                params![state,self.id,&self.line_id,self.revision as i64,&self.nonce,self.kind,self.epoch as i64,self.sequence,&self.fingerprint])
                .map_err(|_|STORAGE)?;
            if changed!=1 { return Err(SCOPE); }
            tx.commit().map_err(|_|STORAGE)
        })
    }
}

#[derive(Clone,Debug,Serialize)]
pub struct InboxMetadata {
    pub inbox_id:i64,pub classification:&'static str,pub reply_state:&'static str,pub created_at:String,
}
pub(super) fn metadata(conn:&Connection,line:&str,limit:usize)->Result<Vec<InboxMetadata>, &'static str> {
    if !account_store::valid_line_id(line) { return Err(SCOPE); }
    let mut statement=conn.prepare("SELECT id,reply_state,created_at FROM app_voice_inbox WHERE line_id=?1 ORDER BY id DESC LIMIT ?2")
        .map_err(|_|STORAGE)?;
    let raw=statement.query_map(params![line,limit.clamp(1,128) as i64],|row|Ok((row.get::<_,i64>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?)))
        .map_err(|_|STORAGE)?;
    let mut output=Vec::new();
    for row in raw {
        let (id,state,stamp)=row.map_err(|_|STORAGE)?;
        let state=match state.as_str(){"pending"=>"pending","direct_submitted"=>"direct_submitted","batch_queued"=>"batch_queued",_=>return Err(STORAGE)};
        output.push(InboxMetadata{inbox_id:id,classification:"unclassified",reply_state:state,created_at:stamp});
    }
    Ok(output)
}
pub(super) fn delete_private_for_account(conn:&Connection,line:&str)->Result<(), &'static str> {
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_inbox')",[],|row|row.get(0)).map_err(|_|STORAGE)?;
    if exists { conn.execute("DELETE FROM app_voice_inbox WHERE line_id=?1",[line]).map_err(|_|STORAGE)?; }
    Ok(())
}

/// Private future classifier input. No API Serialize/Debug payload preview.
#[cfg(test)]
struct PrivateMessage { body:Zeroizing<Vec<u8>>,sender:Option<Zeroizing<Vec<u8>>>,time:Option<Zeroizing<Vec<u8>>> }
#[cfg(test)]
fn private_message(database:&Database,line:&str,id:i64)->Result<PrivateMessage, &'static str> {
    database.with_conn(|conn| conn.query_row("SELECT body,sender,opaque_time FROM app_voice_inbox WHERE id=?1 AND line_id=?2",
        params![id,line],|row|Ok(PrivateMessage{body:Zeroizing::new(row.get(0)?),sender:row.get::<_,Option<Vec<u8>>>(1)?.map(Zeroizing::new),
            time:row.get::<_,Option<Vec<u8>>>(2)?.map(Zeroizing::new)})).map_err(|_|STORAGE))
}

#[cfg(test)]
#[path="account_inbox_tests.rs"]
mod tests;
