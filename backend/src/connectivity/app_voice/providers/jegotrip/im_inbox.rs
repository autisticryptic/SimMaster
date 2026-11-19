//! Original IM persistence, separate from SMS and call history/projections.
//! A receipt authorizes only its original IM reply, never SDK or media readiness.
//! Raw content/attachments remain private BLOBs; no attachment path is written.
use super::{account_runs::{self,RunToken},account_store,im_delivery::{ImBinding,ImKey,ImReplyTicket,IncomingImDelivery},im_text::ImText,
    rpc_pending::RpcConnectionScope,session::AttemptOwner};
use crate::platform::db::{Database,insert_app_event_for_conn};
use rusqlite::{params,Connection,OptionalExtension,Transaction,TransactionBehavior};
use serde::Serialize;
use std::{fmt,sync::{Arc,atomic::{AtomicBool,Ordering}},time::Instant};
use zeroize::Zeroizing;
const STORAGE:&str="app_account_im_storage_failed";
const SCOPE:&str="app_account_im_scope_invalid";
const INVALID:&str="app_account_im_message_invalid";
const FULL:&str="app_account_im_inbox_full";
const MAX_PER_ACCOUNT:i64=1024;
const MAX_TOTAL:i64=4096;
const MAX_TOTAL_BYTES:i64=16*1024*1024;
#[path="im_associations.rs"]
mod associations;
pub(super) use associations::{AssociationAdmission,ImPendingAssociation};
pub(super) use associations::{retire_run_associations,delete_account};
#[path="im_cursor.rs"]
mod cursor;
pub(super) use cursor::{CursorSession,GapPlan,GapCommitted,OnlinePosition};
#[path="im_status_inbox.rs"]
mod statuses;
pub(super) use statuses::CommittedImStatus;
#[path="im_received_journal.rs"]
mod receipts;
pub(super) use receipts::{ReceivedWritePermit,ReceivedWait,ReceivedAdmission,ReceivedReceiptView,begin_received,metadata as received_receipts};
pub(super) use receipts::{ReadStatusPrepared,begin_read_status};
#[path="im_user_read_journal.rs"]
mod user_reads;
pub(super) use user_reads::{UserReadAction,UserReadWait,UserReadPermit,UserReadPermitWait,UserReadView,BoundUserRead,metadata as user_read_history};
#[path="im_outbox.rs"]
mod outbox;
pub use outbox::{ImSendMessage,ImSendView,ImSendFailure};
pub(super) use outbox::{ImSendAction,ImSendInput,ImSendPermit,ImSendWait,ImSendPermitWait,PreparedImSend,history as im_send_history};
#[path="im_read_status.rs"]
mod read_status;
pub(super) use read_status::{ReadStatusBudget,ReadStatusTargets,ReadStatusAction,ReadStatusWait,ReadStatusTargetView,ReadStatusView,history as read_status_history};
pub(super) use read_status::{retire_targets as retire_read_status_targets,delete_account as delete_read_status_account};
#[path="im_read_reconciliation.rs"]
mod reconciliation;
pub(super) use reconciliation::{ImReadObservation,ImReconciliationAction,ImReconciliationWait};

pub(super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_messages(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),
        box_kind TEXT NOT NULL CHECK(box_kind IN ('p2p','org')),message_index INTEGER NOT NULL,
        fingerprint BLOB NOT NULL CHECK(typeof(fingerprint)='blob' AND length(fingerprint)=32),
        original_time INTEGER NOT NULL,body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=65536),
        text TEXT NOT NULL CHECK(typeof(text)='text' AND length(CAST(text AS BLOB))<=65536),
        sender BLOB NOT NULL CHECK(typeof(sender)='blob' AND length(sender)<=65536),
        attachment_count INTEGER NOT NULL CHECK(attachment_count BETWEEN 0 AND 128),
        deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1)),created_at TEXT NOT NULL,
        UNIQUE(line_id,revision,identity_key,box_id,message_index));
        CREATE INDEX IF NOT EXISTS idx_app_voice_im_messages_line ON app_voice_im_messages(line_id,id);")
        .map_err(|_|STORAGE)?;
    associations::initialize_schema(conn)?;
    cursor::initialize_schema(conn)?;
    statuses::initialize_schema(conn)?;
    receipts::initialize_schema(conn)?;
    user_reads::initialize_schema(conn)?;
    outbox::initialize_schema(conn)?;
    read_status::initialize_schema(conn)
}

