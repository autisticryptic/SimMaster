//! Composes owned ARC RPC sessions with recovered call-packet framing.
//!
//! This is an offline protocol engine for a future Linux backend, NOT a
//! HostingBackend/provider. The caller must supply verified routing/config and
//! TLS plaintext packet boundaries. Socket, bootstrap routes, trust/handshake,
//! batch epoch scheduling and incoming-object dispatch are still separate.
use super::{
    arc_session::{ArcLoginInputs, ArcPhase, ArcReply, ArcRpcSession},
    rpc_batch::BatchPayload,
    rpc_pending::{HostingRpcTracker, RpcConnectionScope},
    rpc_stream::{CodecError, RpcProperty},
    session::AttemptOwner,
    transport_packets::{encode_call_packets, CallAssembler, CallKind, TransportBytes, TransportError, TLS_FRAGMENT_THRESHOLD},
};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError { Rpc(CodecError), Transport(TransportError) }

impl From<CodecError> for ProtocolError { fn from(error: CodecError) -> Self { Self::Rpc(error) } }
impl From<TransportError> for ProtocolError { fn from(error: TransportError) -> Self { Self::Transport(error) } }

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
}

pub struct ArcProtocol { rpc: ArcRpcSession, assembler: CallAssembler }

impl fmt::Debug for ArcProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ArcProtocol(<redacted>)") }
}

impl ArcProtocol {
    pub fn new(owner: AttemptOwner) -> Self {
        Self { rpc: ArcRpcSession::new(owner), assembler: CallAssembler::default() }
    }

    pub fn connection_scope(&self) -> RpcConnectionScope { self.rpc.connection_scope() }
    pub fn phase(&self) -> ArcPhase { self.rpc.phase() }
    pub fn take_hosting_tracker(&mut self) -> Result<HostingRpcTracker, ProtocolError> {
        Ok(self.rpc.take_hosting_tracker()?)
    }
    pub fn retire_request(&mut self, id: i32) {
        self.rpc.retire_request(id);
        if self.rpc.phase() == ArcPhase::Closed { self.assembler.close(); }
    }
    pub fn close(&mut self) { self.rpc.close(); self.assembler.close(); }

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

    pub fn prepare_swap_calls(&mut self, payload: &BatchPayload<'_>, routing: &[RpcProperty<'_>])
        -> Result<OwnedCallPackets, ProtocolError> {
        let (id, bytes) = self.rpc.prepare_swap_calls(payload, routing)?;
        self.packets(id, bytes.expose_to_transport())
    }

    /// The supplied packet is from the original authenticated TLS engine, not
    /// raw TCP/HTTP/API or a native notification. Old scopes are rejected before
    /// decoding/copying, so stale input cannot poison a newer fragment assembly.
    /// Reply contents are passed synchronously to avoid storing borrowed data or
    /// assigning their owner on receipt. Unimplemented server requests close the
    /// engine; this cannot be used as an inbound-SMS endpoint.
    pub fn on_plaintext_packet<F>(&mut self, owner: &AttemptOwner, connection: &RpcConnectionScope,
        packet: &[u8], deliver: F) -> Result<bool, ProtocolError>
        where F: for<'a> FnOnce(ProtocolEvent<'a>) {
        if !self.rpc.accepts_scope(owner, connection) { return Ok(false); }
        let message = match self.assembler.on_packet(packet) {
            Ok(Some(message)) => message,
            Ok(None) => return Ok(false),
            Err(error) => { self.close(); return Err(error.into()); }
        };
        match message.kind {
            CallKind::Reply => match self.rpc.on_reply(owner, connection, message.payload.expose_to_transport()) {
                Ok(Some(reply)) => {
                    if self.rpc.phase() == ArcPhase::Closed { self.assembler.close(); }
                    deliver(ProtocolEvent::Reply(reply));
                    Ok(true)
                }
                Ok(None) => Ok(false),
                Err(error) => { self.close(); Err(error.into()) }
            },
            CallKind::Request => { self.close(); Err(TransportError::UnsupportedKind.into()) },
            kind => {
                if kind == CallKind::Release { self.close(); }
                deliver(ProtocolEvent::Control(kind));
                Ok(true)
            }
        }
    }
}

#[cfg(test)]
#[path = "arc_protocol_tests.rs"]
mod tests;
