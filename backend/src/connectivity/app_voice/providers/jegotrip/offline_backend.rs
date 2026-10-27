//! Offline composition: owned ARC packets -> validated batch scheduler ->
//! original hosting callbacks / explicitly configured incoming message tickets.
//!
//! No HostingBackend, socket, endpoint, TLS trust/handshake, public inbound API,
//! provider registration or message/call readiness is supplied by this engine.
use super::{
    arc_protocol::{ArcProtocol, IncomingProtocolMessage, OwnedCallPackets, ProtocolError, ProtocolEvent},
    arc_session::{ArcLoginInputs, ArcPhase, ArcReply, OwnedBootstrapReply, OwnedLogoutReply},
    batch_scheduler::{BatchScheduler, ServerRequestTicket},
    call_binding::{CallIncomingBinding,CallDispatcher},
    call_delivery::{IncomingCallDelivery,CallReplyTicket,CallReplySource},
    call_control::{CallControlTicket,OwnedCallOperationReply},
    rpc_calls::{CallNoticeKind,NativeCallId,CallOperation,notice_kind,encode_notice_result},
    native_events::IncomingMessage,
    rpc_incoming::{encode_online_message_result, IncomingBinding, IncomingDispatcher},
    rpc_pending::{HostingRpcTracker, OwnedRpcCallback, OwnedUnhostingReply, RpcConnectionScope},
    packet_transport::{OwnedPacketTransport, PacketRoute},
    tls_engine::{TlsEngine, TlsError, TlsParameters, VerifiedPlaintextPackets},
    rpc_stream::{CodecError, RpcProperty},
    session::{AttemptOwner, GatewayRegistration},
};
use std::{collections::BTreeMap, fmt};

pub struct ScheduledIncomingMessage { ticket: ServerRequestTicket, message: IncomingMessage }

impl ScheduledIncomingMessage {
    pub fn owner(&self) -> &AttemptOwner { self.ticket.owner() }
    pub fn message(&self) -> &IncomingMessage { &self.message }
    pub fn ticket(&self) -> &ServerRequestTicket { &self.ticket }
}

impl fmt::Debug for ScheduledIncomingMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ScheduledIncomingMessage(<redacted>)") }
}

pub enum OfflineEvent {
    Bootstrap(OwnedBootstrapReply),
    Logout(OwnedLogoutReply),
    Unhosting(OwnedUnhostingReply),
    Callback(OwnedRpcCallback),
    Message(ScheduledIncomingMessage),
    DirectMessage(IncomingProtocolMessage),
    BatchRetirement(super::batch_scheduler::BatchReplayRetirement),
    Call(IncomingCallDelivery),
    CallOperation(OwnedCallOperationReply),
    Outgoing(OwnedCallPackets),
    Control(super::transport_packets::CallKind),
}

impl fmt::Debug for OfflineEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OfflineEvent(<redacted>)") }
}

pub(super) enum CallCompletion{Direct(OwnedCallPackets),BatchQueued}

pub struct OfflineBackend {
    protocol: ArcProtocol,
    tracker: HostingRpcTracker,
    scheduler: Option<BatchScheduler>,
    dispatcher: Option<IncomingDispatcher>,
    call_dispatcher:Option<CallDispatcher>,
    call_pending:BTreeMap<(u32,i32),(CallNoticeKind,NativeCallId)>,
    call_acks:BTreeMap<(u32,i32),(CallControlTicket,bool)>,
    swaps: BTreeMap<i32, u64>,
}

impl fmt::Debug for OfflineBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OfflineBackend").field("phase", &self.phase()).field("swaps", &self.swaps.len())
            .field("materials", &"<redacted>").finish()
    }
}

