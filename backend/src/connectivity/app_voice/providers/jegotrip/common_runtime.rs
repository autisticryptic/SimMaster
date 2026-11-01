//! Original physical NetStream -> bounded Common messages and controls.
//! This owner never creates a socket/task or authenticates an SDK account.
//! Decoded calls retain the original path/scope brand through queued delivery.
use super::{channel_session::ScopedRecords,common_control::{CommonControls,ControlError},
    common_packets::{self,CommonAssembler,CommonError,CommonMessage,NATIVE_FRAGMENT_THRESHOLD},
    netstream_runtime::{NetstreamPath,NetstreamError},rpc_pending::RpcConnectionScope,
    session::AttemptOwner,transport_packets::{CallKind,TransportBytes}};
use std::{fmt,sync::{Arc,Weak,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};

const MAX_CALLS:usize=256;
const MAX_GROUP_BYTES:usize=1024*1024;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum CommonIoError{Scope,Configuration,Timeout,Packet(CommonError),Control(ControlError),Netstream(NetstreamError)}
impl From<CommonError> for CommonIoError{fn from(error:CommonError)->Self{Self::Packet(error)}}
impl From<ControlError> for CommonIoError{fn from(error:ControlError)->Self{Self::Control(error)}}
impl From<NetstreamError> for CommonIoError{fn from(error:NetstreamError)->Self{Self::Netstream(error)}}
struct CancelGuard{scope:RpcConnectionScope,armed:bool}
impl Drop for CancelGuard{fn drop(&mut self){if self.armed{self.scope.close();}}}

pub(super) struct ScopedCommonCalls{owner:AttemptOwner,scope:RpcConnectionScope,live:Weak<AtomicBool>,calls:Vec<CommonMessage>}
impl fmt::Debug for ScopedCommonCalls{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ScopedCommonCalls(<redacted>)")}}
pub(super) struct CommonPath{
    owner:AttemptOwner,scope:RpcConnectionScope,stream:NetstreamPath,assembler:CommonAssembler,
    controls:CommonControls,live:Arc<AtomicBool>,compress_above:Option<usize>,
}
impl fmt::Debug for CommonPath{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommonPath(<redacted>)")}}
impl Drop for CommonPath{fn drop(&mut self){self.close();}}
impl CommonPath{
    pub fn from_original(stream:NetstreamPath,partial_timeout:Duration,compress_above:Option<usize>)->Result<Self,CommonIoError>{
        let owner=stream.owner();let scope=stream.scope();let now=Instant::now();
        if !stream.ready_for(&owner,&scope){return Err(CommonIoError::Scope);}
        if compress_above.is_some_and(|value|value==0||value>super::rpc_stream::MAX_STREAM_BYTES){return Err(CommonIoError::Configuration);}
        Ok(Self{owner,scope,stream,assembler:CommonAssembler::new(partial_timeout,now)?,controls:CommonControls::new(now),live:Arc::new(AtomicBool::new(true)),compress_above})
    }
    pub fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub fn owner(&self)->AttemptOwner{self.owner.clone()}
    pub fn is_open_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.original(owner,scope)}
    fn original(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        &self.owner==owner&&self.scope.matches(scope)&&self.scope.is_open()&&self.live.load(Ordering::Acquire)&&self.stream.ready_for(owner,scope)
    }
    pub fn close(&mut self){self.live.store(false,Ordering::Release);self.scope.close();self.assembler.close();self.controls.close();self.stream.close();}
    pub fn next_deadline(&self)->Option<Instant>{
        if !self.original(&self.owner,&self.scope){return None;}
        self.assembler.deadline().into_iter().chain(self.controls.deadline()).min()
    }
    fn bound(&self,deadline:Instant)->Instant{self.next_deadline().map_or(deadline,|old|old.min(deadline))}
    fn tick(&mut self,deadline:Instant)->Result<(),CommonIoError>{
        if !self.original(&self.owner,&self.scope){return Err(CommonIoError::Scope);}
        let now=Instant::now();if now>=deadline{return Err(CommonIoError::Timeout);}
        self.assembler.tick(now)?;self.controls.tick(now)?;Ok(())
    }
    pub fn delay_ms(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<u64>{
        if !self.original(owner,scope){return None;}self.controls.delay_ms()
    }
    pub fn calls_for<'a>(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,group:&'a ScopedCommonCalls)->Option<&'a [CommonMessage]>{
        if !self.original(owner,scope)||&group.owner!=owner||!group.scope.matches(scope){return None;}
        let live=group.live.upgrade()?;if !Arc::ptr_eq(&live,&self.live)||!live.load(Ordering::Acquire){return None;}
        let now=Instant::now();if group.calls.iter().any(|call|now>=call.deadline){return None;}Some(&group.calls)
    }
    /// Caller supplies only the original RPC payload; this never parses or
    ///changes its call ID, id/domain/token metadata or ownership.
    pub async fn send_call(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,kind:CallKind,payload:&[u8],deadline:Instant)->Result<bool,CommonIoError>{
        if !self.original(owner,scope){return Ok(false);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            if !matches!(kind,CallKind::Request|CallKind::Reply){return Err(CommonIoError::Configuration);}
            self.tick(deadline)?;
            let packets=common_packets::encode(kind,payload,NATIVE_FRAGMENT_THRESHOLD,self.compress_above)?;
            self.tick(deadline)?;let end=self.bound(deadline);
            if !self.stream.send_records(owner,scope,&packets,end).await?{return Err(CommonIoError::Scope);}
            self.tick(deadline)?;Ok(true)
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    /// Adapts this actor's already prepared plaintext ARC call packets to the
    ///native Common fragment/compression policy. No TLS capability is forged;
    ///RPC IDs and all metadata remain byte-identical in the payload.
    pub async fn send_prepared_packets(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,packets:&[TransportBytes],deadline:Instant)->Result<bool,CommonIoError>{
        if !self.original(owner,scope){return Ok(false);}
        let result=async{
            self.tick(deadline)?;if packets.is_empty()||packets.len()>MAX_CALLS{return Err(CommonIoError::Configuration);}
            let mut assembler=super::transport_packets::CallAssembler::default();let mut message=None;
            for (index,packet) in packets.iter().enumerate(){
                let decoded=assembler.on_packet(packet.expose_to_transport()).map_err(|_|CommonIoError::Configuration)?;
                if decoded.is_some(){if index+1!=packets.len(){return Err(CommonIoError::Configuration);}message=decoded;}
            }
            let message=message.ok_or(CommonIoError::Configuration)?;
            self.send_call(owner,scope,message.kind,message.payload.expose_to_transport(),deadline).await
        }.await;
        if result.is_err(){self.close();}result
    }
    pub async fn send_ping(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)->Result<bool,CommonIoError>{
        if !self.original(owner,scope){return Ok(false);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            self.tick(deadline)?;let now=Instant::now();let end=deadline.min(now.checked_add(Duration::from_secs(30)).ok_or(CommonIoError::Configuration)?);
            self.controls.prepare_ping(end,now)?;let io_end=self.bound(end);
            if !self.stream.send_records(owner,scope,&[TransportBytes::from_bytes(vec![2])],io_end).await?{return Err(CommonIoError::Scope);}
            self.tick(end)?;Ok(true)
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    fn decode_groups(&mut self,groups:&[ScopedRecords],deadline:Instant)->Result<Vec<CommonMessage>,CommonIoError>{
        let mut calls=Vec::new();let mut expanded=0usize;let mut count=0usize;
        for group in groups{
            let admitted=self.stream.received_for(&self.owner,&self.scope,group).ok_or(CommonIoError::Scope)?;
            let records=self.stream.records_for(&self.owner,&self.scope,group).ok_or(CommonIoError::Scope)?;
            for record in records{
                count+=1;if count>MAX_CALLS{return Err(CommonError::Bound.into());}
                let now=Instant::now();
                if let Some(call)=self.assembler.on_packet_budgeted(record.expose_to_transport(),deadline,admitted,now,MAX_GROUP_BYTES.saturating_sub(expanded))?{
                    expanded=expanded.checked_add(call.payload.expose_to_transport().len()).ok_or(CommonIoError::Configuration)?;calls.push(call);
                }
                if expanded>MAX_GROUP_BYTES.saturating_sub(self.assembler.retained_bytes()){return Err(CommonError::Bound.into());}
            }
        }
        self.tick(deadline)?;Ok(calls)
    }
    async fn dispatch_groups(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,groups:&[ScopedRecords],deadline:Instant)->Result<Option<ScopedCommonCalls>,CommonIoError>{
        let messages=self.decode_groups(groups,deadline)?;
        let plan=self.controls.on_messages(messages,Instant::now())?;
        if let Some(pong_end)=plan.pong_deadlines.iter().copied().min(){
            let packets:Vec<_>=plan.pong_deadlines.iter().map(|_|TransportBytes::from_bytes(vec![14])).collect();
            let end=self.bound(deadline.min(pong_end));
            if !self.stream.send_records(owner,scope,&packets,end).await?{return Err(CommonIoError::Scope);}
        }
        self.tick(deadline)?;
        if plan.calls.is_empty(){return Ok(None);}
        let now=Instant::now();if plan.calls.iter().any(|call|now>=call.deadline){return Err(CommonIoError::Timeout);}
        Ok(Some(ScopedCommonCalls{owner:self.owner.clone(),scope:self.scope.clone(),live:Arc::downgrade(&self.live),calls:plan.calls}))
    }
    /// A bounded nonblocking-read step for the original ARC actor. Required
    ///control writes may await, still bounded by the original hard deadlines.
    pub async fn try_receive_calls(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)->Result<Option<ScopedCommonCalls>,CommonIoError>{
        if !self.original(owner,scope){return Ok(None);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            self.tick(deadline)?;let end=self.bound(deadline);
            let Some(groups)=self.stream.try_receive_records(owner,scope,end).await? else{return Ok(None);};
            self.dispatch_groups(owner,scope,&groups,end).await
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    /// Receive a whole original NetStream group before emitting Pongs or calls.
    /// A malformed later packet or Release4 revokes the entire staged result;
    /// Ping/Pong traffic never extends pending call, fragment or ping budgets.
    pub async fn receive_calls(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)->Result<Option<ScopedCommonCalls>,CommonIoError>{
        if !self.original(owner,scope){return Ok(None);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            loop{
                self.tick(deadline)?;let end=self.bound(deadline);
                let groups=self.stream.receive_records(owner,scope,end).await?.ok_or(CommonIoError::Scope)?;
                if let Some(calls)=self.dispatch_groups(owner,scope,&groups,end).await?{return Ok(Some(calls));}
            }
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
}

#[cfg(test)]
#[path="common_runtime_tests.rs"]
pub(super) mod tests;
