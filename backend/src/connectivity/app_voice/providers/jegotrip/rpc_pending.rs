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
use std::{collections::BTreeMap, sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex}};

#[derive(Clone)]
pub struct RpcConnectionScope(Arc<AtomicBool>, Arc<Mutex<()>>, Arc<tokio::sync::Notify>);

impl RpcConnectionScope {
    pub(super) fn fresh() -> Self {
        Self(Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(())), Arc::new(tokio::sync::Notify::new()))
    }
    pub(super) fn matches(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
    pub(super) fn is_open(&self) -> bool { self.0.load(Ordering::Acquire) }
    pub(super) fn close(&self) {
        // Serializes terminal close with one NONBLOCKING IO poll. Already
        // accepted/kernel-buffered bytes cannot be recalled by this gate.
        let _gate = self.1.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.0.store(false, Ordering::Release); self.2.notify_waiters();
    }
    /// No await, blocking IO, callbacks or reentrant close are permitted inside
    /// this closure. The actor releases the gate on Pending and rechecks before
    /// every new write/flush poll, not only when borrowing a byte snapshot.
    pub(super) fn with_io_gate<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        let _gate = match self.1.lock() {
            Ok(gate) => gate,
            Err(poisoned) => {
                let _gate = poisoned.into_inner();
                self.0.store(false, Ordering::Release); self.2.notify_waiters();
                return None;
            }
        };
        if !self.is_open() { return None; }
        Some(operation())
    }
    pub(super) async fn closed(&self) {
        loop {
            let notified = self.2.notified(); tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_open() { return; }
            notified.await;
        }
    }
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
    submission_sequence: i32,
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    arguments: RpcBytes,
}

impl PreparedHostingCall {
    pub(super) fn matches(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        self.sequence == self.submission_sequence && &self.owner == owner
            && self.connection.matches(connection) && self.connection.is_open()
    }

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

/// Privately matched unregister result from one original pending operation.
/// It is never a Hosted/channel callback and cannot complete another register.
pub struct OwnedUnhostingReply { owner: AttemptOwner, connection: RpcConnectionScope, sequence: i32, accepted: bool, rejected: bool }
impl OwnedUnhostingReply {
    pub fn sequence(&self) -> i32 { self.sequence }
    pub fn accepted(&self) -> bool { self.accepted }
    /// Distinguish a decoded native negative/remote failure from malformed data.
    pub(super) fn rejected(&self) -> bool { self.rejected }
    pub(super) fn matches(&self, owner: &AttemptOwner, connection: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(connection) && self.connection.is_open()
    }
}
impl std::fmt::Debug for OwnedUnhostingReply { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("OwnedUnhostingReply(<redacted>)") } }

pub struct OwnedRpcCallback {
    pub owner: AttemptOwner,
    pub callback: Callback,
}

pub struct HostingRpcTracker {
    connection: RpcConnectionScope,
    batch_owner: Option<AttemptOwner>,
    next_id: Option<i32>,
    pending: BTreeMap<i32, PendingRequest>,
    unhosting: Vec<OwnedUnhostingReply>,
}

impl Default for HostingRpcTracker {
    fn default() -> Self { Self::new() }
}

impl HostingRpcTracker {
    /// Standalone Common-envelope tracker for offline/mock callers only.
    /// Native nested hosting requires the original ARC session's one-time tracker.
    pub fn new() -> Self {
        Self { connection: RpcConnectionScope::fresh(), batch_owner: None,
            next_id: Some(1), pending: BTreeMap::new(), unhosting: Vec::new() }
    }

    pub(super) fn for_arc_session(owner: AttemptOwner, connection: RpcConnectionScope) -> Self {
        Self { connection, batch_owner: Some(owner), next_id: Some(1), pending: BTreeMap::new(), unhosting: Vec::new() }
    }

