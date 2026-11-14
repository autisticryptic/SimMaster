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
    OfflineHint(super::rpc_offline::IncomingOfflineHint),
    OfflinePage(super::rpc_offline::OwnedOfflinePage),
    ClientEx(super::rpc_client_ex::OwnedClientExNotice),
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
    client_ex: Option<super::rpc_client_ex::ClientExDispatcher>,
    call_pending:BTreeMap<(u32,i32),(CallNoticeKind,NativeCallId)>,
    offline_hints: std::collections::BTreeSet<(u32, i32)>,
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
        Self { protocol, tracker, scheduler: None, dispatcher: None, call_dispatcher:None,client_ex:None,call_pending:BTreeMap::new(),offline_hints:std::collections::BTreeSet::new(),call_acks:BTreeMap::new(),swaps: BTreeMap::new() }
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
    pub(super) fn alive_link(&self)->std::sync::Arc<super::alive_session::AliveLink>{self.protocol.alive_link()}
    pub(super) fn claim_common_transport(&mut self)->Result<(),ProtocolError>{self.protocol.claim_common_transport()}
    pub(super) fn bind_common_identity(&mut self,identity:&[u8])->Result<(),ProtocolError>{self.protocol.bind_common_identity(identity)}
    /// Only original CommonPath-produced groups enter this bridge. Normalising
    ///a completed logical message to the existing protocol parser does not
    ///forge VerifiedPlaintextPackets, an innerTLS handshake, or a new scope.
    pub(super) fn on_common_calls(&mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,
        path:&super::common_runtime::CommonPath,group:&super::common_runtime::ScopedCommonCalls,now_ms:u64)->Result<Vec<OfflineEvent>,ProtocolError>{
        if !self.connection_scope().matches(scope){return Ok(Vec::new());}
        let Some(calls)=path.calls_for(owner,scope,group) else{return Ok(Vec::new());};
        let mut events=Vec::new();
        for call in calls{
            if !matches!(call.kind,super::transport_packets::CallKind::Request|super::transport_packets::CallKind::Reply){self.close();return Err(super::transport_packets::TransportError::UnsupportedKind.into());}
            let mut packet=zeroize::Zeroizing::new(Vec::with_capacity(call.payload.expose_to_transport().len()+1));
            packet.push(call.kind as u8);packet.extend_from_slice(call.payload.expose_to_transport());
            events.extend(self.on_plaintext_packet(owner,scope,&packet,now_ms)?);
        }
        Ok(events)
    }
    pub fn close(&mut self) {
        self.protocol.close(); if let Some(s) = &mut self.scheduler { s.close(); }
        self.tracker.retire_all();
        self.call_dispatcher=None;self.client_ex=None;self.call_pending.clear();self.call_acks.clear();self.offline_hints.clear();
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
        let packets=self.protocol.prepare_logout(routing)?;self.call_dispatcher=None;self.client_ex=None;self.call_pending.clear();self.call_acks.clear();self.offline_hints.clear();Ok(packets)
    }
    pub fn prepare_bootstrap(&mut self, key: &[u8], routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> { self.protocol.prepare_bootstrap(key, routing) }

    pub(super) fn prepare_offline(&mut self, position: i64, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        if self.dispatcher.is_none() || self.scheduler.is_none() { return Err(CodecError::InvalidField.into()); }
        self.protocol.prepare_offline(position, routing)
    }

    /// After matched ARC login, explicitly install original local epoch and
    /// registered incoming receiver. None keeps incoming unsupported, not ready.
    pub fn configure(&mut self, incarnation: u32, incoming: Option<IncomingBinding<'_>>) -> Result<(), ProtocolError> {
        if self.scheduler.is_some() || self.protocol.native_setup().is_some() || self.phase() != ArcPhase::Authenticated || incarnation == 0 {
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

    pub(super) fn arm_native_receivers(&mut self, setup: std::sync::Arc<super::native_receiver::NativeReceiverSetup>) -> Result<(), ProtocolError> {
        self.protocol.arm_native_receivers(setup)
    }
    pub(super) fn configure_native_receivers(&mut self, incarnation: u32) -> Result<(), ProtocolError> {
        if self.scheduler.is_some() || self.dispatcher.is_some() || self.call_dispatcher.is_some() || self.client_ex.is_some()
            || self.protocol.native_setup().is_none() || incarnation == 0 { return Err(CodecError::InvalidField.into()); }
        // The original login synchronously activated the direct dispatchers.
        // Install the SAME native policy for authenticated batch delivery, not
        // a copied/empty legacy domain or a receiver inferred from this request.
        self.dispatcher = Some(self.protocol.native_message_dispatcher()?);
        self.call_dispatcher = Some(self.protocol.native_call_dispatcher()?);
        self.client_ex = self.protocol.captured_client_ex();
        self.scheduler = Some(self.protocol.take_batch_scheduler(incarnation)?);
        Ok(())
    }

    pub(super) fn arm_client_ex(&mut self, binding: IncomingBinding<'_>) -> Result<(), ProtocolError> {
        self.protocol.arm_client_ex(binding)
    }
    pub(super) fn configure_client_ex(&mut self) -> Result<(), ProtocolError> {
        if self.client_ex.is_some() || self.scheduler.is_none() || self.dispatcher.is_none() || self.phase() != ArcPhase::Authenticated {
            return Err(CodecError::InvalidField.into());
        }
        if self.protocol.captured_client_ex().is_none() { self.protocol.install_client_ex()?; }
        self.client_ex = Some(self.protocol.captured_client_ex().ok_or(CodecError::InvalidField)?); Ok(())
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
        let offline_hints = &mut self.offline_hints;
        let client_ex = &self.client_ex;
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
                        if super::rpc_client_ex::method_kind(incoming.request().method).is_some() {
                            let action = match client_ex.as_ref().map(|dispatcher| dispatcher.on_batch(&incoming)).transpose() {
                                Ok(Some(Some(action))) => action,
                                _ => { processing_error = Some(CodecError::InvalidField.into()); return; }
                            };
                            // Control-result queuing is not a SMS/call consumer
                            // ACK or proof of remote delivery. A valid revocation
                            // has already been locked onto this original scope.
                            match scheduler.complete_server_request(incoming.ticket(), 0, action.result.expose_to_transport()) {
                                Ok(true) => {},
                                Ok(false) => { processing_error = Some(CodecError::InvalidField.into()); return; },
                                Err(error) => { processing_error = Some(error.into()); return; },
                            }
                            if let Some(notice) = action.notice {
                                let revoked = notice.is_revocation_for(owner, scope);
                                events.push(OfflineEvent::ClientEx(notice));
                                if revoked { break; }
                            }
                            continue;
                        }
                        if incoming.request().method == super::rpc_offline::MORE_OFFLINE_METHOD.as_bytes() {
                            let position = match dispatcher.as_ref().map(|dispatcher| dispatcher.on_batch_offline_hint(&incoming)).transpose() {
                                Ok(Some(Some(position))) => position,
                                _ => { processing_error = Some(CodecError::InvalidField.into()); return; }
                            };
                            let key = (incoming.ticket().peer_incarnation(), incoming.ticket().sequence());
                            if offline_hints.len() >= super::rpc_stream::MAX_MAP_ENTRIES || !offline_hints.insert(key) {
                                processing_error = Some(CodecError::TooLarge.into()); return;
                            }
                            events.push(OfflineEvent::OfflineHint(super::rpc_offline::IncomingOfflineHint::batch(incoming.ticket().owner().clone(),
                                scope.clone(), position, incoming.into_ticket())));
                            continue;
                        }
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
                ProtocolEvent::Reply(ArcReply::Offline(page)) => events.push(OfflineEvent::OfflinePage(page)),
                ProtocolEvent::OfflineHint(hint) => events.push(OfflineEvent::OfflineHint(hint)),
                ProtocolEvent::Reply(ArcReply::Bootstrap(configuration)) => events.push(OfflineEvent::Bootstrap(configuration)),
                ProtocolEvent::Control(kind) => events.push(OfflineEvent::Control(kind)),
                ProtocolEvent::Incoming(message) => events.push(OfflineEvent::DirectMessage(message)),
                ProtocolEvent::IncomingCall(call)=>events.push(OfflineEvent::Call(call)),
                ProtocolEvent::ClientEx { reply, notice } => {
                    events.push(OfflineEvent::Outgoing(reply));
                    if let Some(notice) = notice { events.push(OfflineEvent::ClientEx(notice)); }
                }
                ProtocolEvent::OutgoingReply(reply) => events.push(OfflineEvent::Outgoing(reply)),
            }
        });
        if let Err(error) = result { self.close(); return Err(error); }
        if let Some(error) = processing_error { self.close(); return Err(error); }
        for event in &mut events{
            if scope.session_revoked() { break; }
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
        if self.call_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence())) || self.offline_hints.contains(&(ticket.peer_incarnation(),ticket.sequence())) {return Err(CodecError::InvalidField.into());}
        let bytes = encode_online_message_result(accepted)?;
        let result = self.scheduler.as_mut().ok_or(CodecError::InvalidField)?
            .complete_server_request(ticket, 0, bytes.expose_to_transport());
        match result { Ok(value) => Ok(value), Err(error) => {
            if self.phase() == ArcPhase::Closed { self.close(); }
            Err(error.into())
        } }
    }

    pub(super) fn complete_offline_hint(&mut self, hint: &super::rpc_offline::IncomingOfflineHint, accepted: bool)
        -> Result<Option<CallCompletion>, ProtocolError> {
        if !hint.matches(&self.tracker.batch_owner()?, &self.protocol.connection_scope()) { return Ok(None); }
        match hint.ticket() {
            super::rpc_offline::OfflineHintTicket::Direct(ticket) => self.protocol.complete_offline_hint(ticket, accepted).map(|packets| packets.map(CallCompletion::Direct)),
            super::rpc_offline::OfflineHintTicket::Batch(ticket) => {
                let key = (ticket.peer_incarnation(), ticket.sequence());
                if !self.offline_hints.contains(&key) { return Ok(None); }
                let result = encode_online_message_result(accepted)?;
                if self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.complete_server_request(ticket, 0, result.expose_to_transport())? {
                    self.offline_hints.remove(&key); Ok(Some(CallCompletion::BatchQueued))
                } else { Ok(None) }
            }
        }
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
