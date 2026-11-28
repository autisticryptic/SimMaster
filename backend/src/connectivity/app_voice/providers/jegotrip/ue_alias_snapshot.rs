//! A single private #User observation on one matched Common login plan. The
//! bootstrap domain is NOT an unconditional Arc.Realm. No URI-to-owner API.
use super::super::{arc_session::ArcLoginInputs, im_inbox::ImReceiptGate, native_rpc_budget::NativeRpcReservation,
    router_connector::RouterPolicy, rpc_batch::BatchRequest, rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError, RpcProperty}, runtime_provision::ProvisionedCommonLogin,
    session::AttemptOwner, ue_alias_data::AliasRows};
use std::{fmt, mem::size_of, sync::{Arc, Mutex, MutexGuard, Weak}, time::Instant};

const METHOD: &[u8] = b"getRelations.User.User";
#[derive(Default)]
struct State {
    retired: bool,
    waiting: Weak<AliasRead>,
    // Distinguishes this read's full reply from cached data left by a failed
    // refresh or a selective notification. Neither weak link owns a read.
    observed: Weak<AliasRead>,
    snapshot: Option<AliasRows>,
}
pub(in super::super) struct AliasOrigin {
    owner: AttemptOwner, scope: RpcConnectionScope, gate: Arc<ImReceiptGate>,
    state: Mutex<State>, _charge: Arc<NativeRpcReservation>,
}
impl AliasOrigin {
    /// Called while binding the actual login, before its request is prepared.
    /// Never reconstruct this brand after login from equal-looking strings.
    pub(in super::super) fn from_plan(owner: &AttemptOwner, scope: &RpcConnectionScope, router: &RouterPolicy,
        plan: &ProvisionedCommonLogin, inputs: &ArcLoginInputs<'_>, gate: Arc<ImReceiptGate>) -> Result<Arc<Self>, CodecError> {
        let fields = plan.fields_for(owner, scope, router).map_err(|_| CodecError::InvalidField)?;
        if plan.bootstrap_call_id() <= 0 || !gate.live() || scope.session_revoked()
            || inputs.domain_name != fields.domain_name || inputs.domain_id != fields.domain_id
            || inputs.application_id != fields.application_id || inputs.device_info_cc != fields.device_info_cc
            || inputs.client_properties.iter().any(|p| matches!(p.key, b"DomainName" | b"Arc.Realm")) {
            return Err(CodecError::InvalidField);
        }
        let charge = scope.reserve_native_rpc(size_of::<Self>() + owner.line_id().len() + 128)?;
        Ok(Arc::new(Self { owner: owner.clone(), scope: scope.clone(), gate, state: Mutex::default(), _charge: charge }))
    }
    fn state(&self) -> MutexGuard<'_, State> { self.state.lock().unwrap_or_else(|p| p.into_inner()) }
    fn live(&self) -> bool { self.scope.is_open() && !self.scope.session_revoked() && self.gate.live() }
    pub(in super::super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.live() && !self.state().retired
    }
    pub(in super::super) fn retire(&self) {
        let mut state = self.state(); *state = State { retired: true, ..State::default() };
    }
    pub(in super::super) fn reported_pair(&self, kind: &[u8], value: &[u8]) -> Option<bool> {
        let s = self.state();
        if !self.live() || s.retired || kind.is_empty() || value.is_empty() { return None; }
        s.snapshot.as_ref().map(|rows| rows.contains(kind, value))
    }
    // A private negative-only mutation. The caller must authenticate the
    // original notification before using it; never creates a known empty set.
    pub(in super::super) fn preempt(&self, kind: &[u8], value: &[u8]) {
        if kind.is_empty() || value.is_empty() { return; }
        let mut s = self.state();
        if !self.live() || s.retired { return; }
        s.waiting = Weak::new();
        s.observed = Weak::new();
        if let Some(rows) = &mut s.snapshot { rows.preempt(kind, value); }
    }
}
impl fmt::Debug for AliasOrigin { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AliasOrigin(<redacted>)") } }

pub(in super::super) struct AliasRead { origin: Arc<AliasOrigin>, sequence: i32, deadline: Instant, _charge: Arc<NativeRpcReservation> }
impl AliasRead {
    pub(super) fn start(origin: &Arc<AliasOrigin>, sequence: i32, deadline: Instant) -> Result<Arc<Self>, CodecError> {
        let mut s = origin.state();
        let now = Instant::now();
        if !origin.live() || s.retired || sequence <= 0 || now >= deadline
            || deadline.saturating_duration_since(now) > std::time::Duration::from_secs(30)
            || s.waiting.upgrade().is_some_and(|old| now < old.deadline) { return Err(CodecError::InvalidField); }
        let charge = origin.scope.reserve_native_rpc(size_of::<Self>() + 4096)?;
        let read = Arc::new(Self { origin: origin.clone(), sequence, deadline, _charge: charge });
        s.waiting = Arc::downgrade(&read);
        s.observed = Weak::new();
        Ok(read)
    }
    pub(super) fn sequence(&self) -> i32 { self.sequence }
    fn is_waiting(&self, s: &State) -> bool {
        std::ptr::eq(s.waiting.as_ptr(), self) && !s.retired && self.origin.live() && Instant::now() < self.deadline
    }
    pub(in super::super) fn pending(&self) -> bool { self.is_waiting(&self.origin.state()) }
    pub(in super::super) fn cancel(&self) {
        let mut s = self.origin.state(); if std::ptr::eq(s.waiting.as_ptr(), self) { s.waiting = Weak::new(); }
    }
    pub(super) fn nested(&self) -> BatchRequest<'static> {
        BatchRequest { method: METHOD, arguments: &[0, 0, 1, 0, 0], metadata: vec![RpcProperty { key: b"id", value: b"#User" }] }
    }
    pub(super) fn stage(self: &Arc<Self>, code: u32, bytes: &[u8]) -> StagedAliases {
        let rows = if self.pending() { AliasRows::decode(&self.origin.scope, code, bytes) } else { Err(CodecError::InvalidField) };
        StagedAliases { read: self.clone(), rows }
    }
}
impl fmt::Debug for AliasRead { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AliasRead(<redacted>)") } }
/// No raw rows/bytes leave this scope. A failed refresh leaves the last good
/// snapshot unchanged. The original interest and absolute deadline are checked
/// again *after* decoding and all sibling event validation, under the same lock.
pub(in super::super) struct StagedAliases { read: Arc<AliasRead>, rows: Result<AliasRows, CodecError> }
impl StagedAliases {
    pub(super) fn sequence(&self) -> i32 { self.read.sequence }
    pub(super) fn belongs_to(&self, read: &Arc<AliasRead>) -> bool { Arc::ptr_eq(&self.read, read) }
    pub(in super::super) fn discard(self) { self.read.cancel(); }
    pub(super) fn finish(self, origin: &Arc<AliasOrigin>) {
        if !Arc::ptr_eq(&self.read.origin, origin) { return; }
        let mut s = origin.state();
        if !self.read.is_waiting(&s) { return; }
        s.waiting = Weak::new();
        if let Ok(rows) = self.rows {
            s.snapshot = Some(rows);
            s.observed = Arc::downgrade(&self.read);
        }
    }
}
pub(in super::super) struct AliasWait(Arc<AliasRead>);
impl AliasWait {
    pub(in super::super) fn original(read: Arc<AliasRead>) -> Self { Self(read) }
    pub(in super::super) fn sequence(&self) -> i32 { self.0.sequence }
    /// Read-only DATA from exactly this successful, still-current full reply.
    /// Failure, supersession or even a selective mutation cannot borrow it.
    pub(in super::super) fn observed_pair(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, kind: &[u8], value: &[u8]) -> Option<bool> {
        let read = &self.0; let origin = &read.origin; let s = origin.state();
        if &origin.owner != owner || !origin.scope.matches(scope) || !origin.live() || s.retired
            || Instant::now() >= read.deadline || !std::ptr::eq(s.observed.as_ptr(), Arc::as_ptr(read))
            || kind.is_empty() || value.is_empty() { return None; }
        s.snapshot.as_ref().map(|rows| rows.contains(kind, value))
    }
}
impl Drop for AliasWait { fn drop(&mut self) { self.0.cancel(); } }
