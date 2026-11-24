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
    InviteNotification(Box<super::native_invite_notifications::IncomingInviteNotification>),
    Im(Box<super::im_delivery::IncomingImDelivery>),
    ImStatus(Box<super::im_status_delivery::IncomingImStatus>),
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
    im_binding: Option<std::sync::Arc<super::im_delivery::ImBinding>>,
    im_pending:BTreeMap<(u32,i32),([u8;32],std::time::Instant)>,
    im_completed:BTreeMap<(u32,i32),(super::im_delivery::ImReplyTicket,super::im_inbox::CommittedIm)>,
    status_pending:BTreeMap<(u32,i32),([u8;32],std::time::Instant)>,
    status_completed:BTreeMap<(u32,i32),(super::im_status_delivery::ImStatusTicket,super::im_inbox::CommittedImStatus)>,
    call_pending:BTreeMap<(u32,i32),(CallNoticeKind,NativeCallId)>,
    invite_pending:BTreeMap<(u32,i32),(u64,[u8;32],std::time::Instant)>,
    invite_completed:BTreeMap<(u32,i32),super::native_invite_notifications::InviteReplyTicket>,
    offline_hints: std::collections::BTreeSet<(u32, i32)>,
    call_acks:BTreeMap<(u32,i32),(CallControlTicket,bool)>,
    swaps: BTreeMap<i32, u64>,
    client_actor:super::transport_actor::ClientReadState,
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
        Self { protocol, tracker, scheduler: None, dispatcher: None, call_dispatcher:None,client_ex:None,im_binding:None,im_pending:BTreeMap::new(),im_completed:BTreeMap::new(),status_pending:BTreeMap::new(),status_completed:BTreeMap::new(),call_pending:BTreeMap::new(),invite_pending:BTreeMap::new(),invite_completed:BTreeMap::new(),offline_hints:std::collections::BTreeSet::new(),call_acks:BTreeMap::new(),swaps: BTreeMap::new(),client_actor:super::transport_actor::ClientReadState::default() }
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
        self.tracker.retire_all();self.client_actor.clear();
        self.call_dispatcher=None;self.client_ex=None;self.call_pending.clear();self.invite_pending.clear();self.invite_completed.clear();self.call_acks.clear();self.offline_hints.clear();
        self.swaps.clear();self.dispatcher=None;self.im_binding=None;self.im_pending.clear();self.im_completed.clear();self.status_pending.clear();self.status_completed.clear();
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
        let packets=self.protocol.prepare_logout(routing)?;self.status_pending.clear();self.status_completed.clear();self.im_binding=None;self.im_pending.clear();self.im_completed.clear();self.call_dispatcher=None;self.client_ex=None;self.call_pending.clear();self.invite_pending.clear();self.invite_completed.clear();self.call_acks.clear();self.offline_hints.clear();Ok(packets)
    }
    pub fn prepare_bootstrap(&mut self, key: &[u8], routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> { self.protocol.prepare_bootstrap(key, routing) }

    pub(super) fn prepare_im_read(&mut self,query:std::sync::Arc<super::rpc_im::ImQuery>,gate:std::sync::Arc<super::im_inbox::ImReceiptGate>,deadline:std::time::Instant)->Result<OwnedCallPackets,ProtocolError>{
        if self.im_binding.is_none()||self.scheduler.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}
        self.protocol.prepare_im_read(query,gate,deadline)
    }
    pub(super) fn take_im_replies(&mut self)->Vec<super::rpc_im::OwnedImReply>{self.protocol.take_im_replies()}
    pub(super) fn retire_im_read(&mut self,id:i32){self.protocol.retire_im_read(id);}
    pub(super) fn prepare_received_write(&mut self,permit:std::sync::Arc<super::im_inbox::ReceivedWritePermit>,deadline:std::time::Instant)->Result<OwnedCallPackets,ProtocolError>{
        if self.im_binding.is_none()||self.scheduler.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}
        self.protocol.prepare_received_write(permit,deadline)
    }
    pub(super) fn take_received_replies(&mut self)->Vec<super::rpc_im_received::OwnedReceivedReply>{self.protocol.take_received_replies()}
    pub(super) fn retire_received_write(&mut self,id:i32){self.protocol.retire_received_write(id);}
    pub(super) fn prepare_im_send(&mut self,permit:std::sync::Arc<super::im_inbox::ImSendPermit>,deadline:std::time::Instant)->Result<OwnedCallPackets,ProtocolError>{if self.im_binding.is_none()||self.scheduler.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}self.protocol.prepare_im_send(permit,deadline)}
    pub(super) fn take_im_send_replies(&mut self)->Vec<super::rpc_im_send::OwnedImSendReply>{self.protocol.take_im_send_replies()}
    pub(super) fn retire_im_send(&mut self,id:i32){self.protocol.retire_im_send(id);}
    pub(super) fn captured_send_identity(&self)->Result<std::sync::Arc<super::arc_session::ImSendIdentity>,ProtocolError>{if self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}self.protocol.captured_send_identity().map_err(Into::into)}
    pub(super) fn captured_relation_identity(&self)->Result<std::sync::Arc<super::arc_session::RelationIdentity>,ProtocolError>{if self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}self.protocol.captured_relation_identity().map_err(Into::into)}
    pub(super) fn prepare_relation_update(&mut self,permit:std::sync::Arc<super::relation_journal::RelationPermit>,deadline:std::time::Instant)->Result<OwnedCallPackets,ProtocolError>{
        if self.im_binding.is_none()||self.scheduler.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}self.protocol.prepare_relation_update(permit,deadline)
    }
    pub(super) fn take_relation_replies(&mut self)->Vec<super::rpc_relation_update::OwnedRelationReply>{self.protocol.take_relation_replies()}
    pub(super) fn retire_relation_update(&mut self,id:i32){self.protocol.retire_relation_update(id);}
    pub(super) fn captured_user_entry_auth(&self)->Result<Box<super::arc_session::UserEntryAuth>,ProtocolError>{
        if self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}self.protocol.captured_user_entry_auth().map_err(Into::into)
    }
    pub(super) fn captured_im_binding(&self)->Option<std::sync::Arc<super::im_delivery::ImBinding>>{
        if self.phase()!=ArcPhase::Authenticated{return None;}self.im_binding.clone()
    }
    pub(super) fn prepare_user_read_write(&mut self,permit:std::sync::Arc<super::im_inbox::UserReadPermit>,deadline:std::time::Instant)->Result<OwnedCallPackets,ProtocolError>{
        if self.im_binding.is_none()||self.scheduler.is_none()||self.phase()!=ArcPhase::Authenticated{return Err(CodecError::InvalidField.into());}self.protocol.prepare_user_read_write(permit,deadline)
    }
    pub(super) fn take_user_read_replies(&mut self)->Vec<super::im_user_read_rpc::OwnedUserReadReply>{self.protocol.take_user_read_replies()}
    pub(super) fn retire_user_read_write(&mut self,id:i32){self.protocol.retire_user_read_write(id);}

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
        self.im_binding = self.protocol.captured_im_binding();
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
    pub(super) fn invite_write_guard(&self) -> (Vec<std::sync::Arc<super::native_invite_notifications::ConsumerClaim>>, Option<std::time::Instant>) {
        let claims = self.invite_completed.values().filter_map(|ticket| ticket.consumer_claim()).collect();
        let deadline = self.invite_completed.values().map(|ticket| ticket.completion_deadline()).min();
        (claims, deadline)
    }
    pub(super) fn im_write_guard(&self)->(Vec<std::sync::Arc<super::im_inbox::ImReceiptGate>>,Option<std::time::Instant>){
        (self.im_completed.values().map(|(_,proof)|proof.gate()).chain(self.status_completed.values().map(|(_,proof)|proof.gate())).collect(),self.im_completed.values().map(|(ticket,_)|ticket.completion_deadline()).chain(self.status_completed.values().map(|(ticket,_)|ticket.deadline())).min())
    }
    pub fn prepare_swap(&mut self, now_ms: u64, routing: &[RpcProperty<'_>]) -> Result<OwnedCallPackets, ProtocolError> {
        if self.status_completed.values().any(|(ticket,proof)|!proof.authorizes(ticket,std::time::Instant::now())){
            self.close();return Err(CodecError::InvalidField.into());
        }
        if self.im_completed.values().any(|(ticket,proof)|!proof.authorizes(ticket,std::time::Instant::now())){
            self.close();return Err(CodecError::InvalidField.into());
        }
        // A queued positive result is not a peer ACK. Keep its ORIGINAL
        // consumer/deadline until exact result-bearing epoch retirement. Never
        // replace an unknown submitted positive with a new negative result.
        if self.invite_completed.values().any(|ticket| !ticket.positive_is_current()) {
            self.close(); return Err(CodecError::InvalidField.into());
        }
        let Some(scheduler) = self.scheduler.as_mut() else { return Err(CodecError::InvalidField.into()); };
        let batch = match scheduler.prepare(now_ms) {
            Ok(batch) => batch, Err(error) => { self.close(); return Err(error.into()); }
        };
        let result_sequences=batch.payload().results.keys().copied().collect::<Vec<_>>();
        let packets = match self.protocol.prepare_scheduled_swap(&batch, routing) {
            Ok(packets) => packets,
            Err(ProtocolError::Batch(super::batch_scheduler::BatchError::Backpressure)) if !batch.is_encoded()&&!batch.client_holders().is_empty()=>{
                // Original shared-budget refusal occurred before ARC encoding.
                // Abandon this unsent snapshot, preserving unrelated WRITE
                // work/epochs. The READ caller retires only its own interest.
                self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.cancel_prepared();
                return Err(ProtocolError::Batch(super::batch_scheduler::BatchError::Backpressure));
            }
            Err(error) => { self.close(); return Err(error); }
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
        let invite_pending = &mut self.invite_pending;
        let invite_completed = &mut self.invite_completed;
        let status_pending=&mut self.status_pending;let status_completed=&mut self.status_completed;
        let im_binding=&self.im_binding;let im_pending=&mut self.im_pending;let im_completed=&mut self.im_completed;
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
                    tracker.consume_scheduled_client_results(&mut delivery);
                    for reply in tracker.take_unhosting_replies() { events.push(OfflineEvent::Unhosting(reply)); }
                    for incoming in delivery.requests {
                        if super::rpc_client_ex::method_kind(incoming.request().method).is_some() {
                            let action = match client_ex.as_ref().map(|dispatcher| dispatcher.on_batch(&incoming)).transpose() {
                                Ok(Some(Some(action))) => action,
                                _ => { processing_error = Some(CodecError::InvalidField.into()); return; }
                            };
                            if let Some(notice)=action.im{
                                let key=(incoming.ticket().peer_incarnation(),incoming.ticket().sequence());
                                if im_pending.len()+status_pending.len()>=super::rpc_stream::MAX_MAP_ENTRIES||im_pending.contains_key(&key){processing_error=Some(CodecError::TooLarge.into());return;}
                                let delivery=match super::im_delivery::IncomingImDelivery::batch(*notice,incoming.into_ticket()){
                                    Ok(delivery)=>delivery,Err(error)=>{processing_error=Some(error.into());return;}
                                };
                                im_pending.insert(key,(*delivery.ticket().fingerprint(),delivery.ticket().deadline()));
                                events.push(OfflineEvent::Im(Box::new(delivery)));continue;
                            }
                            if let Some(data) = action.invite {
                                let key = (incoming.ticket().peer_incarnation(), incoming.ticket().sequence());
                                if invite_pending.len() >= super::native_invite_notifications::MAX_INVITE_HOLDERS || invite_pending.contains_key(&key) {
                                    processing_error = Some(CodecError::TooLarge.into()); return;
                                }
                                let delivery = match super::native_invite_notifications::IncomingInviteNotification::from_batch(data, incoming.into_ticket()) {
                                    Ok(delivery) => delivery, Err(error) => { processing_error = Some(error.into()); return; }
                                };
                                invite_pending.insert(key, (delivery.notification_id(), delivery.fingerprint(), delivery.deadline()));
                                events.push(OfflineEvent::InviteNotification(Box::new(delivery)));
                                continue;
                            }
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
                        if let Some(binding)=im_binding{
                            let notice=match super::im_delivery::batch_message(binding,&incoming){Ok(notice)=>notice,Err(error)=>{processing_error=Some(error.into());return;}};
                            if let Some(notice)=notice{
                                let key=(incoming.ticket().peer_incarnation(),incoming.ticket().sequence());
                                if im_pending.len()+status_pending.len()>=super::rpc_stream::MAX_MAP_ENTRIES||im_pending.contains_key(&key){processing_error=Some(CodecError::TooLarge.into());return;}
                                let delivery=match super::im_delivery::IncomingImDelivery::batch(notice,incoming.into_ticket()){
                                    Ok(delivery)=>delivery,Err(error)=>{processing_error=Some(error.into());return;}
                                };
                                im_pending.insert(key,(*delivery.ticket().fingerprint(),delivery.ticket().deadline()));
                                events.push(OfflineEvent::Im(Box::new(delivery)));continue;
                            }
                            let notice=match super::im_status_delivery::batch(binding,&incoming){Ok(notice)=>notice,Err(error)=>{processing_error=Some(error.into());return;}};
                            if let Some(notice)=notice{
                                let key=(incoming.ticket().peer_incarnation(),incoming.ticket().sequence());
                                if status_pending.len()+im_pending.len()>=super::rpc_stream::MAX_MAP_ENTRIES||status_pending.contains_key(&key){processing_error=Some(CodecError::TooLarge.into());return;}
                                let delivery=match super::im_status_delivery::IncomingImStatus::batch(notice,incoming.into_ticket()){
                                    Ok(delivery)=>delivery,Err(error)=>{processing_error=Some(error.into());return;}
                                };
                                status_pending.insert(key,(*delivery.ticket().fingerprint(),delivery.ticket().deadline()));
                                events.push(OfflineEvent::ImStatus(Box::new(delivery)));continue;
                            }
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
                    if let Some(retirement) = retirement {
                        for sequence in retirement.acknowledged() { invite_completed.remove(&(retirement.peer_incarnation(), *sequence)); im_completed.remove(&(retirement.peer_incarnation(),*sequence)); status_completed.remove(&(retirement.peer_incarnation(),*sequence)); }
                        events.push(OfflineEvent::BatchRetirement(retirement));
                    }
                }
                ProtocolEvent::Reply(ArcReply::Logout(reply)) => events.push(OfflineEvent::Logout(reply)),
                ProtocolEvent::Reply(ArcReply::Call(reply))=>events.push(OfflineEvent::CallOperation(reply)),
                ProtocolEvent::Reply(ArcReply::Offline(page)) => events.push(OfflineEvent::OfflinePage(page)),
                ProtocolEvent::OfflineHint(hint) => events.push(OfflineEvent::OfflineHint(hint)),
                ProtocolEvent::Reply(ArcReply::Bootstrap(configuration)) => events.push(OfflineEvent::Bootstrap(configuration)),
                ProtocolEvent::Control(kind) => events.push(OfflineEvent::Control(kind)),
                ProtocolEvent::Incoming(message) => events.push(OfflineEvent::DirectMessage(message)),
                ProtocolEvent::IncomingCall(call)=>events.push(OfflineEvent::Call(call)),
                ProtocolEvent::InviteNotification(invite)=>events.push(OfflineEvent::InviteNotification(invite)),
                ProtocolEvent::Im(delivery)=>events.push(OfflineEvent::Im(delivery)),
                ProtocolEvent::ImStatus(delivery)=>events.push(OfflineEvent::ImStatus(delivery)),
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
        if self.status_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence())){return Err(CodecError::InvalidField.into());}
        if self.call_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence())) || self.offline_hints.contains(&(ticket.peer_incarnation(),ticket.sequence())) || self.invite_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence())) || self.im_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence())) {return Err(CodecError::InvalidField.into());}
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
    pub(super) fn complete_im(&mut self,ticket:&super::im_delivery::ImReplyTicket,proof:Option<&super::im_inbox::CommittedIm>)->Result<Option<CallCompletion>,ProtocolError>{
        let scope=self.protocol.connection_scope();if self.phase()!=ArcPhase::Authenticated||!scope.is_open()||scope.session_revoked(){return Ok(None);}
        if !ticket.matches(&self.tracker.batch_owner()?,&scope){return Ok(None);}
        if proof.is_some_and(|proof|!proof.authorizes(ticket,std::time::Instant::now())){return Err(CodecError::InvalidField.into());}
        match ticket.source(){
            super::im_delivery::ImReplySource::Direct(_)=>self.protocol.complete_im(ticket,proof).map(|reply|reply.map(CallCompletion::Direct)),
            super::im_delivery::ImReplySource::Batch(inner)=>{
                let key=(inner.peer_incarnation(),inner.sequence());
                if self.im_pending.get(&key)!=Some(&(*ticket.fingerprint(),ticket.deadline())){return Ok(None);}
                if proof.is_some()&&self.im_completed.len()+self.status_completed.len()>=super::rpc_stream::MAX_MAP_ENTRIES{return Err(CodecError::TooLarge.into());}
                let result=encode_online_message_result(proof.is_some())?;
                if self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.complete_server_request(inner,0,result.expose_to_transport())?{
                    self.im_pending.remove(&key);if let Some(proof)=proof{self.im_completed.insert(key,(ticket.captured_copy(),proof.captured_copy()));}
                    Ok(Some(CallCompletion::BatchQueued))
                }else{Ok(None)}
            }
        }
    }

    pub(super) fn complete_im_status(&mut self,ticket:&super::im_status_delivery::ImStatusTicket,proof:Option<&super::im_inbox::CommittedImStatus>)->Result<Option<CallCompletion>,ProtocolError>{
        let scope=self.protocol.connection_scope();if self.phase()!=ArcPhase::Authenticated||!scope.is_open()||scope.session_revoked(){return Ok(None);}
        if !ticket.matches(&self.tracker.batch_owner()?,&scope){return Ok(None);}
        if proof.is_some_and(|proof|!proof.authorizes(ticket,std::time::Instant::now())){return Err(CodecError::InvalidField.into());}
        match ticket.source(){
            super::im_delivery::ImReplySource::Direct(_)=>self.protocol.complete_im_status(ticket,proof).map(|p|p.map(CallCompletion::Direct)),
            super::im_delivery::ImReplySource::Batch(inner)=>{
                let key=(inner.peer_incarnation(),inner.sequence());
                if self.status_pending.get(&key)!=Some(&(*ticket.fingerprint(),ticket.deadline())){return Ok(None);}
                if proof.is_some()&&self.status_completed.len()+self.im_completed.len()>=super::rpc_stream::MAX_MAP_ENTRIES{return Err(CodecError::TooLarge.into());}
                let result=encode_online_message_result(proof.is_some())?;
                if self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.complete_server_request(inner,0,result.expose_to_transport())?{
                    self.status_pending.remove(&key);if let Some(proof)=proof{self.status_completed.insert(key,(ticket.captured_copy(),proof.captured_copy()));}
                    Ok(Some(CallCompletion::BatchQueued))
                }else{Ok(None)}
            }
        }
    }
    pub(super) fn complete_invite_notification(&mut self, ticket: &super::native_invite_notifications::InviteReplyTicket, accepted: bool)
        -> Result<Option<CallCompletion>, ProtocolError> {
        let scope = self.protocol.connection_scope();
        if !scope.is_open() || scope.session_revoked() { return Ok(None); }
        if !ticket.matches(&self.tracker.batch_owner()?, &scope) { return Ok(None); }
        match ticket.source() {
            super::native_invite_notifications::InviteReplySource::Direct(_) => self.protocol.complete_invite_notification(ticket, accepted).map(|reply| reply.map(CallCompletion::Direct)),
            super::native_invite_notifications::InviteReplySource::Batch(inner) => {
                let key = (inner.peer_incarnation(), inner.sequence());
                if self.invite_pending.get(&key) != Some(&(ticket.notification_id(), *ticket.fingerprint(), ticket.deadline())) { return Ok(None); }
                if accepted && (self.invite_completed.len() >= super::native_invite_notifications::MAX_INVITE_HOLDERS || !ticket.positive_is_current()) { return Err(CodecError::InvalidField.into()); }
                let result = super::native_invite_notifications::encode_completion(accepted)?;
                if self.scheduler.as_mut().ok_or(CodecError::InvalidField)?.complete_server_request(inner, 0, result.expose_to_transport())? {
                    self.invite_pending.remove(&key);
                    if accepted { self.invite_completed.insert(key, ticket.captured_copy()); }
                    Ok(Some(CallCompletion::BatchQueued))
                } else { Ok(None) }
            }
        }
    }

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

#[path="offline_client_user_entry.rs"]
mod client_user_entry;

#[cfg(test)]
#[path = "offline_backend_tests.rs"]
mod tests;
