//! Composes owned ARC RPC sessions with recovered call-packet framing.
//!
//! This is an offline protocol engine for a future Linux backend, NOT a
//! HostingBackend/provider. The caller must supply verified routing/config and
//! TLS plaintext packet boundaries. Socket, bootstrap routes, trust/handshake,
//! configured incoming-message dispatch are offline only, not wire readiness.
use super::{
    batch_scheduler::{BatchError, BatchScheduler, ScheduledBatch},
    call_binding::{CallIncomingBinding,CallDispatcher},
    call_delivery::{IncomingCallDelivery,CallReplyTicket,CallReplySource},
    call_control::CallControlTicket,
    rpc_calls::{CallNoticeKind,NativeCallId,CallOperation,encode_notice_result},
    arc_session::{ArcLoginInputs, ArcPhase, ArcReply, ArcRpcSession},
    rpc_batch::BatchPayload,
    rpc_envelope::{decode_request, encode_reply},
    rpc_incoming::{IncomingBinding, IncomingDispatcher, encode_online_message_result},
    rpc_pending::{HostingRpcTracker, RpcConnectionScope},
    packet_transport::{OwnedPacketTransport, PacketRoute},
    tls_engine::{TlsEngine, TlsError, TlsParameters},
    rpc_stream::{CodecError, RpcProperty},
    session::AttemptOwner,
    transport_packets::{encode_call_packets, CallAssembler, CallKind, TransportBytes, TransportError, TLS_FRAGMENT_THRESHOLD},
};
use std::{collections::BTreeMap, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError { Rpc(CodecError), Transport(TransportError), Batch(BatchError) }

impl From<CodecError> for ProtocolError { fn from(error: CodecError) -> Self { Self::Rpc(error) } }
impl From<TransportError> for ProtocolError { fn from(error: TransportError) -> Self { Self::Transport(error) } }
impl From<BatchError> for ProtocolError { fn from(error: BatchError) -> Self { Self::Batch(error) } }

pub struct OwnedCallPackets { pub call_id: i32, pub packets: Vec<TransportBytes> }

impl fmt::Debug for OwnedCallPackets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedCallPackets").field("call_id", &self.call_id)
            .field("packet_count", &self.packets.len()).field("data", &"<redacted>").finish()
    }
}

pub enum ProtocolEvent<'a> {
    Reply(ArcReply<'a>),
    Control(CallKind),
    Incoming(IncomingProtocolMessage),
    IncomingCall(IncomingCallDelivery),
    OutgoingReply(OwnedCallPackets),
}

pub struct DirectReplyTicket { owner: AttemptOwner, connection: RpcConnectionScope, call_id: i32 }

impl DirectReplyTicket {
    pub fn is_open(&self) -> bool { self.connection.is_open() }
    pub(super) fn request_id(&self) -> i32 { self.call_id }
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(scope) && self.connection.is_open()
    }
    pub(super) fn captured_copy(&self) -> Self {
        Self { owner: self.owner.clone(), connection: self.connection.clone(), call_id: self.call_id }
    }
}

impl fmt::Debug for DirectReplyTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("DirectReplyTicket(<redacted>)") }
}

pub struct IncomingProtocolMessage {
    owner: AttemptOwner,
    message: super::native_events::IncomingMessage,
    reply: DirectReplyTicket,
}

impl IncomingProtocolMessage {
    pub fn owner(&self) -> &AttemptOwner { &self.owner }
    pub fn message(&self) -> &super::native_events::IncomingMessage { &self.message }
    pub fn reply_ticket(&self) -> &DirectReplyTicket { &self.reply }
    pub fn into_parts(self) -> (AttemptOwner, super::native_events::IncomingMessage, DirectReplyTicket) {
        (self.owner, self.message, self.reply)
    }
}

impl fmt::Debug for IncomingProtocolMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingProtocolMessage(<redacted>)") }
}

#[derive(Clone,Copy)]
enum DirectResponse{Message,Call{kind:CallNoticeKind,native_id:NativeCallId}}
struct PendingDirect { magic: Vec<u8>, response:DirectResponse }