impl OfflineBackend {
    pub fn new(owner: AttemptOwner) -> Self {
        let mut protocol = ArcProtocol::new(owner);
        let tracker = protocol.take_hosting_tracker().expect("fresh ARC tracker");
        Self { protocol, tracker, scheduler: None, dispatcher: None, call_dispatcher:None,call_pending:BTreeMap::new(),call_acks:BTreeMap::new(),swaps: BTreeMap::new() }
    }
    pub fn phase(&self) -> ArcPhase { self.protocol.phase() }
    pub(super) fn is_pending(&self, id: i32) -> bool { self.protocol.is_pending(id) }
    pub fn connection_scope(&self) -> RpcConnectionScope { self.protocol.connection_scope() }
    /// Opaque lower stream adapter, not a raw-bytes-to-RPC shortcut. A future
    /// trusted TLS engine must still establish actual plaintext packet boundaries.
    pub fn take_packet_transport(&mut self, route: PacketRoute<'_>) -> Result<OwnedPacketTransport, ProtocolError> {
        self.protocol.take_packet_transport(route)
    }
    pub fn take_tls_engine(&mut self, route: PacketRoute<'_>, parameters: TlsParameters<'_>, now: std::time::Instant)
        -> Result<TlsEngine, TlsError> { self.protocol.take_tls_engine(route, parameters, now) }
    /// Only privately constructed, TLS-verified original-scope packets enter
    /// this path; opaque carriers cannot construct this delivery capability.
    pub fn on_verified_plaintext(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        plaintext: VerifiedPlaintextPackets, now_ms: u64) -> Result<Vec<OfflineEvent>, ProtocolError> {
        let Some(packets) = plaintext.packets_for(owner, scope) else { return Ok(vec![]); };
        let mut events = Vec::new();
        for packet in packets { events.extend(self.on_plaintext_packet(owner, scope, packet.expose_to_transport(), now_ms)?); }
        Ok(events)
    }
    pub fn close(&mut self) {
        self.protocol.close(); if let Some(s) = &mut self.scheduler { s.close(); }
        self.tracker.retire_all();
        self.call_dispatcher=None;self.call_pending.clear();self.call_acks.clear();
        self.swaps.clear();self.dispatcher=None;
    }
    pub(super) fn configure_call_control(&mut self,routing:&[RpcProperty<'_>])->Result<(),ProtocolError>{
        if self.call_dispatcher.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}
        self.protocol.configure_call_control(routing)
    }
    pub(super) fn call_acknowledgement_ready(&self,ticket:&CallControlTicket)->bool{self.protocol.call_acknowledgement_ready(ticket)}
    pub(super) fn prepare_call(&mut self,ticket:&CallControlTicket,operation:CallOperation,properties:&[RpcProperty<'_>])->Result<OwnedCallPackets,ProtocolError>{self.protocol.prepare_call(ticket,operation,properties)}
    pub fn prepare_login(&mut self, inputs: ArcLoginInputs<'_>, routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> { self.protocol.prepare_login(inputs, routing) }
    pub fn prepare_logout(&mut self, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        let packets=self.protocol.prepare_logout(routing)?;self.call_dispatcher=None;self.call_pending.clear();self.call_acks.clear();Ok(packets)
    }
    pub fn prepare_bootstrap(&mut self, key: &[u8], routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> { self.protocol.prepare_bootstrap(key, routing) }

    /// After matched ARC login, explicitly install original local epoch and
    /// registered incoming receiver. None keeps incoming unsupported, not ready.
    pub fn configure(&mut self, incarnation: u32, incoming: Option<IncomingBinding<'_>>) -> Result<(), ProtocolError> {
        if self.scheduler.is_some() || self.phase() != ArcPhase::Authenticated || incarnation == 0 {
            return Err(CodecError::InvalidField.into());
        }
        if let Some(binding) = incoming {
            let captured = IncomingBinding { object_id: binding.object_id, domain: binding.domain, receiver_kind: binding.receiver_kind };
            self.protocol.install_incoming(captured)?;
            self.dispatcher = Some(IncomingDispatcher::for_arc_session(self.tracker.batch_owner()?,
                self.protocol.connection_scope(), binding)?);
        }
        self.scheduler = Some(self.protocol.take_batch_scheduler(incarnation)?);
        Ok(())
    }

    pub fn configure_calls(&mut self,binding:CallIncomingBinding<'_>)->Result<(),ProtocolError>{
        if self.call_dispatcher.is_some()||self.scheduler.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}
        let copy=CallIncomingBinding{object_id:binding.object_id,domain:binding.domain};self.protocol.install_calls(copy)?;
        self.call_dispatcher=Some(CallDispatcher::for_arc_session(self.tracker.batch_owner()?,self.protocol.connection_scope(),binding)?);Ok(())
    }

    pub fn queue_hosting(&mut self, owner: AttemptOwner, uri: &str, status: i32,
        registration: &GatewayRegistration<'_>, metadata: &[RpcProperty<'_>]) -> Result<i32, ProtocolError> {
        let scheduler = self.scheduler.as_mut().ok_or(CodecError::InvalidField)?;
        let call = self.tracker.prepare_batch_update_status(owner, uri, status, registration)?;
        if let Err(error) = scheduler.queue_hosting(&call, metadata) {
            self.tracker.retire_request(call.sequence); return Err(error.into());
        }
        Ok(call.sequence)
    }

    /// No live loop: a future backend chooses scheduling/transport deadlines.
    /// All retransmits preserve retained nested sequences; never auto-login.
    pub fn prepare_swap(&mut self, now_ms: u64, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        let Some(scheduler) = self.scheduler.as_mut() else { return Err(CodecError::InvalidField.into()); };
        let batch = match scheduler.prepare(now_ms) {
            Ok(batch) => batch, Err(error) => { self.close(); return Err(error.into()); }
        };
        let result_sequences=batch.payload().results.keys().copied().collect::<Vec<_>>();
        let packets = match self.protocol.prepare_scheduled_swap(&batch, routing) {
            Ok(packets) => packets, Err(error) => { self.close(); return Err(error); }
        };
        if let Err(error) = self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.commit_submission(batch, now_ms) {
            self.close(); return Err(error.into());
        }
        for sequence in result_sequences{
            let key=self.call_acks.keys().find(|key|key.1==sequence).copied();
            if let Some(key)=key{if let Some((ticket,accepted))=self.call_acks.remove(&key){self.protocol.call_acknowledged(&ticket,accepted)?;}}
        }
        self.swaps.insert(packets.call_id, now_ms);
        Ok(packets)
    }

    pub fn retire_request(&mut self, call_id: i32) {
        self.protocol.retire_request(call_id); self.swaps.remove(&call_id);
        if self.phase() == ArcPhase::Closed { self.close(); }
    }

    pub fn retire_hosting(&mut self, sequence: i32) {
        self.tracker.retire_request(sequence);
        if let Some(scheduler) = &mut self.scheduler { scheduler.retire_client_request(sequence); }
        if self.phase() == ArcPhase::Closed { self.close(); }
    }

    /// Controls and messages are events only; the controller separately checks
    /// expiry/hosting/readiness, and the inbox consumer decides ticket acceptance.
    pub fn on_plaintext_packet(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        packet: &[u8], now_ms: u64) -> Result<Vec<OfflineEvent>, ProtocolError> {
        let mut events = Vec::new(); let mut processing_error = None;
        let scheduler = &mut self.scheduler; let tracker = &mut self.tracker;
        let dispatcher = &self.dispatcher;let call_dispatcher=&self.call_dispatcher;let call_pending=&mut self.call_pending;let swaps = &mut self.swaps;
        let result = self.protocol.on_plaintext_packet(owner, scope, packet, |event| {
            match event {
                ProtocolEvent::Reply(ArcReply::Login(callback)) | ProtocolEvent::Reply(ArcReply::Failure(callback)) =>
                    events.push(OfflineEvent::Callback(callback)),
                ProtocolEvent::Reply(ArcReply::Swap(reply)) => {
                    swaps.remove(&reply.call_id());
                    let Some(scheduler) = scheduler.as_mut() else { processing_error = Some(ProtocolError::Rpc(CodecError::InvalidField)); return; };
                    let mut delivery = match scheduler.on_reply(&reply, now_ms) {
                        Ok(Some(delivery)) => delivery, Ok(None) => return,
                        Err(error) => { processing_error = Some(error.into()); return; }
                    };
                    let retirement = delivery.retirement.take();
                    for callback in tracker.on_scheduled_delivery(&delivery) { events.push(OfflineEvent::Callback(callback)); }
                    for reply in tracker.take_unhosting_replies() { events.push(OfflineEvent::Unhosting(reply)); }
                    for incoming in delivery.requests {
                        if let Some(kind)=notice_kind(incoming.request().method){
                            let Some(dispatcher)=call_dispatcher.as_ref()else{processing_error=Some(super::transport_packets::TransportError::UnsupportedKind.into());return;};
                            let decoded=match dispatcher.on_batch_request(&incoming){Ok(value)=>value,Err(error)=>{processing_error=Some(error.into());return;}};
                            if let Some(notice)=decoded{
                                let key=(incoming.ticket().peer_incarnation(),incoming.ticket().sequence());
                                if call_pending.len()>=super::rpc_stream::MAX_MAP_ENTRIES||call_pending.contains_key(&key){processing_error=Some(CodecError::TooLarge.into());return;}
                                call_pending.insert(key,(kind,notice.native_id()));
                                match IncomingCallDelivery::from_batch(notice,incoming.into_ticket()){
                                    Ok(call)=>events.push(OfflineEvent::Call(call)),Err(error)=>{processing_error=Some(error.into());return;}
                                }
                            }else{
                                let result=match encode_notice_result(kind,false,&[]){Ok(bytes)=>bytes,Err(error)=>{processing_error=Some(error.into());return;}};
                                if let Err(error)=scheduler.complete_server_request(incoming.ticket(),0,result.expose_to_transport()){processing_error=Some(error.into());return;}
                            }
                            continue;
                        }
                        if incoming.request().method != super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()
                            || dispatcher.is_none() {
                            processing_error = Some(super::transport_packets::TransportError::UnsupportedKind.into());
                            return;
                        }
                        let message = match dispatcher.as_ref().map(|d| d.on_batch_request(&incoming)).transpose() {
                            Ok(Some(value)) => value, Ok(None) => None,
                            Err(error) => { processing_error = Some(error.into()); return; }
                        };
                        if let Some(message) = message {
                            events.push(OfflineEvent::Message(ScheduledIncomingMessage { ticket: incoming.into_ticket(), message }));
                        }
                        else {
                            let bytes = match encode_online_message_result(false) { Ok(bytes) => bytes, Err(error) => { processing_error = Some(error.into()); return; } };
                            if let Err(error) = scheduler.complete_server_request(incoming.ticket(), 0, bytes.expose_to_transport()) {
                                processing_error = Some(error.into()); return;
                            }
                        }
                    }
                    if let Some(retirement) = retirement { events.push(OfflineEvent::BatchRetirement(retirement)); }
                }
                ProtocolEvent::Reply(ArcReply::Logout(reply)) => events.push(OfflineEvent::Logout(reply)),
                ProtocolEvent::Reply(ArcReply::Call(reply))=>events.push(OfflineEvent::CallOperation(reply)),
                ProtocolEvent::Reply(ArcReply::Bootstrap(configuration)) => events.push(OfflineEvent::Bootstrap(configuration)),
                ProtocolEvent::Control(kind) => events.push(OfflineEvent::Control(kind)),
                ProtocolEvent::Incoming(message) => events.push(OfflineEvent::DirectMessage(message)),
                ProtocolEvent::IncomingCall(call)=>events.push(OfflineEvent::Call(call)),
                ProtocolEvent::OutgoingReply(reply) => events.push(OfflineEvent::Outgoing(reply)),
            }
        });
        if let Err(error) = result { self.close(); return Err(error); }
        if let Some(error) = processing_error { self.close(); return Err(error); }
        for event in &mut events{
            if let OfflineEvent::Call(call)=event{
                if matches!(call.ticket().source(),CallReplySource::Batch(_)){
                    let control=self.protocol.register_call_notice(call.notice())?;call.attach_control(control)?;
                }
            }
        }
        if self.phase() == ArcPhase::Closed { self.close(); }
        Ok(events)
    }

    pub fn complete_message(&mut self, ticket: &ServerRequestTicket, accepted: bool) -> Result<bool, ProtocolError> {
        if self.call_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence())){return Err(CodecError::InvalidField.into());}
        let bytes = encode_online_message_result(accepted)?;
        let result = self.scheduler.as_mut().ok_or(CodecError::InvalidField)?
            .complete_server_request(ticket, 0, bytes.expose_to_transport());
        match result { Ok(value) => Ok(value), Err(error) => {
            if self.phase() == ArcPhase::Closed { self.close(); }
            Err(error.into())
        } }
    }

