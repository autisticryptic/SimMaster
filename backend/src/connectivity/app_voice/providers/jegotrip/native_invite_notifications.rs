//! Original ClientEx Invite DATA and its original DIRECT/batch reply capability.
//!
//! 600E5C looks up the current call container by the notification identifier;
//! 612690 stores the onInvited BE64 identifier in that container's +16 slot.
//! Equal bits are only a scoped lookup key, never a call-control/media grant.
//! Notify.CallId/Caller remain opaque values, not endpoints or routing overrides.
use super::{arc_protocol::DirectReplyTicket, batch_scheduler::ServerRequestTicket,
    rpc_calls::NativeCallId, rpc_pending::RpcConnectionScope, rpc_stream::{CodecError, RpcProperty}, session::AttemptOwner};
use serde::{Deserialize, Serialize};
use ring::digest::{Context, SHA256};
use std::{fmt, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use zeroize::Zeroizing;

/// Explicit local consumer policy, not a recovered SDK timer or an auto-answer
/// setting. Absence leaves the existing known-command NACK path unchanged.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeInviteConfig { pub timeout_ms: u64 }
impl fmt::Debug for NativeInviteConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("NativeInviteConfig(<redacted>)") }
}
impl NativeInviteConfig {
    pub(super) fn timeout(self) -> Result<Duration, CodecError> {
        if !(100..=30_000).contains(&self.timeout_ms) { return Err(CodecError::InvalidField); }
        Ok(Duration::from_millis(self.timeout_ms))
    }
}

pub(super) const MAX_INVITE_VALUE_BYTES: usize = 4 * 1024;
pub(super) const MAX_INVITE_GROUPS: usize = 64;
pub(super) const MAX_INVITE_HOLDERS: usize = 256;
pub(super) const MAX_INVITE_RETAINED_BYTES: usize = 1024 * 1024;

pub(super) struct InviteData {
    owner: AttemptOwner, scope: RpcConnectionScope, notification_id: u64,
    call_id: Zeroizing<Vec<u8>>, caller: Zeroizing<Vec<u8>>, deadline: Instant,
}
impl fmt::Debug for InviteData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("InviteData(<redacted>)") }
}
impl InviteData {
    /// Only the authenticated dispatcher captures this value. Its metadata
    /// binding and current ARC token are checked before this allocation.
    pub(super) fn capture(owner: &AttemptOwner, scope: &RpcConnectionScope, notification_id: u64,
        properties: &[RpcProperty<'_>], config: NativeInviteConfig, now: Instant) -> Result<Self, CodecError> {
        if !scope.is_open() || scope.session_revoked() || notification_id == 0 || properties.len() != 3 {
            return Err(CodecError::InvalidField);
        }
        let value = |key: &[u8]| properties.iter().find(|p| p.key == key).map(|p| p.value);
        if value(b"Notify.Command") != Some(b"Invite".as_slice()) { return Err(CodecError::InvalidField); }
        for (index, p) in properties.iter().enumerate() {
            if properties[..index].iter().any(|prior| prior.key == p.key) { return Err(CodecError::DuplicateKey); }
            if !matches!(p.key, b"Notify.Command" | b"Notify.CallId" | b"Notify.Caller") { return Err(CodecError::InvalidField); }
        }
        let call_id = value(b"Notify.CallId").ok_or(CodecError::InvalidField)?;
        let caller = value(b"Notify.Caller").ok_or(CodecError::InvalidField)?;
        if call_id.len() > MAX_INVITE_VALUE_BYTES || caller.len() > MAX_INVITE_VALUE_BYTES { return Err(CodecError::TooLarge); }
        let deadline = now.checked_add(config.timeout()?).ok_or(CodecError::InvalidField)?;
        Ok(Self { owner: owner.clone(), scope: scope.clone(), notification_id,
            call_id: Zeroizing::new(call_id.to_vec()), caller: Zeroizing::new(caller.to_vec()), deadline })
    }
    fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.is_open() && !scope.session_revoked()
    }
    fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Context::new(&SHA256); hash.update(b"jegotrip-invite-association-v1\0");
        hash.update(&self.notification_id.to_be_bytes());
        for value in [&self.call_id, &self.caller] {
            hash.update(&(value.len() as u32).to_be_bytes()); hash.update(value.as_slice());
        }
        let digest = hash.finish(); let mut fingerprint = [0;32]; fingerprint.copy_from_slice(digest.as_ref()); fingerprint
    }
}

