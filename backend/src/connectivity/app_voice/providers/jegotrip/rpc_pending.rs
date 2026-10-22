//! Correlates recovered RPC envelopes to the original submitted hosting work.
//!
//! This is a local demultiplexer for a future authenticated backend, not a socket
//! implementation. Never recreate owners on reply receipt or retry an unknown
//! result automatically. Native request ids are positive signed ints and are not
//! reused within a tracker; a new connection needs a new tracker incarnation.
use super::{
    controller::Callback,
    native_events::NativeOperation,
    rpc_arguments::{decode_update_status_result, encode_update_status, UPDATE_STATUS_METHOD},
    rpc_batch::BatchRequest,
    rpc_envelope::{decode_reply, encode_request},
    rpc_stream::{CodecError, RpcBytes, RpcProperty, MAX_MAP_ENTRIES},
    session::{AttemptOwner, BackendFailure, GatewayRegistration, STATUS_REGISTER, STATUS_UNREGISTER},
};
use std::{collections::BTreeMap, sync::{atomic::{AtomicBool, Ordering}, Arc}};

#[derive(Clone)]
pub struct RpcConnectionScope(Arc<AtomicBool>);

impl RpcConnectionScope {
    pub(super) fn fresh() -> Self { Self(Arc::new(AtomicBool::new(true))) }
    pub(super) fn matches(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
    pub(super) fn is_open(&self) -> bool { self.0.load(Ordering::Acquire) }
    pub(super) fn close(&self) { self.0.store(false, Ordering::Release); }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery { Envelope, Batch }

struct PendingRequest {
    owner: AttemptOwner,
    operation: NativeOperation,
    delivery: Delivery,
}

pub struct PreparedHostingCall {
    pub sequence: i32,
    arguments: RpcBytes,
}

impl PreparedHostingCall {
    pub fn as_batch_request<'a>(&'a self, metadata: &[RpcProperty<'a>]) -> BatchRequest<'a> {
        BatchRequest { method: UPDATE_STATUS_METHOD.as_bytes(),
            arguments: self.arguments.expose_to_transport(), metadata: metadata.to_vec() }
    }
}

impl std::fmt::Debug for PreparedHostingCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedHostingCall").field("sequence", &self.sequence)
            .field("data", &"<redacted>").finish()
    }
}

pub struct OwnedRpcCallback {
    pub owner: AttemptOwner,
    pub callback: Callback,
}

pub struct HostingRpcTracker {
    connection: RpcConnectionScope,
    batch_owner: Option<AttemptOwner>,
    next_id: Option<i32>,
    pending: BTreeMap<i32, PendingRequest>,
}

impl Default for HostingRpcTracker {
    fn default() -> Self { Self::new() }
}

impl HostingRpcTracker {
    /// Standalone Common-envelope tracker for offline/mock callers only.
    /// Native nested hosting requires the original ARC session's one-time tracker.
    pub fn new() -> Self {
        Self { connection: RpcConnectionScope::fresh(), batch_owner: None,
            next_id: Some(1), pending: BTreeMap::new() }
    }

    pub(super) fn for_arc_session(owner: AttemptOwner, connection: RpcConnectionScope) -> Self {
        Self { connection, batch_owner: Some(owner), next_id: Some(1), pending: BTreeMap::new() }
    }

    /// Capture this when the authenticated connection installs its receive task.
    /// A stale task cannot feed bytes into a new connection's correlation map.
    pub fn connection_scope(&self) -> RpcConnectionScope { self.connection.clone() }

    /// Common-envelope helper retained for offline codec/mock callers. The
    /// native ARC hosting path uses prepare_batch_update_status instead: it
    /// nests this method inside swapCalls rather than sending it to the socket.
    /// Caller supplies metadata captured from the native session/object agent
    /// (including id/domain/__magic where required); the codec does not guess it.
    pub fn prepare_update_status(&mut self, owner: AttemptOwner, current_user_uri: &str,
        status: i32, registration: &GatewayRegistration<'_>, metadata: &[RpcProperty<'_>])
        -> Result<(i32, RpcBytes), CodecError> {
        if !self.connection.is_open() { self.pending.clear(); return Err(CodecError::InvalidField); }
        if self.pending.len() >= MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let id = self.next_id.ok_or(CodecError::InvalidCallId)?;
        let operation = match status {
            STATUS_REGISTER => NativeOperation::HostingRegister,
            STATUS_UNREGISTER => NativeOperation::HostingUnregister,
            _ => return Err(CodecError::InvalidField),
        };
        let arguments = encode_update_status(current_user_uri, status, registration)
            .map_err(|code| if code == "jegotrip_rpc_arguments_too_large" {
                CodecError::TooLarge
            } else { CodecError::InvalidField })?;
        let bytes = encode_request(id, metadata, UPDATE_STATUS_METHOD.as_bytes(),
            arguments.expose_to_transport())?;
        self.next_id = id.checked_add(1);
        self.pending.insert(id, PendingRequest { owner, operation, delivery: Delivery::Envelope });
        Ok((id, bytes))
    }