    pub(super) fn direct_replay_frontier(&self) -> Option<super::arc_protocol::DirectReplayFrontier> {
        self.protocol.direct_replay_frontier()
    }
    pub fn complete_direct_message(&mut self, ticket: &super::arc_protocol::DirectReplyTicket, accepted: bool)
        -> Result<Option<OwnedCallPackets>, ProtocolError> { self.protocol.complete_incoming(ticket, accepted) }
    pub(super) fn complete_call(&mut self,ticket:&CallReplyTicket,accepted:bool,properties:&[RpcProperty<'_>])->Result<Option<CallCompletion>,ProtocolError>{
        if !ticket.matches(&self.tracker.batch_owner()?,&self.protocol.connection_scope()){return Ok(None);}
        match ticket.source(){
            CallReplySource::Direct(_)=>self.protocol.complete_call(ticket,accepted,properties).map(|value|value.map(CallCompletion::Direct)),
            CallReplySource::Batch(inner)=>{
                let key=(inner.peer_incarnation(),inner.sequence());
                if self.call_pending.get(&key)!=Some(&(ticket.kind(),ticket.native_id())){return Ok(None);}
                let result=encode_notice_result(ticket.kind(),accepted,properties)?;
                if self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.complete_server_request(inner,0,result.expose_to_transport())?{
                    if ticket.kind()==CallNoticeKind::Invited{if let Some(control)=ticket.control_ticket(){self.call_acks.insert(key,(control.captured_copy(),accepted));}}
                    self.call_pending.remove(&key);Ok(Some(CallCompletion::BatchQueued))
                }else{Ok(None)}
            }
        }
    }
}

#[cfg(test)]
#[path = "offline_backend_tests.rs"]
mod tests;