    /// Capture this when the authenticated connection installs its receive task.
    /// A stale task cannot feed bytes into a new connection's correlation map.
    pub fn connection_scope(&self) -> RpcConnectionScope { self.connection.clone() }
    pub(super) fn batch_owner(&self) -> Result<AttemptOwner, CodecError> {
        self.batch_owner.clone().filter(|_| self.connection.is_open()).ok_or(CodecError::InvalidField)
    }

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
        self.pending.insert(sequence, PendingRequest { owner: owner.clone(), operation, delivery: Delivery::Batch });
        Ok(PreparedHostingCall { sequence, submission_sequence: sequence, owner,
            connection: self.connection.clone(), arguments })
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
        // Scheduled batches must pass validated ACK/epoch/dedup processing first;
        // a raw matched outer result cannot bypass that state machine.
        if batch.is_scheduled() { return Err(CodecError::InvalidField); }
        let reply = batch.reply();
        if !reply.success || !reply.footer.reason.is_empty() {
            self.pending.clear();
            return Err(CodecError::RemoteFailure);
        }
        let mut callbacks = Vec::new();
        for (&sequence, result) in &reply.payload.results {
            if !self.pending.get(&sequence).is_some_and(|request| request.delivery == Delivery::Batch) { continue; }
            let Some(request) = self.pending.remove(&sequence) else { continue; };
            if let Some(callback) = self.status_completed(request, sequence, result.result_code, result.result_stream) { callbacks.push(callback); }
        }
        Ok(callbacks)
    }

    pub(super) fn on_scheduled_delivery(&mut self, delivery: &super::batch_scheduler::BatchDelivery)
        -> Vec<OwnedRpcCallback> {
        if !self.connection.is_open() { self.pending.clear(); return vec![]; }
        let Some(owner) = self.batch_owner.as_ref() else { return vec![]; };
        if !delivery.matches(owner, &self.connection) { return vec![]; }
        let mut callbacks = Vec::new();
        for (sequence, result) in delivery.results() {
            if !self.pending.get(&sequence).is_some_and(|p| p.delivery == Delivery::Batch) { continue; }
            let Some(request) = self.pending.remove(&sequence) else { continue; };
            if let Some(callback) = self.status_completed(request, sequence, result.result_code, result.result_stream) { callbacks.push(callback); }
        }
        callbacks
    }

    fn status_completed(&mut self, request: PendingRequest, sequence: i32, code: u32, bytes: &[u8]) -> Option<OwnedRpcCallback> {
        if request.operation == NativeOperation::HostingUnregister {
            if self.unhosting.len() >= MAX_MAP_ENTRIES { self.connection.close(); self.pending.clear(); self.unhosting.clear(); return None; }
            let outcome = decode_update_status_result(code, bytes);
            let accepted = outcome.as_ref().is_ok_and(|(accepted, _)| *accepted);
            let rejected = matches!(outcome, Ok((false, _)) | Err(CodecError::RemoteFailure));
            self.unhosting.push(OwnedUnhostingReply { owner: request.owner, connection: self.connection.clone(), sequence, accepted, rejected });
            None
        } else if request.operation == NativeOperation::HostingRegister {
            let callback = match decode_update_status_result(code, bytes) { Ok((success, _)) => Callback::HostingAck { success },
                Err(_) => Callback::Failure(BackendFailure::HostingRejected) };
            Some(OwnedRpcCallback { owner: request.owner, callback })
        } else { None }
    }
    pub(super) fn take_unhosting_replies(&mut self) -> Vec<OwnedUnhostingReply> {
        if !self.connection.is_open() { self.retire_all(); return Vec::new(); }
        std::mem::take(&mut self.unhosting)
    }
    pub(super) fn retire_all(&mut self) { self.pending.clear(); self.unhosting.clear(); }

    /// Retire pending work AND any completion not yet handed to its consumer.
    /// Already borrowed/copied results remain scoped snapshots, not revocable.
    pub fn retire_request(&mut self, id: i32) {
        self.pending.remove(&id);
        self.unhosting.retain(|reply| reply.sequence != id);
    }

    pub fn retire_owner(&mut self, owner: &AttemptOwner) {
        self.pending.retain(|_, request| &request.owner != owner);
        self.unhosting.retain(|reply| &reply.owner != owner);
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
        Ok(self.status_completed(request, reply.call_id, reply.result_code, reply.result_stream))
    }
}

#[cfg(test)]
#[path = "rpc_pending_tests.rs"]
mod tests;
