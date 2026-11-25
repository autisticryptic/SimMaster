//! System IM control consumption, distinct from chat history and outbound
//! relation WRITE outcomes. Original IM cursor and this ledger share one SQL
//! transaction; saved data never reconstructs a current SDK receipt by itself.
use super::{account_runs::{self,RunToken},account_store,im_data::{ImBox,ImMessage},im_delivery::{ImBinding,ImKey,ImReplyTicket},im_inbox::ImReceiptGate,native_rpc_budget::NativeRpcReservation,org_relation_sync::{self,OrgRelationSync},relation_journal::RelationConfiguration,rpc_stream::Writer};
use crate::platform::db::{Database,AppEventEntry,insert_app_event_for_conn};
use rusqlite::{params,Connection,OptionalExtension};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
const STORAGE:&str="app_account_org_relation_storage_failed";const SCOPE:&str="app_account_org_relation_scope_invalid";const FULL:&str="app_account_org_relation_full";
fn hash(parts:&[&[u8]])->[u8;32]{let mut h=ring::digest::Context::new(&ring::digest::SHA256);for p in parts{h.update(&(p.len()as u64).to_be_bytes());h.update(p);}let mut out=[0;32];out.copy_from_slice(h.finish().as_ref());out}
pub(super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_org_relation_messages(
 id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),message_index INTEGER NOT NULL,
 fingerprint BLOB NOT NULL CHECK(typeof(fingerprint)='blob' AND length(fingerprint)=32),body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=65536),receipt_epoch TEXT NOT NULL CHECK(length(receipt_epoch)=32),
 group_hash BLOB NOT NULL CHECK(length(group_hash)=32),label_hash BLOB NOT NULL CHECK(length(label_hash)=32),base_time INTEGER,update_time INTEGER,relation_type INTEGER NOT NULL,org_deleted INTEGER NOT NULL CHECK(org_deleted IN(0,1)),relation_supplied INTEGER NOT NULL CHECK(relation_supplied IN(0,1)),continuity TEXT NOT NULL,created_at TEXT NOT NULL,
 UNIQUE(line_id,revision,identity_key,message_index));
 CREATE TABLE IF NOT EXISTS app_voice_org_relation_heads(line_id TEXT NOT NULL,revision INTEGER NOT NULL,identity_key BLOB NOT NULL,receipt_epoch TEXT NOT NULL,group_hash BLOB NOT NULL,observed_time INTEGER NOT NULL,target_time INTEGER NOT NULL,gap INTEGER NOT NULL CHECK(gap IN(0,1)),PRIMARY KEY(line_id,revision,identity_key,receipt_epoch,group_hash));
 CREATE TABLE IF NOT EXISTS app_voice_org_relation_members(line_id TEXT NOT NULL,revision INTEGER NOT NULL,identity_key BLOB NOT NULL,receipt_epoch TEXT NOT NULL,group_hash BLOB NOT NULL,label_hash BLOB NOT NULL,relation_type INTEGER NOT NULL,relation_body BLOB NOT NULL CHECK(typeof(relation_body)='blob' AND length(relation_body)<=65536),update_time INTEGER NOT NULL,removed INTEGER NOT NULL CHECK(removed IN(0,1)),PRIMARY KEY(line_id,revision,identity_key,receipt_epoch,group_hash,label_hash));") .map_err(|_|STORAGE)}
