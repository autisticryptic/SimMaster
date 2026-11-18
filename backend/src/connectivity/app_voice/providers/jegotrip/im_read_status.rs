//! Manual read-position status intent and live original continuous-span targets.
//! A SQLite target/history row, raw index, status bool or UDMR result is DATA,
//! never a producer of this operation. Only the current consumer stores spans.
use super::*;
use super::cursor::GapCommitted;
use super::receipts::ReceivedWritePermit;
use std::{collections::BTreeMap,sync::{Mutex,Weak}};
const MAX_LOCAL:i64=128;const MAX_GLOBAL:i64=4096;
const FULL:&str="app_account_im_read_status_full";
const STATUS_SCOPE:&str="app_account_im_read_status_scope_invalid";
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|e|e.into_inner())}
pub(in super::super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_read_status_targets(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,
        run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),
        target_index INTEGER NOT NULL CHECK(typeof(target_index)='integer' AND target_index>=0),span_hash BLOB NOT NULL CHECK(typeof(span_hash)='blob' AND length(span_hash)=32),
        observed_at TEXT NOT NULL,UNIQUE(line_id,run_nonce,scope_epoch,box_id));
        CREATE TABLE IF NOT EXISTS app_voice_im_read_status_actions(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,
        target_id INTEGER NOT NULL,identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),
        target_index INTEGER NOT NULL CHECK(typeof(target_index)='integer' AND target_index>=0),span_hash BLOB NOT NULL CHECK(typeof(span_hash)='blob' AND length(span_hash)=32),
        user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,
        state INTEGER NOT NULL CHECK(state IN(0,1,2,3)),request_id INTEGER CHECK(request_id>0),result_code INTEGER,updated_at TEXT NOT NULL,
        UNIQUE(line_id,user_nonce));") .map_err(|_|STORAGE)
}
fn exists(conn:&Connection,table:&str)->Result<bool,&'static str>{conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get(0)).map_err(|_|STORAGE)}
pub(in super::super) fn retire_targets(conn:&Connection,run:&RunToken)->Result<(),&'static str>{if exists(conn,"app_voice_im_read_status_targets")?{conn.execute("DELETE FROM app_voice_im_read_status_targets WHERE line_id=?1 AND run_nonce=?2",params![run.line_id(),run.nonce()]).map_err(|_|STORAGE)?;}Ok(())}
pub(in super::super) fn delete_account(conn:&Connection,line:&str)->Result<(),&'static str>{if account_store::metadata(conn,line)?.is_some(){return Err(STATUS_SCOPE);}for table in ["app_voice_im_read_status_targets","app_voice_im_read_status_actions"]{if exists(conn,table)?{conn.execute(&format!("DELETE FROM {table} WHERE line_id=?1"),[line]).map_err(|_|STORAGE)?;}}Ok(())}
#[derive(Serialize)]pub struct ReadStatusTargetView{pub id:i64,pub box_kind:&'static str,pub target_index:String,pub observed_at:String}
#[derive(Serialize)]pub struct ReadStatusView{pub id:i64,pub target_id:i64,pub box_kind:&'static str,pub target_index:String,pub state:&'static str,pub updated_at:String}
impl fmt::Debug for ReadStatusView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ReadStatusView(<redacted>)")}}
fn kind(box_id:&[u8])->&'static str{if box_id.starts_with(b"Org/"){"org"}else if box_id.starts_with(b"P2P/"){"p2p"}else{"unknown"}}
fn state_name(s:i64)->Result<&'static str,&'static str>{match s{0=>Ok("uncertain"),1=>Ok("confirmed"),2=>Ok("rejected"),3=>Ok("not_sent"),_=>Err(STORAGE)}}
/// Aggregate live target accounting for this manager, across all original runs.
/// It is not an SDK/WRITE grant and is never reset by stop/restart or clear.
pub(in super::super) struct ReadStatusBudget{state:Mutex<(usize,usize)>}
impl ReadStatusBudget{pub(in super::super) fn original()->Arc<Self>{Arc::new(Self{state:Mutex::new((0,0))})}#[cfg(test)]pub(in super::super) fn snapshot(&self)->(usize,usize){*lock(&self.state)}}
struct TargetResources{state:Mutex<(usize,usize)>,aggregate:Arc<ReadStatusBudget>,retired:AtomicBool}
pub(in super::super) struct TargetReservation{resources:Arc<TargetResources>,bytes:usize}
impl TargetResources{fn reserve(self:&Arc<Self>,bytes:usize)->Result<TargetReservation,&'static str>{if self.retired.load(Ordering::SeqCst){return Err(STATUS_SCOPE);}let mut state=lock(&self.state);let total=state.1.checked_add(bytes).ok_or(FULL)?;if state.0>=MAX_LOCAL as usize||total>1024*1024{return Err(FULL);}let mut aggregate=lock(&self.aggregate.state);let global=aggregate.1.checked_add(bytes).ok_or(FULL)?;if aggregate.0>=MAX_GLOBAL as usize||global>16*1024*1024{return Err(FULL);}state.0+=1;state.1=total;aggregate.0+=1;aggregate.1=global;Ok(TargetReservation{resources:self.clone(),bytes})}}
impl Drop for TargetReservation{fn drop(&mut self){let mut state=lock(&self.resources.state);let mut aggregate=lock(&self.resources.aggregate.state);state.0=state.0.saturating_sub(1);state.1=state.1.saturating_sub(self.bytes);aggregate.0=aggregate.0.saturating_sub(1);aggregate.1=aggregate.1.saturating_sub(self.bytes);}}
pub(in super::super) struct ReadStatusTarget{id:i64,span:Arc<GapCommitted>,observed_at:String,_reservation:TargetReservation}
impl ReadStatusTarget{
    fn live(&self)->bool{!self._reservation.resources.retired.load(Ordering::SeqCst)}
    fn validate_current(&self,conn:&Connection)->Result<(),&'static str>{if !self.live(){return Err(STATUS_SCOPE);}self.span.validate_current(conn)?;let context=self.span.receipt_context();let(box_id,index,hash)=self.span.receipt_span();let matching:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_read_status_targets WHERE id=?1 AND line_id=?2 AND revision=?3 AND run_nonce=?4 AND scope_epoch=?5 AND identity_key=?6 AND box_id=?7 AND target_index=?8 AND span_hash=?9)",params![self.id,context.run.line_id(),context.run.revision()as i64,context.run.nonce(),context.binding.receipt_epoch(),context.binding.identity().as_slice(),box_id,index,hash.as_slice()],|r|r.get(0)).map_err(|_|STORAGE)?;if !matching{return Err(STATUS_SCOPE);}Ok(())}
}
pub(in super::super) struct ReadStatusTargets{database:Arc<Database>,run:Arc<RunToken>,gate:Arc<ImReceiptGate>,targets:Mutex<BTreeMap<i64,Arc<ReadStatusTarget>>>,retired:AtomicBool,resources:Arc<TargetResources>}
impl fmt::Debug for ReadStatusTargets{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ReadStatusTargets(<redacted>)")}}
impl ReadStatusTargets{
    pub(in super::super) fn original(database:Arc<Database>,run:Arc<RunToken>,gate:Arc<ImReceiptGate>,aggregate:Arc<ReadStatusBudget>)->Arc<Self>{Arc::new(Self{database,run,gate,targets:Mutex::new(BTreeMap::new()),retired:AtomicBool::new(false),resources:Arc::new(TargetResources{state:Mutex::new((0,0)),aggregate,retired:AtomicBool::new(false)})})}
    pub(in super::super) fn clear(&self){let mut targets=lock(&self.targets);self.retired.store(true,Ordering::SeqCst);self.resources.retired.store(true,Ordering::SeqCst);targets.clear();}
    pub(in super::super) fn observe(&self,span:&GapCommitted)->Result<(),&'static str>{
        let context=span.receipt_context();if self.retired.load(Ordering::SeqCst)||!self.gate.live()||!Arc::ptr_eq(&self.gate,&context.gate)||!Arc::ptr_eq(&self.run,&context.run)||!span.is_current(&self.database){return Err(STATUS_SCOPE);}
        if lock(&self.targets).values().any(|target|target.span.same_captured_span(span)){return Ok(());}
        let(box_id,index,hash)=span.receipt_span();let observed_at=chrono::Utc::now().to_rfc3339();
        let old={let mut targets=lock(&self.targets);let old=targets.values().find(|v|v.span.box_bytes()==box_id).map(|v|v.id);if let Some(id)=old{targets.remove(&id);}old};
        // Removing the map reference is NOT a refund while an API/WRITE still
        // retains its original target/span. Its own last-holder Drop pays back.
        let reservation=self.resources.reserve(span.retained_bytes().checked_add(128).ok_or(FULL)?)?;
        let id=self.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;context.validate(&tx)?;span.validate_current(&tx)?;
            let existing:Option<i64>=tx.query_row("SELECT id FROM app_voice_im_read_status_targets WHERE line_id=?1 AND run_nonce=?2 AND scope_epoch=?3 AND box_id=?4",params![self.run.line_id(),self.run.nonce(),context.binding.receipt_epoch(),box_id],|r|r.get(0)).optional().map_err(|_|STORAGE)?;
            let id=if let Some(id)=existing{tx.execute("UPDATE app_voice_im_read_status_targets SET target_index=?2,span_hash=?3,observed_at=?4 WHERE id=?1",params![id,index,hash.as_slice(),observed_at]).map_err(|_|STORAGE)?;id}else{let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_read_status_targets",[self.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;if local>=MAX_LOCAL||total>=MAX_GLOBAL{return Err(FULL);}tx.execute("INSERT INTO app_voice_im_read_status_targets(line_id,revision,run_nonce,scope_epoch,identity_key,box_id,target_index,span_hash,observed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![self.run.line_id(),self.run.revision()as i64,self.run.nonce(),context.binding.receipt_epoch(),context.binding.identity().as_slice(),box_id,index,hash.as_slice(),observed_at]).map_err(|_|STORAGE)?;tx.last_insert_rowid()};span.validate_current(&tx)?;if self.retired.load(Ordering::SeqCst){return Err(STATUS_SCOPE);}tx.commit().map_err(|_|STORAGE)?;Ok(id)})?;
        let target=Arc::new(ReadStatusTarget{id,span:Arc::new(span.captured_copy()),observed_at,_reservation:reservation});let mut targets=lock(&self.targets);if self.retired.load(Ordering::SeqCst)||!self.gate.live(){return Err(STATUS_SCOPE);}if let Some(old)=old{targets.remove(&old);}targets.insert(id,target);Ok(())
    }
    #[cfg(test)]pub(in super::super) fn target_resources(&self)->(usize,usize){*lock(&self.resources.state)}
    #[cfg(test)]pub(in super::super) fn reserve_for_test(&self,bytes:usize)->Result<TargetReservation,&'static str>{self.resources.reserve(bytes)}
    pub(in super::super) fn list(&self)->Vec<ReadStatusTargetView>{if self.retired.load(Ordering::SeqCst){return Vec::new();}let targets=lock(&self.targets).values().cloned().collect::<Vec<_>>();targets.into_iter().filter(|v|!self.retired.load(Ordering::SeqCst)&&v.span.is_current(&self.database)).take(16).map(|v|ReadStatusTargetView{id:v.id,box_kind:kind(v.span.box_bytes()),target_index:v.span.end().to_string(),observed_at:v.observed_at.clone()}).collect()}
    pub(in super::super) fn get(&self,id:i64,expected_index:&str)->Result<Arc<ReadStatusTarget>,&'static str>{let target=lock(&self.targets).get(&id).cloned().ok_or("app_account_im_read_status_target_missing")?;if self.retired.load(Ordering::SeqCst)||!self.gate.live()||target.span.end().to_string()!=expected_index||!target.span.is_current(&self.database){return Err("app_account_im_read_status_target_conflict");}self.database.with_conn(|conn|target.validate_current(conn))?;Ok(target)}
}
pub(in super::super) struct ReadStatusAction{database:Arc<Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,target:Arc<ReadStatusTarget>,nonce:Zeroizing<String>,deadline:Instant,permit:Mutex<Weak<ReceivedWritePermit>>}
impl fmt::Debug for ReadStatusAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ReadStatusAction(<redacted>)")}}
impl ReadStatusAction{
    pub(in super::super) fn original(database:Arc<Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,target:Arc<ReadStatusTarget>,nonce:&str,deadline:Instant)->Result<Arc<Self>,&'static str>{
        let context=target.span.receipt_context();if nonce.len()!=32||!nonce.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c))||!Arc::ptr_eq(&run,&context.run)||!Arc::ptr_eq(&parent_gate,&context.gate)||!context.binding.matches(&owner,&scope)||!target.live()||!target.span.is_current(&database)||Instant::now()>=deadline{return Err(STATUS_SCOPE);}
        Ok(Arc::new(Self{database,run,parent_gate,gate:ImReceiptGate::fresh(),owner,scope,target,nonce:Zeroizing::new(nonce.to_owned()),deadline,permit:Mutex::new(Weak::new())}))
    }
    pub(in super::super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(in super::super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(in super::super) fn deadline(&self)->Instant{self.deadline}
    pub(in super::super) fn live(&self)->bool{self.target.live()&&self.parent_gate.live()&&self.gate.live()&&self.scope.is_open()&&!self.scope.session_revoked()&&Instant::now()<self.deadline}
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(in super::super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.parent_gate.clone()}
    pub(in super::super) fn database(&self)->Arc<Database>{self.database.clone()}
    pub(in super::super) fn span(&self)->&Arc<GapCommitted>{&self.target.span}
    pub(in super::super) fn binding(&self)->Arc<ImBinding>{self.target.span.receipt_context().binding}
    pub(in super::super) fn attach(&self,p:&Arc<ReceivedWritePermit>){*lock(&self.permit)=Arc::downgrade(p);}
    pub(in super::super) fn cancel(&self){self.gate.retire();if let Some(p)=lock(&self.permit).upgrade(){p.cancel();}}
    pub(in super::super) fn admit(self:&Arc<Self>,conn:&Connection)->Result<ManualAdmission,&'static str>{
        if !self.live(){return Err(STATUS_SCOPE);}let context=self.target.span.receipt_context();context.validate(conn)?;self.target.validate_current(conn)?;let(box_id,index,span_hash)=self.target.span.receipt_span();
        let old:Option<(i64,i64,i64,Vec<u8>,i64,Vec<u8>,Vec<u8>,String,String)>=conn.query_row("SELECT id,target_id,revision,identity_key,target_index,span_hash,box_id,run_nonce,scope_epoch FROM app_voice_im_read_status_actions WHERE line_id=?1 AND user_nonce=?2",params![self.run.line_id(),self.nonce.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).optional().map_err(|_|STORAGE)?;
        if let Some((id,target_id,revision,identity,saved_index,saved_hash,saved_box,saved_run,saved_epoch))=old{if target_id!=self.target.id||revision!=self.run.revision()as i64||identity.as_slice()!=context.binding.identity().as_slice()||saved_index!=index||saved_hash.as_slice()!=span_hash.as_slice()||saved_box.as_slice()!=box_id||saved_run!=self.run.nonce()||saved_epoch!=context.binding.receipt_epoch(){return Err("app_account_im_read_status_nonce_conflict");}return Ok(ManualAdmission::Replay(view(conn,id)?));}
        let unknown:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_read_status_actions WHERE line_id=?1 AND identity_key=?2 AND box_id=?3 AND state=0)",params![self.run.line_id(),context.binding.identity().as_slice(),box_id],|r|r.get(0)).map_err(|_|STORAGE)?;if unknown{return Err("app_account_im_receipt_uncertain");}
        let(local,total):(i64,i64)=conn.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_read_status_actions",[self.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;if local>=MAX_LOCAL||total>=MAX_GLOBAL{return Err(FULL);}
        conn.execute("INSERT INTO app_voice_im_read_status_actions(line_id,revision,target_id,identity_key,box_id,target_index,span_hash,user_nonce,run_nonce,scope_epoch,state,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,?11)",params![self.run.line_id(),self.run.revision()as i64,self.target.id,context.binding.identity().as_slice(),box_id,index,span_hash.as_slice(),self.nonce.as_str(),self.run.nonce(),context.binding.receipt_epoch(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;Ok(ManualAdmission::Fresh(Arc::new(ManualReadStatusIntent{action:self.clone(),row:conn.last_insert_rowid()})))
    }
}
pub(in super::super) enum ManualAdmission{Fresh(Arc<ManualReadStatusIntent>),Replay(ReadStatusView)}
pub(in super::super) struct ManualReadStatusIntent{action:Arc<ReadStatusAction>,row:i64}
impl ManualReadStatusIntent{
    pub(in super::super) fn live(&self)->bool{self.action.live()}
    pub(in super::super) fn deadline(&self)->Instant{self.action.deadline()}
    pub(in super::super) fn belongs_to(&self,action:&ReadStatusAction)->bool{std::ptr::eq(self.action.as_ref(),action)}
    fn fixed_context(&self,conn:&Connection)->Result<(),&'static str>{let context=self.action.target.span.receipt_context();let(box_id,index,hash)=self.action.target.span.receipt_span();let matching:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_read_status_actions WHERE id=?1 AND line_id=?2 AND revision=?3 AND target_id=?4 AND identity_key=?5 AND box_id=?6 AND target_index=?7 AND span_hash=?8 AND user_nonce=?9 AND run_nonce=?10 AND scope_epoch=?11)",params![self.row,self.action.run.line_id(),self.action.run.revision()as i64,self.action.target.id,context.binding.identity().as_slice(),box_id,index,hash.as_slice(),self.action.nonce.as_str(),self.action.run.nonce(),context.binding.receipt_epoch()],|r|r.get(0)).map_err(|_|STORAGE)?;if !matching{return Err(STATUS_SCOPE);}Ok(())}
    pub(in super::super) fn arm(&self,conn:&Connection,id:i32)->Result<(),&'static str>{self.fixed_context(conn)?;self.action.target.validate_current(conn)?;let n=conn.execute("UPDATE app_voice_im_read_status_actions SET request_id=?3 WHERE id=?1 AND user_nonce=?2 AND state=0 AND request_id IS NULL AND run_nonce=?4 AND scope_epoch=?5",params![self.row,self.action.nonce.as_str(),id,self.action.run.nonce(),self.action.target.span.receipt_context().binding.receipt_epoch()]).map_err(|_|STORAGE)?;if n!=1||!self.live(){return Err(STATUS_SCOPE);}Ok(())}
    pub(in super::super) fn cancel_queued(&self,conn:&Connection)->Result<(),&'static str>{self.fixed_context(conn)?;conn.execute("UPDATE app_voice_im_read_status_actions SET state=3,updated_at=?3 WHERE id=?1 AND user_nonce=?2 AND state=0 AND request_id IS ?4",params![self.row,self.action.nonce.as_str(),chrono::Utc::now().to_rfc3339(),self.request_id(conn)?]).map_err(|_|STORAGE)?;Ok(())}
    fn request_id(&self,conn:&Connection)->Result<Option<i32>,&'static str>{conn.query_row("SELECT request_id FROM app_voice_im_read_status_actions WHERE id=?1 AND user_nonce=?2",params![self.row,self.action.nonce.as_str()],|r|r.get(0)).map_err(|_|STORAGE)}
    pub(in super::super) fn resolve(&self,conn:&Connection,id:i32,accepted:bool,code:u32)->Result<(),&'static str>{self.fixed_context(conn)?;let(_,index,hash)=self.action.target.span.receipt_span();let n=conn.execute("UPDATE app_voice_im_read_status_actions SET state=?4,result_code=?5,updated_at=?6 WHERE id=?1 AND user_nonce=?2 AND state=0 AND request_id=?3 AND run_nonce=?7 AND scope_epoch=?8 AND target_index=?9 AND span_hash=?10",params![self.row,self.action.nonce.as_str(),id,if accepted{1}else{2},i64::from(code),chrono::Utc::now().to_rfc3339(),self.action.run.nonce(),self.action.target.span.receipt_context().binding.receipt_epoch(),index,hash.as_slice()]).map_err(|_|STORAGE)?;if n!=1||!self.live(){return Err(STATUS_SCOPE);}Ok(())}
    pub(in super::super) fn result(&self)->Result<ReadStatusView,&'static str>{self.action.database.with_conn(|conn|{self.fixed_context(conn)?;view(conn,self.row)})}
}
fn view(conn:&Connection,id:i64)->Result<ReadStatusView,&'static str>{conn.query_row("SELECT id,target_id,box_id,target_index,state,updated_at FROM app_voice_im_read_status_actions WHERE id=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,Vec<u8>>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?))).map_err(|_|STORAGE).and_then(|(id,target_id,box_id,index,state,updated_at)|{if updated_at.len()>64{return Err(STORAGE);}Ok(ReadStatusView{id,target_id,box_kind:kind(&box_id),target_index:index.to_string(),state:state_name(state)?,updated_at})})}
pub(in super::super) fn history(conn:&Connection,line:&str,limit:usize)->Result<Vec<ReadStatusView>,&'static str>{if !(1..=128).contains(&limit)||account_store::metadata(conn,line)?.is_none(){return Err(INVALID);}let mut q=conn.prepare("SELECT id FROM app_voice_im_read_status_actions WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;let ids=q.query_map(params![line,limit as i64],|r|r.get::<_,i64>(0)).map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;ids.into_iter().map(|id|view(conn,id)).collect()}
pub(in super::super) struct ReadStatusWait{action:Arc<ReadStatusAction>}
impl ReadStatusWait{pub(in super::super) fn original(action:Arc<ReadStatusAction>)->Self{Self{action}}}
impl Drop for ReadStatusWait{fn drop(&mut self){self.action.cancel();}}
