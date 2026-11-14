//! Original owned legacy stcp/Common IO. Seeded XOR is not TLS authentication.
//! This module is reached only after explicit policy admission; no SDK library
//! is loaded. Main-session revocation gates every nonblocking socket poll.
use super::{alive_profile::AccountAliveEndpoint,netstream_records::{RecordSecurity,RecordSender,RecordReceiver},
    packet_stream::{PacketStreamDecoder,encode_packet_stream_frame},common_packets::{self,CommonAssembler,CommonMessage},common_control::CommonControls,
    rpc_pending::RpcConnectionScope,transport_packets::CallKind};
use std::{io,pin::Pin,future::poll_fn,task::Poll,time::{Duration,Instant}};
use tokio::{net::TcpStream,io::{AsyncRead,AsyncWrite,ReadBuf}};
use ring::rand::{SecureRandom,SystemRandom};
const IO:&str="app_account_alive_io_failed";const TIMEOUT:&str="app_account_alive_timeout";const PROTOCOL:&str="app_account_alive_protocol_invalid";
struct IoGuard{scope:RpcConnectionScope,success:bool}
impl Drop for IoGuard{fn drop(&mut self){if !self.success{self.scope.close();}}}
pub(super) struct AliveIo{socket:TcpStream,main:RpcConnectionScope,scope:RpcConnectionScope,sender:RecordSender,receiver:RecordReceiver,
    frames:PacketStreamDecoder,assembler:CommonAssembler,controls:CommonControls,partial_end:Option<Instant>,partial:Duration,io:Duration}