pub(super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{if account_store::metadata(c,line)?.is_some(){return Err(SCOPE);}for table in ["app_voice_org_relation_members","app_voice_org_relation_heads","app_voice_org_relation_messages"]{let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get(0)).map_err(|_|STORAGE)?;if exists{c.execute(&format!("DELETE FROM {table} WHERE line_id=?1"),[line]).map_err(|_|STORAGE)?;}}Ok(())}
/// One original IM inbox quota across chat and control rows. Counts/bytes are
/// numeric DATA; this helper neither parses bodies nor constructs a receipt.
pub(super) fn usage(c:&Connection,line:&str)->Result<(i64,i64,i64),&'static str>{c.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),COALESCE(SUM(length(body)+length(CAST(receipt_epoch AS BLOB))),0) FROM app_voice_org_relation_messages",[line],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|STORAGE)}
/// Historical collision check only; absence never grants system consumption.
pub(super) fn contains_saved(c:&Connection,run:&RunToken,binding:&ImBinding,key:&ImKey)->Result<bool,&'static str>{
 if key.box_id().bytes()!=org_relation_sync::SYSTEM_BOX{return Ok(false);}c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_org_relation_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND message_index=?4)",params![run.line_id(),run.revision()as i64,binding.identity().as_slice(),key.index()],|r|r.get(0)).map_err(|_|STORAGE)
}
pub(super) struct OrgContext{database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,configuration:Arc<RelationConfiguration>,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for OrgContext{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalOrgRelationContext(<redacted>)")}}
impl OrgContext{
 pub(super) fn original(database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,configuration:Arc<RelationConfiguration>)->Result<Arc<Self>,&'static str>{
  if !binding.policy().org_relation_sync()||run.line_id()!=binding.owner().line_id()||!gate.live()||!binding.matches(binding.owner(),&binding.scope()){return Err(SCOPE);}let budget=binding.scope().reserve_native_rpc(std::mem::size_of::<Self>()+64).map_err(|_|FULL)?;let s=Arc::new(Self{database,run,binding,gate,configuration,_budget:budget});s.database.with_conn(|c|s.validate(c))?;Ok(s)
 }
 pub(super) fn matches(&self,run:&Arc<RunToken>,binding:&Arc<ImBinding>,gate:&Arc<ImReceiptGate>)->bool{Arc::ptr_eq(&self.run,run)&&Arc::ptr_eq(&self.binding,binding)&&Arc::ptr_eq(&self.gate,gate)}
 pub(super) fn validate(&self,c:&Connection)->Result<(),&'static str>{
  if !self.gate.live()||!self.binding.matches(self.binding.owner(),&self.binding.scope())||!account_runs::runtime_matches(c,&self.run)?{return Err(SCOPE);}self.configuration.require_current(c,self.run.line_id(),self.run.revision(),&self.binding.scope())?;account_runs::require_live(c,self.run.line_id(),self.run.revision(),self.run.nonce())?;
  if !self.gate.live()||!self.binding.matches(self.binding.owner(),&self.binding.scope()){return Err(SCOPE);}Ok(())
 }
 pub(super) fn record_current(&self,key:&Arc<ImKey>,id:i64,fingerprint:&[u8;32])->bool{self.database.with_conn(|c|self.lookup(c,key).map(|v|v.is_some_and(|(saved,h)|saved==id&&&h==fingerprint))).unwrap_or(false)}
 pub(super) fn owns_database(&self,db:&Database)->bool{std::ptr::eq(self.database.as_ref(),db)}
 pub(super) fn insert(&self,c:&Connection,r:&OrgRecord)->Result<(i64,Option<AppEventEntry>),&'static str>{
  self.validate(c)?;if !Arc::ptr_eq(&r.binding,&self.binding){return Err(SCOPE);}let data=r.decoded()?;
  let chat:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND message_index=?5)",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),org_relation_sync::SYSTEM_BOX,r.key.index()],|row|row.get(0)).map_err(|_|STORAGE)?;if chat{return Err("app_account_org_relation_conflict");}
  let prior:Option<(i64,Option<Vec<u8>>,Option<String>)>=c.query_row("SELECT id,CASE WHEN typeof(fingerprint)='blob' AND length(fingerprint)=32 THEN fingerprint ELSE NULL END,CASE WHEN typeof(receipt_epoch)='text' AND length(receipt_epoch)=32 THEN receipt_epoch ELSE NULL END FROM app_voice_org_relation_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND message_index=?4",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),r.key.index()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|_|STORAGE)?;
  let old_id=if let Some((id,saved,epoch))=prior{if id<=0||saved.ok_or(STORAGE)?.as_slice()!=r.fingerprint.as_slice(){return Err("app_account_org_relation_conflict");}if epoch.ok_or(STORAGE)?==self.binding.receipt_epoch(){self.validate(c)?;return Ok((id,None));}Some(id)}else{None};
  let(local,total,bytes)=usage(c,self.run.line_id())?;
  let(chat_local,chat_total,chat_bytes):(i64,i64,i64)=c.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),COALESCE(SUM(length(body)+length(CAST(text AS BLOB))+length(sender)+COALESCE(length(info_type),0)+COALESCE(length(CAST(info_epoch AS BLOB)),0)),0) FROM app_voice_im_messages",[self.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|STORAGE)?;
  if old_id.is_none()&&(local.checked_add(chat_local).is_none_or(|n|n>=1024)||total.checked_add(chat_total).is_none_or(|n|n>=4096)||bytes.checked_add(chat_bytes).and_then(|n|n.checked_add(r.raw.len()as i64+32)).is_none_or(|n|n>16*1024*1024)){return Err(FULL);}
  let group=hash(&[b"org-rel-group-v1",data.group.bytes()]);let label=hash(&[b"org-rel-label-v1",data.update_id]);
  let continuity=if let (Some(base),Some(update))=(data.base,data.update){if update<=base{"unsequenced"}else{
   let head:Option<(i64,i64,i64)>=c.query_row("SELECT observed_time,target_time,gap FROM app_voice_org_relation_heads WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND receipt_epoch=?4 AND group_hash=?5",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.binding.receipt_epoch(),group.as_slice()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional().map_err(|_|STORAGE)?;
   if head.is_none(){let(local,total):(i64,i64)=c.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_org_relation_heads",[self.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;if local>=128||total>=4096{return Err(FULL);}}
   if head.is_some_and(|(a,b,g)|a>b||!matches!(g,0|1)){return Err(STORAGE);}let(kind,observed,target,gap)=match head{None=>("unseeded",update,update,0),Some((old,target,gap))if update<=old=>("stale",old,target,gap),Some((old,target,0))if base==old=>("partial_chain",update,target.max(update),0),Some((old,target,_))=>("gap",old,target.max(update),1)};
   c.execute("INSERT INTO app_voice_org_relation_heads(line_id,revision,identity_key,receipt_epoch,group_hash,observed_time,target_time,gap) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(line_id,revision,identity_key,receipt_epoch,group_hash) DO UPDATE SET observed_time=excluded.observed_time,target_time=excluded.target_time,gap=excluded.gap",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.binding.receipt_epoch(),group.as_slice(),observed,target,gap]).map_err(|_|STORAGE)?;
   if matches!(kind,"unseeded"|"partial_chain"){
    // OrgDel is retained as server DATA, never a local account deletion or
    // authority to erase an entire roster not actually observed here.
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_org_relation_members WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND receipt_epoch=?4 AND group_hash=?5 AND label_hash=?6)",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.binding.receipt_epoch(),group.as_slice(),label.as_slice()],|row|row.get(0)).map_err(|_|STORAGE)?;
    if !exists{let(local,total):(i64,i64)=c.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_org_relation_members",[self.run.line_id()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|STORAGE)?;if local>=1024||total>=4096{return Err(FULL);}}
    let retained:i64=c.query_row("SELECT COALESCE(SUM(length(relation_body)),0) FROM app_voice_org_relation_members",[],|row|row.get(0)).map_err(|_|STORAGE)?;
    let previous:i64=c.query_row("SELECT COALESCE(SUM(length(relation_body)),0) FROM app_voice_org_relation_members WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND receipt_epoch=?4 AND group_hash=?5 AND label_hash=?6",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.binding.receipt_epoch(),group.as_slice(),label.as_slice()],|row|row.get(0)).map_err(|_|STORAGE)?;
    if retained.checked_sub(previous).and_then(|v|v.checked_add(r.relation.len()as i64)).is_none_or(|v|v>16*1024*1024){return Err(FULL);}
    c.execute("INSERT INTO app_voice_org_relation_members(line_id,revision,identity_key,receipt_epoch,group_hash,label_hash,relation_type,relation_body,update_time,removed) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(line_id,revision,identity_key,receipt_epoch,group_hash,label_hash) DO UPDATE SET relation_type=excluded.relation_type,relation_body=excluded.relation_body,update_time=excluded.update_time,removed=excluded.removed",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.binding.receipt_epoch(),group.as_slice(),label.as_slice(),i64::from(data.relation.flags),r.relation.as_slice(),update,i64::from(data.relation.flags==20)]).map_err(|_|STORAGE)?;
   }kind
  }}else{"unsequenced"};
  if let Some(id)=old_id{c.execute("UPDATE app_voice_org_relation_messages SET receipt_epoch=?2 WHERE id=?1",params![id,self.binding.receipt_epoch()]).map_err(|_|STORAGE)?;self.validate(c)?;return Ok((id,None));}
  let at=chrono::Utc::now().to_rfc3339();c.execute("INSERT INTO app_voice_org_relation_messages(line_id,revision,identity_key,message_index,fingerprint,body,receipt_epoch,group_hash,label_hash,base_time,update_time,relation_type,org_deleted,relation_supplied,continuity,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),r.key.index(),r.fingerprint.as_slice(),r.raw.as_slice(),self.binding.receipt_epoch(),group.as_slice(),label.as_slice(),data.base,data.update,i64::from(data.relation.flags),i64::from(data.org_deleted),i64::from(data.relation_supplied),continuity,at]).map_err(|_|STORAGE)?;
  let id=c.last_insert_rowid();let payload=serde_json::json!({"org_relation_event_id":id,"source":"system_org_rel_sync","message_index":r.key.index().to_string(),"base_time":data.base.map(|v|v.to_string()),"update_time":data.update.map(|v|v.to_string()),"removal":data.relation.flags==20,"org_deleted":data.org_deleted,"continuity":continuity,"full_roster_verified":false}).to_string();let event=insert_app_event_for_conn(c,"app_voice.org_relation_observed",Some(self.run.line_id()),Some("app_voice"),&payload,&at).map_err(|_|STORAGE)?;self.validate(c)?;Ok((id,Some(event)))
 }
 pub(super) fn lookup(&self,c:&Connection,key:&Arc<ImKey>)->Result<Option<(i64,[u8;32])>,&'static str>{
  self.validate(c)?;if key.box_id().bytes()!=org_relation_sync::SYSTEM_BOX{return Ok(None);}let row:Option<(i64,Option<Vec<u8>>)>=c.query_row("SELECT id,CASE WHEN typeof(fingerprint)='blob' AND length(fingerprint)=32 THEN fingerprint ELSE NULL END FROM app_voice_org_relation_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND message_index=?4 AND receipt_epoch=?5",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),key.index(),self.binding.receipt_epoch()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
  row.map(|(id,h)|{if id<=0{return Err(STORAGE);}let hash=h.ok_or(STORAGE)?.try_into().map_err(|_|STORAGE)?;Ok((id,hash))}).transpose()
 }
 pub(super) fn saved(&self,c:&Connection,key:&Arc<ImKey>)->Result<Option<(i64,[u8;32])>,&'static str>{
  self.validate(c)?;if key.box_id().bytes()!=org_relation_sync::SYSTEM_BOX{return Ok(None);}let row:Option<(i64,Option<Vec<u8>>)>=c.query_row("SELECT id,CASE WHEN typeof(fingerprint)='blob' AND length(fingerprint)=32 THEN fingerprint ELSE NULL END FROM app_voice_org_relation_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND message_index=?4",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),key.index()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
  row.map(|(id,h)|{if id<=0{return Err(STORAGE);}Ok((id,h.ok_or(STORAGE)?.try_into().map_err(|_|STORAGE)?))}).transpose()
 }
 pub(super) fn receipt(self:&Arc<Self>,key:Arc<ImKey>,fingerprint:[u8;32],id:i64,record:Option<Arc<OrgRecord>>)->Result<Arc<OrgReceipt>,&'static str>{
  // A captured record prepaid the receipt before SQL/cursor commit. A later
  // proof allocation must not fail capacity after durable consumed advances.
  let budget=match &record{Some(r)=>r._budget.clone(),None=>self.binding.scope().reserve_native_rpc(std::mem::size_of::<OrgReceipt>()+128).map_err(|_|FULL)?};Ok(Arc::new(OrgReceipt{context:self.clone(),key,fingerprint,id,_record:record,_budget:budget}))
 }
}
pub(super) struct OrgRecord{binding:Arc<ImBinding>,pub key:Arc<ImKey>,pub fingerprint:[u8;32],raw:Zeroizing<Vec<u8>>,relation:Zeroizing<Vec<u8>>,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for OrgRecord{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalOrgRelationRecord(<redacted>)")}}
impl OrgRecord{
 pub(super) fn capture(binding:Arc<ImBinding>,box_id:ImBox<'_>,message:&ImMessage<'_>)->Result<Option<Arc<Self>>,&'static str>{
  if message.kind!=org_relation_sync::KIND{return Ok(None);}if !binding.policy().org_relation_sync(){return Ok(None);}if box_id.bytes()!=org_relation_sync::SYSTEM_BOX{return Err(SCOPE);}
  let properties=message.attachments.iter().chain(&message.parameters).try_fold(0usize,|n,p|n.checked_add(8+p.key.len()+p.value.len())).ok_or(FULL)?;
  let size=37usize.checked_add(message.sender_uid.len()+message.kind.len()+message.content.len()+properties).ok_or(FULL)?;if size>65536{return Err(FULL);}
  let single=message.parameters.iter().find(|p|p.key==b"UpdateRel").map(|p|org_relation_sync::single_scratch(p.value)).transpose().map_err(|_|"app_account_org_relation_invalid")?.unwrap_or(0);
  let peak=size.checked_mul(3).and_then(|n|n.checked_add((message.attachments.len()+message.parameters.len())*std::mem::size_of::<super::rpc_stream::RpcProperty<'_>>()+single+std::mem::size_of::<Self>()+std::mem::size_of::<OrgReceipt>()+256)).ok_or(FULL)?;
  let budget=binding.scope().reserve_native_rpc(peak).map_err(|_|FULL)?;let value=org_relation_sync::decode(box_id,message).map_err(|_|"app_account_org_relation_invalid")?.ok_or(SCOPE)?;
  let rel_size=17+value.relation.first.len()+value.relation.second.len()+value.relation.properties.iter().map(|p|8+p.key.len()+p.value.len()).sum::<usize>();let mut w=Writer::measured(rel_size).map_err(|_|FULL)?;w.u32(value.relation.flags).map_err(|_|FULL)?;w.bytes(value.relation.first).map_err(|_|FULL)?;w.bytes(value.relation.second).map_err(|_|FULL)?;w.properties(&value.relation.properties).map_err(|_|FULL)?;let relation=Zeroizing::new(w.finish().expose_to_transport().to_vec());
  let mut w=Writer::measured(size).map_err(|_|FULL)?;w.i64(message.index).map_err(|_|FULL)?;w.i64(message.time).map_err(|_|FULL)?;w.bytes(message.sender_uid).map_err(|_|FULL)?;w.bytes(message.kind).map_err(|_|FULL)?;w.bytes(message.content).map_err(|_|FULL)?;w.properties(&message.attachments).map_err(|_|FULL)?;w.properties(&message.parameters).map_err(|_|FULL)?;let raw=Zeroizing::new(w.finish().expose_to_transport().to_vec());
  let key=ImKey::capture(&binding,box_id,message.index);let fingerprint=key.message_fingerprint(&raw);Ok(Some(Arc::new(Self{binding,key,fingerprint,raw,relation,_budget:budget})))
 }
 fn decoded(&self)->Result<OrgRelationSync<'_>,&'static str>{let message=ImMessage::decode(&self.raw).map_err(|_|SCOPE)?;org_relation_sync::decode(self.key.box_id(),&message).map_err(|_|SCOPE)?.ok_or(SCOPE)}
 pub(super) fn bytes(&self)->usize{self.raw.len()+self.relation.len()+256}
}
pub(super) struct OrgReceipt{context:Arc<OrgContext>,key:Arc<ImKey>,fingerprint:[u8;32],id:i64,_record:Option<Arc<OrgRecord>>,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for OrgReceipt{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalOrgRelationReceipt(<redacted>)")}}
#[derive(serde::Serialize)]
pub struct OrgRelationView{pub id:i64,pub message_index:String,pub base_time:Option<String>,pub update_time:Option<String>,pub removal:bool,pub org_deleted:bool,pub relation_supplied:bool,pub continuity:String,pub full_roster_verified:bool,pub received_at:String}
impl fmt::Debug for OrgRelationView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OrgRelationView(<redacted>)")}}
pub(super) fn history(c:&Connection,line:&str,limit:usize)->Result<Vec<OrgRelationView>,&'static str>{
 if !(1..=128).contains(&limit)||account_store::metadata(c,line)?.is_none(){return Err(SCOPE);}
 let mut q=c.prepare("SELECT id,message_index,base_time,update_time,relation_type,org_deleted,relation_supplied,CASE WHEN continuity IN('unseeded','partial_chain','gap','stale','unsequenced') THEN continuity ELSE NULL END,CASE WHEN typeof(created_at)='text' AND length(CAST(created_at AS BLOB)) BETWEEN 1 AND 64 THEN created_at ELSE NULL END FROM app_voice_org_relation_messages WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
 let rows=q.query_map(params![line,limit as i64],|r|{let ty:i64=r.get(4)?;let deleted:i64=r.get(5)?;let supplied:i64=r.get(6)?;if !(0..=i64::from(u32::MAX)).contains(&ty)||!matches!(deleted,0|1)||!matches!(supplied,0|1){return Err(rusqlite::Error::InvalidQuery);}Ok(OrgRelationView{id:r.get(0)?,message_index:r.get::<_,i64>(1)?.to_string(),base_time:r.get::<_,Option<i64>>(2)?.map(|v|v.to_string()),update_time:r.get::<_,Option<i64>>(3)?.map(|v|v.to_string()),removal:ty==20,org_deleted:deleted==1,relation_supplied:supplied==1,continuity:r.get(7)?,full_roster_verified:false,received_at:r.get(8)?})}).map_err(|_|STORAGE)?;
 rows.map(|r|{let v=r.map_err(|_|STORAGE)?;if v.id<=0||chrono::DateTime::parse_from_rfc3339(&v.received_at).is_err(){return Err(STORAGE);}Ok(v)}).collect()
}
impl OrgReceipt{
 pub(super) fn authorizes(&self,ticket:&ImReplyTicket,now:Instant)->bool{self.id>0&&ticket.binding().receipt_epoch()==self.context.binding.receipt_epoch()&&ticket.matches(self.context.binding.owner(),&self.context.binding.scope())&&ticket.key().digest()==self.key.digest()&&(!ticket.is_message()||ticket.fingerprint()==&self.fingerprint)&&now<ticket.completion_deadline()&&self.context.record_current(&self.key,self.id,&self.fingerprint)&&self.context.gate.live()&&ticket.matches(self.context.binding.owner(),&self.context.binding.scope())&&Instant::now()<ticket.completion_deadline()}
 pub(super) fn matches_record(&self,c:&Connection)->Result<bool,&'static str>{Ok(self.context.lookup(c,&self.key)?.is_some_and(|(id,h)|id==self.id&&h==self.fingerprint))}
}