/// Local consumer cancellation brand, never serializable or caller-supplied.
/// Retiring the original consumer synchronously revokes queued positive work.
pub(super) struct ConsumerClaim { owner: AttemptOwner, scope: RpcConnectionScope, notification_id: u64, fingerprint: [u8;32], deadline: Instant,
    live: AtomicBool, consumed: AtomicBool, call: std::sync::Mutex<Option<super::call_control::CallControlTicket>>, changed: tokio::sync::Notify,
    #[cfg(test)] component: AtomicBool,
}
impl ConsumerClaim {
    pub(super) fn new(owner: &AttemptOwner, scope: &RpcConnectionScope, notification_id: u64, fingerprint: [u8;32], deadline: Instant) -> Arc<Self> {
        Arc::new(Self { owner: owner.clone(), scope: scope.clone(), notification_id, fingerprint, deadline,
            live: AtomicBool::new(true), consumed: AtomicBool::new(false), call: std::sync::Mutex::new(None), changed: tokio::sync::Notify::new(),
            #[cfg(test)] component: AtomicBool::new(false), })
    }
    pub(super) fn confirm(&self, receipt: &super::native_invite_receipts::InviteConsumptionReceipt, call: &super::call_control::CallControlTicket) -> bool {
        if !self.is_live() || !self.scope.is_open() || self.scope.session_revoked() || Instant::now() >= self.deadline
            || !receipt.matches(self.notification_id,&self.fingerprint) || !call.matches(&self.owner,&self.scope)
            || call.native_id().expose_to_backend() != self.notification_id || !call.is_live() { return false; }
        {
            let mut captured = self.call.lock().unwrap_or_else(|poison| poison.into_inner());
            if captured.as_ref().is_some_and(|original| !original.same_call(call)) { return false; }
            if captured.is_none() { *captured = Some(call.captured_copy()); }
        }
        self.consumed.store(true,Ordering::SeqCst); self.changed.notify_waiters(); self.is_live()
    }
    #[cfg(test)]
    pub(super) fn fixture_consumed(owner: &AttemptOwner, scope: &RpcConnectionScope, notification_id: u64, fingerprint: [u8;32], deadline: Instant) -> Arc<Self> {
        let claim = Self::new(owner,scope,notification_id,fingerprint,deadline); claim.component.store(true,Ordering::SeqCst); claim.consumed.store(true,Ordering::SeqCst); claim
    }
    pub(super) fn cancel(&self) { self.live.store(false, Ordering::SeqCst); self.changed.notify_waiters(); }
    pub(super) fn is_live(&self) -> bool {
        self.live.load(Ordering::SeqCst) && self.call.lock().unwrap_or_else(|poison| poison.into_inner()).as_ref().is_none_or(|call| call.is_live())
    }
    fn may_ack(&self, notification_id: u64, fingerprint: &[u8;32]) -> bool {
        let bound = self.call.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        #[cfg(test)] let bound = bound || self.component.load(Ordering::SeqCst);
        bound && self.is_live() && self.consumed.load(Ordering::SeqCst) && self.scope.is_open() && !self.scope.session_revoked()
            && notification_id == self.notification_id && fingerprint == &self.fingerprint
    }
    pub(super) async fn cancelled(&self) {
        loop {
            let wake = self.changed.notified(); tokio::pin!(wake); wake.as_mut().enable(); if !self.is_live() { return; }
            let call = self.call.lock().unwrap_or_else(|poison| poison.into_inner()).as_ref().map(|call| call.captured_copy());
            tokio::select! { biased;
                _ = self.scope.session_ended() => return,
                _ = async { if let Some(call) = &call { call.revoked().await; } else { std::future::pending::<()>().await; } } => return,
                _ = wake => {},
            }
        }
    }
}
impl fmt::Debug for ConsumerClaim { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("InviteConsumerClaim(<redacted>)") } }

pub(super) enum InviteReplySource { Direct(DirectReplyTicket), Batch(ServerRequestTicket) }
pub struct InviteReplyTicket { source: InviteReplySource, notification_id: u64, fingerprint: [u8; 32], deadline: Instant, completion_deadline: Instant,
    claim: Option<Arc<ConsumerClaim>> }
impl fmt::Debug for InviteReplyTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("InviteReplyTicket(<redacted>)") }
}
impl InviteReplyTicket {
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        if !scope.is_open() || scope.session_revoked() { return false; }
        match &self.source {
            InviteReplySource::Direct(ticket) => ticket.matches(owner, scope),
            InviteReplySource::Batch(ticket) => ticket.matches(owner, scope),
        }
    }
    pub(super) fn source(&self) -> &InviteReplySource { &self.source }
    pub(super) fn notification_id(&self) -> u64 { self.notification_id }
    pub(super) fn fingerprint(&self) -> &[u8; 32] { &self.fingerprint }
    pub(super) fn deadline(&self) -> Instant { self.deadline }
    pub(super) fn completion_deadline(&self) -> Instant { self.completion_deadline }
    pub(super) fn clipped_to(&self, deadline: Instant, claim: &Arc<ConsumerClaim>) -> Self {
        let mut ticket = self.captured_copy(); ticket.completion_deadline = ticket.completion_deadline.min(deadline);
        if ticket.claim.as_ref().is_none_or(|original| Arc::ptr_eq(original,claim)) { ticket.claim = Some(claim.clone()); } ticket
    }
    pub(super) fn positive_is_current(&self) -> bool {
        self.claim.as_ref().is_some_and(|claim| self.matches(&claim.owner,&claim.scope) && claim.may_ack(self.notification_id,&self.fingerprint))
            && Instant::now() < self.completion_deadline
    }
    pub(super) fn consumer_claim(&self) -> Option<Arc<ConsumerClaim>> { self.claim.clone() }
    pub(super) fn captured_copy(&self) -> Self {
        let source = match &self.source {
            InviteReplySource::Direct(ticket) => InviteReplySource::Direct(ticket.captured_copy()),
            InviteReplySource::Batch(ticket) => InviteReplySource::Batch(ticket.captured_copy()),
        };
        Self { source, notification_id: self.notification_id, fingerprint: self.fingerprint, deadline: self.deadline, completion_deadline: self.completion_deadline, claim: self.claim.clone() }
    }
}

