//! Original-owned Router TCP/TLS -> reliable Basic Channel -> NetStream records.
//! This component neither spawns tasks nor releases account/run leases. The
//! caller must drop this physical owner and join its task before releasing those
//! leases. Record availability is NOT SDK login, hosting or media readiness.
use super::{channel_session::{ChannelSession, ScopedRecords, SessionError}, object_reference::ReferenceEndpoint,
    router_ports::ListenProtocol, router_runtime::{RouterIoError,RouterPath}, rpc_pending::RpcConnectionScope,
    session::AttemptOwner, transport_packets::TransportBytes};
use ring::rand::{SecureRandom,SystemRandom};
use std::{fmt,time::{Duration,Instant}};

const MAX_GROUPS:usize=64;
const MAX_QUEUED_BYTES:usize=1024*1024;
const CHANNEL_MTU:usize=1200; // conservative transmit policy, not a peer-MTU claim
const MAX_RECORD_WAIT:Duration=Duration::from_secs(30);
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum NetstreamError{Scope,Configuration,Timeout,Entropy,Router(RouterIoError),Session(SessionError)}
impl From<RouterIoError> for NetstreamError{fn from(error:RouterIoError)->Self{Self::Router(error)}}
impl From<SessionError> for NetstreamError{fn from(error:SessionError)->Self{Self::Session(error)}}
struct CancelGuard{scope:RpcConnectionScope,armed:bool}
impl Drop for CancelGuard{fn drop(&mut self){if self.armed{self.scope.close();}}}

