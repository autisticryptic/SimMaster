//! Exact recvOnlineMessage.MessageReceiver.Client codec and configured binding.
//!
//! The Android receiver kind is NOT inferred from SipGateway or an outbound
//! method name. Bind the original registered kind/object/domain explicitly.
//! No new message channel readiness, SMS classification, gateway mutation,
//! persistence ACK, public endpoint or socket is granted by this decoder.
use super::{
    batch_scheduler::IncomingBatchRequest,
    native_events::IncomingMessage,
    rpc_batch::BatchRequest,
    rpc_envelope::RpcRequest,
    rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError, Reader, RpcBytes, RpcProperty, Writer, MAX_FIELD_BYTES, MAX_MAP_ENTRIES},
    session::AttemptOwner,
};
use std::fmt;

pub const ONLINE_MESSAGE_METHOD: &str = "recvOnlineMessage.MessageReceiver.Client";

pub struct OnlineMessage<'a> {
    pub receiver_kind: &'a [u8],
    pub properties: Vec<RpcProperty<'a>>,
    pub body: &'a [u8],
}

impl fmt::Debug for OnlineMessage<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OnlineMessage(<redacted>)") }
}

pub fn decode_online_message(arguments: &[u8]) -> Result<OnlineMessage<'_>, CodecError> {
    let mut reader = Reader::versioned(arguments)?;
    let receiver_kind = reader.bytes()?;
    if receiver_kind.is_empty() { return Err(CodecError::InvalidField); }
    let properties = reader.properties()?;
    let body = reader.bytes()?;
    reader.finish()?;
    Ok(OnlineMessage { receiver_kind, properties, body })
}

/// Result streams have type0+boolean followed by the mandatory Common footer,
/// not request version words. This acknowledges only configured local delivery;
/// the eventual backend must await its real inbox consumer before accepting.
pub fn encode_online_message_result(accepted: bool) -> Result<RpcBytes, CodecError> {
    let mut writer = Writer::new();
    writer.boolean(accepted)?;
    writer.bytes(b"")?;
    Ok(writer.finish())
}

pub struct IncomingBinding<'a> {
    pub object_id: &'a [u8],
    pub domain: &'a [u8],
    pub receiver_kind: &'a [u8],
}

impl fmt::Debug for IncomingBinding<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingBinding(<redacted>)") }
}

/// Captured when the original authenticated ARC incarnation installs receivers,
/// not assigned while processing a packet. Exact kind only: native wildcard
/// registry and generic receivers are intentionally NOT accepted by this subset.
pub struct IncomingDispatcher {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    object_id: Vec<u8>,
    domain: Vec<u8>,
    receiver_kind: Vec<u8>,
}

impl fmt::Debug for IncomingDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingDispatcher(<redacted>)") }
}

impl IncomingDispatcher {
    pub(super) fn for_arc_session(owner: AttemptOwner, connection: RpcConnectionScope, binding: IncomingBinding<'_>)
        -> Result<Self, CodecError> {
        for value in [binding.object_id, binding.domain, binding.receiver_kind] {
            if value.is_empty() || value.iter().any(u8::is_ascii_control)
                || value.windows(4).any(|v| v == b"ENC(") { return Err(CodecError::InvalidField); }
            if value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        }
        if binding.receiver_kind.contains(&b'*') || !connection.is_open() { return Err(CodecError::InvalidField); }
        Ok(Self { owner, connection, object_id: binding.object_id.to_vec(), domain: binding.domain.to_vec(),
            receiver_kind: binding.receiver_kind.to_vec() })
    }

    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(scope) && self.connection.is_open()
    }

    /// Returns no event for an unrelated original scope or an unsupported method
    /// before parsing its arguments. In-scope malformed routing is an error.
    pub fn on_batch_request(&self, request: &IncomingBatchRequest) -> Result<Option<IncomingMessage>, CodecError> {
        if !request.ticket().matches(&self.owner, &self.connection) { return Ok(None); }
        self.dispatch(&request.request())
    }

    pub(super) fn on_direct_request(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, request: &RpcRequest<'_>)
        -> Result<Option<IncomingMessage>, CodecError> {
        if !self.matches(owner, scope) { return Ok(None); }
        self.dispatch(&BatchRequest { method: request.method, arguments: request.arguments, metadata: request.metadata.clone() })
    }

    fn dispatch(&self, request: &BatchRequest<'_>) -> Result<Option<IncomingMessage>, CodecError> {
        if request.method != ONLINE_MESSAGE_METHOD.as_bytes() { return Ok(None); }
        if request.metadata.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        for (index, p) in request.metadata.iter().enumerate() {
            if p.key.len() > MAX_FIELD_BYTES || p.value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
            if request.metadata[..index].iter().any(|prior| prior.key == p.key) { return Err(CodecError::DuplicateKey); }
        }
        let property = |key: &[u8]| request.metadata.iter().find(|p| p.key == key).map(|p| p.value);
        if property(b"id") != Some(self.object_id.as_slice()) || property(b"domain") != Some(self.domain.as_slice()) {
            return Err(CodecError::InvalidField);
        }
        // __magic is opaque native request dedup input, never interpreted as
        // credentials, text, a line id or a delivery-readiness assertion.
        let message = decode_online_message(request.arguments)?;
        if message.receiver_kind != self.receiver_kind { return Ok(None); }
        let find = |key: &[u8]| message.properties.iter().find(|p| p.key == key).map(|p| p.value.to_vec());
        Ok(Some(IncomingMessage { body: message.body.to_vec(), from: find(b"Sip.Message.From"), time: find(b"Sip.Message.Time") }))
    }
}

#[cfg(test)]
#[path = "rpc_incoming_tests.rs"]
mod tests;
