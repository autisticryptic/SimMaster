//! Bounded original-receiver CallClientEx dispatch, not call/media readiness.
//!
//! Object/domain are captured once from the original ARC receiver binding.
//! Native call ids, peer bytes and properties are DATA ONLY: no answering grant,
//! endpoint/SDP/crypto/time interpretation or SDK notification inference occurs.
//! The integrating runtime must retain the original direct/batch reply ticket
//! separately; a notice neither stores nor creates a reply capability.
use super::{
    batch_scheduler::IncomingBatchRequest,
    rpc_batch::BatchRequest,
    rpc_calls::{decode_notice, CallNotice, CallNoticeKind, NativeCallId, ACCEPTED_METHOD, INVITED_METHOD, RELEASED_METHOD},
    rpc_envelope::RpcRequest,
    rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError, RpcProperty, MAX_FIELD_BYTES, MAX_MAP_ENTRIES, MAX_STREAM_BYTES},
    session::AttemptOwner,
};
use std::fmt;
use zeroize::Zeroizing;

pub struct CallIncomingBinding<'a> {
    pub object_id: &'a [u8],
    pub domain: &'a [u8],
}

impl fmt::Debug for CallIncomingBinding<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallIncomingBinding(<redacted>)")
    }
}

struct Property {
    key: Zeroizing<Vec<u8>>,
    value: Zeroizing<Vec<u8>>,
}

/// An owned data snapshot with immutable original provenance, not a ticket.
/// Private zeroizing buffers outlive the packet/dispatcher but never expose an
/// unchecked peer/property accessor. No incoming-call or media state is set.
pub struct OwnedCallNotice {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    kind: CallNoticeKind,
    native_id: NativeCallId,
    peer: Option<Zeroizing<Vec<u8>>>,
    properties: Vec<Property>,
}

impl fmt::Debug for OwnedCallNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OwnedCallNotice(<redacted>)")
    }
}

impl OwnedCallNotice {
    pub fn owner(&self) -> &AttemptOwner { &self.owner }
    pub fn connection_scope(&self) -> RpcConnectionScope { self.scope.clone() }
    pub fn kind(&self) -> CallNoticeKind { self.kind }
    pub fn native_id(&self) -> NativeCallId { self.native_id }

    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }

    /// Checked borrowed DATA ONLY. Closing the original scope prevents new
    /// access; already borrowed/copied snapshots cannot be physically revoked.
    pub fn data_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<CallNotice<'_>> {
        if !self.matches(owner, scope) { return None; }
        Some(CallNotice {
            kind: self.kind,
            call_id: self.native_id,
            peer: self.peer.as_ref().map(|peer| peer.as_slice()),
            properties: self.properties.iter().map(|p| RpcProperty {
                key: p.key.as_slice(), value: p.value.as_slice(),
            }).collect(),
        })
    }
}

/// Installed by the original ARC integration, never rebound on packet receipt.
/// No public construction/injection API or wildcard registry is provided.
pub struct CallDispatcher {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    object_id: Zeroizing<Vec<u8>>,
    domain: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for CallDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CallDispatcher(<redacted>)")
    }
}

impl CallDispatcher {
    pub(super) fn for_arc_session(owner: AttemptOwner, scope: RpcConnectionScope, binding: CallIncomingBinding<'_>)
        -> Result<Self, CodecError> {
        if !scope.is_open() { return Err(CodecError::InvalidField); }
        let mut bytes = 0;
        for value in [binding.object_id, binding.domain] {
            add_field(&mut bytes, value)?;
            if value.is_empty() || value.contains(&b'*') || value.iter().any(u8::is_ascii_control)
                || value.windows(4).any(|part| part == b"ENC(") {
                return Err(CodecError::InvalidField);
            }
        }
        Ok(Self { owner, scope, object_id: Zeroizing::new(binding.object_id.to_vec()),
            domain: Zeroizing::new(binding.domain.to_vec()) })
    }

    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }

    /// Reject unrelated original tickets before examining routing or arguments.
    /// The caller must keep/wrap this request's original ticket separately.
    pub fn on_batch_request(&self, incoming: &IncomingBatchRequest) -> Result<Option<OwnedCallNotice>, CodecError> {
        if !incoming.ticket().matches(&self.owner, &self.scope) { return Ok(None); }
        let request: BatchRequest<'_> = incoming.request();
        self.dispatch(request.method, request.arguments, &request.metadata)
    }

    pub(super) fn on_direct_request(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, request: &RpcRequest<'_>)
        -> Result<Option<OwnedCallNotice>, CodecError> {
        if !self.matches(owner, scope) { return Ok(None); }
        // Borrow metadata rather than cloning an unbounded caller-supplied map.
        self.dispatch(request.method, request.arguments, &request.metadata)
    }

    fn dispatch(&self, method: &[u8], arguments: &[u8], metadata: &[RpcProperty<'_>])
        -> Result<Option<OwnedCallNotice>, CodecError> {
        if method != INVITED_METHOD.as_bytes() && method != RELEASED_METHOD.as_bytes()
            && method != ACCEPTED_METHOD.as_bytes() { return Ok(None); }
        let mut metadata_bytes = 0;
        measure_properties(metadata, &mut metadata_bytes)?;
        let property = |key: &[u8]| metadata.iter().find(|p| p.key == key).map(|p| p.value);
        if property(b"id") != Some(self.object_id.as_slice()) || property(b"domain") != Some(self.domain.as_slice()) {
            return Err(CodecError::InvalidField);
        }
        let Some(notice) = decode_notice(method, arguments)? else { return Ok(None); };
        // Bound all peer/property bytes before allocating any owned payload.
        // Metadata (__magic included) is only validated, not copied/interpreted.
        let mut copied_bytes = 0;
        if let Some(peer) = notice.peer { add_field(&mut copied_bytes, peer)?; }
        measure_properties(&notice.properties, &mut copied_bytes)?;
        let owned = OwnedCallNotice {
            owner: self.owner.clone(), scope: self.scope.clone(), kind: notice.kind, native_id: notice.call_id,
            peer: notice.peer.map(|peer| Zeroizing::new(peer.to_vec())),
            properties: notice.properties.iter().map(|p| Property {
                key: Zeroizing::new(p.key.to_vec()), value: Zeroizing::new(p.value.to_vec()),
            }).collect(),
        };
        // Also suppress publication if terminal close occurred during capture.
        if !self.scope.is_open() { return Ok(None); }
        Ok(Some(owned))
    }
}

fn add_field(total: &mut usize, value: &[u8]) -> Result<(), CodecError> {
    if value.len() > MAX_FIELD_BYTES { return Err(CodecError::TooLarge); }
    *total = total.checked_add(value.len()).ok_or(CodecError::TooLarge)?;
    if *total > MAX_STREAM_BYTES { return Err(CodecError::TooLarge); }
    Ok(())
}

fn measure_properties(properties: &[RpcProperty<'_>], total: &mut usize) -> Result<(), CodecError> {
    if properties.len() > MAX_MAP_ENTRIES { return Err(CodecError::TooLarge); }
    for (index, p) in properties.iter().enumerate() {
        add_field(total, p.key)?;
        add_field(total, p.value)?;
        if properties[..index].iter().any(|prior| prior.key == p.key) { return Err(CodecError::DuplicateKey); }
    }
    Ok(())
}

#[cfg(test)]
#[path = "call_binding_tests.rs"]
mod tests;