    /// The native hosting request goes INSIDE the signed swapCalls request map,
    /// not directly to the socket. Capture its owner/operation now, while the
    /// outer ARC session separately owns login/auth metadata and batch epochs.
    pub fn prepare_batch_update_status(&mut self, owner: AttemptOwner, current_user_uri: &str,
        status: i32, registration: &GatewayRegistration<'_>) -> Result<PreparedHostingCall, CodecError> {
        if !self.connection.is_open() { self.pending.clear(); return Err(CodecError::InvalidField); }
        if self.batch_owner.as_ref() != Some(&owner) { return Err(CodecError::InvalidField); }
        if self.pending.len() >= MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
        let sequence = self.next_id.ok_or(CodecError::InvalidCallId)?;
        let operation = match status {
            STATUS_REGISTER => NativeOperation::HostingRegister,
            STATUS_UNREGISTER => NativeOperation::HostingUnregister,
            _ => return Err(CodecError::InvalidField),
        };
        let arguments = encode_update_status(current_user_uri, status, registration)
            .map_err(|code| if code == "jegotrip_rpc_arguments_too_large" { CodecError::TooLarge }
                else { CodecError::InvalidField })?;
        self.next_id = sequence.checked_add(1);
        self.pending.insert(sequence, PendingRequest { owner, operation, delivery: Delivery::Batch });
        Ok(PreparedHostingCall { sequence, arguments })
    }

    /// Accept only a successfully decoded original outer batch carrying the ARC
    /// owner/connection captured before submission, never provenance supplied at
    /// receipt. ARC failure/closure invalidates this tracker too. Each original
    /// nested result is consumed once; direct envelopes cannot complete batches.
    pub fn on_batch_reply(&mut self, batch: &super::arc_session::OwnedBatchReply<'_>)
        -> Result<Vec<OwnedRpcCallback>, CodecError> {
        if !self.connection.is_open() { self.pending.clear(); return Ok(vec![]); }
        let owner = self.batch_owner.as_ref().ok_or(CodecError::InvalidField)?;
        if !batch.matches(owner, &self.connection) { return Ok(vec![]); }
        let reply = batch.reply();
        if !reply.success || !reply.footer.reason.is_empty() {
            self.pending.clear();
            return Err(CodecError::RemoteFailure);
        }
        let mut callbacks = Vec::new();
        for (&sequence, result) in &reply.payload.results {
            if !self.pending.get(&sequence).is_some_and(|request| request.delivery == Delivery::Batch) { continue; }
            let Some(request) = self.pending.remove(&sequence) else { continue; };
            if request.operation != NativeOperation::HostingRegister { continue; }
            let callback = match decode_update_status_result(result.result_code, result.result_stream) {
                Ok((success, _)) => Callback::HostingAck { success },
                Err(_) => Callback::Failure(BackendFailure::HostingRejected),
            };
            callbacks.push(OwnedRpcCallback { owner: request.owner, callback });
        }
        Ok(callbacks)
    }

    /// Submission errors/timeouts must retire the originally captured request.
    pub fn retire_request(&mut self, id: i32) { self.pending.remove(&id); }

    pub fn retire_owner(&mut self, owner: &AttemptOwner) {
        self.pending.retain(|_, request| &request.owner != owner);
    }

    /// A failed outer RPC or malformed nested result consumes its request and
    /// fails that owner closed. Unknown/duplicate/retired/stale-connection replies
    /// cannot acknowledge a newer request, even if their ids coincide.
    pub fn on_reply(&mut self, connection: &RpcConnectionScope, bytes: &[u8])
        -> Result<Option<OwnedRpcCallback>, CodecError> {
        if !self.connection.is_open() { self.pending.clear(); return Ok(None); }
        if !Arc::ptr_eq(&self.connection.0, &connection.0) { return Ok(None); }
        let reply = decode_reply(bytes)?;
        if !self.pending.get(&reply.call_id).is_some_and(|request| request.delivery == Delivery::Envelope) {
            return Ok(None);
        }
        let Some(request) = self.pending.remove(&reply.call_id) else { return Ok(None); };
        if request.operation != NativeOperation::HostingRegister { return Ok(None); }
        let callback = match decode_update_status_result(reply.result_code, reply.result_stream) {
            Ok((success, _)) => Callback::HostingAck { success },
            Err(_) => Callback::Failure(BackendFailure::HostingRejected),
        };
        Ok(Some(OwnedRpcCallback { owner: request.owner, callback }))
    }
}

#[cfg(test)]
#[path = "rpc_pending_tests.rs"]
mod tests;
