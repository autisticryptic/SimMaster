//! Original read-only interest lifetime, not a serializable IO/reply grant.
//! Queued cancel cannot write; cancel during a possibly partial frame closes
//! the same scope; cancel after complete submission only retires reply interest.
use super::{im_inbox::ImReceiptGate,rpc_pending::RpcConnectionScope};
use std::{fmt,sync::{Arc,atomic::{AtomicU8,Ordering}}};
const QUEUED:u8=0;
const WRITING:u8=1;
const SUBMITTED:u8=2;
const RETIRED:u8=3;
pub(super) struct ImReadInterest{state:AtomicU8,scope:RpcConnectionScope,changed:tokio::sync::Notify}
impl fmt::Debug for ImReadInterest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImReadInterest(<redacted>)")}}
impl ImReadInterest{
    pub(super) fn original(scope:&RpcConnectionScope)->Arc<Self>{Arc::new(Self{state:AtomicU8::new(QUEUED),scope:scope.clone(),changed:tokio::sync::Notify::new()})}
    pub(super) fn begin_write(&self,gate:&ImReceiptGate)->bool{
        if !gate.live()||!self.scope.is_open()||self.scope.session_revoked(){self.cancel();return false;}
        self.state.compare_exchange(QUEUED,WRITING,Ordering::SeqCst,Ordering::SeqCst).is_ok()
    }
    pub(super) fn submitted(&self)->bool{self.state.compare_exchange(WRITING,SUBMITTED,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    pub(super) fn cancel(&self){
        let original=self.state.swap(RETIRED,Ordering::SeqCst);
        if original==WRITING{self.scope.close();}
        self.changed.notify_waiters();
    }
    pub(super) fn is_retired(&self)->bool{self.state.load(Ordering::SeqCst)==RETIRED}
    #[cfg(test)]
    pub(super) fn is_writing(&self)->bool{self.state.load(Ordering::SeqCst)==WRITING}
    pub(super) async fn retired(&self){loop{let changed=self.changed.notified();tokio::pin!(changed);changed.as_mut().enable();if self.is_retired(){return;}changed.await;}}
}
pub(super) struct ImReadWait{interest:Arc<ImReadInterest>}
impl ImReadWait{pub(super) fn original(interest:Arc<ImReadInterest>)->Self{Self{interest}}}
impl Drop for ImReadWait{fn drop(&mut self){self.interest.cancel();}}
