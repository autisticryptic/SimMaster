//! Owning native call heartbeat worker; never acquires/replaces an SDK account.
use super::*;
use super::super::call_control::OwnedCallOperationReply;
use zeroize::Zeroizing;
impl CallService{
    pub(super) async fn accept_control(self:&Arc<Self>,call:&Arc<Call>,body:&[u8])->Result<OwnedCallOperationReply,&'static str>{
        let fields={let mut state=lock(&call.native_timing);let timing=state.as_mut().ok_or(SCOPE)?;
            let fields=timing.accept_properties(&call.ticket,body)?.iter().map(|p|(p.key.to_vec(),Zeroizing::new(p.value.to_vec()))).collect::<Vec<_>>();
            timing.begin_accept(&call.ticket,Instant::now())?;fields};
        let scope=call.ticket.connection_scope();let properties=fields.iter().map(|(key,value)|RpcProperty{key,value}).collect::<Vec<_>>();
        let result=self.backend.call(call.ticket.owner(),&scope,&call.ticket,CallOperation::Accept,&properties).await.map_err(call_error)?;
        lock(&call.native_timing).as_mut().ok_or(SCOPE)?.accepted(&call.ticket,&result,Instant::now())?;
        self.start_native_keepalive(call);
        Ok(result)
    }
    fn start_native_keepalive(self:&Arc<Self>,call:&Arc<Call>){
        let mut slot=call.keepalive.try_lock().expect("original unstarted native call heartbeat");
        debug_assert!(slot.is_none());let weak=Arc::downgrade(self);let original=call.clone();
        *slot=Some(tokio::spawn(async move{native_keepalive(weak,original).await;}));
    }
    pub(super) fn native_media_connected(&self,call:&Call)->Result<(),&'static str>{
        if let Some(timing)=lock(&call.native_timing).as_mut(){timing.connected(&call.ticket,Instant::now())?;}
        call.changed.notify_waiters();Ok(())
    }
    pub(super) async fn join_native_keepalive(&self,call:&Call)->Result<(),&'static str>{
        let mut slot=call.keepalive.lock().await;
        let result=if let Some(task)=slot.as_mut(){task.await.map_err(|_|"app_account_call_cleanup_unconfirmed")}else{Ok(())};
        slot.take();result
    }
}
async fn native_keepalive(service:Weak<CallService>,call:Arc<Call>){
    let result=native_keepalive_loop(&service,&call).await;
    if let Err(error)=result{
        if !call.cancelled.load(Ordering::SeqCst)&&!call.ticket.remote_released(){
            {let mut state=lock(&call.state);if state.error.is_none(){state.error=Some(error);}}
            call.retire();
        }
    }
}
async fn native_keepalive_loop(service:&Weak<CallService>,call:&Call)->Result<(),&'static str>{
    loop{
        // Subscribe before looking at timer/connected state; don't lose the
        // original MediaReady wake if it races the first deadline calculation.
        let changed=call.changed.notified();tokio::pin!(changed);changed.as_mut().enable();
        if !call.ticket.is_live(){return Ok(());}
        let next=lock(&call.native_timing).as_ref().ok_or(SCOPE)?.next(&call.ticket,Instant::now())?;
        tokio::select!{biased;
            _=call.ticket.revoked()=>return Ok(()),
            _=async{if let Some(next)=next{tokio::time::sleep_until(next.into()).await}else{std::future::pending::<()>().await}}=>{},
            _=changed=>{},
        }
        if !call.ticket.is_live(){return Ok(());}
        let current=service.upgrade().ok_or(SCOPE)?;
        if !current.active(call){return Err(SCOPE);}
        {let state=lock(&call.state);if state.phase==Phase::Connected&&state.media.as_ref().is_none_or(|media|!media.scope().is_open()){return Err(SCOPE);}}
        let backend=current.backend.clone();drop(current); // no self-owning service cycle across IO
        let fields=lock(&call.native_timing).as_mut().ok_or(SCOPE)?.take_due(&call.ticket,Instant::now())?;
        let Some(fields)=fields else{continue;};
        let properties=fields.iter().map(|(key,value)|RpcProperty{key,value}).collect::<Vec<_>>();
        // Exactly one original pending heartbeat. The existing RPC deadline is
        // not extended; close joins it rather than detaching or retrying it.
        backend.call(call.ticket.owner(),&call.ticket.connection_scope(),&call.ticket,CallOperation::KeepAlive,&properties).await.map_err(call_error)?;
    }
}