pub(super) struct NetstreamPath{
    owner:AttemptOwner,scope:RpcConnectionScope,router:RouterPath,session:ChannelSession,
    incoming:Vec<ScopedRecords>,incoming_bytes:usize,
}
impl fmt::Debug for NetstreamPath{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NetstreamPath(<redacted>)")}}
impl Drop for NetstreamPath{fn drop(&mut self){self.close();}}
impl NetstreamPath{
    /// Consumes the ORIGINAL physical Router owner. No new socket, endpoint,
    ///trust context or relabelled tracker is created to start this Channel.
    pub fn owner(&self)->AttemptOwner{self.owner.clone()}
    pub fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub fn received_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,group:&ScopedRecords)->Option<Instant>{
        if !self.original(owner,scope){return None;}group.received_for(owner,scope,&self.session)
    }
    pub async fn open(mut router:RouterPath,endpoint:&ReferenceEndpoint,index:usize,deadline:Instant,
        partial_timeout:Duration)->Result<Self,NetstreamError>{
        let owner=router.owner();let scope=router.scope();
        let mut cancelled=CancelGuard{scope:scope.clone(),armed:true};
        let now=Instant::now();let end=deadline.min(now.checked_add(MAX_RECORD_WAIT).ok_or(NetstreamError::Configuration)?);
        if now>=end{return Err(NetstreamError::Timeout);}
        if partial_timeout.is_zero()||partial_timeout>MAX_RECORD_WAIT{return Err(NetstreamError::Configuration);}
        let peer=router.connect_peer(&owner,&scope,endpoint,index,end).await?.ok_or(NetstreamError::Scope)?;
        let lease=router.listen(&owner,&scope,ListenProtocol::Arc,None,false,end).await?.ok_or(NetstreamError::Scope)?;
        let clock=router.channel_clock(&owner,&scope,&lease,&peer).ok_or(NetstreamError::Scope)?;
        let mut random=[0u8;6];SystemRandom::new().fill(&mut random).map_err(|_|NetstreamError::Entropy)?;
        let sequence=u16::from_ne_bytes([random[0],random[1]]);let seed=u32::from_ne_bytes([random[2],random[3],random[4],random[5]]);
        let admitted=Instant::now();if admitted>=end{return Err(NetstreamError::Timeout);}
        let session=ChannelSession::for_original(owner.clone(),scope.clone(),clock,&lease,peer,endpoint,sequence,seed,CHANNEL_MTU,end,
            partial_timeout,admitted)?;
        let mut path=Self{owner,scope,router,session,incoming:Vec::new(),incoming_bytes:0};
        while !path.session.ready_for(&path.owner,&path.scope){path.step(end).await?;}
        // Complete the peer-facing Start ACK on the same physical IO before
        //returning a Channel-ready owner. This still does not log in the SDK.
        path.flush(end).await?;cancelled.armed=false;Ok(path)
    }
    fn original(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        &self.owner==owner&&self.scope.matches(scope)&&self.scope.is_open()
    }
    pub fn close(&mut self){
        self.scope.close();self.session.close();self.incoming.clear();self.incoming_bytes=0;self.router.close();
    }
    pub fn ready_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        self.original(owner,scope)&&self.session.ready_for(owner,scope)
    }
    pub fn records_for<'a>(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,group:&'a ScopedRecords)->Option<&'a [TransportBytes]>{
        if !self.original(owner,scope){return None;}group.records_for(owner,scope,&self.session)
    }
    fn ingest_pending(&mut self)->Result<(),NetstreamError>{
        let deliveries=self.router.take_deliveries(&self.owner,&self.scope).ok_or(NetstreamError::Scope)?;
        if let Some(group)=self.session.on_deliveries(&self.owner,&self.scope,&deliveries,Instant::now())?{
            let records=group.records_for(&self.owner,&self.scope,&self.session).ok_or(NetstreamError::Scope)?;
            if !records.is_empty(){
                let bytes=records.iter().try_fold(0usize,|total,record|total.checked_add(record.expose_to_transport().len())).ok_or(NetstreamError::Configuration)?;
                if self.incoming.len()>=MAX_GROUPS||bytes>MAX_QUEUED_BYTES.saturating_sub(self.incoming_bytes){return Err(NetstreamError::Configuration);}
                self.incoming_bytes+=bytes;self.incoming.push(group);
            }
        }Ok(())
    }
    async fn flush(&mut self,deadline:Instant)->Result<(),NetstreamError>{
        // Bound one burst. Further queued fragments keep next_wake immediate,
        //while every actual write retains its original hard deadline/gate.
        for _ in 0..32{
            let now=Instant::now();if now>=deadline{return Err(NetstreamError::Timeout);}
            let read_end=self.session.bound_io_deadline(&self.owner,&self.scope,deadline).ok_or(NetstreamError::Scope)?;
            // A ready route does not otherwise read during send_datagram. Give
            //ACKs an actual read poll BEFORE choosing another due retry.
            self.router.try_drive_routing(&self.owner,&self.scope,read_end).await?;self.ingest_pending()?;
            let Some(packet)=self.session.next_packet(&self.owner,&self.scope,Instant::now())? else{return Ok(());};
            let (bytes,write_end)=packet.for_io(&self.owner,&self.scope,&self.session,Instant::now(),deadline)?.ok_or(NetstreamError::Scope)?;
            if !self.router.send_datagram(&self.owner,&self.scope,self.session.lease(),self.session.peer(),0,0,bytes,write_end).await?{
                return Err(NetstreamError::Scope);
            }
            tokio::task::yield_now().await;
            // Route-report waiting may have admitted a complete TLS group.
            //Do not defer its Channel validation until after another send.
            self.ingest_pending()?;
        }Ok(())
    }
    async fn step(&mut self,deadline:Instant)->Result<(),NetstreamError>{
        let was_ready=self.session.ready_for(&self.owner,&self.scope);
        let now=Instant::now();if now>=deadline{return Err(NetstreamError::Timeout);}
        if !self.session.tick(&self.owner,&self.scope,now)?{return Err(NetstreamError::Scope);}
        self.ingest_pending()?;
        if let Some(metrics)=self.router.path_metrics(&self.owner,&self.scope){self.session.path_delay(&self.owner,&self.scope,metrics.echoed_delay_ms);}
        self.flush(deadline).await?;
        if !self.incoming.is_empty()||!was_ready&&self.session.ready_for(&self.owner,&self.scope){return Ok(());}
        let hard_end=self.session.bound_io_deadline(&self.owner,&self.scope,deadline).ok_or(NetstreamError::Scope)?;
        let wake=self.session.next_wake().unwrap_or(hard_end).min(hard_end);
        // A successful false result is only a read-side timer wake. Write/flush
        //failures and expired TLS/Router partials remain errors and retire IO.
        self.router.drive_routing_tick(&self.owner,&self.scope,wake,hard_end).await?;
        if !self.session.tick(&self.owner,&self.scope,Instant::now())?{return Err(NetstreamError::Scope);}
        self.ingest_pending()?;Ok(())
    }
    /// Queue records without re-encoding retries, then finish every first
    ///transmission before returning. Success is NOT peer/business acceptance.
    pub async fn send_records(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,records:&[TransportBytes],deadline:Instant)
        ->Result<bool,NetstreamError>{
        if !self.original(owner,scope){return Ok(false);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            if !self.session.ready_for(owner,scope)||records.is_empty()||records.len()>256{return Err(NetstreamError::Configuration);}
            let total=records.iter().try_fold(0usize,|total,record|total.checked_add(record.expose_to_transport().len())).ok_or(NetstreamError::Configuration)?;
            if total>MAX_QUEUED_BYTES{return Err(NetstreamError::Configuration);}
            let now=Instant::now();if now>=deadline{return Err(NetstreamError::Timeout);}
            let record_end=deadline.min(now.checked_add(MAX_RECORD_WAIT).ok_or(NetstreamError::Configuration)?);
            // No await between queue admissions. A later failure closes all
            //staged fragments BEFORE any of this call's packets are written.
            for record in records{
                if !self.session.queue_record(owner,scope,record.expose_to_transport(),record_end,now)?{return Err(NetstreamError::Scope);}
            }
            while self.session.has_queued(owner,scope){self.flush(deadline).await?;}
            self.ingest_pending()?;Ok(true)
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    /// A caller-owned actor may poll commands/timers between these bounded
    ///steps. No pending receive future is cancelled and no fresh read deadline
    ///replaces a retained Channel expiration.
    pub async fn try_receive_records(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)
        ->Result<Option<Vec<ScopedRecords>>,NetstreamError>{
        if !self.original(owner,scope){return Ok(None);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            if !self.session.tick(owner,scope,Instant::now())?{return Err(NetstreamError::Scope);}
            self.ingest_pending()?;
            if let Some(metrics)=self.router.path_metrics(owner,scope){self.session.path_delay(owner,scope,metrics.echoed_delay_ms);}
            self.flush(deadline).await?;
            if self.incoming.is_empty(){return Ok(None);}
            self.incoming_bytes=0;Ok(Some(std::mem::take(&mut self.incoming)))
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
    pub async fn receive_records(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,deadline:Instant)
        ->Result<Option<Vec<ScopedRecords>>,NetstreamError>{
        if !self.original(owner,scope){return Ok(None);}
        let mut cancelled=CancelGuard{scope:self.scope.clone(),armed:true};
        let result=async{
            loop{
                if !self.incoming.is_empty(){
                    self.flush(deadline).await?;self.incoming_bytes=0;
                    return Ok(Some(std::mem::take(&mut self.incoming)));
                }
                self.step(deadline).await?;
            }
        }.await;
        if result.is_ok(){cancelled.armed=false;}else{self.close();}result
    }
}

#[cfg(test)]
#[path="netstream_runtime_tests.rs"]
pub(super) mod tests;