pub(super) struct ImReceiptGate{live:AtomicBool,changed:tokio::sync::Notify}
impl ImReceiptGate{
    pub(super) fn fresh()->Arc<Self>{Arc::new(Self{live:AtomicBool::new(true),changed:tokio::sync::Notify::new()})}
    pub(super) fn retire(&self){self.live.store(false,Ordering::SeqCst);self.changed.notify_waiters();}
    pub(super) fn live(&self)->bool{self.live.load(Ordering::SeqCst)}
    pub(super) async fn retired(&self){loop{let wake=self.changed.notified();tokio::pin!(wake);wake.as_mut().enable();if !self.live(){return;}wake.await;}}
}
impl fmt::Debug for ImReceiptGate{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImReceiptGate(<redacted>)")}}

pub(super) struct ImInboxScope{run:Arc<RunToken>,binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>}
impl fmt::Debug for ImInboxScope{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImInboxScope(<redacted>)")}}
impl ImInboxScope{
    pub(super) fn original(run:Arc<RunToken>,binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>)->Result<Self,&'static str>{
        if binding.owner().line_id()!=run.line_id()||!binding.matches(binding.owner(),&binding.scope())||!gate.live(){return Err(SCOPE);}
        Ok(Self{run,binding,gate})
    }
    fn validate(&self,conn:&Connection)->Result<(),&'static str>{
        if account_store::metadata(conn,self.run.line_id())?.is_none_or(|row|row.revision!=self.run.revision())
            ||!account_runs::runtime_matches(conn,&self.run)?{return Err(SCOPE);}
        account_runs::require_live(conn,self.run.line_id(),self.run.revision(),self.run.nonce())
    }
    fn matches(&self,delivery:&IncomingImDelivery)->bool{
        self.gate.live()&&delivery.matches(self.binding.owner(),&self.binding.scope())
            &&delivery.notice().binding().identity()==self.binding.identity()
            &&delivery.notice().binding().receipt_epoch()==self.binding.receipt_epoch()
    }
    fn permit(&self,key:&Arc<ImKey>,fingerprint:[u8;32],id:i64)->CommittedIm{
        CommittedIm{owner:self.binding.owner().clone(),scope:self.binding.scope(),identity:*self.binding.identity(),receipt_epoch:Zeroizing::new(self.binding.receipt_epoch().to_owned()),key:key.clone(),fingerprint,id,gate:self.gate.clone()}
    }
    pub(super) fn commit(&self,db:&Database,delivery:&IncomingImDelivery)->Result<Option<CommittedIm>,&'static str>{
        if !self.matches(delivery)||delivery.notice().notification_id().is_some(){return Err(SCOPE);}
        let notice=delivery.notice();let Some(message)=notice.message().map_err(|_|INVALID)? else{return Err(INVALID);};
        let Some(text)=ImText::decode(&message).map_err(|_|INVALID)? else{return Ok(None);};
        let key=notice.key();let raw=notice.raw().ok_or(INVALID)?;let fingerprint=*notice.message_fingerprint().ok_or(INVALID)?;
        let size=raw.len().checked_add(text.text().len()).and_then(|n|n.checked_add(message.sender_uid.len())).ok_or(INVALID)?;
        if raw.len()>65536||text.text().len()>65536||message.sender_uid.len()>65536{return Err(INVALID);}
        // Never hold the nonblocking original scope IO gate across SQLite.
        // Historical data may commit during revoke, but the permit below and
        // owning actor still require the same live scope/deadline to reply.
        let(id,event)=db.with_conn(|conn|{
            let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.validate(&tx)?;
            let existing:Option<(i64,Vec<u8>)>=tx.query_row("SELECT id,fingerprint FROM app_voice_im_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND message_index=?5",
                params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),key.box_id().bytes(),key.index()],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|STORAGE)?;
            if let Some((id,saved))=existing{
                if saved.as_slice()!=fingerprint.as_slice(){return Err("app_account_im_message_conflict");}
                tx.commit().map_err(|_|STORAGE)?;return Ok((id,None));
            }
            let(local,total,bytes):(i64,i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),COALESCE(SUM(length(body)+length(CAST(text AS BLOB))+length(sender)),0) FROM app_voice_im_messages",[self.run.line_id()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).map_err(|_|STORAGE)?;
            if local>=MAX_PER_ACCOUNT||total>=MAX_TOTAL||bytes.checked_add(size as i64).is_none_or(|n|n>MAX_TOTAL_BYTES){return Err(FULL);}
            let kind=match key.box_id().kind(){super::im_data::BoxKind::P2P=>"p2p",super::im_data::BoxKind::Org=>"org"};let now=chrono::Utc::now().to_rfc3339();
            tx.execute("INSERT INTO app_voice_im_messages(line_id,revision,identity_key,box_id,box_kind,message_index,fingerprint,original_time,body,text,sender,attachment_count,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),key.box_id().bytes(),kind,key.index(),fingerprint.as_slice(),message.time,raw,text.text(),message.sender_uid,message.attachments.len()as i64,now]).map_err(|_|STORAGE)?;
            let id=tx.last_insert_rowid();let payload=serde_json::json!({"im_id":id,"kind":"text","transport":"app_voice_im"}).to_string();
            let event=insert_app_event_for_conn(&tx,"im.received",Some(self.run.line_id()),Some("app_voice_im"),&payload,&now).map_err(|_|STORAGE)?;
            tx.commit().map_err(|_|STORAGE)?;Ok((id,Some(event)))
        })?;
        if let Some(event)=event{db.publish_committed_app_event(event);}
        Ok(Some(self.permit(key,fingerprint,id)))
    }
    /// A CURRENT original notification may confirm an already durably consumed
    /// message in the SAME native UID/selector/config context. A deleted row
    /// stays a private receipt tombstone: no content/event is recreated.
    pub(super) fn lookup(&self,db:&Database,delivery:&IncomingImDelivery)->Result<Option<CommittedIm>,&'static str>{
        if !self.matches(delivery)||delivery.notice().notification_id().is_none(){return Err(SCOPE);}
        let key=delivery.notice().key();
        let row:Option<(i64,Vec<u8>)>=db.with_conn(|conn|{
            let tx=Transaction::new_unchecked(conn,TransactionBehavior::Deferred).map_err(|_|STORAGE)?;self.validate(&tx)?;
            let row=tx.query_row("SELECT id,fingerprint FROM app_voice_im_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND message_index=?5",
                params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),key.box_id().bytes(),key.index()],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|STORAGE)?;
            tx.commit().map_err(|_|STORAGE)?;Ok(row)
        })?;
        row.map(|(id,fingerprint)|{let fingerprint:[u8;32]=fingerprint.try_into().map_err(|_|STORAGE)?;Ok(self.permit(key,fingerprint,id))}).transpose()
    }
}

