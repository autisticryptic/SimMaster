//! Independent Group relation WRITE intent journal. Stored rows/results are
//! DATA, never current SDK auth, a new WRITE permit or permission to resend.
use super::{account_runs::{self,RunToken},account_store,arc_session::RelationIdentity,
    group_directory_data::{GroupId,Relation},group_relation_updates::{self,Removal},
    im_delivery::ImBinding,im_inbox::ImReceiptGate,native_rpc_budget::NativeRpcReservation,
    relation_update_policy::NativeRelationUpdateConfig as Policy,rpc_pending::RpcConnectionScope,
    rpc_stream::MAX_STREAM_BYTES,session::AttemptOwner};
use crate::platform::db::Database;
use rusqlite::{params,Connection,OptionalExtension,Transaction,TransactionBehavior};
use serde::Serialize;
use std::{fmt,sync::{Arc,Mutex,Weak,atomic::{AtomicI64,AtomicU8,Ordering}},time::Instant};
use zeroize::Zeroizing;
#[path="relation_intent.rs"]
mod intents;
pub(super) use intents::{RelationIntent,IntentAdmission,RelationIntentWait};
pub(crate) use intents::RelationIntentView;
pub(super) use intents::history as intent_history;
const STORAGE:&str="app_account_relation_storage_failed";
const SCOPE:&str="app_account_relation_scope_invalid";
const INVALID:&str="app_account_relation_request_invalid";
const UNKNOWN:&str="app_account_relation_uncertain";
const QUEUED:u8=0;const WRITING:u8=1;const SUBMITTED:u8=2;const RESOLVED:u8=3;const RETIRED:u8=4;const CLOSED:u8=5;
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|p|p.into_inner())}
fn hash(parts:&[&[u8]])->[u8;32]{let mut h=ring::digest::Context::new(&ring::digest::SHA256);for p in parts{h.update(&(p.len() as u64).to_be_bytes());h.update(p);}let mut out=[0;32];out.copy_from_slice(h.finish().as_ref());out}
/// Frozen bytes of the configuration actually loaded by this original run.
/// Hashes are private DATA, not SDK authentication or a relation WRITE grant.
pub(super) struct RelationConfiguration(Zeroizing<[u8;32]>);
impl fmt::Debug for RelationConfiguration{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationConfiguration(<redacted>)")}}
impl RelationConfiguration{
    pub(super) fn capture(c:&Connection,line:&str,revision:u64,scope:Option<&RpcConnectionScope>)->Result<Self,&'static str>{
        let(kind,length,stored):(String,i64,u64)=c.query_row("SELECT typeof(private_config),length(private_config),revision FROM app_voice_accounts WHERE line_id=?1",[line],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|SCOPE)?;
        if kind!="blob"||!(1..=256*1024).contains(&length)||stored!=revision{return Err(SCOPE);}
        let _scratch=scope.map(|s|s.reserve_native_rpc(length as usize+64)).transpose().map_err(|_|"app_account_relation_capacity")?;
        // The CASE also fences growth between queries from another connection;
        // an oversized/replaced blob is never copied under a smaller charge.
        let raw:Option<Vec<u8>>=c.query_row("SELECT CASE WHEN typeof(private_config)='blob' AND length(private_config)=?2 AND revision=?3 THEN private_config ELSE NULL END FROM app_voice_accounts WHERE line_id=?1",params![line,length,revision as i64],|r|r.get(0)).map_err(|_|SCOPE)?;
        let raw=Zeroizing::new(raw.ok_or(SCOPE)?);Ok(Self(Zeroizing::new(hash(&[b"original-relation-config-v1",&raw]))))
    }
    pub(super) fn require_current(&self,c:&Connection,line:&str,revision:u64,scope:&RpcConnectionScope)->Result<(),&'static str>{
        let current=Self::capture(c,line,revision,Some(scope))?;if *current.0!=*self.0{return Err(SCOPE);}Ok(())
    }
}
pub(super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{
    c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_relation_writes(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),intent_hash BLOB NOT NULL CHECK(typeof(intent_hash)='blob' AND length(intent_hash)=32),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,
        target_kind TEXT NOT NULL CHECK(target_kind IN('buddy','group')),additions INTEGER NOT NULL CHECK(additions BETWEEN 0 AND 128),
        updates INTEGER NOT NULL CHECK(updates BETWEEN 0 AND 128),removals INTEGER NOT NULL CHECK(removals BETWEEN 0 AND 128),
        arguments_hash BLOB CHECK(arguments_hash IS NULL OR(typeof(arguments_hash)='blob' AND length(arguments_hash)=32)),
        state INTEGER NOT NULL CHECK(state IN(0,1,2,3)),request_id INTEGER CHECK(request_id>0),result_code INTEGER,base_time INTEGER,update_time INTEGER,
        updated_at TEXT NOT NULL,UNIQUE(line_id,user_nonce));
        CREATE INDEX IF NOT EXISTS idx_relation_writes_line ON app_voice_relation_writes(line_id,id);")
        .map_err(|_|STORAGE)?;
    intents::initialize_schema(c)
}
pub(super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(c,line)?.is_some(){return Err(SCOPE);}
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_relation_writes')",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if exists{c.execute("DELETE FROM app_voice_relation_writes WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}intents::delete_account(c,line)
}
/// Immutable maps captured synchronously under the original input reservation.
/// For Buddy, the real identifier is taken later ONLY from RelationIdentity.
pub(crate) struct RelationMessage{group:Option<Zeroizing<Vec<u8>>>,maps:Zeroizing<Vec<u8>>,nonce:Zeroizing<String>,counts:[usize;3],budget:Arc<NativeRpcReservation>}
impl fmt::Debug for RelationMessage{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationMessage(<redacted>)")}}
impl RelationMessage{
    pub(super) fn capture(scope:&RpcConnectionScope,group:Option<&[u8]>,adds:&[Relation<'_>],updates:&[Relation<'_>],removes:&[Removal<'_>],nonce:&str,policy:Policy)->Result<Self,&'static str>{
        policy.validate()?;
        if nonce.len()!=32||!nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return Err(INVALID);}
        if !policy.admits(adds.len(),updates.len(),removes.len()){return Err("app_account_relation_not_installed");}
        if let Some(g)=group{GroupId::parse(g).map_err(|_|INVALID)?;}
        group_relation_updates::reject_conflicting_changes(adds,updates,removes).map_err(|_|INVALID)?;
        let shape_id=group.unwrap_or(b"shape-only");let capacity=group_relation_updates::encoded_size(shape_id,adds,updates,removes).map_err(|_|INVALID)?;
        let scratch=adds.len().max(updates.len()).max(removes.len())*std::mem::size_of::<&Relation<'_>>()
            +adds.iter().chain(updates).map(|r|r.properties.len()).max().unwrap_or(0)*std::mem::size_of::<&super::rpc_stream::RpcProperty<'_>>();
        let peak=capacity.checked_mul(2).and_then(|n|n.checked_add(group.map_or(0,|g|g.len())+nonce.len()+std::mem::size_of::<Self>()+std::mem::size_of::<RelationAction>()+std::mem::size_of::<RelationConfiguration>()+std::mem::size_of::<RelationIntent>()+scratch+256)).ok_or("app_account_relation_capacity")?;
        let budget=scope.reserve_native_rpc(peak).map_err(|_|"app_account_relation_capacity")?;
        let bytes=group_relation_updates::encode_updates(shape_id,adds,updates,removes).map_err(|_|INVALID)?;
        let start=5+4+shape_id.len();let maps=Zeroizing::new(bytes.expose_to_transport()[start..].to_vec());
        Ok(Self{group:group.map(|g|Zeroizing::new(g.to_vec())),maps,nonce:Zeroizing::new(nonce.to_owned()),counts:[adds.len(),updates.len(),removes.len()],budget})
    }
    fn fingerprint(&self)->[u8;32]{hash(&[b"original-relation-body-v1",&[u8::from(self.group.is_some())],self.group.as_deref().map_or(b"".as_slice(),|g|g.as_slice()),&self.maps])}
    fn target_kind(&self)->&'static str{if self.group.is_some(){"group"}else{"buddy"}}
    fn matches(&self,scope:&RpcConnectionScope,policy:Policy)->bool{self.budget.belongs_to(scope)&&policy.admits(self.counts[0],self.counts[1],self.counts[2])}
    fn arguments_size(&self,uid:&[u8])->Result<usize,&'static str>{let id=self.group.as_deref().map_or(uid,|g|g.as_slice());let size=9usize.checked_add(id.len()).and_then(|n|n.checked_add(self.maps.len())).ok_or("app_account_relation_capacity")?;if size>MAX_STREAM_BYTES{return Err("app_account_relation_capacity");}Ok(size)}
    fn arguments(&self,uid:&[u8])->Result<Zeroizing<Vec<u8>>,&'static str>{
        let id=self.group.as_deref().map_or(uid,|g|g.as_slice());if id.is_empty()||id.len()>super::rpc_stream::MAX_FIELD_BYTES{return Err(SCOPE);}
        let size=self.arguments_size(uid)?;let mut bytes=Zeroizing::new(Vec::with_capacity(size));bytes.extend_from_slice(&[0,0,1,0,0]);bytes.extend_from_slice(&(id.len() as u32).to_be_bytes());bytes.extend_from_slice(id);bytes.extend_from_slice(&self.maps);Ok(bytes)
    }
}
pub(super) enum PreparedRelation{Fresh(Arc<RelationAction>),Replay(RelationWriteView)}
struct PermitSlot{cancelled:bool,permit:Weak<RelationPermit>}
pub(super) struct RelationAction{database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,identity:Arc<RelationIdentity>,parent:Arc<ImReceiptGate>,gate:Arc<ImReceiptGate>,message:Arc<RelationMessage>,intent:Option<Arc<RelationIntent>>,intent_hash:[u8;32],configuration:Arc<RelationConfiguration>,row:AtomicI64,deadline:Instant,permit:Mutex<PermitSlot>}
impl fmt::Debug for RelationAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationAction(<redacted>)")}}
impl RelationAction{
    #[cfg(test)]
    pub(super) fn original(database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,identity:Arc<RelationIdentity>,parent:Arc<ImReceiptGate>,message:RelationMessage,deadline:Instant,policy:Policy)->Result<PreparedRelation,&'static str>{
        let configuration=Arc::new(database.with_conn(|c|RelationConfiguration::capture(c,run.line_id(),run.revision(),Some(&binding.scope())))?);
        Self::original_with_configuration(database,run,binding,identity,parent,message,deadline,policy,configuration)
    }
    #[cfg(test)]
    pub(super) fn original_with_configuration(database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,identity:Arc<RelationIdentity>,parent:Arc<ImReceiptGate>,message:RelationMessage,deadline:Instant,policy:Policy,configuration:Arc<RelationConfiguration>)->Result<PreparedRelation,&'static str>{
        Self::original_captured(database,run,binding,identity,parent,Arc::new(message),deadline,policy,configuration,None)
    }
    fn original_captured(database:Arc<Database>,run:Arc<RunToken>,binding:Arc<ImBinding>,identity:Arc<RelationIdentity>,parent:Arc<ImReceiptGate>,message:Arc<RelationMessage>,deadline:Instant,policy:Policy,configuration:Arc<RelationConfiguration>,intent:Option<Arc<RelationIntent>>)->Result<PreparedRelation,&'static str>{
        if !identity.authenticates(&binding)||!message.matches(&binding.scope(),policy)||!parent.live()||Instant::now()>=deadline||run.line_id()!=binding.owner().line_id()||intent.as_ref().is_some_and(|i|!i.binds(binding.owner(),&binding.scope(),&message)){return Err(SCOPE);}
        let uid=identity.uid_for(binding.owner(),&binding.scope()).ok_or(SCOPE)?;
        let intent_hash=hash(&[b"original-relation-intent-v1",binding.identity(),uid,&message.fingerprint()]);
        let gate=intent.as_ref().map_or_else(ImReceiptGate::fresh,|i|i.gate());
        let action=Arc::new(Self{database,run,binding,identity,parent,gate,message,intent,intent_hash,configuration,row:AtomicI64::new(0),deadline,permit:Mutex::new(PermitSlot{cancelled:false,permit:Weak::new()})});
        let(id,replay)=action.database.with_conn(|c|{
            let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;action.validate(&tx)?;
            let old:Option<(i64,Vec<u8>)>=tx.query_row("SELECT id,intent_hash FROM app_voice_relation_writes WHERE line_id=?1 AND user_nonce=?2",params![action.run.line_id(),action.message.nonce.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
            if let Some((id,hash))=old{if hash.as_slice()!=action.intent_hash.as_slice(){return Err("app_account_relation_nonce_conflict");}return Ok((id,true));}
            let unknown:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE line_id=?1 AND state=0)",[action.run.line_id()],|r|r.get(0)).map_err(|_|STORAGE)?;if unknown{return Err(UNKNOWN);}
            let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_relation_writes",[action.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;if local>=128||total>=4096{return Err("app_account_relation_full");}
            tx.execute("INSERT INTO app_voice_relation_writes(line_id,revision,user_nonce,intent_hash,identity_key,run_nonce,scope_epoch,target_kind,additions,updates,removals,state,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,?12)",params![action.run.line_id(),action.run.revision() as i64,action.message.nonce.as_str(),action.intent_hash.as_slice(),action.binding.identity().as_slice(),action.run.nonce(),action.binding.receipt_epoch(),action.message.target_kind(),action.message.counts[0] as i64,action.message.counts[1] as i64,action.message.counts[2] as i64,chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
            let id=tx.last_insert_rowid();if let Some(intent)=&action.intent{intent.commit_binding(&tx,id)?;}action.validate(&tx)?;tx.commit().map_err(|_|STORAGE)?;Ok((id,false))
        })?;
        if replay{return Ok(PreparedRelation::Replay(action.database.with_conn(|c|view(c,id))?));}
        action.row.store(id,Ordering::SeqCst);Ok(PreparedRelation::Fresh(action))
    }
    fn live(&self)->bool{self.parent.live()&&self.gate.live()&&Instant::now()<self.deadline&&self.identity.authenticates(&self.binding)}
    fn validate(&self,c:&Connection)->Result<(),&'static str>{
        if !self.live()||account_store::metadata(c,self.run.line_id())?.is_none_or(|a|a.revision!=self.run.revision())||!account_runs::runtime_matches(c,&self.run)?{return Err(SCOPE);}
        self.configuration.require_current(c,self.run.line_id(),self.run.revision(),&self.scope())?;
        let row=self.row.load(Ordering::SeqCst);if row>0{if let Some(intent)=&self.intent{intent.require_bound(c,row)?;}}
        account_runs::require_live(c,self.run.line_id(),self.run.revision(),self.run.nonce())
    }
    pub(super) fn owner(&self)->&AttemptOwner{self.binding.owner()}
    pub(super) fn scope(&self)->RpcConnectionScope{self.binding.scope()}
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn permit(self:&Arc<Self>,policy:Policy)->Result<Arc<RelationPermit>,&'static str>{
        let mut slot=lock(&self.permit);if slot.cancelled||slot.permit.upgrade().is_some()||!self.live()||!self.message.matches(&self.scope(),policy){return Err(SCOPE);}
        let uid=self.identity.uid_for(self.owner(),&self.scope()).ok_or(SCOPE)?;let size=self.message.arguments_size(uid)?;
        let budget=self.scope().reserve_native_rpc(size.checked_add(std::mem::size_of::<RelationPermit>()+64).ok_or("app_account_relation_capacity")?).map_err(|_|"app_account_relation_capacity")?;
        let arguments=self.message.arguments(uid)?;let arguments_hash=hash(&[b"original-relation-args-v1",&arguments]);let row=self.row.load(Ordering::SeqCst);
        self.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.validate(&tx)?;
            let n=tx.execute("UPDATE app_voice_relation_writes SET arguments_hash=?3 WHERE id=?1 AND user_nonce=?2 AND intent_hash=?4 AND run_nonce=?5 AND scope_epoch=?6 AND state=0 AND arguments_hash IS NULL AND request_id IS NULL",params![row,self.message.nonce.as_str(),arguments_hash.as_slice(),self.intent_hash.as_slice(),self.run.nonce(),self.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
            if n!=1{return Err(SCOPE);}self.validate(&tx)?;tx.commit().map_err(|_|STORAGE)
        })?;
        let permit=Arc::new(RelationPermit{action:self.clone(),arguments,arguments_hash,phase:AtomicU8::new(QUEUED),request:Mutex::new(None),transition:Mutex::new(()),budget,changed:tokio::sync::Notify::new()});slot.permit=Arc::downgrade(&permit);drop(slot);
        if !self.live(){permit.cancel();return Err(SCOPE);}Ok(permit)
    }
    pub(super) fn cancel(&self){
        let permit={let mut s=lock(&self.permit);s.cancelled=true;s.permit.upgrade()};
        if let Some(p)=permit{p.cancel();}else{let row=self.row.load(Ordering::SeqCst);if row>0{let _=self.database.with_conn(|c|c.execute("UPDATE app_voice_relation_writes SET state=3,updated_at=?4 WHERE id=?1 AND user_nonce=?2 AND intent_hash=?3 AND state=0 AND request_id IS NULL",params![row,self.message.nonce.as_str(),self.intent_hash.as_slice(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE));}}
        self.gate.retire();
    }
}
impl Drop for RelationAction{fn drop(&mut self){self.cancel();}}
struct Request{id:i32,deadline:Instant}
pub(super) struct RelationPermit{action:Arc<RelationAction>,arguments:Zeroizing<Vec<u8>>,arguments_hash:[u8;32],phase:AtomicU8,request:Mutex<Option<Request>>,transition:Mutex<()>,budget:Arc<NativeRpcReservation>,changed:tokio::sync::Notify}
impl fmt::Debug for RelationPermit{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationPermit(<redacted>)")}}
impl RelationPermit{
    pub(super) fn binding(&self)->&Arc<ImBinding>{&self.action.binding}
    pub(super) fn arguments(&self)->&[u8]{&self.arguments}
    pub(super) fn deadline(&self)->Instant{self.action.deadline}
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.action.gate()}
    pub(super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.action.parent.clone()}
    pub(super) fn token_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<&[u8]>{self.action.identity.token_for(owner,scope)}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.action.live()&&self.binding().matches(owner,scope)&&self.budget.belongs_to(scope)&&self.phase.load(Ordering::SeqCst)<RESOLVED}
    pub(super) fn retained_bytes(&self)->usize{self.arguments.len()+self.action.message.maps.len()+256}
    fn fixed(&self,c:&Connection)->Result<(),&'static str>{
        let yes:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND user_nonce=?5 AND intent_hash=?6 AND arguments_hash=?7 AND run_nonce=?8 AND scope_epoch=?9)",params![self.action.row.load(Ordering::SeqCst),self.action.run.line_id(),self.action.run.revision() as i64,self.binding().identity().as_slice(),self.action.message.nonce.as_str(),self.action.intent_hash.as_slice(),self.arguments_hash.as_slice(),self.action.run.nonce(),self.binding().receipt_epoch()],|r|r.get(0)).map_err(|_|STORAGE)?;if !yes{return Err(SCOPE);}Ok(())
    }
    pub(super) fn arm(&self,id:i32,deadline:Instant)->Result<(),&'static str>{
        let _transition=lock(&self.transition);let end=deadline.min(self.deadline());let mut request=lock(&self.request);if id<=0||request.is_some()||self.phase.load(Ordering::SeqCst)!=QUEUED||!self.action.live()||Instant::now()>=end{return Err(SCOPE);}
        self.action.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.action.validate(&tx)?;self.fixed(&tx)?;
            let n=tx.execute("UPDATE app_voice_relation_writes SET request_id=?3 WHERE id=?1 AND user_nonce=?2 AND state=0 AND request_id IS NULL",params![self.action.row.load(Ordering::SeqCst),self.action.message.nonce.as_str(),id]).map_err(|_|STORAGE)?;if n!=1||Instant::now()>=end{return Err(SCOPE);}self.action.validate(&tx)?;tx.commit().map_err(|_|STORAGE)
        })?;*request=Some(Request{id,deadline:end});Ok(())
    }
    pub(super) fn current(&self,id:i32)->bool{self.action.live()&&lock(&self.request).as_ref().is_some_and(|r|r.id==id&&Instant::now()<r.deadline)}
    pub(super) fn begin_write(&self,id:i32)->bool{
        let _transition=lock(&self.transition);
        if !self.current(id)||self.phase.load(Ordering::SeqCst)!=QUEUED{return false;}
        if self.action.database.with_conn(|c|{self.action.validate(c)?;self.fixed(c)}).is_err(){return false;}
        self.current(id)&&self.phase.compare_exchange(QUEUED,WRITING,Ordering::SeqCst,Ordering::SeqCst).is_ok()
    }
    pub(super) fn submitted(&self)->bool{let _transition=lock(&self.transition);self.phase.compare_exchange(WRITING,SUBMITTED,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    pub(super) fn retired(&self)->bool{self.phase.load(Ordering::SeqCst)>=RETIRED}
    pub(super) fn closed_original_io(&self)->bool{self.phase.load(Ordering::SeqCst)==CLOSED}
    pub(super) fn resolve(&self,reply:&super::rpc_relation_update::OwnedRelationReply)->Result<RelationWriteView,&'static str>{
        let _transition=lock(&self.transition);let id=reply.request().id();if !reply.belongs_to(self)||!self.current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNKNOWN);}
        let(data,event)=self.action.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.action.validate(&tx)?;self.fixed(&tx)?;
            let n=tx.execute("UPDATE app_voice_relation_writes SET state=?4,result_code=?5,base_time=?6,update_time=?7,updated_at=?8 WHERE id=?1 AND user_nonce=?2 AND request_id=?3 AND state=0",params![self.action.row.load(Ordering::SeqCst),self.action.message.nonce.as_str(),id,if reply.accepted(){1}else{2},i64::from(reply.code()),reply.base_time(),reply.update_time(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
            if n!=1||!self.current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNKNOWN);}let data=view(&tx,self.action.row.load(Ordering::SeqCst))?;
            let payload=serde_json::json!({"relation_write_id":data.id,"target_kind":data.target_kind,"state":data.state,"additions":data.additions,"updates":data.updates,"removals":data.removals,"base_time":data.base_time,"update_time":data.update_time}).to_string();
            let event=crate::platform::db::insert_app_event_for_conn(&tx,"app_voice.relations_updated",Some(self.action.run.line_id()),Some("app_voice"),&payload,&data.updated_at).map_err(|_|STORAGE)?;
            if !self.current(id){return Err(UNKNOWN);}tx.commit().map_err(|_|STORAGE)?;self.phase.store(RESOLVED,Ordering::SeqCst);Ok((data,event))
        })?;
        self.action.database.publish_committed_app_event(event);Ok(data)
    }
    pub(super) fn cancel(&self){
        let _transition=lock(&self.transition);let old=loop{let p=self.phase.load(Ordering::SeqCst);if p>=RESOLVED{return;}let to=if p==QUEUED{RETIRED}else{CLOSED};if self.phase.compare_exchange(p,to,Ordering::SeqCst,Ordering::SeqCst).is_ok(){break p;}};
        if old==QUEUED{let _=self.action.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.fixed(&tx)?;tx.execute("UPDATE app_voice_relation_writes SET state=3,updated_at=?3 WHERE id=?1 AND user_nonce=?2 AND state=0",params![self.action.row.load(Ordering::SeqCst),self.action.message.nonce.as_str(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;tx.commit().map_err(|_|STORAGE)});}else{self.action.scope().close();}
        self.changed.notify_waiters();
    }
    pub(super) async fn cancelled(&self){loop{let n=self.changed.notified();tokio::pin!(n);n.as_mut().enable();if self.retired(){return;}n.await;}}
}
impl Drop for RelationPermit{fn drop(&mut self){self.cancel();}}
#[derive(Serialize)]pub struct RelationWriteView{pub id:i64,pub nonce_tag:String,pub target_kind:&'static str,pub state:&'static str,pub additions:usize,pub updates:usize,pub removals:usize,pub base_time:Option<String>,pub update_time:Option<String>,pub updated_at:String}
impl fmt::Debug for RelationWriteView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationWriteView(<redacted>)")}}
fn view(c:&Connection,id:i64)->Result<RelationWriteView,&'static str>{
    c.query_row("SELECT target_kind,state,additions,updates,removals,base_time,update_time,updated_at,user_nonce FROM app_voice_relation_writes WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,usize>(2)?,r.get::<_,usize>(3)?,r.get::<_,usize>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<i64>>(6)?,r.get::<_,String>(7)?,r.get::<_,String>(8)?))).map_err(|_|STORAGE).and_then(|(kind,state,adds,updates,removes,base,update,at,nonce)|{
        let nonce=Zeroizing::new(nonce);if id<=0||nonce.len()!=32||!nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||at.len()>64||chrono::DateTime::parse_from_rfc3339(&at).is_err()||[adds,updates,removes].iter().any(|n|*n>128)||adds+updates+removes==0||base.is_some()!=update.is_some()||matches!(state,0|3)&&base.is_some()||matches!(state,1|2)&&base.is_none(){return Err(STORAGE);}let tag=hash(&[b"original-relation-ui-nonce-v1",nonce.as_bytes()]).iter().map(|b|format!("{b:02x}")).collect();
        Ok(RelationWriteView{id,nonce_tag:tag,target_kind:match kind.as_str(){"buddy"=>"buddy","group"=>"group",_=>return Err(STORAGE)},state:match state{0=>"uncertain",1=>"confirmed",2=>"rejected",3=>"not_sent",_=>return Err(STORAGE)},additions:adds,updates,removals:removes,base_time:base.map(|v|v.to_string()),update_time:update.map(|v|v.to_string()),updated_at:at})
    })
}
pub(super) fn historical_view(c:&Connection,line:&str,id:i64)->Result<RelationWriteView,&'static str>{
    let belongs:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE id=?1 AND line_id=?2)",params![id,line],|r|r.get(0)).map_err(|_|STORAGE)?;
    if !belongs||account_store::metadata(c,line)?.is_none(){return Err(SCOPE);}view(c,id)
}
pub(super) fn history(c:&Connection,line:&str,limit:usize)->Result<Vec<RelationWriteView>,&'static str>{
    if !(1..=128).contains(&limit)||account_store::metadata(c,line)?.is_none(){return Err(INVALID);}let mut q=c.prepare("SELECT id FROM app_voice_relation_writes WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;let ids=q.query_map(params![line,limit as i64],|r|r.get::<_,i64>(0)).map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;ids.into_iter().map(|id|view(c,id)).collect()
}
pub(super) struct RelationWait(Arc<RelationAction>);
impl RelationWait{pub(super) fn original(a:Arc<RelationAction>)->Self{Self(a)}}
impl Drop for RelationWait{fn drop(&mut self){self.0.cancel();}}
pub(super) struct RelationPermitWait(Arc<RelationPermit>);
impl RelationPermitWait{pub(super) fn original(a:Arc<RelationPermit>)->Self{Self(a)}}
impl Drop for RelationPermitWait{fn drop(&mut self){self.0.cancel();}}
