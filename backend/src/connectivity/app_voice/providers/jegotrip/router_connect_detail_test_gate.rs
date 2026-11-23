//! Test-only scheduling hold for one explicitly armed physical Router policy.
//! Never compiled into deployment binaries; no endpoint/identity/trust override,
//! new task, global registry, replacement DNS stack or renewed deadline.
use super::{connect_failure_detail::ConnectFailureStep,router_connector::poll_before,
    rpc_pending::RpcConnectionScope,runtime_connector::ConnectError,session::AttemptOwner};
use std::sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}};
use tokio::sync::Notify;

pub(crate) struct RouterConnectTestGate {
    step:ConnectFailureStep,
    original:Mutex<Option<(AttemptOwner,RpcConnectionScope)>>,
    entered:AtomicBool,released:AtomicBool,changed:Notify,
}
impl RouterConnectTestGate {
    pub(crate) fn arm(step:ConnectFailureStep)->Arc<Self>{
        Arc::new(Self{step,original:Mutex::new(None),entered:AtomicBool::new(false),released:AtomicBool::new(false),changed:Notify::new()})
    }
    pub(crate) async fn pause(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,step:ConnectFailureStep,deadline:std::time::Instant)->Result<(),ConnectError>{
        if step!=self.step||self.released.load(Ordering::Acquire){return Ok(());}
        {
            let mut original=self.original.lock().unwrap();
            if let Some((who,source))=original.as_ref(){
                if who!=owner||!source.matches(scope){return Ok(());}
            }else{*original=Some((owner.clone(),scope.clone()));}
        }
        self.entered.store(true,Ordering::Release);self.changed.notify_waiters();
        poll_before(scope,async{
            loop{
                let notified=self.changed.notified();tokio::pin!(notified);notified.as_mut().enable();
                if self.released.load(Ordering::Acquire){return;}
                notified.await;
            }
        },deadline).await
    }
    pub(crate) async fn entered(&self){
        loop{
            let notified=self.changed.notified();tokio::pin!(notified);notified.as_mut().enable();
            if self.entered.load(Ordering::Acquire){return;}
            notified.await;
        }
    }
    pub(crate) fn original_scope(&self)->Option<(AttemptOwner,RpcConnectionScope)>{self.original.lock().unwrap().clone()}
    pub(crate) fn release(&self){self.released.store(true,Ordering::Release);self.changed.notify_waiters();}
}
