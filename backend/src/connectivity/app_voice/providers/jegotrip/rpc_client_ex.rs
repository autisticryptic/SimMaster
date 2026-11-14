//! Exact ClientEx controls on the original authenticated MessageReceiver object.
//!
//! These are control notifications, not SMS/call payloads or readiness grants.
//! Keep the existing explicit object/domain subset; native default adapters and
//! their separate DIRECT/batch domain policies are not inferred here. Positive
//! checkAlive requires the optional original-bound AccountAlive runtime's
//! current registration. Without it, SDK/HostingAck alone still returns false
//! with interval zero; a true Alive result never marks SMS/call/media ready.
use super::{
    batch_scheduler::IncomingBatchRequest,
    rpc_incoming::IncomingBinding,
    rpc_envelope::RpcRequest,
    rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError, Reader, RpcBytes, RpcProperty, Writer, MAX_FIELD_BYTES, MAX_MAP_ENTRIES, MAX_STREAM_BYTES},
    session::AttemptOwner,
};
use std::fmt;
use zeroize::Zeroizing;

pub const CHECK_ALIVE_METHOD: &str = "checkAlive.ClientEx.Client";
pub const NOTIFY_CALLS_METHOD: &str = "notifyCalls.ClientEx.Client";
pub const NOTIFY_DISCONNECT_METHOD: &str = "notifyDisconnect.ClientEx.Client";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClientExKind { CheckAlive, NotifyCalls, NotifyDisconnect }
pub(super) fn method_kind(method: &[u8]) -> Option<ClientExKind> {
    match method {
        value if value == CHECK_ALIVE_METHOD.as_bytes() => Some(ClientExKind::CheckAlive),
        value if value == NOTIFY_CALLS_METHOD.as_bytes() => Some(ClientExKind::NotifyCalls),
        value if value == NOTIFY_DISCONNECT_METHOD.as_bytes() => Some(ClientExKind::NotifyDisconnect),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DisconnectReason { AnotherDevice, SameDevice, ServerKick, Other }
fn disconnect_reason(bytes: &[u8]) -> DisconnectReason {
    if bytes.starts_with(b"another-device-logined") { DisconnectReason::AnotherDevice }
    else if bytes.starts_with(b"same-device-login") { DisconnectReason::SameDevice }
    else if bytes.starts_with(b"server-kick-off") { DisconnectReason::ServerKick }
    else { DisconnectReason::Other }
}

/// 655DA8 forwards these two exact SDK command shapes to separate local
/// Message/Invite consumers. Their notification ID is not an SMS receipt or
/// call-control grant. Until those consumers are installed, answer false for
/// these recognized requests instead of treating the whole stream as corrupt.
/// Unknown commands/fields still fail closed; no business value is interpreted.
fn recognized_notification_command(properties: &[RpcProperty<'_>]) -> bool {
    let command = properties.iter().find(|p| p.key == b"Notify.Command").map(|p| p.value);
    let keys: [&[u8]; 3] = match command {
        Some(b"Message") => [b"Notify.Command", b"Notify.MsgIdx", b"Notify.Box"],
        Some(b"Invite") => [b"Notify.Command", b"Notify.CallId", b"Notify.Caller"],
        _ => return false,
    };
    properties.len() == keys.len() && keys.iter().all(|key| properties.iter().any(|p| p.key == *key))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClientExControl { PollHint { notification_id: u64 }, Revoked { reason: DisconnectReason } }

pub struct OwnedClientExNotice {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    control: ClientExControl,
}
impl OwnedClientExNotice {
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.is_open() && !scope.session_revoked()
    }
    pub(super) fn is_revocation_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.session_revoked()
            && matches!(self.control, ClientExControl::Revoked { .. })
    }
    pub(super) fn control(&self) -> ClientExControl { self.control }
}
impl fmt::Debug for OwnedClientExNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OwnedClientExNotice(<redacted>)") }
}

/// Captured deployment data may be armed before login, but only the matched
/// ARC login can turn it into a token-bound dispatcher. This catches controls
/// coalesced with that login reply without admitting unauthenticated controls.
pub(super) struct ClientExRegistration { object: Zeroizing<Vec<u8>>, domain: Zeroizing<Vec<u8>> }
impl ClientExRegistration {
    pub(super) fn capture(binding: IncomingBinding<'_>) -> Result<Self, CodecError> {
        let mut total = 0usize;
        for value in [binding.object_id, binding.domain] {
            if value.is_empty() || value.contains(&b'*') || value.iter().any(u8::is_ascii_control)
                || value.windows(4).any(|part| part == b"ENC(") { return Err(CodecError::InvalidField); }
            if value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
            total = total.checked_add(value.len()).ok_or(CodecError::TooLarge)?;
        }
        if total > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
        Ok(Self { object: Zeroizing::new(binding.object_id.to_vec()), domain: Zeroizing::new(binding.domain.to_vec()) })
    }
    pub(super) fn binding(&self) -> IncomingBinding<'_> {
        IncomingBinding { object_id: &self.object, domain: &self.domain, receiver_kind: b"" }
    }
}