pub struct ArcProtocol {
    rpc: ArcRpcSession, assembler: CallAssembler, incoming: Option<IncomingDispatcher>,
    call_incoming:Option<CallDispatcher>,
    direct_pending: BTreeMap<i32, PendingDirect>, direct_seen: BTreeMap<i32, Vec<u8>>, direct_last_id: i32,
}

impl fmt::Debug for ArcProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ArcProtocol(<redacted>)") }
}

impl ArcProtocol {
    pub fn new(owner: AttemptOwner) -> Self {
        Self { rpc: ArcRpcSession::new(owner), assembler: CallAssembler::default(), incoming: None, call_incoming:None,
            direct_pending: BTreeMap::new(), direct_seen: BTreeMap::new(), direct_last_id: 0 }
    }

    pub fn connection_scope(&self) -> RpcConnectionScope { self.rpc.connection_scope() }
    pub fn phase(&self) -> ArcPhase { self.rpc.phase() }
    pub(super) fn is_pending(&self, id: i32) -> bool { self.rpc.is_pending(id) }
    pub fn take_packet_transport(&mut self, route: PacketRoute<'_>) -> Result<OwnedPacketTransport, ProtocolError> {
        Ok(self.rpc.take_packet_transport(route)?)
    }
    pub fn take_tls_engine(&mut self, route: PacketRoute<'_>, parameters: TlsParameters<'_>, now: std::time::Instant)
        -> Result<TlsEngine, TlsError> { self.rpc.take_tls_engine(route, parameters, now) }
    pub fn take_hosting_tracker(&mut self) -> Result<HostingRpcTracker, ProtocolError> {
        Ok(self.rpc.take_hosting_tracker()?)
    }
    pub fn retire_request(&mut self, id: i32) {
        self.rpc.retire_request(id);
        if self.rpc.phase() == ArcPhase::Closed { self.assembler.close(); }
    }
    pub fn close(&mut self) {
        self.rpc.close(); self.assembler.close(); self.incoming = None; self.call_incoming=None; self.direct_pending.clear(); self.direct_seen.clear();
    }

    pub fn take_batch_scheduler(&mut self, incarnation: u32) -> Result<BatchScheduler, ProtocolError> {
        Ok(self.rpc.take_batch_scheduler(incarnation)?)
    }

    pub fn install_incoming(&mut self, binding: IncomingBinding<'_>) -> Result<(), ProtocolError> {
        if self.incoming.is_some() { return Err(CodecError::InvalidField.into()); }
        self.incoming = Some(self.rpc.incoming_dispatcher(binding)?);
        Ok(())
    }

    pub fn install_calls(&mut self,binding:CallIncomingBinding<'_>)->Result<(),ProtocolError>{
        if self.call_incoming.is_some(){return Err(CodecError::InvalidField.into());}
        self.call_incoming=Some(self.rpc.incoming_call_dispatcher(binding)?);Ok(())
    }

