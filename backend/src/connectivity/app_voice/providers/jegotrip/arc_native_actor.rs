//! Original ARC command actor using real Router/SARC/Common, not an innerTLS
//! emulation. All protocol state/owner/scope is created BEFORE network work.
use super::*;
use super::super::{common_runtime::{CommonPath,CommonIoError},common_packets::CommonError,common_control::ControlError,
    netstream_runtime::{NetstreamPath,NetstreamError},router_runtime::{RouterPath,RouterIoError,RouterTimeouts},
    router_connector::RouterPolicy,object_reference::ReferenceEndpoint,transport_packets::TransportBytes};

pub(crate) struct PreparedCommonRuntime{
    owner:AttemptOwner,scope:RpcConnectionScope,backend:Option<OfflineBackend>,policy:Option<RuntimePolicy>,
    start:Instant,attempt_end:Instant,transferred:bool,
}
impl fmt::Debug for PreparedCommonRuntime{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("PreparedCommonRuntime(<redacted>)")}}
impl Drop for PreparedCommonRuntime{fn drop(&mut self){if !self.transferred{self.scope.close();}}}
pub(crate) struct ConnectedCommonRuntime{
    owner:AttemptOwner,scope:RpcConnectionScope,backend:Option<OfflineBackend>,common:Option<CommonPath>,policy:Option<RuntimePolicy>,
    start:Instant,attempt_end:Instant,attached:bool,
}
impl fmt::Debug for ConnectedCommonRuntime{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ConnectedCommonRuntime(<redacted>)")}}
impl Drop for ConnectedCommonRuntime{fn drop(&mut self){if !self.attached{self.scope.close();}}}

pub(crate) fn prepare_common(owner:AttemptOwner,policy:RuntimePolicy,attempt_end:Instant)->Result<PreparedCommonRuntime,RuntimeError>{
    let start=Instant::now();
    if !policy.valid()||attempt_end.checked_duration_since(start).is_none_or(|left|left.is_zero()||left>Duration::from_secs(120)){return Err(RuntimeError::InvalidConfiguration);}
    let mut backend=OfflineBackend::new(owner.clone());backend.claim_common_transport().map_err(|_|RuntimeError::InvalidConfiguration)?;
    let scope=backend.connection_scope();
    Ok(PreparedCommonRuntime{owner,scope,backend:Some(backend),policy:Some(policy),start,attempt_end,transferred:false})
}
impl PreparedCommonRuntime{
    pub fn connection_scope(&self)->RpcConnectionScope{self.scope.clone()}
    /// No actor is spawned here. The caller first acquires its owning-handle
    ///slot, then consumes ConnectedCommonRuntime::attach with no await gap.
    pub async fn connect(mut self,router_policy:&RouterPolicy,access:&[RpcProperty<'_>],endpoint:&ReferenceEndpoint,index:usize,
        mut timeouts:RouterTimeouts,compress_above:Option<usize>)->Result<ConnectedCommonRuntime,RuntimeError>{
        if !self.scope.is_open(){return Err(RuntimeError::Scope);}
        let now=Instant::now();if now>=self.attempt_end{return Err(RuntimeError::Timeout);}
        if !matches!(endpoint.scheme(),b"arc"|b"sarc")||index>=32||compress_above.is_some_and(|value|value==0||value>MAX_PARAMETER_BYTES){return Err(RuntimeError::InvalidConfiguration);}
        let host=endpoint.router_host().map_err(|_|RuntimeError::InvalidConfiguration)?;
        if matches!(host,super::super::object_reference::RouterHost::Address(_))&&index!=0{return Err(RuntimeError::InvalidConfiguration);}
        timeouts.startup=timeouts.startup.min(self.attempt_end.duration_since(now));
        let connecting=async{
            let router=RouterPath::open(self.owner.clone(),self.scope.clone(),router_policy,access,timeouts).await.map_err(router_error)?;
            let stream=NetstreamPath::open(router,endpoint,index,self.attempt_end,timeouts.partial).await.map_err(netstream_error)?;
            CommonPath::from_original(stream,timeouts.partial,compress_above).map_err(common_error)
        };
        let common=tokio::select!{biased;
            _=self.scope.closed()=>return Err(RuntimeError::Closed),
            result=tokio::time::timeout_at(self.attempt_end.into(),connecting)=>result.map_err(|_|RuntimeError::Timeout)??,
        };
        if common.owner()!=self.owner||!common.scope().matches(&self.scope){return Err(RuntimeError::Scope);}
        let backend=self.backend.take().ok_or(RuntimeError::Closed)?;let policy=self.policy.take().ok_or(RuntimeError::Closed)?;
        self.transferred=true;
        Ok(ConnectedCommonRuntime{owner:self.owner.clone(),scope:self.scope.clone(),backend:Some(backend),common:Some(common),policy:Some(policy),
            start:self.start,attempt_end:self.attempt_end,attached:false})
    }
}
impl ConnectedCommonRuntime{
    pub fn connection_scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub fn attach(mut self)->Result<(RuntimeHandle,mpsc::Receiver<RuntimeEvent>),RuntimeError>{
        if !self.scope.is_open(){return Err(RuntimeError::Scope);}if Instant::now()>=self.attempt_end{return Err(RuntimeError::Timeout);}
        let common=self.common.take().ok_or(RuntimeError::Closed)?;let backend=self.backend.take().ok_or(RuntimeError::Closed)?;
        let policy=self.policy.take().ok_or(RuntimeError::Closed)?;let owner=self.owner.clone();let scope=self.scope.clone();
        if !common.is_open_for(&owner,&scope)||!backend.connection_scope().matches(&scope){return Err(RuntimeError::Scope);}
        let(commands_tx,commands_rx)=mpsc::channel(MAX_COMMANDS);let(events_tx,events_rx)=mpsc::channel(MAX_EVENTS);
        let actor:Actor<super::super::runtime_connector::BoxRuntimeIo>=Actor{io:None,owner:owner.clone(),scope:scope.clone(),backend,tls:None,common:Some(common),native_attempt_end:Some(self.attempt_end),
            commands:commands_rx,events:events_tx,policy,timing:None,start:self.start,pending:std::collections::BTreeMap::new(),logout_pending:None,unhosting_pending:None,call_pending:std::collections::BTreeMap::new()};
        self.attached=true;let task=tokio::spawn(actor.run());
        Ok((RuntimeHandle{owner,scope,commands:commands_tx,task:Some(task)},events_rx))
    }
}
fn router_error(error:RouterIoError)->RuntimeError{match error{
    RouterIoError::Timeout=>RuntimeError::Timeout,RouterIoError::Scope=>RuntimeError::Closed,RouterIoError::Tls(error)=>RuntimeError::Tls(error),
    RouterIoError::Configuration|RouterIoError::Entropy=>RuntimeError::InvalidConfiguration,
    RouterIoError::Connect(super::super::runtime_connector::ConnectError::Timeout)=>RuntimeError::Timeout,
    RouterIoError::Connect(_)|RouterIoError::Io=>RuntimeError::Io,_=>RuntimeError::Protocol,
}}
fn netstream_error(error:NetstreamError)->RuntimeError{match error{
    NetstreamError::Scope=>RuntimeError::Closed,NetstreamError::Timeout=>RuntimeError::Timeout,
    NetstreamError::Configuration|NetstreamError::Entropy=>RuntimeError::InvalidConfiguration,NetstreamError::Router(error)=>router_error(error),
    NetstreamError::Session(super::super::channel_session::SessionError::Channel(super::super::reliable_channel::ChannelError::Timeout))=>RuntimeError::Timeout,
    _=>RuntimeError::Protocol,
}}
fn common_error(error:CommonIoError)->RuntimeError{match error{
    CommonIoError::Scope=>RuntimeError::Closed,CommonIoError::Timeout|CommonIoError::Packet(CommonError::Timeout)|CommonIoError::Control(ControlError::Timeout)=>RuntimeError::Timeout,
    CommonIoError::Configuration=>RuntimeError::InvalidConfiguration,CommonIoError::Control(ControlError::RemoteReleased)=>RuntimeError::Closed,
    CommonIoError::Netstream(error)=>netstream_error(error),_=>RuntimeError::Protocol,
}}
impl<I:AsyncRead+AsyncWrite+Unpin+Send+'static> Actor<I>{
    fn common_io_end(&self,extra:Option<Instant>)->Result<Instant,RuntimeError>{
        let mut end=Instant::now().checked_add(self.policy.io_timeout).ok_or(RuntimeError::Timeout)?;
        for value in self.pending.values().copied().chain(extra).chain(self.unhosting_pending.as_ref().map(|pending|pending.deadline))
            .chain(self.call_pending.values().map(|pending|pending.deadline)).chain(self.common.as_ref().and_then(CommonPath::next_deadline)){
            end=end.min(value);
        }
        if self.backend.phase()!=ArcPhase::Authenticated{if let Some(attempt)=self.native_attempt_end{end=end.min(attempt);}}
        if Instant::now()>=end{return Err(RuntimeError::Timeout);}Ok(end)
    }
    fn common_tick(&mut self)->Result<(),RuntimeError>{
        if !self.scope.is_open(){return Err(RuntimeError::Closed);}
        let now=Instant::now();self.pending.retain(|id,_|self.backend.is_pending(*id));
        if self.logout_pending.as_ref().is_some_and(|p|p.completion.is_closed())
            ||self.unhosting_pending.as_ref().is_some_and(|p|p.completion.is_closed())
            ||self.call_pending.values().any(|p|p.completion.is_closed()){return Err(RuntimeError::Cancelled);}
        self.common_io_end(None)?;
        if self.pending.values().any(|end|now>=*end){return Err(RuntimeError::Timeout);}Ok(())
    }
    pub(super) async fn common_loop_run(&mut self)->RuntimeError{
        if self.tls.is_some()||self.io.is_some(){return RuntimeError::InvalidConfiguration;}
        let mut timer=tokio::time::interval(Duration::from_millis(20));timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop{
            if let Err(error)=self.common_tick(){return error;}
            if self.events.is_closed(){return RuntimeError::Closed;}
            // One queued command gets service even when control IO made every
            //timer pulse overdue. We still service a read/timer step afterward,
            //so neither a command flood nor continuous Pongs can starve peers.
            match self.commands.try_recv(){
                Ok(command)=>if !command.completion.is_closed(){if let Err(error)=self.command(command).await{return error;}},
                Err(mpsc::error::TryRecvError::Disconnected)=>return RuntimeError::Closed,
                Err(mpsc::error::TryRecvError::Empty)=>{},
            }
            tokio::select!{biased;
                _=self.scope.closed()=>return RuntimeError::Closed,
                _=self.events.closed()=>return RuntimeError::Closed,
                _=timer.tick()=>{
                    if let Err(error)=self.receive_common_step().await{return error;}
                    if self.backend.phase()==ArcPhase::Closed{return RuntimeError::Protocol;}
                }
                command=self.commands.recv()=>{
                    let Some(command)=command else{return RuntimeError::Closed;};if command.completion.is_closed(){continue;}
                    if let Err(error)=self.command(command).await{return error;}
                }
            }
        }
    }
    async fn receive_common_step(&mut self)->Result<(),RuntimeError>{
        let end=self.common_io_end(None)?;let owner=self.owner.clone();let scope=self.scope.clone();
        let common=self.common.as_mut().ok_or(RuntimeError::Closed)?;
        let input=tokio::select!{biased;
            _=scope.closed()=>return Err(RuntimeError::Closed),_=self.events.closed()=>return Err(RuntimeError::Closed),
            result=common.try_receive_calls(&owner,&scope,end)=>result.map_err(common_error)?,
        };
        if let Some(input)=input{
            let elapsed=self.elapsed_ms(Instant::now());
            let events=self.backend.on_common_calls(&owner,&scope,self.common.as_ref().ok_or(RuntimeError::Closed)?,&input,elapsed).map_err(|_|RuntimeError::Protocol)?;
            if Instant::now()>=end{return Err(RuntimeError::Timeout);}
            // Startup is a one-way phase. A later LogoutPending state must
            //never resurrect an expired login/initial-connection budget.
            if self.backend.phase()==ArcPhase::Authenticated{self.native_attempt_end=None;}
            self.pending.retain(|id,_|self.backend.is_pending(*id));self.deliver(events).await?;
        }Ok(())
    }
    pub(super) async fn send_common_packets(&mut self,packets:&[TransportBytes],completion:Option<&mut oneshot::Sender<Result<RuntimeReceipt,RuntimeError>>>,extra:Option<Instant>)->Result<(),RuntimeError>{
        let end=self.common_io_end(extra)?;let owner=self.owner.clone();let scope=self.scope.clone();
        let common=self.common.as_mut().ok_or(RuntimeError::Closed)?;
        let sending=common.send_prepared_packets(&owner,&scope,packets,end);
        let outcome=if let Some(completion)=completion{
            tokio::select!{biased;
                _=scope.closed()=>Err(RuntimeError::Closed),_=self.events.closed()=>Err(RuntimeError::Closed),_=completion.closed()=>Err(RuntimeError::Cancelled),
                result=sending=>result.map_err(common_error),
            }
        }else{
            tokio::select!{biased;
                _=scope.closed()=>Err(RuntimeError::Closed),_=self.events.closed()=>Err(RuntimeError::Closed),
                result=sending=>result.map_err(common_error),
            }
        };
        match outcome{Ok(true)=>Ok(()),Ok(false)=>Err(RuntimeError::Closed),Err(error)=>{self.scope.close();Err(error)}}
    }
}

#[cfg(test)]
#[path="arc_native_actor_tests.rs"]
mod tests;
