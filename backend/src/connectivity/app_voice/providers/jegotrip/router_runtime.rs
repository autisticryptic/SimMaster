//! Original-owned raw TCP -> endpoint TLS -> Router connect runtime.
//! No task is detached: this object owns the socket until Drop/close, and the
//! caller's owning task must be joined before its account/run lease is released.
//! Router assignment is logical routing data, never SDK/hosting/incoming proof.
use super::{native_router::{RouterAssignment, RouterConnectError, RouterConnectTracker, PreparedRouterControl},
    router_ports::{RouterPorts, PortLease, PortDelivery, ListenProtocol, PortError},
    packet_transport::{OwnedWireFrames, RouterTiming}, rpc_pending::RpcConnectionScope,
    rpc_stream::RpcProperty, router_connector::{poll_before, ConnectedRouterIo, RouterPolicy, RouterTcpConnector},
    runtime_connector::{BoxRuntimeIo, ConnectError}, session::AttemptOwner,
    tls_engine::{EndpointTlsEngine, TlsError, TlsPhase, VerifiedEndpointPackets}, transport_packets::TransportBytes};
use ring::rand::{SecureRandom, SystemRandom};
use std::{fmt, time::{Duration, Instant}};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouterIoError { Connect(ConnectError), Tls(TlsError), Control(RouterConnectError), Scope, Io, Timeout, Entropy, Configuration }
impl From<ConnectError> for RouterIoError {fn from(e:ConnectError)->Self{Self::Connect(e)}}
impl From<TlsError> for RouterIoError {fn from(e:TlsError)->Self{Self::Tls(e)}}
impl From<RouterConnectError> for RouterIoError {fn from(e:RouterConnectError)->Self{Self::Control(e)}}
impl From<PortError> for RouterIoError {fn from(e:PortError)->Self{match e {
    PortError::Closed=>Self::Scope, PortError::ClockRollback=>Self::Control(RouterConnectError::ClockRollback),
    PortError::HostChanged|PortError::Protocol=>Self::Control(RouterConnectError::Protocol), _=>Self::Configuration,
}}}
const NODE_PROBE_INTERVAL:Duration=Duration::from_secs(3);
const MAX_DELIVERY_BYTES:usize=super::router_packets::MAX_ROUTED_BODY_BYTES*2;
#[derive(Clone, Copy)]
pub(super) struct RouterTimeouts {pub connect:Duration,pub handshake:Duration,pub partial:Duration,pub io:Duration,pub startup:Duration}
impl RouterTimeouts { fn validate(self)->Result<(),RouterIoError> {
    for (value,max) in [(self.connect,10),(self.handshake,30),(self.partial,30),(self.io,30),(self.startup,30)] {
        if value.is_zero() || value>Duration::from_secs(max) {return Err(RouterIoError::Configuration);}
    } Ok(())
} }
struct CancelGuard {scope:RpcConnectionScope,armed:bool}
impl Drop for CancelGuard {fn drop(&mut self){if self.armed {self.scope.close();}}}

pub struct RouterPath {owner:AttemptOwner,scope:RpcConnectionScope,io:Option<BoxRuntimeIo>,engine:EndpointTlsEngine,
    tracker:RouterConnectTracker,assignment:Option<RouterAssignment>,ports:RouterPorts,
    deliveries:Vec<PortDelivery>,delivery_bytes:usize,last_node_probe:Option<Instant>,io_timeout:Duration,start:Instant}