    pub(super) fn configure_call_control(&mut self,routing:&[RpcProperty<'_>])->Result<(),ProtocolError>{Ok(self.rpc.configure_call_control(routing)?)}
    pub(super) fn register_call_notice(&mut self,notice:&super::call_binding::OwnedCallNotice)->Result<Option<CallControlTicket>,ProtocolError>{Ok(self.rpc.register_call_notice(notice)?)}
    pub(super) fn call_acknowledged(&mut self,ticket:&CallControlTicket,accepted:bool)->Result<(),ProtocolError>{Ok(self.rpc.call_acknowledged(ticket,accepted)?)}
    pub(super) fn call_acknowledgement_ready(&self,ticket:&CallControlTicket)->bool{self.rpc.call_acknowledgement_ready(ticket)}
    pub(super) fn prepare_call(&mut self,ticket:&CallControlTicket,operation:CallOperation,properties:&[RpcProperty<'_>])->Result<OwnedCallPackets,ProtocolError>{
        let(id,bytes)=self.rpc.prepare_call(ticket,operation,properties)?;self.packets(id,bytes.expose_to_transport())
    }

    pub fn prepare_scheduled_swap(&mut self, batch: &ScheduledBatch, routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> {
        let (id, bytes) = self.rpc.prepare_scheduled_swap(batch, routing)?;
        self.packets(id, bytes.expose_to_transport())
    }

    fn packets(&mut self, id: i32, bytes: &[u8]) -> Result<OwnedCallPackets, ProtocolError> {
        match encode_call_packets(CallKind::Request, bytes, TLS_FRAGMENT_THRESHOLD) {
            Ok(packets) => Ok(OwnedCallPackets { call_id: id, packets }),
            Err(error) => { self.rpc.retire_request(id); self.close(); Err(error.into()) }
        }
    }

    pub fn prepare_bootstrap(&mut self, application_key: &[u8], routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> {
        let (id, bytes) = self.rpc.prepare_bootstrap(application_key, routing)?;
        self.packets(id, bytes.expose_to_transport())
    }

    pub fn prepare_login(&mut self, inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> {
        let (id, bytes) = self.rpc.prepare_login(inputs, routing)?;
        self.packets(id, bytes.expose_to_transport())
    }

    pub fn prepare_logout(&mut self, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        let (id, bytes) = self.rpc.prepare_logout(routing)?;
        self.incoming = None; self.call_incoming=None; self.direct_pending.clear(); self.direct_seen.clear();
        self.packets(id, bytes.expose_to_transport())
    }

    pub fn prepare_swap_calls(&mut self, payload: &BatchPayload<'_>, routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> {
        let (id, bytes) = self.rpc.prepare_swap_calls(payload, routing)?;
        self.packets(id, bytes.expose_to_transport())
    }

    /// The supplied packet is from the original authenticated TLS engine, not
    /// raw TCP/HTTP/API or a native notification. Old scopes are rejected before
    /// decoding/copying, so stale input cannot poison a newer fragment assembly.
    /// Reply contents are passed synchronously to avoid storing borrowed data or
    /// assigning their owner on receipt. Without an explicitly configured
    /// original receiver binding, server requests fail closed. This method is
    /// not a public inbound-SMS endpoint or a raw TCP/TLS read decoder.
    pub fn on_plaintext_packet<F>(&mut self, owner: &AttemptOwner, connection: &RpcConnectionScope,
        packet: &[u8], deliver: F) -> Result<bool, ProtocolError>
        where F: for<'a> FnOnce(ProtocolEvent<'a>) {
        if self.rpc.phase() == ArcPhase::Closed { self.close(); return Ok(false); }
        if !self.rpc.accepts_scope(owner, connection) { return Ok(false); }
        let message = match self.assembler.on_packet(packet) {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(false),
            Err(error) => { self.close(); return Err(error.into()); }
        };
        match message.kind {
            CallKind::Reply => match self.rpc.on_reply(owner, connection, message.payload.expose_to_transport()) {
                Ok(Some(reply)) => {
                    if self.rpc.phase() == ArcPhase::Closed { self.close(); }
                    deliver(ProtocolEvent::Reply(reply));
                    Ok(true)
                }
                Ok(None) => Ok(false),
                Err(error) => { self.close(); Err(error.into()) }
            },
            CallKind::Request if matches!(self.rpc.phase(), ArcPhase::LogoutPending | ArcPhase::LoggedOut) => Ok(false),
            CallKind::Request => self.incoming_request(owner, connection, message.payload.expose_to_transport(), deliver),
            kind => {
                if kind == CallKind::Release { self.close(); }
                deliver(ProtocolEvent::Control(kind));
                Ok(true)
            }
        }
    }

    /// The input is a complete trusted plaintext call packet; direct server IDs
    /// must be monotonic/non-reused in this subset. Binary magic duplicates are
    /// stopped before inbox delivery. No accepted reply is emitted until the
    /// local consumer explicitly completes its original ticket.
    fn incoming_request<F>(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, bytes: &[u8], deliver: F)
        -> Result<bool, ProtocolError> where F: for<'a> FnOnce(ProtocolEvent<'a>) {
        if self.rpc.phase() != ArcPhase::Authenticated || self.incoming.is_none() && self.call_incoming.is_none() {
            self.close(); return Err(TransportError::UnsupportedKind.into());
        }
        let request = match decode_request(bytes) { Ok(request) => request, Err(error) => { self.close(); return Err(error.into()); } };
        let call_kind=match request.method {
            method if method==super::rpc_calls::INVITED_METHOD.as_bytes()=>Some(CallNoticeKind::Invited),
            method if method==super::rpc_calls::RELEASED_METHOD.as_bytes()=>Some(CallNoticeKind::Released),
            method if method==super::rpc_calls::ACCEPTED_METHOD.as_bytes()=>Some(CallNoticeKind::Accepted),_=>None,
        };
        if request.method != super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes() && call_kind.is_none() {
            self.close(); return Err(TransportError::UnsupportedKind.into());
        }
        if self.direct_pending.contains_key(&request.call_id) || self.direct_seen.contains_key(&request.call_id) { return Ok(false); }
        if request.call_id <= self.direct_last_id { self.close(); return Err(CodecError::InvalidCallId.into()); }
        let Some(magic) = request.metadata.iter().find(|p| p.key == b"__magic").map(|p| p.value) else {
            self.close(); return Err(CodecError::InvalidField.into());
        };
        if magic.is_empty() { self.close(); return Err(CodecError::InvalidField.into()); }
        if self.direct_pending.values().any(|p| p.magic == magic) || self.direct_seen.values().any(|prior| prior == magic) {
            return Ok(false);
        }
        let retained_bytes = self.direct_pending.values().map(|p| p.magic.len()).sum::<usize>()
            + self.direct_seen.values().map(Vec::len).sum::<usize>();
        if self.direct_pending.len() + self.direct_seen.len() >= super::rpc_stream::MAX_MAP_ENTRIES
            || magic.len() > 1024
            || retained_bytes + magic.len() > super::rpc_stream::MAX_STREAM_BYTES {
            self.close(); return Err(CodecError::TooLarge.into());
        }
        if let Some(kind)=call_kind{
            let dispatcher=match self.call_incoming.as_ref(){Some(value)=>value,None=>{self.close();return Err(TransportError::UnsupportedKind.into());}};
            let decoded=match dispatcher.on_direct_request(owner,scope,&request){Ok(value)=>value,Err(error)=>{self.close();return Err(error.into());}};
            self.direct_last_id=request.call_id;
            if let Some(notice)=decoded{
                let control=self.rpc.register_call_notice(&notice)?;
                self.direct_pending.insert(request.call_id,PendingDirect{magic:magic.to_vec(),response:DirectResponse::Call{kind,native_id:notice.native_id()}});
                let original=self.rpc.owner().clone();let ticket=DirectReplyTicket{owner:original,connection:self.rpc.connection_scope(),call_id:request.call_id};
                deliver(ProtocolEvent::IncomingCall(IncomingCallDelivery::from_direct(notice,ticket)?.with_control(control)?));
            }else{
                let result=encode_notice_result(kind,false,&[])?;let reply=encode_reply(request.call_id,0,result.expose_to_transport())?;
                let packets=encode_call_packets(CallKind::Reply,reply.expose_to_transport(),TLS_FRAGMENT_THRESHOLD)?;
                self.record_direct(request.call_id,magic.to_vec());deliver(ProtocolEvent::OutgoingReply(OwnedCallPackets{call_id:request.call_id,packets}));
            }
            return Ok(true);
        }
        let decoded = match self.incoming.as_ref().ok_or(CodecError::InvalidField)?.on_direct_request(owner, scope, &request) {
            Ok(value) => value, Err(error) => { self.close(); return Err(error.into()); }
        };
        self.direct_last_id = request.call_id;
        if let Some(message) = decoded {
            self.direct_pending.insert(request.call_id, PendingDirect { magic: magic.to_vec(), response:DirectResponse::Message });
            let original_owner = self.rpc.owner().clone();
            deliver(ProtocolEvent::Incoming(IncomingProtocolMessage { owner: original_owner.clone(), message,
                reply: DirectReplyTicket { owner: original_owner, connection: self.rpc.connection_scope(), call_id: request.call_id } }));
        } else {
            let result = encode_online_message_result(false)?;
            let reply = encode_reply(request.call_id, 0, result.expose_to_transport())?;
            let packets = encode_call_packets(CallKind::Reply, reply.expose_to_transport(), TLS_FRAGMENT_THRESHOLD)?;
            self.record_direct(request.call_id, magic.to_vec());
            deliver(ProtocolEvent::OutgoingReply(OwnedCallPackets { call_id: request.call_id, packets }));
        }
        Ok(true)
    }

    fn record_direct(&mut self, id: i32, magic: Vec<u8>) {
        self.direct_seen.insert(id, magic);
        // No eviction/replay shortcut. Capacity pressure terminates the scope
        // before another inbox event, rather than forgetting duplicate keys.
    }

    pub fn complete_incoming(&mut self, ticket: &DirectReplyTicket, accepted: bool)
        -> Result<Option<OwnedCallPackets>, ProtocolError> {
        if self.rpc.phase() == ArcPhase::Closed { self.close(); return Ok(None); }
        if !self.rpc.accepts_scope(&ticket.owner, &ticket.connection) { return Ok(None); }
        if self.direct_pending.get(&ticket.call_id).is_some_and(|pending|!matches!(pending.response,DirectResponse::Message)){
            return Err(CodecError::InvalidField.into());
        }
        let Some(pending) = self.direct_pending.remove(&ticket.call_id) else { return Ok(None); };
        let result = encode_online_message_result(accepted)?;
        let reply = encode_reply(ticket.call_id, 0, result.expose_to_transport())?;
        let packets = encode_call_packets(CallKind::Reply, reply.expose_to_transport(), TLS_FRAGMENT_THRESHOLD)?;
        self.record_direct(ticket.call_id, pending.magic);
        Ok(Some(OwnedCallPackets { call_id: ticket.call_id, packets }))
    }
    pub fn complete_call(&mut self,ticket:&CallReplyTicket,accepted:bool,properties:&[RpcProperty<'_>])->Result<Option<OwnedCallPackets>,ProtocolError>{
        let CallReplySource::Direct(inner)=ticket.source()else{return Err(CodecError::InvalidField.into());};
        if !ticket.matches(self.rpc.owner(),&self.rpc.connection_scope()) || !self.rpc.accepts_scope(&inner.owner,&inner.connection){return Ok(None);}
        let Some(pending)=self.direct_pending.get(&inner.call_id)else{return Ok(None);};
        if !matches!(&pending.response,DirectResponse::Call{kind,native_id} if *kind==ticket.kind()&&*native_id==ticket.native_id()){
            return Err(CodecError::InvalidField.into());
        }
        // Construct/validate the entire reply before consuming the original
        // pending capability; malformed local properties retain its retry.
        let result=encode_notice_result(ticket.kind(),accepted,properties)?;let reply=encode_reply(inner.call_id,0,result.expose_to_transport())?;
        let packets=encode_call_packets(CallKind::Reply,reply.expose_to_transport(),TLS_FRAGMENT_THRESHOLD)?;
        if ticket.kind()==CallNoticeKind::Invited{if let Some(control)=ticket.control_ticket(){self.rpc.call_acknowledged(control,accepted)?;}}
        let pending=self.direct_pending.remove(&inner.call_id).ok_or(CodecError::InvalidField)?;
        self.record_direct(inner.call_id,pending.magic);Ok(Some(OwnedCallPackets{call_id:inner.call_id,packets}))
    }
}

impl Drop for ArcProtocol {
    fn drop(&mut self) { self.close(); }
}

#[cfg(test)]
#[path = "arc_protocol_tests.rs"]
mod tests;
