//! An internal READ interest, never account consent, a URI ownership proof or a
//! WRITE capability. Original actor admission and installed origin are separate.
use super::{batch_scheduler::AliasWait, im_inbox::ImReceiptGate, native_rpc_budget::NativeRpcReservation,
    rpc_pending::RpcConnectionScope, rpc_stream::CodecError, session::AttemptOwner};
use std::{fmt,sync::{Arc,atomic::{AtomicBool,Ordering}},time::Instant};

pub(super) struct AliasAction {
    owner: AttemptOwner, scope: RpcConnectionScope, gate: Arc<ImReceiptGate>, interest: Arc<ImReceiptGate>,
    deadline: Instant, claimed: AtomicBool,
    #[cfg(test)] test: std::sync::Mutex<Option<Arc<super::router_runtime::tests::client_read_io::ClientReadIoTest>>>,
    _charge: Arc<NativeRpcReservation>,
}
impl AliasAction {
    pub(super) fn capture(owner:&AttemptOwner,scope:&RpcConnectionScope,gate:Arc<ImReceiptGate>,deadline:Instant)->Result<Arc<Self>,CodecError>{
        let now=Instant::now();
        if !scope.is_open()||scope.session_revoked()||!gate.live()||now>=deadline||deadline.saturating_duration_since(now)>std::time::Duration::from_secs(30){return Err(CodecError::InvalidField);}
        let charge=scope.reserve_native_rpc(std::mem::size_of::<Self>()+owner.line_id().len()+256)?;
        Ok(Arc::new(Self{owner:owner.clone(),scope:scope.clone(),gate,interest:ImReceiptGate::fresh(),deadline,claimed:AtomicBool::new(false),
            #[cfg(test)]test:std::sync::Mutex::new(None),_charge:charge}))
    }
    pub(super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn live(&self)->bool{self.scope.is_open()&&!self.scope.session_revoked()&&self.gate.live()&&self.interest.live()&&Instant::now()<self.deadline}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{&self.owner==owner&&self.scope.matches(scope)&&self.live()}
    pub(super) fn claim(&self)->bool{self.live()&&self.claimed.compare_exchange(false,true,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    pub(super) fn cancel(&self){self.interest.retire();}
    pub(super) fn cancellation_gate(&self)->Arc<ImReceiptGate>{self.interest.clone()}
    pub(super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) async fn retired(&self){tokio::select!{biased;_=self.scope.session_ended()=>{},_=self.gate.retired()=>{},_=self.interest.retired()=>{}}}
    #[cfg(test)] pub(super) fn set_test(&self,test:Arc<super::router_runtime::tests::client_read_io::ClientReadIoTest>){assert!(self.test.lock().unwrap().replace(test).is_none());}
    #[cfg(test)] pub(super) fn test(&self)->Option<Arc<super::router_runtime::tests::client_read_io::ClientReadIoTest>>{self.test.lock().unwrap().clone()}
}
impl fmt::Debug for AliasAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AliasAction(<redacted>)")}}
pub(super) struct AliasActionWait{action:Arc<AliasAction>,done:bool}
impl AliasActionWait{pub(super) fn original(action:Arc<AliasAction>)->Self{Self{action,done:false}}pub(super) fn disarm(&mut self){self.done=true;}}
impl Drop for AliasActionWait{fn drop(&mut self){if !self.done{self.action.cancel();}}}
pub(super) struct AliasObservation{wait:AliasWait,action:Arc<AliasAction>}
impl AliasObservation{
    pub(super) fn original(wait:AliasWait,action:Arc<AliasAction>)->Result<Self,CodecError>{
        if !action.live()||wait.outcome()!=Some(Ok(())){return Err(CodecError::InvalidField);}Ok(Self{wait,action})
    }
    pub(super) fn pair(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,kind:&[u8],value:&[u8])->Option<bool>{
        if !self.action.matches(owner,scope){return None;}self.wait.observed_pair(owner,scope,kind,value)
    }
    pub(super) fn current(&self)->bool{self.action.live()&&self.wait.outcome()==Some(Ok(()))}
    pub(super) fn expired(&self)->bool{Instant::now()>=self.action.deadline()||matches!(self.wait.sampled_outcome(),super::batch_scheduler::AliasOutcome::Expired)}
}
impl fmt::Debug for AliasObservation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AliasObservation(<redacted>)")}}
