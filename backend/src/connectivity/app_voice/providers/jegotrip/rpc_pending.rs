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
    rpc_envelope::{decode_reply, encode_request},
    rpc_stream::{CodecError, RpcBytes, RpcProperty, MAX_MAP_ENTRIES},
    session::{AttemptOwner, BackendFailure, GatewayRegistration, STATUS_REGISTER, STATUS_UNREGISTER},
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct RpcConnectionScope(Arc<()>);

struct PendingRequest {
    owner: AttemptOwner,
    operation: NativeOperation,
}

pub struct OwnedRpcCallback {
    pub owner: AttemptOwner,
    pub callback: Callback,
}

pub struct HostingRpcTracker {
    connection: RpcConnectionScope,
    next_id: Option<i32>,
    pending: BTreeMap<i32, PendingRequest>,
}

impl Default for HostingRpcTracker {
    fn default() -> Self { Self::new() }
}

impl HostingRpcTracker {
    pub fn new() -> Self {
        Self { connection: RpcConnectionScope(Arc::new(())), next_id: Some(1), pending: BTreeMap::new() }
    }

    /// Capture this when the authenticated connection installs its receive task.
    /// A stale task cannot feed bytes into a new connection's correlation map.
    pub fn connection_scope(&self) -> RpcConnectionScope { self.connection.clone() }

    /// Caller supplies metadata captured from the native session/object agent
    /// (including id/domain/__magic where required); the codec does not guess it.
    pub fn prepare_update_status(&mut self, owner: AttemptOwner, current_user_uri: &str,
        status: i32, registration: &GatewayRegistration<'_>, metadata: &[RpcProperty<'_>])
        -> Result<(i32, RpcBytes), CodecError> {
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
        self.pending.insert(id, PendingRequest { owner, operation });
        Ok((id, bytes))
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
        if !Arc::ptr_eq(&self.connection.0, &connection.0) { return Ok(None); }
        let reply = decode_reply(bytes)?;
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
