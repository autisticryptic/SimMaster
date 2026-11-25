//! Durable ORIGINAL incoming Group.Update observations; no relation WRITE/ACK
//! reconciliation, directory granting, SDK readiness or complete roster claim.
use super::{account_runs::{self,RunToken},account_store,group_event_delivery::{IncomingGroupEvent,GroupEventNotice,GroupEventTicket},im_inbox::ImReceiptGate,native_rpc_budget::NativeRpcReservation,relation_journal::RelationConfiguration};
use crate::platform::db::{Database,insert_app_event_for_conn};
use rusqlite::{params,Connection,OptionalExtension,Transaction,TransactionBehavior};
use serde::Serialize;
use std::{fmt,sync::Arc,time::Instant};
const STORAGE:&str="app_account_relation_event_storage_failed";
const SCOPE:&str="app_account_relation_event_scope_invalid";
const FULL:&str="app_account_relation_event_full";
fn digest(parts:&[&[u8]])->[u8;32]{let mut h=ring::digest::Context::new(&ring::digest::SHA256);for p in parts{h.update(&(p.len()as u64).to_be_bytes());h.update(p);}let mut result=[0;32];result.copy_from_slice(h.finish().as_ref());result}
pub(super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{
    c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_relation_events(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(length(identity_key)=32),receipt_epoch TEXT NOT NULL CHECK(length(receipt_epoch)=32),target_hash BLOB NOT NULL CHECK(length(target_hash)=32),
        target_kind TEXT NOT NULL CHECK(target_kind IN('buddy','group')),base_time INTEGER NOT NULL,update_time INTEGER NOT NULL,
        fingerprint BLOB NOT NULL CHECK(length(fingerprint)=32),body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=65536),
        updates INTEGER NOT NULL CHECK(updates BETWEEN 0 AND 128),removals INTEGER NOT NULL CHECK(removals BETWEEN 0 AND 128),
        continuity TEXT NOT NULL CHECK(continuity IN('unseeded','partial_chain','gap','stale')),created_at TEXT NOT NULL,
        UNIQUE(line_id,revision,identity_key,receipt_epoch,target_hash,update_time));
        CREATE INDEX IF NOT EXISTS idx_relation_events_line ON app_voice_relation_events(line_id,id);
        CREATE TABLE IF NOT EXISTS app_voice_relation_event_heads(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL,identity_key BLOB NOT NULL,receipt_epoch TEXT NOT NULL,target_hash BLOB NOT NULL,
        observed_time INTEGER NOT NULL,target_time INTEGER NOT NULL,gap INTEGER NOT NULL CHECK(gap IN(0,1)),
        PRIMARY KEY(line_id,revision,identity_key,receipt_epoch,target_hash));")
        .map_err(|_|STORAGE)
}
pub(super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(c,line)?.is_some(){return Err(SCOPE);}
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_relation_events')",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if exists{c.execute("DELETE FROM app_voice_relation_events WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;c.execute("DELETE FROM app_voice_relation_event_heads WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())
}
pub(super) struct CommittedGroupEvent{notice:Arc<GroupEventNotice>,run:Arc<RunToken>,gate:Arc<ImReceiptGate>,id:i64,database:Arc<Database>,configuration:Arc<RelationConfiguration>}
impl fmt::Debug for CommittedGroupEvent{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommittedGroupEvent(<redacted>)")}}
impl CommittedGroupEvent{
    pub(super) fn authorizes(&self,ticket:&GroupEventTicket,now:Instant)->bool{
        let binding=self.notice.binding().im();self.id>0&&self.gate.live()&&self.run.line_id()==binding.owner().line_id()&&ticket.matches(binding.owner(),&binding.scope())&&Arc::ptr_eq(&self.notice,ticket.notice())&&ticket.fingerprint()==self.notice.fingerprint()&&now<ticket.deadline()&&self.configuration_current().is_ok()
            &&self.gate.live()&&ticket.matches(binding.owner(),&binding.scope())&&Instant::now()<ticket.deadline()
    }
    // Called before polling transport, never from inside the nonblocking IO
    // lock. SQL history alone cannot replace the original run/configuration.
    fn configuration_current(&self)->Result<(),&'static str>{self.database.with_conn(|c|{
        self.configuration.require_current(c,self.run.line_id(),self.run.revision(),&self.notice.binding().im().scope())?;
        if !account_runs::runtime_matches(c,&self.run)?{return Err(SCOPE);}account_runs::require_live(c,self.run.line_id(),self.run.revision(),self.run.nonce())
    })}
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) fn captured_copy(&self)->Self{Self{notice:self.notice.clone(),run:self.run.clone(),gate:self.gate.clone(),id:self.id,database:self.database.clone(),configuration:self.configuration.clone()}}
}
fn validate(c:&Connection,run:&RunToken,delivery:&IncomingGroupEvent,gate:&ImReceiptGate,configuration:&RelationConfiguration)->Result<(),&'static str>{
    if !gate.live()||Instant::now()>=delivery.ticket().deadline()||!delivery.matches(delivery.owner(),&delivery.scope())||delivery.owner().line_id()!=run.line_id()||account_store::metadata(c,run.line_id())?.is_none_or(|r|r.revision!=run.revision())||!account_runs::runtime_matches(c,run)?{return Err(SCOPE);}
    configuration.require_current(c,run.line_id(),run.revision(),&delivery.scope())?;
    account_runs::require_live(c,run.line_id(),run.revision(),run.nonce())?;
    if !gate.live()||Instant::now()>=delivery.ticket().deadline()||!delivery.matches(delivery.owner(),&delivery.scope()){return Err(SCOPE);}Ok(())
}
pub(super) fn commit(db:&Arc<Database>,run:Arc<RunToken>,delivery:&IncomingGroupEvent,gate:Arc<ImReceiptGate>,configuration:&Arc<RelationConfiguration>)->Result<CommittedGroupEvent,&'static str>{
    if !gate.live()||Instant::now()>=delivery.ticket().deadline(){return Err(SCOPE);}
    let notice=delivery.notice();let binding=notice.binding().im();
    let target_hash=digest(&[b"original-relation-event-target-v1",binding.identity(),notice.identifier()]);
    // Retained notice accounting predates parsing and SQL. Do not hold original
    // nonblocking IO gates over SQLite; all later reply polls recheck the brand.
    let data=notice.decoded().map_err(|_|SCOPE)?;let updates=data.relations.iter().filter(|r|r.flags!=20).count();let removals=data.relations.len()-updates;
    let _scratch:Arc<NativeRpcReservation>=delivery.scope().reserve_native_rpc(1024).map_err(|_|FULL)?;
    let(id,event)=db.with_conn(|c|{
        let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;validate(&tx,&run,delivery,&gate,configuration)?;
        let existing:Option<(i64,Option<Vec<u8>>)>=tx.query_row("SELECT id,CASE WHEN typeof(fingerprint)='blob' AND length(fingerprint)=32 THEN fingerprint ELSE NULL END FROM app_voice_relation_events WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND receipt_epoch=?4 AND target_hash=?5 AND update_time=?6",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),binding.receipt_epoch(),target_hash.as_slice(),notice.update_time()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
        if let Some((id,old))=existing{let old=old.ok_or(STORAGE)?;if id<=0{return Err(STORAGE);}if old.as_slice()!=notice.fingerprint().as_slice(){return Err("app_account_relation_event_conflict");}validate(&tx,&run,delivery,&gate,configuration)?;tx.commit().map_err(|_|STORAGE)?;return Ok((id,None));}
        let(local,total,bytes):(i64,i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),COALESCE(SUM(length(body)),0) FROM app_voice_relation_events",[run.line_id()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|STORAGE)?;
        if local>=1024||total>=4096||bytes.checked_add(notice.body().len()as i64).is_none_or(|n|n>16*1024*1024){return Err(FULL);}
        let head:Option<(i64,i64,i64)>=tx.query_row("SELECT observed_time,target_time,gap FROM app_voice_relation_event_heads WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND receipt_epoch=?4 AND target_hash=?5",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),binding.receipt_epoch(),target_hash.as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_|STORAGE)?;
        if head.is_some_and(|(current,target,gap)|target<current||!matches!(gap,0|1)){return Err(STORAGE);}
        // A first PARTIAL event is unseeded, never a complete roster. A gap does
        // not advance the observed-chain position. Stored newer DATA, counts or
        // READ history cannot clear it or resolve any unknown outbound WRITE.
        let(continuity,current,target,gap)=match head{
            None=>("unseeded",notice.update_time(),notice.update_time(),0),
            Some((current,target,gap))if notice.update_time()<=current=>("stale",current,target,gap),
            Some((current,target,0))if notice.base_time()==current=>("partial_chain",notice.update_time(),target.max(notice.update_time()),0),
            Some((current,target,_))=>("gap",current,target.max(notice.update_time()),1),
        };
        let at=chrono::Utc::now().to_rfc3339();tx.execute("INSERT INTO app_voice_relation_events(line_id,revision,identity_key,receipt_epoch,target_hash,target_kind,base_time,update_time,fingerprint,body,updates,removals,continuity,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),binding.receipt_epoch(),target_hash.as_slice(),notice.target().name(),notice.base_time(),notice.update_time(),notice.fingerprint().as_slice(),notice.body(),updates as i64,removals as i64,continuity,at]).map_err(|_|STORAGE)?;
        let id=tx.last_insert_rowid();tx.execute("INSERT INTO app_voice_relation_event_heads(line_id,revision,identity_key,receipt_epoch,target_hash,observed_time,target_time,gap) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(line_id,revision,identity_key,receipt_epoch,target_hash) DO UPDATE SET observed_time=excluded.observed_time,target_time=excluded.target_time,gap=excluded.gap",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),binding.receipt_epoch(),target_hash.as_slice(),current,target,gap]).map_err(|_|STORAGE)?;
        let payload=serde_json::json!({"relation_event_id":id,"source":"original_group_update","target_kind":notice.target().name(),"updates":updates,"removals":removals,"base_time":notice.base_time().to_string(),"update_time":notice.update_time().to_string(),"continuity":continuity,"full_roster_verified":false}).to_string();
        let event=insert_app_event_for_conn(&tx,"app_voice.relation_event_observed",Some(run.line_id()),Some("app_voice"),&payload,&at).map_err(|_|STORAGE)?;
        validate(&tx,&run,delivery,&gate,configuration)?;tx.commit().map_err(|_|STORAGE)?;Ok((id,Some(event)))
    })?;
    if let Some(event)=event{db.publish_committed_app_event(event);}
    Ok(CommittedGroupEvent{notice:notice.clone(),run,gate,id,database:db.clone(),configuration:configuration.clone()})
}
#[derive(Serialize)]
pub struct GroupEventView{pub id:i64,pub target_kind:String,pub updates:usize,pub removals:usize,pub base_time:String,pub update_time:String,pub continuity:String,pub full_roster_verified:bool,pub received_at:String}
impl fmt::Debug for GroupEventView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("GroupEventView(<redacted>)")}}
fn ordered_times(view:&GroupEventView)->bool{match(view.base_time.parse::<i64>(),view.update_time.parse::<i64>()){(Ok(base),Ok(update))=>update>base,_=>false}}
pub(super) fn history(c:&Connection,line:&str,limit:usize)->Result<Vec<GroupEventView>,&'static str>{
    if !(1..=128).contains(&limit)||account_store::metadata(c,line)?.is_none(){return Err(SCOPE);}
    // CASE rejects corrupt public projections before SQLite copies unbounded
    // text into Rust. Do not silently skip a corrupt row as an empty history.
    let mut s=c.prepare("SELECT id,CASE WHEN typeof(target_kind)='text' AND target_kind IN('buddy','group') THEN target_kind ELSE NULL END,updates,removals,base_time,update_time,CASE WHEN typeof(continuity)='text' AND continuity IN('unseeded','partial_chain','gap','stale') THEN continuity ELSE NULL END,CASE WHEN typeof(created_at)='text' AND length(CAST(created_at AS BLOB)) BETWEEN 1 AND 64 THEN created_at ELSE NULL END FROM app_voice_relation_events WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=s.query_map(params![line,limit as i64],|r|Ok(GroupEventView{id:r.get(0)?,target_kind:r.get(1)?,updates:r.get(2)?,removals:r.get(3)?,base_time:r.get::<_,i64>(4)?.to_string(),update_time:r.get::<_,i64>(5)?.to_string(),continuity:r.get(6)?,full_roster_verified:false,received_at:r.get(7)?})).map_err(|_|STORAGE)?;
    rows.map(|r|{let v=r.map_err(|_|STORAGE)?;if v.id<=0||!ordered_times(&v)||v.updates.checked_add(v.removals).is_none_or(|n|n>128)||!matches!(v.target_kind.as_str(),"buddy"|"group")||!matches!(v.continuity.as_str(),"unseeded"|"partial_chain"|"gap"|"stale")||v.received_at.len()>64||chrono::DateTime::parse_from_rfc3339(&v.received_at).is_err(){return Err(STORAGE);}Ok(v)}).collect()
}
