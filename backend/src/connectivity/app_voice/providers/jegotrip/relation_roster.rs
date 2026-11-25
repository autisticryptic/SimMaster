//! Current-original local relation roster. A complete getRelations2 snapshot
//! (native IsPartialUpdate=false) is distinct from partial Group.Update/system
//! IM DATA. Explicit directory READ still owns all IO; no automatic queries,
//! relationship WRITE reconciliation, grants or guessed remote versions.
use super::{account_runs::{self,RunToken},account_store,group_directory_data::{GroupId,Relation},im_delivery::ImBinding,im_inbox::ImReceiptGate,native_rpc_budget::NativeRpcReservation,relation_journal::RelationConfiguration,rpc_im::{ImQuery,ImReplyData,OwnedImReply},rpc_stream::{CodecError,Writer,MAX_MAP_ENTRIES}};
use crate::platform::db::{Database,insert_app_event_for_conn};
use rusqlite::{params,Connection,OptionalExtension,Transaction,TransactionBehavior};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;
const STORAGE:&str="app_account_relation_roster_storage_failed";const SCOPE:&str="app_account_relation_roster_scope_invalid";const CAPACITY:&str="app_account_relation_roster_capacity";
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRelationRosterConfig{pub observe:bool}
impl fmt::Debug for NativeRelationRosterConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeRelationRosterConfig(<redacted>)")}}
impl NativeRelationRosterConfig{pub(super) fn validate(self)->Result<(),&'static str>{if self.observe{Ok(())}else{Err(SCOPE)}}}
fn hash(parts:&[&[u8]])->[u8;32]{let mut h=ring::digest::Context::new(&ring::digest::SHA256);for p in parts{h.update(&(p.len()as u64).to_be_bytes());h.update(p);}let mut out=[0;32];out.copy_from_slice(h.finish().as_ref());out}
fn key(group:Option<&[u8]>)->[u8;32]{hash(&[b"original-roster-target-v1",&[u8::from(group.is_some())],group.unwrap_or(b"")])}
fn label_hash(label:&[u8])->[u8;32]{hash(&[b"original-roster-label-v1",label])}
pub(super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_relation_rosters(
 id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),receipt_epoch TEXT NOT NULL CHECK(length(receipt_epoch)=32),target_key BLOB NOT NULL CHECK(length(target_key)=32),target_kind TEXT NOT NULL CHECK(target_kind IN('buddy','group')),
 serial INTEGER NOT NULL CHECK(serial>0),baseline_time INTEGER,observed_time INTEGER,target_time INTEGER,complete INTEGER NOT NULL CHECK(complete IN(0,1)),gap INTEGER NOT NULL CHECK(gap IN(0,1)),full_required INTEGER NOT NULL CHECK(full_required IN(0,1)),updated_at TEXT NOT NULL,
 UNIQUE(line_id,revision,identity_key,receipt_epoch,target_key));
 CREATE TABLE IF NOT EXISTS app_voice_relation_roster_members(roster_id INTEGER NOT NULL,label_key BLOB NOT NULL CHECK(length(label_key)=32),relation BLOB NOT NULL CHECK(typeof(relation)='blob' AND length(relation)<=65536),PRIMARY KEY(roster_id,label_key));") .map_err(|_|STORAGE)}
pub(super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{if account_store::metadata(c,line)?.is_some(){return Err(SCOPE);}let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_relation_rosters')",[],|r|r.get(0)).map_err(|_|STORAGE)?;if exists{c.execute("DELETE FROM app_voice_relation_roster_members WHERE roster_id IN(SELECT id FROM app_voice_relation_rosters WHERE line_id=?1)",[line]).map_err(|_|STORAGE)?;c.execute("DELETE FROM app_voice_relation_rosters WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())}
#[derive(Clone,Copy)]
struct Head{id:i64,serial:i64,base:Option<i64>,observed:Option<i64>,target:Option<i64>,complete:bool,gap:bool,full_required:bool}
fn head(c:&Connection,run:&RunToken,binding:&ImBinding,target:&[u8;32])->Result<Option<Head>,&'static str>{
 let value=c.query_row("SELECT id,serial,baseline_time,observed_time,target_time,complete,gap,full_required FROM app_voice_relation_rosters WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND receipt_epoch=?4 AND target_key=?5",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),binding.receipt_epoch(),target.as_slice()],|r|{let complete:i64=r.get(5)?;let gap:i64=r.get(6)?;let required:i64=r.get(7)?;if !matches!(complete,0|1)||!matches!(gap,0|1)||!matches!(required,0|1){return Err(rusqlite::Error::InvalidQuery);}Ok(Head{id:r.get(0)?,serial:r.get(1)?,base:r.get(2)?,observed:r.get(3)?,target:r.get(4)?,complete:complete==1,gap:gap==1,full_required:required==1})}).optional().map_err(|_|STORAGE)?;
 if value.is_some_and(|h|h.id<=0||h.serial<=0||h.complete&&(h.base.is_none()||h.observed.is_none())||matches!((h.observed,h.target),(Some(a),Some(b))if a>b)){return Err(STORAGE);}Ok(value)
}
fn require_current(c:&Connection,run:&RunToken,binding:&ImBinding,gate:&ImReceiptGate,configuration:&RelationConfiguration)->Result<(),&'static str>{
 if !binding.policy().relation_roster()||!gate.live()||!binding.matches(binding.owner(),&binding.scope())||run.line_id()!=binding.owner().line_id()||!account_runs::runtime_matches(c,run)?{return Err(SCOPE);}configuration.require_current(c,run.line_id(),run.revision(),&binding.scope())?;account_runs::require_live(c,run.line_id(),run.revision(),run.nonce())?;if !gate.live()||!binding.matches(binding.owner(),&binding.scope()){return Err(SCOPE);}Ok(())
}
fn ensure(c:&Connection,run:&RunToken,binding:&ImBinding,group:Option<&[u8]>)->Result<Head,&'static str>{
 let target=key(group);if let Some(h)=head(c,run,binding,&target)?{return Ok(h);}let(local,total):(i64,i64)=c.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_relation_rosters",[run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;if local>=128||total>=4096{return Err(CAPACITY);}
 c.execute("INSERT INTO app_voice_relation_rosters(line_id,revision,identity_key,receipt_epoch,target_key,target_kind,serial,complete,gap,full_required,updated_at) VALUES(?1,?2,?3,?4,?5,?6,1,0,0,0,?7)",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),binding.receipt_epoch(),target.as_slice(),if group.is_some(){"group"}else{"buddy"},chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;head(c,run,binding,&target)?.ok_or(STORAGE)
}
struct Member{key:[u8;32],bytes:Zeroizing<Vec<u8>>,remove:bool}
fn measure(r:&Relation<'_>)->Result<usize,CodecError>{if r.label.is_empty()||r.label.len()>65536||r.first.len()>65536||r.second.len()>65536||r.properties.len()>MAX_MAP_ENTRIES{return Err(CodecError::InvalidField);}r.properties.iter().enumerate().try_fold(21+r.label.len()+r.first.len()+r.second.len(),|sum,(i,p)|{if p.key.len()>65536||p.value.len()>65536||r.properties[..i].iter().any(|q|q.key==p.key){return Err(CodecError::InvalidField);}sum.checked_add(8+p.key.len()+p.value.len()).ok_or(CodecError::TooLarge)})}
struct Changes{values:Vec<Member>,_budget:Arc<NativeRpcReservation>}
impl Changes{
 fn capture(binding:&ImBinding,adds:&[Relation<'_>],updates:&[Relation<'_>],removed:&[&[u8]])->Result<Self,&'static str>{
  if adds.len()>128||updates.len()>128||removed.len()>128{return Err(CAPACITY);}let mut size=(adds.len()+updates.len()+removed.len())*std::mem::size_of::<Member>()+128;
  for(i,r)in adds.iter().chain(updates).enumerate(){if adds.iter().chain(updates).take(i).any(|q|q.label==r.label)||removed.contains(&r.label){return Err(SCOPE);}size=size.checked_add(measure(r).map_err(|_|SCOPE)?.checked_mul(2).ok_or(CAPACITY)?).ok_or(CAPACITY)?;}
  for(i,label)in removed.iter().enumerate(){if label.is_empty()||label.len()>65536||removed[..i].contains(label){return Err(SCOPE);}}
  size=size.checked_add(adds.iter().chain(updates).map(|r|r.properties.len()).max().unwrap_or(0)*std::mem::size_of::<&super::rpc_stream::RpcProperty<'_>>()).ok_or(CAPACITY)?;
  let budget=binding.scope().reserve_native_rpc(size).map_err(|_|CAPACITY)?;let mut values=Vec::with_capacity(adds.len()+updates.len()+removed.len());
  for r in adds.iter().chain(updates){let mut w=Writer::measured(measure(r).map_err(|_|SCOPE)?).map_err(|_|CAPACITY)?;w.bytes(r.label).map_err(|_|SCOPE)?;w.u32(r.flags).map_err(|_|SCOPE)?;w.bytes(r.first).map_err(|_|SCOPE)?;w.bytes(r.second).map_err(|_|SCOPE)?;w.properties(&r.properties).map_err(|_|SCOPE)?;values.push(Member{key:label_hash(r.label),bytes:Zeroizing::new(w.finish().expose_to_transport().to_vec()),remove:r.flags==20});}
  for label in removed{values.push(Member{key:label_hash(label),bytes:Zeroizing::new(Vec::new()),remove:true});}Ok(Self{values,_budget:budget})
 }
 fn apply(&self,c:&Connection,id:i64)->Result<(),&'static str>{
  // Callers hold the original event/read transaction. Keys are unique across
  // all buckets. Remove the touched old values first, including replacements,
  // so capacity reflects the final delta rather than arbitrary wire ordering.
  // Any later error rolls back these deletes together with the head/event.
  for m in &self.values{c.execute("DELETE FROM app_voice_relation_roster_members WHERE roster_id=?1 AND label_key=?2",params![id,m.key.as_slice()]).map_err(|_|STORAGE)?;}
  for m in self.values.iter().filter(|m|!m.remove){let(local,total,bytes):(i64,i64,i64)=c.query_row("SELECT COALESCE(SUM(roster_id=?1),0),COUNT(*),COALESCE(SUM(length(relation)),0) FROM app_voice_relation_roster_members",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|STORAGE)?;
    if local>=1024||total>=4096||bytes.checked_add(m.bytes.len()as i64).is_none_or(|n|n>16*1024*1024){return Err(CAPACITY);}c.execute("INSERT INTO app_voice_relation_roster_members(roster_id,label_key,relation) VALUES(?1,?2,?3)",params![id,m.key.as_slice(),m.bytes.as_slice()]).map_err(|_|STORAGE)?;}
  Ok(())
 }
}
/// Binds a user-authorized current READ to the observed serial BEFORE its
/// result. A later event/snapshot must not be overwritten by this older read.
pub(super) struct RosterRead{database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,configuration:Arc<RelationConfiguration>,query:Arc<ImQuery>,target:[u8;32],serial:Option<i64>,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for RosterRead{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRosterRead(<redacted>)")}}
impl RosterRead{
 pub(super) fn capture(database:Arc<Database>,c:&Connection,run:Arc<RunToken>,binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,configuration:Arc<RelationConfiguration>,query:Arc<ImQuery>)->Result<Self,&'static str>{
  let ImQuery::Relations{group,..}=query.as_ref()else{return Err(SCOPE);};require_current(c,&run,&binding,&gate,&configuration)?;let target=key(group.as_ref().map(|g|g.as_slice()));let serial=head(c,&run,&binding,&target)?.map(|h|h.serial);let budget=binding.scope().reserve_native_rpc(std::mem::size_of::<Self>()+128).map_err(|_|CAPACITY)?;Ok(Self{database,run,binding,gate,configuration,query,target,serial,_budget:budget})
 }
 pub(super) fn commit(&self,db:&Database,reply:&OwnedImReply)->Result<RosterReadView,&'static str>{
  if !std::ptr::eq(self.database.as_ref(),db)||!Arc::ptr_eq(reply.request().query_arc(),&self.query)||!Arc::ptr_eq(reply.request().binding(),&self.binding)||!Arc::ptr_eq(&reply.request().gate(),&self.gate)||!reply.matches(self.binding.owner(),&self.binding.scope()){return Err(SCOPE);}
  db.with_conn(|c|require_current(c,&self.run,&self.binding,&self.gate,&self.configuration))?;
  let ImReplyData::Relations(value)=reply.result().map_err(|_|SCOPE)?else{return Err(SCOPE);};let ImQuery::Relations{group,since,..}=self.query.as_ref()else{return Err(SCOPE);};let group=group.as_ref().map(|g|g.as_slice());
  if !value.accepted{return Ok(RosterReadView{outcome:"rejected",complete_roster:false,original_time:Some(value.time.to_string()),members:0});}
  // Native false returns RelationList in the FIRST map. Native true returns
  // Added/Updated/Removed; only this branch uses the original requested since.
  let changes=if value.reported{Changes::capture(&self.binding,&value.first,&value.second,&value.names)?}else{if value.first.iter().any(|r|r.flags==20){return Err(SCOPE);}Changes::capture(&self.binding,&value.first,&[],&[])?};
  let(view,event)=db.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;require_current(&tx,&self.run,&self.binding,&self.gate,&self.configuration)?;
   if !reply.matches(self.binding.owner(),&self.binding.scope()){return Err(SCOPE);}let old=head(&tx,&self.run,&self.binding,&self.target)?;
   if old.map(|h|h.serial)!=self.serial{return Ok((RosterReadView{outcome:"superseded",complete_roster:false,original_time:Some(value.time.to_string()),members:0},None));}
   if old.and_then(|h|h.target).is_some_and(|target|value.time<target){return Ok((RosterReadView{outcome:"stale",complete_roster:false,original_time:Some(value.time.to_string()),members:0},None));}
   if value.reported&&(value.time<*since||old.is_none_or(|h|!h.complete||h.full_required||h.observed!=Some(*since))){return Ok((RosterReadView{outcome:"baseline_required",complete_roster:false,original_time:Some(value.time.to_string()),members:0},None));}
   if value.reported&&value.time==*since&&!changes.values.is_empty(){return Err("app_account_relation_roster_conflict");}
   if !value.reported&&old.is_some_and(|h|h.complete&&!h.gap&&h.observed==Some(value.time)){
    let h=old.ok_or(SCOPE)?;let count:i64=tx.query_row("SELECT COUNT(*) FROM app_voice_relation_roster_members WHERE roster_id=?1",[h.id],|r|r.get(0)).map_err(|_|STORAGE)?;
    if count!=changes.values.len()as i64{return Err("app_account_relation_roster_conflict");}
    for m in &changes.values{let same:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_roster_members WHERE roster_id=?1 AND label_key=?2 AND relation=?3)",params![h.id,m.key.as_slice(),m.bytes.as_slice()],|r|r.get(0)).map_err(|_|STORAGE)?;if !same{return Err("app_account_relation_roster_conflict");}}
   }
   let h=ensure(&tx,&self.run,&self.binding,group)?;if !value.reported{tx.execute("DELETE FROM app_voice_relation_roster_members WHERE roster_id=?1",[h.id]).map_err(|_|STORAGE)?;}changes.apply(&tx,h.id)?;
   let at=chrono::Utc::now().to_rfc3339();let serial=h.serial.checked_add(1).ok_or(CAPACITY)?;let baseline=if value.reported{h.base.ok_or(SCOPE)?}else{value.time};tx.execute("UPDATE app_voice_relation_rosters SET serial=?2,baseline_time=?3,observed_time=?4,target_time=?4,complete=1,gap=0,full_required=0,updated_at=?5 WHERE id=?1",params![h.id,serial,baseline,value.time,at]).map_err(|_|STORAGE)?;
   let members:i64=tx.query_row("SELECT COUNT(*) FROM app_voice_relation_roster_members WHERE roster_id=?1",[h.id],|r|r.get(0)).map_err(|_|STORAGE)?;
   let event=insert_app_event_for_conn(&tx,"app_voice.relation_roster_observed",Some(self.run.line_id()),Some("app_voice"),&serde_json::json!({"roster_id":h.id,"partial":value.reported,"members":members,"original_time":value.time.to_string(),"complete_roster":true}).to_string(),&at).map_err(|_|STORAGE)?;
   require_current(&tx,&self.run,&self.binding,&self.gate,&self.configuration)?;if !reply.matches(self.binding.owner(),&self.binding.scope()){return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)?;Ok((RosterReadView{outcome:if value.reported{"incremental"}else{"snapshot"},complete_roster:true,original_time:Some(value.time.to_string()),members:members as usize},Some(event)))
  })?;if let Some(event)=event{db.publish_committed_app_event(event);}Ok(view)
 }
}
#[derive(Serialize)]
pub struct RosterReadView{pub outcome:&'static str,pub complete_roster:bool,pub original_time:Option<String>,pub members:usize}
impl fmt::Debug for RosterReadView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RosterReadView(<redacted>)")}}
/// Called inside the same original durable event transaction. No original
/// snapshot exists: retain only gap target; do not promote partial DATA to full.
pub(super) fn observe(c:&Connection,run:&RunToken,binding:&ImBinding,group:Option<&[u8]>,base:Option<i64>,update:Option<i64>,relations:&[Relation<'_>])->Result<(),&'static str>{
 if !binding.policy().relation_roster(){return Ok(());}if group.is_some_and(|g|GroupId::parse(g).is_err())||!binding.matches(binding.owner(),&binding.scope())||run.line_id()!=binding.owner().line_id(){return Err(SCOPE);}let changes=Changes::capture(binding,&[],relations,&[])?;let h=ensure(c,run,binding,group)?;
 let serial=h.serial.checked_add(1).ok_or(CAPACITY)?;let target=match(h.target,update){(Some(a),Some(b))=>Some(a.max(b)),(a,b)=>a.or(b)};
 let stale=matches!((h.observed,update),(Some(old),Some(new))if new<old);let contiguous=h.complete&&!h.gap&&!h.full_required&&base==h.observed&&matches!((base,update),(Some(a),Some(b))if b>a);
 let unknown=!matches!((base,update),(Some(a),Some(b))if b>a)||matches!((h.observed,update),(Some(old),Some(new))if new==old);
 if !stale&&contiguous{changes.apply(c,h.id)?;}let observed=if !stale&&contiguous{update}else{h.observed};let gap=if stale{h.gap}else{!contiguous};let required=h.full_required||!stale&&unknown;
 c.execute("UPDATE app_voice_relation_rosters SET serial=?2,observed_time=?3,target_time=?4,gap=?5,full_required=?6,updated_at=?7 WHERE id=?1",params![h.id,serial,observed,target,i64::from(gap),i64::from(required),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;Ok(())
}
#[derive(Serialize)]
pub struct RosterView{pub id:i64,pub target_kind:String,pub baseline_time:Option<String>,pub observed_time:Option<String>,pub target_time:Option<String>,pub historical_complete:bool,pub gap:bool,pub full_required:bool,pub members:usize,pub updated_at:String}
impl fmt::Debug for RosterView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RosterView(<redacted>)")}}
pub(super) fn history(c:&Connection,line:&str,limit:usize)->Result<Vec<RosterView>,&'static str>{
 if !(1..=128).contains(&limit)||account_store::metadata(c,line)?.is_none(){return Err(SCOPE);}let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_relation_rosters')",[],|r|r.get(0)).map_err(|_|STORAGE)?;if !exists{return Ok(Vec::new());}
 let mut q=c.prepare("SELECT id,CASE WHEN target_kind IN('buddy','group') THEN target_kind ELSE NULL END,baseline_time,observed_time,target_time,complete,gap,full_required,(SELECT COUNT(*) FROM app_voice_relation_roster_members m WHERE m.roster_id=r.id),CASE WHEN typeof(updated_at)='text' AND length(CAST(updated_at AS BLOB)) BETWEEN 1 AND 64 THEN updated_at ELSE NULL END FROM app_voice_relation_rosters r WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
 let rows=q.query_map(params![line,limit as i64],|r|{let complete:i64=r.get(5)?;let gap:i64=r.get(6)?;let required:i64=r.get(7)?;if !matches!(complete,0|1)||!matches!(gap,0|1)||!matches!(required,0|1){return Err(rusqlite::Error::InvalidQuery);}Ok(RosterView{id:r.get(0)?,target_kind:r.get(1)?,baseline_time:r.get::<_,Option<i64>>(2)?.map(|v|v.to_string()),observed_time:r.get::<_,Option<i64>>(3)?.map(|v|v.to_string()),target_time:r.get::<_,Option<i64>>(4)?.map(|v|v.to_string()),historical_complete:complete==1&&gap==0&&required==0,gap:gap==1,full_required:required==1,members:r.get(8)?,updated_at:r.get(9)?})}).map_err(|_|STORAGE)?;
 rows.map(|r|{let v=r.map_err(|_|STORAGE)?;if v.id<=0||v.members>1024||chrono::DateTime::parse_from_rfc3339(&v.updated_at).is_err(){return Err(STORAGE);}Ok(v)}).collect()
}