pub struct CommittedIm{owner:AttemptOwner,scope:RpcConnectionScope,identity:[u8;32],receipt_epoch:Zeroizing<String>,key:Arc<ImKey>,fingerprint:[u8;32],id:i64,gate:Arc<ImReceiptGate>}
impl fmt::Debug for CommittedIm{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommittedIm(<redacted>)")}}
impl CommittedIm{
    pub(super) fn authorizes(&self,ticket:&ImReplyTicket,now:Instant)->bool{
        self.gate.live()&&self.id>0&&ticket.matches(&self.owner,&self.scope)&&ticket.binding().identity()==&self.identity&&ticket.binding().receipt_epoch()==self.receipt_epoch.as_str()&&ticket.key().digest()==self.key.digest()
            &&(!ticket.is_message()||ticket.fingerprint()==&self.fingerprint)&&now<ticket.completion_deadline()
    }
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) fn key_digest(&self)->&[u8;32]{self.key.digest()}
    pub(super) fn captured_copy(&self)->Self{Self{owner:self.owner.clone(),scope:self.scope.clone(),identity:self.identity,receipt_epoch:self.receipt_epoch.clone(),key:self.key.clone(),fingerprint:self.fingerprint,id:self.id,gate:self.gate.clone()}}
}
#[derive(Serialize)]
pub struct ImMessageView{pub id:i64,pub box_kind:&'static str,pub message_index:String,pub text:String,pub attachment_count:u32,pub received_at:String}
impl fmt::Debug for ImMessageView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImMessageView(<redacted>)")}}
pub(super) fn list(conn:&Connection,line:&str,limit:usize)->Result<Vec<ImMessageView>,&'static str>{
    if !(1..=128).contains(&limit)||account_store::metadata(conn,line)?.is_none(){return Err(INVALID);}
    let mut query=conn.prepare("SELECT id,box_kind,message_index,text,attachment_count,created_at FROM app_voice_im_messages WHERE line_id=?1 AND deleted=0 AND typeof(text)='text' AND length(CAST(text AS BLOB))<=65536 AND typeof(created_at)='text' AND length(CAST(created_at AS BLOB))<=64 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=query.query_map(params![line,limit as i64],|row|{let kind:String=row.get(1)?;Ok(ImMessageView{id:row.get(0)?,box_kind:if kind=="p2p"{"p2p"}else if kind=="org"{"org"}else{"unknown"},message_index:row.get::<_,i64>(2)?.to_string(),text:row.get(3)?,attachment_count:row.get(4)?,received_at:row.get(5)?})}).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
pub(super) fn delete(conn:&Connection,line:&str,id:i64)->Result<bool,&'static str>{
    if id<=0||account_store::metadata(conn,line)?.is_none(){return Err(INVALID);}
    // Keep only identity/fingerprint tombstone for legitimate peer redelivery;
    // do not expose an attachment path, restore deleted text or emit new IM.
    conn.execute("UPDATE app_voice_im_messages SET deleted=1,body=X'',text='',sender=X'',attachment_count=0 WHERE id=?1 AND line_id=?2 AND deleted=0",params![id,line]).map(|count|count==1).map_err(|_|STORAGE)
}