pub(super) struct ClientExAction { pub result: RpcBytes, pub notice: Option<OwnedClientExNotice> }
pub(super) struct ClientExDispatcher {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    object: Zeroizing<Vec<u8>>,
    domain: Zeroizing<Vec<u8>>,
    token: Zeroizing<Vec<u8>>,
    native: Option<super::native_receiver::NativeReceiverBinding>,
    alive:Option<std::sync::Arc<super::alive_session::AliveLink>>,
}
impl fmt::Debug for ClientExDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ClientExDispatcher(<redacted>)") }
}
impl ClientExDispatcher {
    /// Only ArcRpcSession calls this with its matched login's actual ARC token.
    /// The installation option never supplies an App/phone token or a grant.
    pub(super) fn for_arc_session(owner: AttemptOwner, scope: RpcConnectionScope, binding: IncomingBinding<'_>, token: &[u8])
        -> Result<Self, CodecError> {
        if !scope.is_open() || scope.session_revoked() || token.is_empty() { return Err(CodecError::InvalidField); }
        let mut total = token.len();
        for value in [binding.object_id, binding.domain] {
            if value.is_empty() || value.contains(&b'*') || value.iter().any(u8::is_ascii_control)
                || value.windows(4).any(|part| part == b"ENC(") { return Err(CodecError::InvalidField); }
            total = total.checked_add(value.len()).ok_or(CodecError::TooLarge)?;
        }
        if total > MAX_STREAM_BYTES || [binding.object_id, binding.domain, token].iter().any(|value| value.len() > MAX_FIELD_BYTES) {
            return Err(CodecError::TooLarge);
        }
        Ok(Self { owner, scope, object: Zeroizing::new(binding.object_id.to_vec()),
            domain: Zeroizing::new(binding.domain.to_vec()), token: Zeroizing::new(token.to_vec()), native: None,alive:None })
    }
    pub(super) fn for_native_client(owner: AttemptOwner, scope: RpcConnectionScope, policy: std::sync::Arc<super::native_receiver::NativeReceiverPolicy>, token: &[u8])
        -> Result<Self, CodecError> {
        if token.is_empty() || token.len() > MAX_FIELD_BYTES { return Err(CodecError::InvalidField); }
        let native = super::native_receiver::NativeReceiverBinding::original(owner.clone(), scope.clone(), policy)?;
        Ok(Self { owner, scope, object: Zeroizing::new(super::native_receiver::MESSAGE_OBJECT.to_vec()), domain: Zeroizing::new(Vec::new()),
            token: Zeroizing::new(token.to_vec()), native: Some(native),alive:None })
    }
    pub(super) fn with_alive(mut self,alive:std::sync::Arc<super::alive_session::AliveLink>)->Result<Self,CodecError>{
        if self.alive.is_some()||!alive.matches(&self.owner,&self.scope)||!alive.current_token(&self.token){return Err(CodecError::InvalidField);}
        self.alive=Some(alive);Ok(self)
    }
    pub(super) fn matches_binding(&self, binding: &IncomingBinding<'_>) -> bool {
        self.native.is_none() && self.object.as_slice() == binding.object_id && self.domain.as_slice() == binding.domain
    }
    pub(super) fn captured_copy(&self) -> Self {
        Self { owner: self.owner.clone(), scope: self.scope.clone(), object: Zeroizing::new(self.object.to_vec()),
            domain: Zeroizing::new(self.domain.to_vec()), token: Zeroizing::new(self.token.to_vec()), native: self.native.clone(),alive:self.alive.clone() }
    }
    pub(super) fn on_direct(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, request: &RpcRequest<'_>)
        -> Result<Option<ClientExAction>, CodecError> {
        if &self.owner != owner || !self.scope.matches(scope) || !scope.is_open() || scope.session_revoked() { return Ok(None); }
        self.dispatch(request.method, request.arguments, &request.metadata, super::native_receiver::ReceiverTransport::Direct)
    }
    pub(super) fn on_batch(&self, incoming: &IncomingBatchRequest) -> Result<Option<ClientExAction>, CodecError> {
        if !incoming.ticket().matches(&self.owner, &self.scope) || self.scope.session_revoked() { return Ok(None); }
        let request = incoming.request(); self.dispatch(request.method, request.arguments, &request.metadata, super::native_receiver::ReceiverTransport::Batch)
    }
    fn dispatch(&self, method: &[u8], arguments: &[u8], metadata: &[RpcProperty<'_>], transport: super::native_receiver::ReceiverTransport) -> Result<Option<ClientExAction>, CodecError> {
        let Some(kind) = method_kind(method) else { return Ok(None); };
        validate_metadata(metadata)?;
        let property = |key: &[u8]| metadata.iter().find(|property| property.key == key).map(|property| property.value);
        if let Some(native) = &self.native {
            if !native.matches(&self.owner, &self.scope) { return Err(CodecError::InvalidField); }
            native.policy().validate(super::native_receiver::ReceiverObject::Message, metadata, transport)?;
        } else if property(b"id") != Some(self.object.as_slice()) || property(b"domain") != Some(self.domain.as_slice()) {
            return Err(CodecError::InvalidField);
        }
        let mut reader = Reader::versioned(arguments)?;
        let mut notify_accepted = true;
        let control = match kind {
            ClientExKind::CheckAlive => None,
            ClientExKind::NotifyCalls => {
                let notification_id = reader.u64()?;
                let properties = reader.properties()?;
                if !properties.is_empty() {
                    if !recognized_notification_command(&properties) { return Err(CodecError::InvalidField); }
                    // A well-formed, recognized but unsupported SDK command is
                    // an explicit negative result, not a successful poll hint,
                    // fabricated user event, or loss of the authenticated link.
                    notify_accepted = false;
                }
                Some(ClientExControl::PollHint { notification_id })
            }
            ClientExKind::NotifyDisconnect => {
                let reason = disconnect_reason(reader.bytes()?);
                Some(ClientExControl::Revoked { reason })
            }
        };
        reader.finish()?;
        if !self.scope.is_open() || self.scope.session_revoked() { return Ok(None); }
        let alive=if kind==ClientExKind::CheckAlive{self.alive.as_ref().map_or((false,0),|alive|alive.check(&self.owner,&self.scope,&self.token,std::time::Instant::now()))}else{(false,0)};
        let accepted = match kind {
            ClientExKind::CheckAlive => alive.0,
            ClientExKind::NotifyCalls => notify_accepted,
            ClientExKind::NotifyDisconnect => property(b"token") == Some(self.token.as_slice()),
        };
        let result = encode_result(kind, accepted, alive.1)?;
        let notice = if accepted && control.is_some() {
            let control = control.ok_or(CodecError::InvalidField)?;
            if matches!(control, ClientExControl::Revoked { .. }) {
                // Sticky ORIGINAL-scope revocation precedes every reply write,
                // queue admission or await. IO/cancellation/backpressure cannot
                // later relabel a valid kick as retryable transport failure.
                self.scope.mark_session_revoked();
            }
            Some(OwnedClientExNotice { owner: self.owner.clone(), scope: self.scope.clone(), control })
        } else { None };
        Ok(Some(ClientExAction { result, notice }))
    }
}
fn validate_metadata(metadata: &[RpcProperty<'_>]) -> Result<(), CodecError> {
    if metadata.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
    let mut total = 0usize;
    for (index, property) in metadata.iter().enumerate() {
        if property.key.len() > MAX_FIELD_BYTES || property.value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
        total = total.checked_add(property.key.len()).and_then(|total| total.checked_add(property.value.len())).ok_or(CodecError::TooLarge)?;
        if total > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
        if metadata[..index].iter().any(|prior| prior.key == property.key) { return Err(CodecError::DuplicateKey); }
    }
    Ok(())
}
fn encode_result(kind: ClientExKind, accepted: bool, alive_time: u32) -> Result<RpcBytes, CodecError> {
    let mut writer = Writer::new(); writer.boolean(accepted)?;
    if kind == ClientExKind::CheckAlive { writer.u32(alive_time)?; }
    writer.bytes(b"")?; Ok(writer.finish())
}

#[cfg(test)]
#[path = "rpc_client_ex_tests.rs"]
mod tests;
