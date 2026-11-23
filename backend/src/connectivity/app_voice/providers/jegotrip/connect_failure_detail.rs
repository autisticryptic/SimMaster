//! Finite original-owner connector timeout steps; not endpoint/error values or grants.
use super::{rpc_pending::RpcConnectionScope,runtime_connector::ConnectError,session::AttemptOwner};
use serde::Serialize;
use std::sync::{Mutex,atomic::{AtomicU8,Ordering}};

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize)]
#[serde(rename_all="snake_case")]
pub enum ConnectFailureStep { Dns, Tcp }
impl ConnectFailureStep {
    fn byte(self)->u8{match self{Self::Dns=>1,Self::Tcp=>2}}
    fn from_byte(value:u8)->Option<Self>{match value{1=>Some(Self::Dns),2=>Some(Self::Tcp),_=>None}}
}
struct Recorded {owner:AttemptOwner,step:ConnectFailureStep}
/// Same original budget/scope holder; no scope reference, reset, addresses or clock.
pub(super) struct ConnectDiagnosticStore(Mutex<Option<Recorded>>);
impl ConnectDiagnosticStore {
    pub(super) fn original()->Self{Self(Mutex::new(None))}
    pub(super) fn record(&self,owner:&AttemptOwner,step:ConnectFailureStep){
        let mut first=self.0.lock().unwrap_or_else(|p|p.into_inner());
        if first.is_none(){*first=Some(Recorded{owner:owner.clone(),step});}
    }
    pub(super) fn for_owner(&self,owner:&AttemptOwner)->Option<ConnectFailureStep>{
        self.0.lock().unwrap_or_else(|p|p.into_inner()).as_ref().filter(|r|&r.owner==owner).map(|r|r.step)
    }
}
/// Stack-local step observer for ONE original connector future. It performs no
/// IO, owns no task/scope, and never extends the original shared deadline.
pub(super) struct ConnectAttempt<'a>{owner:&'a AttemptOwner,scope:&'a RpcConnectionScope,step:AtomicU8}
impl<'a> ConnectAttempt<'a>{
    pub(super) fn original(owner:&'a AttemptOwner,scope:&'a RpcConnectionScope)->Self{Self{owner,scope,step:AtomicU8::new(0)}}
    pub(super) fn entering(&self,step:ConnectFailureStep){self.step.store(step.byte(),Ordering::Release);}
    pub(super) fn finish(&self,error:ConnectError){
        if error==ConnectError::Timeout{
            if let Some(step)=ConnectFailureStep::from_byte(self.step.load(Ordering::Acquire)){
                self.scope.record_connect_timeout(self.owner,step);
            }
        }
    }
}
#[cfg(test)]
#[path="connect_failure_detail_tests.rs"]
mod tests;