pub struct IncomingInviteNotification { data: InviteData, ticket: InviteReplyTicket }
impl fmt::Debug for IncomingInviteNotification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingInviteNotification(<redacted>)") }
}
impl IncomingInviteNotification {
    pub(super) fn from_direct(data: InviteData, ticket: DirectReplyTicket) -> Result<Self, CodecError> {
        Self::capture(data, InviteReplySource::Direct(ticket))
    }
    pub(super) fn from_batch(data: InviteData, ticket: ServerRequestTicket) -> Result<Self, CodecError> {
        Self::capture(data, InviteReplySource::Batch(ticket))
    }
    fn capture(data: InviteData, source: InviteReplySource) -> Result<Self, CodecError> {
        let ticket = InviteReplyTicket { source, notification_id: data.notification_id, fingerprint: data.fingerprint(), deadline: data.deadline, completion_deadline: data.deadline, claim: None };
        if !ticket.matches(&data.owner, &data.scope) { return Err(CodecError::InvalidField); }
        Ok(Self { data, ticket })
    }
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        self.data.matches(owner, scope) && self.ticket.matches(owner, scope)
    }
    pub(super) fn owner(&self) -> &AttemptOwner { &self.data.owner }
    pub(super) fn connection_scope(&self) -> RpcConnectionScope { self.data.scope.clone() }
    pub(super) fn ticket(&self) -> &InviteReplyTicket { &self.ticket }
    pub(super) fn notification_id(&self) -> u64 { self.data.notification_id }
    pub(super) fn deadline(&self) -> Instant { self.data.deadline }
    pub(super) fn fingerprint(&self) -> [u8; 32] { self.ticket.fingerprint }
    pub(super) fn retained_bytes(&self) -> usize { 128 + self.data.call_id.len() + self.data.caller.len() }
    /// DATA comparison only. The caller must already own the original live
    /// CallControlTicket and validated local call/history; this mints no ticket.
    pub(super) fn identifies(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, call_id: NativeCallId) -> bool {
        self.matches(owner, scope) && self.data.notification_id == call_id.expose_to_backend()
    }
}

pub(super) fn encode_completion(accepted: bool) -> Result<super::rpc_stream::RpcBytes, CodecError> {
    let mut writer = super::rpc_stream::Writer::new(); writer.boolean(accepted)?; writer.bytes(b"")?; Ok(writer.finish())
}