impl Drop for AliveIo{fn drop(&mut self){self.scope.close();}}
impl AliveIo{
    pub(super) async fn connect(endpoint:&AccountAliveEndpoint,main:RpcConnectionScope,scope:RpcConnectionScope,deadline:Instant,partial:Duration,io:Duration)->Result<Self,&'static str>{
        if main.matches(&scope)||!main.is_open()||main.session_revoked()||!scope.is_open(){return Err(IO);}
        let connecting=async{
            let lookup=crate::platform::dns::prepare_socket_addrs(&endpoint.host,endpoint.port).map_err(|_|IO)?;
            let addresses=lookup.resolve().await.map_err(|_|IO)?;
            if addresses.is_empty()||addresses.len()>16{return Err(IO);}
            for address in addresses{
                if !main.is_open()||main.session_revoked()||!scope.is_open(){return Err(IO);}
                if let Ok(socket)=TcpStream::connect(address).await{socket.set_nodelay(true).map_err(|_|IO)?;return Ok(socket);}
            }
            Err(IO)
        };
        let socket=tokio::select!{biased;_=main.session_ended()=>return Err(IO),_=scope.closed()=>return Err(IO),
            result=tokio::time::timeout_at(deadline.into(),connecting)=>result.map_err(|_|TIMEOUT)??};
        if !main.is_open()||main.session_revoked()||!scope.is_open(){return Err(IO);}
        let mut seed=[0;4];SystemRandom::new().fill(&mut seed).map_err(|_|IO)?;
        let now=Instant::now();Ok(Self{socket,main,scope,sender:RecordSender::new(RecordSecurity::SeededXor,Some(u32::from_ne_bytes(seed))).map_err(|_|PROTOCOL)?,
            receiver:RecordReceiver::new(RecordSecurity::SeededXor),frames:PacketStreamDecoder::default(),assembler:CommonAssembler::new(partial,now).map_err(|_|PROTOCOL)?,
            controls:CommonControls::new(now),partial_end:None,partial,io})
    }
    fn live(&self)->bool{self.main.is_open()&&!self.main.session_revoked()&&self.scope.is_open()}
    pub(super) async fn send(&mut self,kind:CallKind,payload:&[u8],deadline:Instant)->Result<(),&'static str>{
        let mut guard=IoGuard{scope:self.scope.clone(),success:false};
        if !self.live(){return Err(IO);}
        let end=deadline.min(Instant::now().checked_add(self.io).ok_or(TIMEOUT)?);
        if Instant::now()>=end{return Err(TIMEOUT);}
        for packet in common_packets::encode(kind,payload,common_packets::NATIVE_FRAGMENT_THRESHOLD,None).map_err(|_|PROTOCOL)?{
            let record=self.sender.encode(packet.expose_to_transport()).map_err(|_|PROTOCOL)?;let frame=encode_packet_stream_frame(record.expose_to_transport()).map_err(|_|PROTOCOL)?;
            let data=frame.expose_to_transport();let mut written=0;
            while written<data.len(){
                let main=self.main.clone();let scope=self.scope.clone();let socket=&mut self.socket;
                let write=poll_fn(|cx|{if Instant::now()>=end{return Poll::Ready(Err(io::Error::from(io::ErrorKind::TimedOut)));}main.with_io_gate(||{
                    if main.session_revoked(){return Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted)));}
                    scope.with_io_gate(||Pin::new(&mut *socket).poll_write(cx,&data[written..])).unwrap_or(Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted))))
                }).unwrap_or(Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted))))});
                let count=tokio::select!{biased;_=main.session_ended()=>return Err(IO),_=scope.closed()=>return Err(IO),result=tokio::time::timeout_at(end.into(),write)=>result.map_err(|_|TIMEOUT)?.map_err(|_|IO)?};
                if count==0||Instant::now()>=end{return Err(IO);}written+=count;
            }
        }guard.success=true;Ok(())
    }
    /// Persistent decoders survive a timer wake; no partial read is restarted.
    /// A malformed later frame/control rejects this whole chunk before any
    /// registration result becomes visible to the owning worker.
    pub(super) async fn receive(&mut self,wake:Instant)->Result<Option<Vec<CommonMessage>>,&'static str>{
        let mut guard=IoGuard{scope:self.scope.clone(),success:false};
        if !self.live(){return Err(IO);}let now=Instant::now();self.assembler.tick(now).map_err(|_|PROTOCOL)?;self.controls.tick(now).map_err(|_|PROTOCOL)?;
        let partial_end=self.partial_end.into_iter().chain(self.assembler.deadline()).chain(self.controls.deadline()).min();
        if partial_end.is_some_and(|end|now>=end){return Err(TIMEOUT);}if now>=wake{guard.success=true;return Ok(None);}
        let deadline=partial_end.map_or(wake,|end|end.min(wake));let mut bytes=[0;8192];let mut buffer=ReadBuf::new(&mut bytes);
        let main=self.main.clone();let scope=self.scope.clone();let socket=&mut self.socket;
        let reading=poll_fn(|cx|main.with_io_gate(||{
            if main.session_revoked(){return Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted)));}
            scope.with_io_gate(||Pin::new(&mut *socket).poll_read(cx,&mut buffer)).unwrap_or(Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted))))
        }).unwrap_or(Poll::Ready(Err(io::Error::from(io::ErrorKind::Interrupted)))));
        let result=tokio::select!{biased;_=main.session_ended()=>return Err(IO),_=scope.closed()=>return Err(IO),result=tokio::time::timeout_at(deadline.into(),reading)=>result};
        let now=Instant::now();
        if result.is_err(){if partial_end.is_some_and(|end|now>=end){return Err(TIMEOUT);}guard.success=true;return Ok(None);}
        result.unwrap().map_err(|_|IO)?;if !self.live(){return Err(IO);}
        if now>=wake{return Err(TIMEOUT);}
        if partial_end.is_some_and(|end|now>=end){return Err(TIMEOUT);}
        if buffer.filled().is_empty(){self.frames.finish_eof().map_err(|_|PROTOCOL)?;self.assembler.finish_eof().map_err(|_|PROTOCOL)?;return Err(IO);}
        let was_partial=self.frames.has_partial();let frames=self.frames.on_bytes(buffer.filled()).map_err(|_|PROTOCOL)?;
        if self.frames.has_partial(){if !was_partial||!frames.is_empty(){self.partial_end=Some(now.checked_add(self.partial).ok_or(TIMEOUT)?);}}
        else{self.partial_end=None;}
        let records=frames.iter().map(|frame|self.receiver.decode(frame.expose_to_transport()).map_err(|_|PROTOCOL)).collect::<Result<Vec<_>,_>>()?;
        let packet_end=wake.min(now.checked_add(self.partial).ok_or(TIMEOUT)?);
        let messages=self.assembler.on_group(&records,packet_end,now).map_err(|_|PROTOCOL)?;let plan=self.controls.on_messages(messages,now).map_err(|_|PROTOCOL)?;
        for end in plan.pong_deadlines{self.send(CallKind::Pong,&[],end).await?;}
        guard.success=true;Ok(Some(plan.calls))
    }
}