impl fmt::Debug for RouterPath {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RouterPath(<redacted>)")}}
impl RouterPath {
    pub(super) async fn open(owner:AttemptOwner,scope:RpcConnectionScope,policy:&RouterPolicy,access:&[RpcProperty<'_>],
        timeouts:RouterTimeouts)->Result<Self,RouterIoError> {
        timeouts.validate()?;
        if !scope.is_open(){return Err(RouterIoError::Scope);}
        let mut cancelled=CancelGuard{scope:scope.clone(),armed:true};let start=Instant::now();
        let deadline=start.checked_add(timeouts.startup).ok_or(RouterIoError::Configuration)?;
        let connected=RouterTcpConnector::new(timeouts.connect.min(timeouts.startup))?.connect(&owner,&scope,policy).await?;
        let result=Self::attach(owner,scope,policy,connected,access,timeouts,start,deadline).await;
        if result.is_ok(){cancelled.armed=false;}result
    }
    async fn attach(owner:AttemptOwner,scope:RpcConnectionScope,policy:&RouterPolicy,connected:ConnectedRouterIo,
        access:&[RpcProperty<'_>],timeouts:RouterTimeouts,start:Instant,deadline:Instant)->Result<Self,RouterIoError> {
        let(index,io)=connected.into_original(&owner,&scope,policy)?;
        let mut random=[0u8;6];SystemRandom::new().fill(&mut random).map_err(|_|RouterIoError::Entropy)?;
        let nonce=u32::from_ne_bytes(random[..4].try_into().map_err(|_|RouterIoError::Entropy)?).max(1);
        let sequence=u16::from_ne_bytes(random[4..].try_into().map_err(|_|RouterIoError::Entropy)?);
        let engine=EndpointTlsEngine::for_original(owner.clone(),scope.clone(),policy.tls_for(index,timeouts.handshake,timeouts.partial)?,start)?;
        // Native default path MTU is 1460 (D657C0). No guessed OS address is
        // derived from router_id; its association stays in the original policy.
        let tracker=RouterConnectTracker::for_original(owner.clone(),scope.clone(),policy.router_id(index)?,nonce,access,
            1460,sequence,timeouts.startup,start)?;
        let ports=tracker.create_ports(&owner,&scope,10000,start)?.ok_or(RouterIoError::Scope)?;
        let mut path=Self{owner,scope,io:Some(io),engine,tracker,assignment:None,ports,
            deliveries:Vec::new(),delivery_bytes:0,last_node_probe:None,io_timeout:timeouts.io,start};
        let wire=path.engine.take_wire_output(&path.owner,&path.scope,Instant::now())?;
        path.write_wire(&wire,deadline).await?;
        while path.engine.phase()!=TlsPhase::Verified {
            let input=path.receive_until(deadline).await?;
            if input.packets_for(&path.owner,&path.scope).is_some_and(|packets|!packets.is_empty()) {return Err(RouterIoError::Configuration);}
        }
        // Before any peer clock observation, native D639F4 clears delay/loss.
        let timing=RouterTiming{word1:Instant::now().duration_since(start).as_millis() as u16,word2:0,byte6:0};
        let request=path.tracker.prepare(&path.owner,&path.scope,timing,Instant::now())?.ok_or(RouterIoError::Scope)?;
        let wire=path.engine.encrypt_packets(&path.owner,&path.scope,request.packets_for(&path.owner,&path.scope).ok_or(RouterIoError::Scope)?,Instant::now())?
            .ok_or(RouterIoError::Scope)?;
        path.write_wire(&wire,deadline).await?;
        loop {
            let input=path.receive_until(deadline).await?;
            let assignment=path.tracker.on_packets(&path.owner,&path.scope,input,Instant::now())?;
            path.flush_feedback(deadline).await?;
            if let Some(assignment)=assignment {path.assignment=Some(assignment);return Ok(path);}
        }
    }
    pub(super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(super) fn ids(&self)->Option<(u32,u32)>{self.assignment.as_ref()?.ids_for(&self.owner,&self.scope)}
    pub(super) fn close(&mut self){
        self.scope.close();self.engine.close();self.tracker.close();self.ports.close();
        self.deliveries.clear();self.delivery_bytes=0;self.io=None;self.assignment=None;
    }
    fn timing(&self)->RouterTiming{
        // No fabricated RTT/loss estimate: unknown word2/loss retain native0.
        // The later path estimator can supply measured values without changing
        // the original wire sequence, assignment or physical connection.
        RouterTiming{word1:Instant::now().duration_since(self.start).as_millis() as u16,word2:0,byte6:0}
    }
    async fn send_control(&mut self,request:&PreparedRouterControl,deadline:Instant)->Result<(),RouterIoError>{
        let wire=self.engine.encrypt_packets(&self.owner,&self.scope,
            request.packets_for(&self.owner,&self.scope).ok_or(RouterIoError::Scope)?,Instant::now())?.ok_or(RouterIoError::Scope)?;
        self.write_wire(&wire,deadline).await
    }
    async fn flush_feedback(&mut self,deadline:Instant)->Result<(),RouterIoError>{
        loop {
            let timing=self.timing();
            let Some(response)=self.tracker.prepare_feedback(&self.owner,&self.scope,timing,Instant::now())? else{break;};
            self.send_control(&response,deadline).await?;
        } Ok(())
    }
    /// Reserve under this ORIGINAL tracker/book, then wait for actual node
    /// traffic. This returns a logical listen, never a Channel/SDK-ready grant.
    pub(super) async fn listen(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,protocol:ListenProtocol,
        requested:Option<u16>,reuse:bool,deadline:Instant)->Result<Option<PortLease>,RouterIoError>{
        if !self.original(owner,scope){return Ok(None);}
        let lease=self.ports.reserve(owner,scope,protocol,requested,reuse,deadline,Instant::now())?.ok_or(RouterIoError::Scope)?;
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async {
            while lease.address_for(owner,scope).is_none(){
                if !self.drive_routing(owner,scope,deadline).await? {return Err(RouterIoError::Scope);}
            }
            if !self.original(owner,scope){return Err(RouterIoError::Scope);}
            Ok(Some(lease))
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    pub(super) fn take_deliveries(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<Vec<PortDelivery>>{
        if !self.original(owner,scope){return None;}
        self.delivery_bytes=0;Some(std::mem::take(&mut self.deliveries))
    }
    /// Service node controls and routed input while retaining same-group data.
    /// The supplied absolute caller deadline is never renewed by keepalives.
    pub(super) async fn drive_routing(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)->Result<bool,RouterIoError>{
        if !self.original(owner,scope){return Ok(false);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=self.drive_until(deadline).await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    async fn drive_until(&mut self,deadline:Instant)->Result<bool,RouterIoError>{
        loop {
            let now=Instant::now();if now>=deadline{return Err(RouterIoError::Timeout);}
            self.engine.tick(&self.owner,&self.scope,now)?;
            self.tracker.tick(&self.owner,&self.scope,now)?;self.ports.tick(&self.owner,&self.scope,now)?;
            if self.dispatch_pending(deadline).await? {return Ok(true);}
            if self.last_node_probe.is_none_or(|last|now.duration_since(last)>=NODE_PROBE_INTERVAL){
                let timing=self.timing();
                let request=self.tracker.prepare_node_probe(&self.owner,&self.scope,timing,now)?.ok_or(RouterIoError::Scope)?;
                self.send_control(&request,deadline).await?;self.last_node_probe=Some(now);
            }
            let pulse=self.last_node_probe.and_then(|last|last.checked_add(NODE_PROBE_INTERVAL)).ok_or(RouterIoError::Configuration)?;
            let mut read_end=deadline.min(pulse);
            if let Some(end)=self.tracker.next_deadline(){read_end=read_end.min(end);}
            let input=match self.receive_step(read_end,deadline).await? {
                Some(input)=>input,
                None=>{
                    let now=Instant::now();
                    // None means a READ wake only, never failed/partial TLS
                    // output. Real IO failures cannot discard admitted input.
                    self.engine.tick(&self.owner,&self.scope,now)?;
                    self.tracker.tick(&self.owner,&self.scope,now)?;
                    if self.scope.is_open()&&now<deadline&&now>=pulse{continue;}
                    return Err(RouterIoError::Timeout);
                }
            };
            if self.tracker.on_packets(&self.owner,&self.scope,input,Instant::now())?.is_some(){return Err(RouterIoError::Configuration);}
            self.flush_feedback(deadline).await?;
            self.dispatch_pending(deadline).await?;
            return Ok(true);
        }
    }
    async fn dispatch_pending(&mut self,deadline:Instant)->Result<bool,RouterIoError>{
        let pending=self.tracker.take_data(&self.owner,&self.scope).ok_or(RouterIoError::Scope)?;
        let any=!pending.is_empty();
        for data in pending {
            let delivery=self.ports.on_data(&self.owner,&self.scope,self.assignment.as_ref().ok_or(RouterIoError::Scope)?,&data,Instant::now())?;
            if let Some(delivery)=delivery {
                let bytes=delivery.retained_bytes();
                if self.deliveries.len()==64||bytes>MAX_DELIVERY_BYTES-self.delivery_bytes{return Err(RouterIoError::Configuration);}
                self.delivery_bytes+=bytes;self.deliveries.push(delivery);
            }
            let timing=self.timing();
            if let Some(response)=self.tracker.prepare_node_reply(&self.owner,&self.scope,&data,timing,Instant::now())?{
                self.send_control(&response,deadline).await?;
            }
        }
        Ok(any)
    }
    fn original(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool {
        &self.owner==owner && self.scope.matches(scope) && self.scope.is_open() && self.io.is_some()
    }
    async fn write_wire(&mut self,wire:&[OwnedWireFrames],deadline:Instant)->Result<(),RouterIoError> {
        let scope=self.scope.clone();let owner=&self.owner;let io=self.io.as_mut().ok_or(RouterIoError::Scope)?;
        let end=deadline.min(Instant::now().checked_add(self.io_timeout).ok_or(RouterIoError::Configuration)?);
        let write=async {
            for output in wire {for frame in output.frames_for(owner,&scope).ok_or(RouterIoError::Scope)? {
                let bytes=frame.expose_to_transport();let mut offset=0;
                while offset<bytes.len(){
                    let n=poll_before(&scope,io.write(&bytes[offset..]),end).await?.map_err(|_|RouterIoError::Io)?;
                    if n==0 {return Err(RouterIoError::Io);}offset+=n;
                }
            }}
            poll_before(&scope,io.flush(),end).await?.map_err(|_|RouterIoError::Io)?;
            if Instant::now()>=end {return Err(RouterIoError::Timeout);}Ok(())
        };
        tokio::select!{biased;_=scope.closed()=>Err(RouterIoError::Scope),
            result=tokio::time::timeout_at(end.into(),write)=>result.unwrap_or(Err(RouterIoError::Timeout))}
    }
    async fn receive_until(&mut self,deadline:Instant)->Result<VerifiedEndpointPackets,RouterIoError> {
        self.receive_step(deadline,deadline).await?.ok_or(RouterIoError::Timeout)
    }
    async fn receive_step(&mut self,read_deadline:Instant,write_deadline:Instant)->Result<Option<VerifiedEndpointPackets>,RouterIoError>{
        let now=Instant::now();self.engine.tick(&self.owner,&self.scope,now)?;
        let end=self.engine.next_deadline().map_or(read_deadline,|tls|tls.min(read_deadline));
        if now>=end{return Ok(None);}
        let mut buffer=[0u8;16384];let io=self.io.as_mut().ok_or(RouterIoError::Scope)?;
        let read=async {poll_before(&self.scope,io.read(&mut buffer),end).await?.map_err(|_|RouterIoError::Io)};
        let outcome=tokio::select!{biased;_=self.scope.closed()=>return Err(RouterIoError::Scope),
            result=tokio::time::timeout_at(end.into(),read)=>result};
        let n=match outcome {
            Err(_)|Ok(Err(RouterIoError::Connect(ConnectError::Timeout)))=>return Ok(None),
            Ok(result)=>result?,
        };
        if n==0{let error=self.engine.finish_eof(&self.owner,&self.scope).err().unwrap_or(TlsError::Closed);return Err(error.into());}
        let progress=self.engine.on_wire_bytes(&self.owner,&self.scope,&buffer[..n],Instant::now())?.ok_or(RouterIoError::Scope)?;
        // A keepalive read wake must not shorten the write/flush budget or turn
        // an IO error into an idle tick after TLS has consumed this input.
        self.write_wire(&progress.wire,write_deadline).await?;
        Ok(Some(progress.plaintext))
    }
    /// Original-only raw endpoint packets; the later Router routing layer must
    /// validate their path/control semantics before any Common/SDK delivery.
    pub(super) async fn receive(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)
        ->Result<Option<VerifiedEndpointPackets>,RouterIoError> {
        if !self.original(owner,scope){return Ok(None);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=self.receive_until(deadline).await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result.map(Some)
    }
    pub(super) async fn send(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,packets:&[TransportBytes],deadline:Instant)
        ->Result<bool,RouterIoError> {
        if !self.original(owner,scope){return Ok(false);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async {
            let wire=self.engine.encrypt_packets(owner,scope,packets,Instant::now())?.ok_or(RouterIoError::Scope)?;
            self.write_wire(&wire,deadline).await
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result.map(|()|true)
    }
}
impl Drop for RouterPath {fn drop(&mut self){self.close();}}

#[cfg(test)]
#[path="router_runtime_tests.rs"]
mod tests;
