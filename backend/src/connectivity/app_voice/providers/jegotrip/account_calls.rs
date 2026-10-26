//! Original-run basic incoming-call supervisor. Notifications, native semantic
//! results, actual media ownership and API/Trunk connected state stay separate.
//! No outbound SDK/IMS/CS/cost lookup or new owner is synthesized on delivery.
use super::{
    account_call_history::{self as history, HistoryTicket}, account_runs::{self, RunToken}, account_store,
    call_control::CallControlTicket, call_delivery::IncomingCallDelivery, call_settings::CallSettings,
    media_endpoint::MediaEndpointPlan, native_audio_sdp::PlainAudioOffer, rpc_calls::{CallNoticeKind, CallOperation},
    runtime_backend::NativeHostingBackend, session::AttemptOwner, rpc_pending::RpcConnectionScope,
    tcp_mpath_audio::{self as audio, AudioHandle, BoundAudioSocket}, rpc_stream::RpcProperty, transport_actor::RuntimeError,
};
use crate::{platform::db::Database, services::trunk::{bridge::{OperatorCommand, OperatorEvent}, operator::OperatorLink, runtime::TrunkRuntime}};
use serde::Serialize;
use std::{collections::BTreeMap, fmt, future::Future, pin::Pin, sync::{Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}}, task::Poll, time::Instant};
use tokio::{sync::{broadcast, oneshot, Mutex as AsyncMutex}, task::JoinHandle};
const SCOPE: &str = "app_account_call_scope_invalid";
const UNAVAILABLE: &str = "app_account_call_runtime_unavailable";
const MAX_CALLS: usize = 128;
const MAX_ACTIVE: usize = 16;
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { value.lock().unwrap_or_else(|poison| poison.into_inner()) }
fn call_error(error:RuntimeError)->&'static str{match error{
    RuntimeError::CallRejected=>"app_account_call_rejected",RuntimeError::CallNotReady=>"app_account_call_not_ready",
    RuntimeError::Backpressure=>"app_account_call_backpressure",RuntimeError::Timeout=>"app_account_call_timeout",
    RuntimeError::Protocol=>"app_account_call_protocol_invalid",_=>SCOPE,
}}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Checking, Ringing, Accepting, Connected, Ending, Ended, Failed }
impl Phase { fn name(self) -> &'static str { match self { Self::Checking=>"checking",Self::Ringing=>"ringing",Self::Accepting=>"accepting",Self::Connected=>"connected",Self::Ending=>"ending",Self::Ended=>"ended",Self::Failed=>"failed" } } }
struct State { phase: Phase, error: Option<&'static str>, socket: Option<BoundAudioSocket>, local:Option<std::net::SocketAddr>, media: Option<Arc<AudioHandle>>, connected: Option<Instant>, release_attempted:bool, accept_queued:bool, end_queued:bool }
struct Call { id: String, ticket: CallControlTicket, offer: PlainAudioOffer, caller: String, history: HistoryTicket,
    received: u64, cancelled: AtomicBool, operation: AsyncMutex<()>, state: Mutex<State>, watch: AsyncMutex<Option<JoinHandle<()>>> }
impl Call {
    fn retire(&self) { self.cancelled.store(true,Ordering::SeqCst); self.ticket.revoke(); let mut state=lock(&self.state);
        state.socket.take(); if let Some(media)=&state.media { media.retire(); } }
    fn view(&self) -> CallView {
        let(phase,media_active,local,error)={let state=lock(&self.state);(state.phase,
            state.phase==Phase::Connected&&self.ticket.is_live()&&state.media.as_ref().is_some_and(|media|media.scope().is_open()),state.local,state.error)};
        // SDP rendering does not retain the call-state lock or affect action
        // admission. Read-only polling cannot reject a legitimate answer.
        let sdp=local.and_then(|local|self.offer.with_local_endpoint(&self.ticket,local).ok()).and_then(|body|std::str::from_utf8(&body).ok().map(str::to_owned));
        CallView{id:self.id.clone(),history_id:self.history.id(),phase:phase.name(),received_at_unix_seconds:self.received,peer:self.caller.clone(),peer_source:"native_invitation_peer",
            media_active:media_active&&self.ticket.is_live(),sdp,last_error:error}
    }
}
#[derive(Clone, Serialize)]
pub struct CallView { pub id:String,pub history_id:i64,pub phase:&'static str,pub received_at_unix_seconds:u64,pub peer:String,
    pub peer_source:&'static str,pub media_active:bool,pub sdp:Option<String>,pub last_error:Option<&'static str> }
impl fmt::Debug for CallView { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("AppCallView(<redacted>)")} }
struct Binding { owner:AttemptOwner,scope:RpcConnectionScope }
pub(super) struct CallService { database:Arc<Database>,run:Arc<RunToken>,backend:Arc<NativeHostingBackend>,settings:CallSettings,expiry:Option<u64>,
    bound:Mutex<Option<Binding>>,accepting:AtomicBool,quiescent:AtomicBool,changed:tokio::sync::Notify,calls:Mutex<BTreeMap<String,Arc<Call>>>,
    tasks:AsyncMutex<Vec<JoinHandle<()>>>,commands:AsyncMutex<Option<JoinHandle<()>>>,operator:OperatorLink,trunk:TrunkRuntime }
impl fmt::Debug for CallService { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("CallService(<redacted>)")} }
impl CallService {
    pub(super) fn new(database:Arc<Database>,run:Arc<RunToken>,backend:Arc<NativeHostingBackend>,settings:CallSettings,expiry:Option<u64>)->Arc<Self> {
        let operator=OperatorLink::default();operator.set_ready(false);
        let trunk=TrunkRuntime::with_operator(operator.clone());
        Arc::new(Self {database,run,backend,settings,expiry,bound:Mutex::new(None),accepting:AtomicBool::new(false),quiescent:AtomicBool::new(false),changed:tokio::sync::Notify::new(),
            calls:Mutex::new(BTreeMap::new()),tasks:AsyncMutex::new(Vec::new()),commands:AsyncMutex::new(None),operator,trunk})
    }
    fn configuration_current(&self)->bool { if self.expiry.is_some_and(|at|at<=super::account_runtime::unix_now()){return false;}
        self.database.with_conn(|conn| {
        let metadata=account_store::metadata(conn,self.run.line_id())?.ok_or(SCOPE)?;
        Ok(metadata.revision==self.run.revision()&&account_runs::runtime_matches(conn,&self.run)?)
    }).unwrap_or(false) }
    fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool { lock(&self.bound).as_ref()
        .is_some_and(|bound|bound.owner==*owner&&bound.scope.matches(scope)&&scope.is_open())&&self.configuration_current() }
    pub(super) fn ready(&self)->bool {
        if !self.accepting.load(Ordering::SeqCst)||!self.operator.incoming_call_allowed()||!self.configuration_current(){return false;}
        let current=lock(&self.bound).as_ref().is_some_and(|bound|bound.scope.is_open()
            &&self.backend.original_call_receiver_ready(&bound.owner,&bound.scope));
        current&&self.operator.has_command_consumer()&&(!self.settings.trunk.as_ref().is_some_and(|profile|profile.enabled)||self.operator.trunk_local_ip().is_some())
    }
    pub(super) async fn start(self:&Arc<Self>,owner:&AttemptOwner,scope:&RpcConnectionScope)->Result<(),&'static str> {
        if !scope.is_open()||!self.configuration_current()||!self.backend.original_call_receiver_ready(owner,scope){return Err(SCOPE);}
        {let mut bound=lock(&self.bound);if let Some(original)=bound.as_ref(){
            if original.owner==*owner&&original.scope.matches(scope){return Ok(());}return Err(SCOPE);
        }*bound=Some(Binding{owner:owner.clone(),scope:scope.clone()});}
        self.accepting.store(true,Ordering::SeqCst);
        let receiver=self.operator.subscribe_commands();let weak=Arc::downgrade(self);let mut command=self.commands.lock().await;
        *command=Some(tokio::spawn(command_loop(weak,receiver)));drop(command);
        if let Some(profile)=&self.settings.trunk{self.trunk.activate_profile(profile).await;}
        if !self.matches(owner,scope){self.retire();return Err(SCOPE);}
        self.accepting.store(true,Ordering::SeqCst);self.operator.set_ready(true);Ok(())
    }
    pub(super) fn retire(&self) { self.accepting.store(false,Ordering::SeqCst);self.changed.notify_waiters();self.operator.set_ready(false);
        for call in lock(&self.calls).values(){call.retire();} }
    async fn stopped(&self){loop{let wake=self.changed.notified();tokio::pin!(wake);wake.as_mut().enable();if !self.accepting.load(Ordering::SeqCst){return;}wake.await;}}
    fn find(&self,id:&str)->Result<Arc<Call>,&'static str>{lock(&self.calls).get(id).cloned().ok_or("app_account_call_not_found")}
    fn original(&self,ticket:&CallControlTicket)->bool{self.matches(ticket.owner(),&ticket.connection_scope())}
    fn active(&self,call:&Call)->bool{self.accepting.load(Ordering::SeqCst)&&self.operator.incoming_call_allowed()&&self.original(&call.ticket)&&call.ticket.is_live()&&!call.cancelled.load(Ordering::SeqCst)}
    pub(super) fn list(&self)->Result<Vec<CallView>,&'static str>{if !self.configuration_current(){return Err(SCOPE);}Ok(lock(&self.calls).values().map(|call|call.view()).collect())}
    pub(super) fn get(&self,id:&str)->Result<CallView,&'static str>{if !self.configuration_current(){return Err(SCOPE);}Ok(self.find(id)?.view())}
    pub(super) fn operator(&self)->OperatorLink{self.operator.clone()}
    pub(super) async fn trunk_status(&self)->crate::services::trunk::runtime::TrunkRuntimeStatus{self.trunk.status().await}
    pub(super) async fn delivery(self:&Arc<Self>,incoming:&IncomingCallDelivery)->Result<(),&'static str>{
        let owner=incoming.owner();let scope=incoming.connection_scope();
        if !self.matches(owner,&scope){return Err(SCOPE);}
        match incoming.notice().kind(){
            CallNoticeKind::Released=>{
                if let Some(ticket)=incoming.control_ticket(){
                    let call=lock(&self.calls).values().find(|call|call.ticket.same_call(ticket)).cloned();
                    if let Some(call)=call{call.retire();let weak=Arc::downgrade(self);self.spawn_task(async move{if let Some(service)=weak.upgrade(){let _guard=call.operation.lock().await;let _=service.finish(&call,false).await;}}).await;}
                }
                self.backend.complete_call(owner,&scope,incoming.ticket(),true,&[]).await.map_err(|_|SCOPE)?;Ok(())
            }
            CallNoticeKind::Accepted=>{self.backend.complete_call(owner,&scope,incoming.ticket(),true,&[]).await.map_err(|_|SCOPE)?;Ok(())}
            CallNoticeKind::Invited=>{
                if !self.ready(){self.backend.complete_call(owner,&scope,incoming.ticket(),false,&[]).await.map_err(|_|SCOPE)?;return Ok(());}
                let Some(ticket)=incoming.control_ticket()else{
                    self.backend.complete_call(owner,&scope,incoming.ticket(),true,&[]).await.map_err(|_|SCOPE)?;return Ok(());
                };
                let allow=self.settings.trunk.as_ref().map(|profile|profile.codec_allow.as_slice()).unwrap_or(&[]);
                let offer=match PlainAudioOffer::native(incoming.notice(),ticket,true).and_then(|offer|offer.restrict(allow)){Ok(offer)=>offer,Err(_)=>{
                    self.backend.complete_call(owner,&scope,incoming.ticket(),false,&[]).await.map_err(|_|SCOPE)?;return Ok(());}};
                let data=incoming.notice().data_for(owner,&scope).ok_or(SCOPE)?;
                let caller=data.peer.and_then(|value|std::str::from_utf8(value).ok()).filter(|value|value.len()<=128&&value.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'+'|b'-'|b'_'|b'.'))).unwrap_or("").to_owned();
                let id=super::account_runtime::local_nonce()?;
                let active=lock(&self.calls).values().filter(|call|!matches!(lock(&call.state).phase,Phase::Ended|Phase::Failed)).count();
                if active>=MAX_ACTIVE{
                    self.backend.complete_call(owner,&scope,incoming.ticket(),false,&[]).await.map_err(|_|SCOPE)?;return Ok(());
                }
                let evict={let calls=lock(&self.calls);if calls.len()>=MAX_CALLS{
                    calls.values().find(|call|lock(&call.state).phase==Phase::Ended).cloned()
                }else{None}};
                if let Some(old)=evict{
                    let mut watch=old.watch.lock().await;if let Some(task)=watch.as_mut(){let _=task.await;}watch.take();
                    lock(&self.calls).remove(&old.id);
                }
                if lock(&self.calls).len()>=MAX_CALLS{
                    self.backend.complete_call(owner,&scope,incoming.ticket(),false,&[]).await.map_err(|_|SCOPE)?;return Ok(());
                }
                let history=history::insert(&self.database,&self.run,&id,&caller)?;
                let call=Arc::new(Call{id:id.clone(),ticket:ticket.captured_copy(),offer,caller,history,received:super::account_runtime::unix_now(),cancelled:AtomicBool::new(false),
                    operation:AsyncMutex::new(()),state:Mutex::new(State{phase:Phase::Checking,error:None,socket:None,local:None,media:None,connected:None,release_attempted:false,accept_queued:false,end_queued:false}),watch:AsyncMutex::new(None)});
                lock(&self.calls).insert(id,call.clone());
                if self.backend.complete_call(owner,&scope,incoming.ticket(),true,&[]).await.is_err(){call.retire();return Err(SCOPE);}
                let weak=Arc::downgrade(self);let admitted=self.spawn_task(async move{if let Some(service)=weak.upgrade(){
                    let _guard=call.operation.lock().await;if let Err(code)=service.ring(&call).await{lock(&call.state).error=Some(code);let _=service.finish(&call,true).await;}
                }}).await;if !admitted{return Err(UNAVAILABLE);}Ok(())
            }
        }
    }
    async fn spawn_task<F>(&self,future:F)->bool where F:Future<Output=()>+Send+'static {
        if !self.accepting.load(Ordering::SeqCst){return false;}
        let mut tasks=self.tasks.lock().await;let mut index=0;
        while index<tasks.len(){if tasks[index].is_finished(){
            let mut task=tasks.swap_remove(index);if (&mut task).await.is_err(){self.retire();}
        }else{index+=1;}}
        if !self.accepting.load(Ordering::SeqCst)||tasks.len()>=512{return false;}
        tasks.push(tokio::spawn(future));true
    }
    async fn publish_state<T>(&self,call:&Call,work:impl FnOnce(&mut State)->T)->Result<T,&'static str>{
        let deadline=Instant::now()+std::time::Duration::from_millis(250);let mut work=Some(work);
        loop{
            if !self.active(call){return Err(SCOPE);}
            let published=call.ticket.with_media_gate(||{
                let Ok(mut state)=call.state.try_lock()else{return None;};
                if call.cancelled.load(Ordering::SeqCst){return None;}
                Some(work.take().expect("one original publication")(&mut state))
            }).ok_or(SCOPE)?;
            if let Some(value)=published{return Ok(value);}
            if Instant::now()>=deadline{return Err("app_account_call_timeout");}
            tokio::task::yield_now().await;
        }
    }
    async fn ring(&self,call:&Arc<Call>)->Result<(),&'static str>{
        if !self.active(call){return Err(SCOPE);}
        let scope=call.ticket.connection_scope();let result=self.backend.call(call.ticket.owner(),&scope,&call.ticket,CallOperation::Inform,&[]).await.map_err(call_error)?;
        if !result.accepted()||!self.active(call){return Err("app_account_call_rejected");}
        let socket=BoundAudioSocket::bind(&call.ticket,self.settings.bind_ip).await.map_err(|error|error.code())?;
        let address=socket.address(&call.ticket).ok_or(SCOPE)?;let body=call.offer.with_local_endpoint(&call.ticket,address).map_err(|error|error.code())?;
        if !self.active(call){return Err(SCOPE);}
        let mut socket=Some(socket);
        self.publish_state(call,|state|{state.socket=socket.take();state.local=Some(address);state.phase=Phase::Ringing;}).await?;
        call.ticket.with_media_gate(||self.operator.send_event(OperatorEvent::Incoming{call_id:call.id.clone(),caller:format!("sip:{}@app-account.invalid",if call.caller.is_empty(){"unknown"}else{call.caller.as_str()}),body:body.to_vec()})).ok_or(SCOPE)?;Ok(())
    }
    pub(super) async fn accept(self:&Arc<Self>,id:&str,body:Vec<u8>)->Result<OperationWaiter,&'static str>{
        let call=self.find(id)?;if !self.active(&call)||lock(&call.state).phase!=Phase::Ringing{return Err(SCOPE);}
        let answer=PlainAudioOffer::trunk(&body,true).map_err(|error|error.code())?;call.offer.matching_payloads(&answer).map_err(|error|error.code())?;
        {let mut state=lock(&call.state);if state.phase!=Phase::Ringing||state.accept_queued{return Err("app_account_call_operation_pending");}state.accept_queued=true;}
        let(sender,receiver)=oneshot::channel();let service=self.clone();let original=call.clone();
        let admitted=self.spawn_task(async move{let _guard=original.operation.lock().await;
            let result=service.accept_original(&original,answer).await;
            if let Err(code)=result{lock(&original.state).error=Some(code);let _=service.finish(&original,true).await;}
            let _=sender.send(result.map(|_|original.view()));
        }).await;
        if !admitted{return Err(SCOPE);}
        Ok(OperationWaiter{call,service:Arc::downgrade(self),receiver,delivered:false})
    }
    async fn accept_original(self:&Arc<Self>,call:&Arc<Call>,answer:PlainAudioOffer)->Result<(),&'static str>{
        if !self.active(call){return Err(SCOPE);}let socket={let mut state=lock(&call.state);
            if state.phase!=Phase::Ringing{return Err(SCOPE);}state.phase=Phase::Accepting;state.socket.take().ok_or(SCOPE)?};
        let address=socket.address(&call.ticket).ok_or(SCOPE)?;let body=answer.with_local_endpoint(&call.ticket,address).map_err(|error|error.code())?;
        let scope=call.ticket.connection_scope();let result=self.backend.call(call.ticket.owner(),&scope,&call.ticket,CallOperation::Accept,&[RpcProperty{key:b"RseEx.SDP",value:&body}]).await.map_err(call_error)?;
        if !result.accepted()||!self.active(call){return Err("app_account_call_rejected");}
        let plan=MediaEndpointPlan::prepare_mpath(&result,&call.ticket,self.settings.profile.clone()).map_err(|error|error.code())?;
        let relay=socket.for_peer(&call.ticket,&call.offer,&answer,Some(self.operator.media_metrics())).map_err(|error|error.code())?;
        let(handle,ready)=audio::spawn(plan,self.settings.connector.clone(),relay).map_err(|error|error.code())?;let handle=Arc::new(handle);
        lock(&call.state).media=Some(handle.clone());
        let ready=ready.await.map_err(|error|error.code())?;
        if !self.active(call)||!ready.original(&call.ticket,&handle.scope()){return Err(SCOPE);}
        self.publish_state(call,|state|{state.phase=Phase::Connected;state.connected=Some(Instant::now());}).await?;
        history::connected(&self.database,&call.history)?;
        call.ticket.with_media_gate(||self.operator.send_event(OperatorEvent::Connected{call_id:call.id.clone()})).ok_or(SCOPE)?;
        let weak=Arc::downgrade(self);let original=call.clone();let mut watch=call.watch.lock().await;
        *watch=Some(tokio::spawn(async move{handle.scope().closed().await;if let Some(service)=weak.upgrade(){
            let _guard=original.operation.lock().await;let _=service.finish(&original,true).await;
        }}));Ok(())
    }
    pub(super) async fn hangup(self:&Arc<Self>,id:&str)->Result<OperationWaiter,&'static str>{
        let call=self.find(id)?;if !self.original(&call.ticket){return Err(SCOPE);}
        {let mut state=lock(&call.state);if state.end_queued{return Err("app_account_call_operation_pending");}state.end_queued=true;}
        call.retire();
        let(sender,receiver)=oneshot::channel();let service=self.clone();let original=call.clone();let admitted=self.spawn_task(async move{
            let _guard=original.operation.lock().await;let result=service.finish(&original,true).await.map(|_|original.view());let _=sender.send(result);
        }).await;if !admitted{return Err(SCOPE);}Ok(OperationWaiter{call,service:Arc::downgrade(self),receiver,delivered:false})
    }
    async fn finish(&self,call:&Call,release:bool)->Result<(),&'static str>{
        let(media,answered,duration)={let mut state=lock(&call.state);if state.phase==Phase::Ended{return Ok(());}
            let answered=state.connected.is_some();let duration=state.connected.map(|at|at.elapsed().as_secs().min(i64::MAX as u64)as i64).unwrap_or(0);
            state.phase=Phase::Ending;state.socket.take();(state.media.clone(),answered,duration)};
        call.ticket.revoke();
        if let Some(media)=media{let closed=media.close().await;if !closed.io_released{return Err("app_account_call_cleanup_unconfirmed");}}
        let scope=call.ticket.connection_scope();let mut failed=None;
        let attempt_release={let mut state=lock(&call.state);let attempt=release&&scope.is_open()&&!call.ticket.remote_released()&&!state.release_attempted;
            if attempt{state.release_attempted=true;}attempt};
        if attempt_release{match self.backend.call(call.ticket.owner(),&scope,&call.ticket,CallOperation::Release,&[]).await{
            Ok(result)if result.accepted()=>{},Ok(_)=>failed=Some("app_account_call_rejected"),Err(_)=>failed=Some("app_account_call_release_failed"),}}
        history::ended(&self.database,&call.history,duration,answered)?;
        {let mut state=lock(&call.state);state.phase=Phase::Ended;if failed.is_some(){state.error=failed;}}
        self.operator.send_event(OperatorEvent::Ended{call_id:call.id.clone()});failed.map_or(Ok(()),Err)
    }
    pub(super) async fn close(self:&Arc<Self>)->Result<(),&'static str>{
        self.retire();
        let mut commands=self.commands.lock().await;if let Some(task)=commands.as_mut(){let _=task.await;}commands.take();drop(commands);
        self.trunk.activate_profile(&crate::platform::config::TrunkProfileConfig::default()).await;
        // Original operation tasks retain their handles across cancellation.
        // They can finish/reject, but retirement prevents new media owners.
        let mut tasks=self.tasks.lock().await;for task in tasks.iter_mut(){let _=task.await;}tasks.clear();drop(tasks);
        let calls=lock(&self.calls).values().cloned().collect::<Vec<_>>();let mut failed=None;let mut complete=true;
        for call in calls{
            {let _guard=call.operation.lock().await;if let Err(code)=self.finish(&call,true).await{
                if code=="app_account_call_cleanup_unconfirmed"{complete=false;}failed.get_or_insert(code);
            }}
            let mut watch=call.watch.lock().await;if let Some(task)=watch.as_mut(){let _=task.await;}watch.take();
        }
        self.quiescent.store(complete,Ordering::SeqCst);
        failed.map_or(Ok(()),Err)
    }
    pub(super) fn resources_released(&self)->bool{self.quiescent.load(Ordering::SeqCst)}
}
impl Drop for CallService{fn drop(&mut self){self.retire();}}
/// A queued connected result is not a transferable or cached readiness grant.
/// Recheck its original service and physical media when the caller consumes it;
/// a confirmed Ended receipt remains deliverable after resource retirement.
pub(super) struct OperationWaiter {
    call: Arc<Call>, service: Weak<CallService>,
    receiver: oneshot::Receiver<Result<CallView, &'static str>>, delivered: bool,
}
impl Future for OperationWaiter {
    type Output = Result<CallView, &'static str>;
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let result = match Pin::new(&mut this.receiver).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result.unwrap_or(Err(SCOPE)),
        };
        this.delivered = true;
        if result.is_err() { this.call.retire(); }
        if result.as_ref().is_ok_and(|view| view.phase == "connected") {
            let current = this.service.upgrade().is_some_and(|service| service.active(&this.call));
            let media_open = {
                let state = lock(&this.call.state);
                state.phase == Phase::Connected && state.media.as_ref().is_some_and(|media| media.scope().is_open())
            };
            if !current || !media_open {
                this.call.retire();
                return Poll::Ready(Err(SCOPE));
            }
        }
        Poll::Ready(result)
    }
}
impl Drop for OperationWaiter{fn drop(&mut self){if !self.delivered{self.call.retire();}}}
async fn command_loop(service:Weak<CallService>,mut commands:broadcast::Receiver<OperatorCommand>){loop{
    let Some(current)=service.upgrade()else{return;};
    let command=tokio::select!{biased;_=current.stopped()=>return,value=commands.recv()=>value};
    let command=match command{Ok(command)=>command,Err(broadcast::error::RecvError::Closed|broadcast::error::RecvError::Lagged(_))=>{current.retire();return;}};
    match command{
        OperatorCommand::AcceptCall{call_id,body}=>{if let Ok(waiter)=current.accept(&call_id,body).await{current.spawn_task(async move{let _=waiter.await;}).await;}},
        OperatorCommand::CancelCall{call_id}|OperatorCommand::HangupCall{call_id}|OperatorCommand::RejectCall{call_id,..}=>{
            if let Ok(waiter)=current.hangup(&call_id).await{current.spawn_task(async move{let _=waiter.await;}).await;}},
        OperatorCommand::StartCall{call_id,..}=>current.operator.send_event(OperatorEvent::Unavailable{call_id}),
        _=>{},
    }
}}
#[cfg(test)]
#[path="account_calls_tests.rs"]
mod tests;
