//! Received-position uncertainty journal. This is a separate remote WRITE,
//! never a read-interest timeout rule or a body/notification/SDK readiness grant.
//! Connected to the controlled IO/consumer path; own Actions remain required.
use super::*;
use super::cursor::GapCommitted;
use super::super::{im_receipt_policy::NativeImReceiptConfig,rpc_im::ImQuery,rpc_im_received::OwnedReceivedReply};
use std::sync::{Mutex,atomic::{AtomicU8,Ordering}};
const QUEUED:u8=0;
const WRITING:u8=1;
const SUBMITTED:u8=2;
const RESOLVED:u8=3;
const RETIRED:u8=4;
const CLOSED_WRITE:u8=5;
const UNRESOLVED:&str="app_account_im_receipt_uncertain";
const MAX_RECEIPTS_ACCOUNT:i64=128;
const MAX_RECEIPTS_TOTAL:i64=4096;
pub(in super::super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_read_receipts(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),
        target_index INTEGER NOT NULL CHECK(target_index>=0),
        span_hash BLOB NOT NULL CHECK(typeof(span_hash)='blob' AND length(span_hash)=32),
        attempt_nonce TEXT NOT NULL CHECK(length(attempt_nonce)=32),run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,
        state INTEGER NOT NULL CHECK(state IN(0,1,2,3)),request_id INTEGER CHECK(request_id>0),
        result_code INTEGER,updated_at TEXT NOT NULL,
        UNIQUE(line_id,revision,identity_key,box_id));") .map_err(|_|STORAGE)
}
pub(in super::super) fn delete_account(conn:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(conn,line)?.is_some(){return Err(SCOPE);}
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_im_read_receipts')",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if exists{conn.execute("DELETE FROM app_voice_im_read_receipts WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())
}
struct OriginalRequest{id:i32,deadline:Instant}
pub(in super::super) struct ReceivedWritePermit{
    context:ImInboxScope,database:Arc<Database>,query:Arc<ImQuery>,span_hash:[u8;32],nonce:Zeroizing<String>,row:i64,
    phase:AtomicU8,request:Mutex<Option<OriginalRequest>>,changed:tokio::sync::Notify,manual:Option<Arc<super::read_status::ManualReadStatusIntent>>,
}
impl fmt::Debug for ReceivedWritePermit{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalReceivedWritePermit(<redacted>)")}}
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|e|e.into_inner())}
impl ReceivedWritePermit{
    fn live_context(&self)->bool{self.context.gate.live()&&self.context.binding.matches(self.context.binding.owner(),&self.context.binding.scope())&&self.manual.as_ref().is_none_or(|manual|manual.live())}
    fn target(&self)->(&[u8],i64){match self.query.as_ref(){ImQuery::SetReceived{box_id,index}=>(box_id,*index),_=>unreachable!("private fixed received query")}}
    fn marker_current(&self,conn:&Connection)->Result<(),&'static str>{let(box_id,index)=self.target();let matching:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_read_receipts WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND box_id=?5 AND target_index=?6 AND span_hash=?7 AND attempt_nonce=?8 AND run_nonce=?9 AND scope_epoch=?10)",params![self.row,self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id,index,self.span_hash.as_slice(),self.nonce.as_str(),self.context.run.nonce(),self.context.binding.receipt_epoch()],|r|r.get(0)).map_err(|_|STORAGE)?;if !matching{return Err(SCOPE);}Ok(())}
    pub(in super::super) fn manual_result_for(&self,action:&super::read_status::ReadStatusAction)->Result<super::read_status::ReadStatusView,&'static str>{
        let manual=self.manual.as_ref().ok_or(SCOPE)?;if !manual.belongs_to(action)||!self.live_context()||self.phase.load(Ordering::SeqCst)!=RESOLVED{return Err(UNRESOLVED);}manual.result()
    }
    pub(in super::super) fn effective_deadline(&self,deadline:Instant)->Instant{self.manual.as_ref().map_or(deadline,|manual|deadline.min(manual.deadline()))}
    pub(in super::super) fn query(&self)->Arc<ImQuery>{self.query.clone()}
    pub(in super::super) fn binding(&self)->&Arc<ImBinding>{&self.context.binding}
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.context.gate.clone()}
    pub(in super::super) fn is_original_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        self.live_context()&&self.context.binding.matches(owner,scope)
    }
    pub(in super::super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        self.is_original_for(owner,scope)&&self.phase.load(Ordering::SeqCst)<RESOLVED
    }
    /// Bind one monotonic ORIGINAL native request ID/deadline before IO. No
    /// second allocation or equivalent query can reuse the same permit.
    pub(in super::super) fn arm_request(&self,id:i32,deadline:Instant)->Result<(),&'static str>{
        let deadline=self.effective_deadline(deadline);
        if id<=0||!self.live_context()||Instant::now()>=deadline||self.phase.load(Ordering::SeqCst)!=QUEUED{return Err(SCOPE);}
        let mut request=lock(&self.request);if request.is_some(){return Err(SCOPE);}
        self.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.validate(&tx)?;self.marker_current(&tx)?;
            let n=tx.execute("UPDATE app_voice_im_read_receipts SET request_id=?4 WHERE id=?1 AND attempt_nonce=?2 AND state=0 AND request_id IS NULL AND run_nonce=?3 AND scope_epoch=?5",
                params![self.row,self.nonce.as_str(),self.context.run.nonce(),id,self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
            if let Some(manual)=&self.manual{manual.arm(&tx,id)?;}
            if n!=1||!self.live_context()||Instant::now()>=deadline{return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)
        })?;
        if !self.live_context()||Instant::now()>=deadline{return Err(SCOPE);}*request=Some(OriginalRequest{id,deadline});Ok(())
    }
    pub(in super::super) fn request_is_current(&self,id:i32)->bool{
        self.live_context()&&lock(&self.request).as_ref().is_some_and(|r|r.id==id&&Instant::now()<r.deadline)
    }
    pub(in super::super) fn begin_write(&self,id:i32)->bool{
        // The uncertainty row already exists. Do not hold a SQLite mutex in
        // the nonblocking physical write gate; only the atomic phase changes.
        self.request_is_current(id)&&self.phase.compare_exchange(QUEUED,WRITING,Ordering::SeqCst,Ordering::SeqCst).is_ok()
    }
    pub(in super::super) fn submitted(&self)->bool{self.phase.compare_exchange(WRITING,SUBMITTED,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    pub(in super::super) fn is_retired(&self)->bool{self.phase.load(Ordering::SeqCst)>=RETIRED}
    /// Private cancellation provenance only: queued/read retirement never
    /// claims that this original WRITE closed IO or supplies a new grant.
    pub(in super::super) fn closed_original_io(&self)->bool{self.phase.load(Ordering::SeqCst)==CLOSED_WRITE}
    #[cfg(test)]
    pub(in super::super) fn has_submitted(&self)->bool{self.phase.load(Ordering::SeqCst)==SUBMITTED}
    pub(in super::super) fn cancel(&self){
        let original=loop{
            let phase=self.phase.load(Ordering::SeqCst);if phase>=RESOLVED{return;}
            let retired=if phase==QUEUED{RETIRED}else{CLOSED_WRITE};
            if self.phase.compare_exchange(phase,retired,Ordering::SeqCst,Ordering::SeqCst).is_ok(){break phase;}
        };
        if original==QUEUED{
            // A still-queued permit cannot have emitted any bytes. Failure to
            // record that fact remains fail-closed uncertain, never retries.
            let _=self.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.marker_current(&tx)?;tx.execute("UPDATE app_voice_im_read_receipts SET state=3,updated_at=?3 WHERE id=?1 AND attempt_nonce=?2 AND state=0",
                params![self.row,self.nonce.as_str(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;if let Some(manual)=&self.manual{manual.cancel_queued(&tx)?;}tx.commit().map_err(|_|STORAGE)});
        }else if matches!(original,WRITING|SUBMITTED){
            // Unlike a readonly fully-submitted request, cancellation of this
            // possible remote write keeps uncertainty and retires original IO.
            self.context.binding.scope().close();
        }
        self.changed.notify_waiters();
    }
    pub(in super::super) async fn cancelled(&self){loop{let changed=self.changed.notified();tokio::pin!(changed);changed.as_mut().enable();if self.is_retired(){return;}changed.await;}}
    /// The original actor calls this only with its matched private write reply,
    /// after whole-payload validation and before publishing a result to caller.
    /// It is outside all physical write gates and cannot create another write.
    pub(in super::super) fn resolve_original(&self,reply:&OwnedReceivedReply)->Result<(),&'static str>{
        let id=reply.request().id();if !reply.belongs_to(self)||!self.request_is_current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNRESOLVED);}
        let accepted=reply.accepted().map_err(|_|UNRESOLVED)?;let result_code=reply.result_code();
        let(box_id,index)=self.target();
        self.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.validate(&tx)?;self.marker_current(&tx)?;
            let n=tx.execute("UPDATE app_voice_im_read_receipts SET state=?8,result_code=?9,updated_at=?10 WHERE id=?1 AND attempt_nonce=?2 AND state=0 AND request_id=?3 AND run_nonce=?4 AND scope_epoch=?5 AND target_index=?6 AND span_hash=?7",
                params![self.row,self.nonce.as_str(),id,self.context.run.nonce(),self.context.binding.receipt_epoch(),index,self.span_hash.as_slice(),if accepted{1}else{2},i64::from(result_code),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
            // The cursor must still bind this current original identity/box;
            // a different run or a rewind never resolves/redeems this marker.
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND consumed>=?5 AND observed_run=?6 AND observed_epoch=?7)",
                params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id,index,self.context.run.nonce(),self.context.binding.receipt_epoch()],|r|r.get(0)).map_err(|_|STORAGE)?;
            if let Some(manual)=&self.manual{manual.resolve(&tx,id,accepted,result_code)?;}
            if n!=1||!exists||!self.request_is_current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNRESOLVED);}tx.commit().map_err(|_|STORAGE)
        })?;
        if !self.request_is_current(id){return Err(UNRESOLVED);}
        self.phase.compare_exchange(SUBMITTED,RESOLVED,Ordering::SeqCst,Ordering::SeqCst).map_err(|_|UNRESOLVED)?;Ok(())
    }
}
pub(in super::super) enum ReceivedAdmission{Fresh(Arc<ReceivedWritePermit>),AlreadyConfirmed}
/// Only the opaque continuous-consumption result can admit a received write.
/// No scalar/index/status/notification/raw read or public DTO is accepted here.
pub(in super::super) fn begin_received(database:Arc<Database>,span:&GapCommitted,policy:&NativeImReceiptConfig)->Result<ReceivedAdmission,&'static str>{
    policy.validate()?;match prepare_received(database,span,None)?{Prepared::Fresh(p)=>Ok(ReceivedAdmission::Fresh(p)),Prepared::AlreadyConfirmed=>Ok(ReceivedAdmission::AlreadyConfirmed),Prepared::ManualReplay(_)=>Err(SCOPE)}
}
pub(in super::super) enum ReadStatusPrepared{Fresh(Arc<ReceivedWritePermit>),Replay(super::read_status::ReadStatusView)}
pub(in super::super) fn begin_read_status(database:Arc<Database>,action:Arc<super::read_status::ReadStatusAction>,policy:&super::super::im_read_status_policy::NativeReadStatusConfig)->Result<ReadStatusPrepared,&'static str>{
    policy.validate()?;if !action.live(){return Err(SCOPE);}let span=action.span().clone();match prepare_received(database,&span,Some(action.clone()))?{Prepared::Fresh(p)=>{action.attach(&p);if !action.live(){p.cancel();return Err(SCOPE);}Ok(ReadStatusPrepared::Fresh(p))},Prepared::ManualReplay(v)=>Ok(ReadStatusPrepared::Replay(v)),Prepared::AlreadyConfirmed=>Err(SCOPE)}
}
enum Prepared{Fresh(Arc<ReceivedWritePermit>),AlreadyConfirmed,ManualReplay(super::read_status::ReadStatusView)}
fn prepare_received(database:Arc<Database>,span:&GapCommitted,manual:Option<Arc<super::read_status::ReadStatusAction>>)->Result<Prepared,&'static str>{
    if !span.is_current(&database){return Err(SCOPE);}
    let context=span.receipt_context();let(box_id,index,span_hash)=span.receipt_span();
    if index<0{return Err(INVALID);}let query=Arc::new(ImQuery::set_received(super::super::im_data::ImBox::parse(box_id).map_err(|_|INVALID)?,index).map_err(|_|INVALID)?);
    let nonce=Zeroizing::new(super::super::account_runtime::local_nonce()?);
    let row=database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;context.validate(&tx)?;span.validate_current(&tx)?;
        let manual_intent=if let Some(action)=&manual{match action.admit(&tx)?{super::read_status::ManualAdmission::Fresh(intent)=>Some(intent),super::read_status::ManualAdmission::Replay(view)=>{tx.commit().map_err(|_|STORAGE)?;return Ok((None,None,Some(view)));}}}else{None};
        // Changing local configuration revision does not acknowledge an old
        // remote WRITE to this same original native identity/Box.
        let unsettled:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_read_receipts WHERE line_id=?1 AND identity_key=?2 AND box_id=?3 AND state=0)",params![context.run.line_id(),context.binding.identity().as_slice(),box_id],|r|r.get(0)).map_err(|_|STORAGE)?;if unsettled{return Err(UNRESOLVED);}
        let old:Option<(i64,i64,i64)>=tx.query_row("SELECT id,target_index,state FROM app_voice_im_read_receipts WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4",
            params![context.run.line_id(),context.run.revision()as i64,context.binding.identity().as_slice(),box_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_|STORAGE)?;
        let row=if let Some((row,target,state))=old{
            if state==0{return Err(UNRESOLVED);}if manual.is_none()&&state==1&&target>=index{tx.commit().map_err(|_|STORAGE)?;return Ok((None,None,None));}
            if target>index||manual.is_none()&&target>=index{return Err("app_account_im_receipt_not_replayed");}
            tx.execute("UPDATE app_voice_im_read_receipts SET target_index=?2,span_hash=?3,attempt_nonce=?4,run_nonce=?5,scope_epoch=?6,state=0,request_id=NULL,result_code=NULL,updated_at=?7 WHERE id=?1 AND state!=0",
                params![row,index,span_hash.as_slice(),nonce.as_str(),context.run.nonce(),context.binding.receipt_epoch(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;row
        }else{
            let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_read_receipts",[context.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
            if local>=MAX_RECEIPTS_ACCOUNT||total>=MAX_RECEIPTS_TOTAL{return Err(FULL);}
            tx.execute("INSERT INTO app_voice_im_read_receipts(line_id,revision,identity_key,box_id,target_index,span_hash,attempt_nonce,run_nonce,scope_epoch,state,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,0,?10)",
                params![context.run.line_id(),context.run.revision()as i64,context.binding.identity().as_slice(),box_id,index,span_hash.as_slice(),nonce.as_str(),context.run.nonce(),context.binding.receipt_epoch(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;tx.last_insert_rowid()
        };
        if !context.gate.live()||!context.binding.matches(context.binding.owner(),&context.binding.scope()){return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)?;Ok((Some(row),manual_intent,None))
    })?;
    let(row,manual_intent,replay)=row;if let Some(replay)=replay{return Ok(Prepared::ManualReplay(replay));}
    let Some(row)=row else{return Ok(Prepared::AlreadyConfirmed);};
    Ok(Prepared::Fresh(Arc::new(ReceivedWritePermit{context,database,query,span_hash,nonce,row,phase:AtomicU8::new(QUEUED),request:Mutex::new(None),changed:tokio::sync::Notify::new(),manual:manual_intent})))
}
#[derive(Serialize)]
pub struct ReceivedReceiptView{pub id:i64,pub box_kind:&'static str,pub target_index:String,pub state:&'static str,pub updated_at:String}
impl fmt::Debug for ReceivedReceiptView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ReceivedReceiptView(<redacted>)")}}
pub(in super::super) fn metadata(conn:&Connection,line:&str,limit:usize)->Result<Vec<ReceivedReceiptView>,&'static str>{
    if !(1..=128).contains(&limit)||account_store::metadata(conn,line)?.is_none(){return Err(INVALID);}
    let mut q=conn.prepare("SELECT id,box_id,target_index,state,updated_at FROM app_voice_im_read_receipts WHERE line_id=?1 AND target_index>=0 AND typeof(updated_at)='text' AND length(CAST(updated_at AS BLOB))<=64 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=q.query_map(params![line,limit as i64],|r|{let box_id:Vec<u8>=r.get(1)?;let state:i64=r.get(3)?;Ok(ReceivedReceiptView{id:r.get(0)?,box_kind:if box_id.starts_with(b"P2P/"){"p2p"}else if box_id.starts_with(b"Org/"){"org"}else{"unknown"},target_index:r.get::<_,i64>(2)?.to_string(),state:match state{0=>"uncertain",1=>"confirmed",2=>"rejected",3=>"not_sent",_=>"unknown"},updated_at:r.get(4)?})}).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
pub(in super::super) struct ReceivedWait{permit:Arc<ReceivedWritePermit>}
impl ReceivedWait{pub(in super::super) fn original(permit:Arc<ReceivedWritePermit>)->Self{Self{permit}}}
impl Drop for ReceivedWait{fn drop(&mut self){self.permit.cancel();}}
