//! Actual original-call Invite consumer. No SMS/IM conversion, endpoint lookup,
//! outbound RPC invention, auto-answer, channel-delivery or media-ready grant.
use super::*;
use super::super::native_invite_notifications::{self as invite, ConsumerClaim, IncomingInviteNotification};
use std::collections::VecDeque;
const INVALID: &str = "app_account_invite_notification_scope_invalid";
const BACKPRESSURE: &str = "app_account_invite_notification_backpressure";
struct Budget { holders: usize, bytes: usize }
struct RetainedInvite { incoming: IncomingInviteNotification, budget: Arc<Mutex<Budget>>, bytes: usize }
impl std::ops::Deref for RetainedInvite { type Target = IncomingInviteNotification; fn deref(&self) -> &Self::Target { &self.incoming } }
impl Drop for RetainedInvite { fn drop(&mut self) {
    let mut budget = lock(&self.budget);
    budget.holders = budget.holders.checked_sub(1).expect("original retained Invite holder");
    budget.bytes = budget.bytes.checked_sub(self.bytes).expect("original retained Invite bytes");
} }
struct GroupState { holders: VecDeque<RetainedInvite>, terminal: Option<bool>, running: bool }
struct Group { owner: AttemptOwner, scope: RpcConnectionScope, notification_id: u64, fingerprint: [u8;32],
    deadline: Instant, claim: Arc<ConsumerClaim>, changed: tokio::sync::Notify, state: Mutex<GroupState> }
pub(super) struct InviteQueue { groups: BTreeMap<u64,Arc<Group>>, budget: Arc<Mutex<Budget>> }
impl Default for InviteQueue { fn default() -> Self { Self { groups: BTreeMap::new(), budget: Arc::new(Mutex::new(Budget { holders: 0, bytes: 0 })) } } }
impl InviteQueue {
    pub(super) fn wake(&self) { for group in self.groups.values() { group.changed.notify_waiters(); } }
    #[cfg(test)]
    pub(super) fn retained_holders(&self) -> usize { lock(&self.budget).holders }
    pub(super) fn retire(&mut self) {
        for group in self.groups.values() { group.claim.cancel(); lock(&group.state).holders.clear(); group.changed.notify_waiters(); }
    }
    fn admit(&mut self, incoming: IncomingInviteNotification) -> Result<(Arc<Group>,bool), &'static str> {
        if !incoming.matches(incoming.owner(),&incoming.connection_scope()) { return Err(INVALID); }
        let retained = incoming.retained_bytes();
        let mut budget = lock(&self.budget);
        let bytes = budget.bytes.checked_add(retained).ok_or(BACKPRESSURE)?;
        if budget.holders >= invite::MAX_INVITE_HOLDERS || bytes > invite::MAX_INVITE_RETAINED_BYTES { return Err(BACKPRESSURE); }
        let group = if let Some(group) = self.groups.get(&incoming.notification_id()) {
            if !incoming.matches(&group.owner,&group.scope) || incoming.fingerprint() != group.fingerprint
                || incoming.deadline() < group.deadline { return Err(INVALID); }
            group.clone()
        } else {
            if self.groups.len() >= invite::MAX_INVITE_GROUPS { return Err(BACKPRESSURE); }
            let group = Arc::new(Group { owner: incoming.owner().clone(), scope: incoming.connection_scope(), notification_id: incoming.notification_id(),
                fingerprint: incoming.fingerprint(), deadline: incoming.deadline(), claim: ConsumerClaim::new(incoming.owner(),&incoming.connection_scope(),incoming.notification_id(),incoming.fingerprint(),incoming.deadline()), changed: tokio::sync::Notify::new(),
                state: Mutex::new(GroupState { holders: VecDeque::new(), terminal: None, running: false }) });
            self.groups.insert(incoming.notification_id(),group.clone()); group
        };
        budget.holders += 1; budget.bytes = bytes; drop(budget);
        let retained = RetainedInvite { incoming, budget: self.budget.clone(), bytes: retained };
        let start = { let mut state = lock(&group.state); let start = !state.running; state.running = true; state.holders.push_back(retained); start };
        group.changed.notify_waiters(); Ok((group,start))
    }
    fn pop(&mut self, group: &Group) -> Option<RetainedInvite> {
        // Moving to IO does not refund the budget: the zeroizing DATA and
        // original ticket remain charged until the actual in-flight drop.
        lock(&group.state).holders.pop_front()
    }
}
impl Drop for InviteQueue { fn drop(&mut self) { self.retire(); } }

impl CallService {
    pub(in super::super) async fn invite_notification(self: &Arc<Self>, incoming: IncomingInviteNotification) -> Result<(), &'static str> {
        let scope = incoming.connection_scope();
        if !self.backend.invite_notifications_enabled() || !incoming.matches(incoming.owner(),&scope)
            || !self.matches(incoming.owner(),&scope) || !self.accepting.load(Ordering::SeqCst) { return Err(INVALID); }
        // Reap/admit under the EXISTING task budget before mutating the queue.
        // No await occurs between successful admission and storing the Join.
        let mut tasks = self.task_admission().await.ok_or(BACKPRESSURE)?;
        if !self.matches(incoming.owner(),&scope) || !self.accepting.load(Ordering::SeqCst) { return Err(INVALID); }
        let (group,start) = lock(&self.invites).admit(incoming)?;
        if start { tasks.push(tokio::spawn(run_group(Arc::downgrade(self),group))); }
        Ok(())
    }
    fn consume_invite(&self, group: &Group) -> Result<Option<bool>, &'static str> {
        if !self.accepting.load(Ordering::SeqCst) || !self.matches(&group.owner,&group.scope) { return Err(INVALID); }
        if Instant::now() >= group.deadline { group.claim.cancel(); return Ok(Some(false)); }
        if let Some(terminal) = lock(&group.state).terminal { return Ok(Some(terminal)); }
        // Native ID bits only locate an EXISTING original call. The notification
        // never constructs or replaces its call/agent/control/offer/history.
        let call = lock(&self.calls).values().find(|call|
            call.ticket.matches(&group.owner,&group.scope) && call.ticket.native_id().expose_to_backend() == group.notification_id).cloned();
        let Some(call) = call else { return Ok(None); };
        if !self.active(&call) { return Ok(Some(false)); }
        let phase = lock(&call.state).phase;
        if phase == Phase::Checking { return Ok(None); }
        if !matches!(phase,Phase::Ringing|Phase::Accepting|Phase::Connected) { return Ok(Some(false)); }
        let receipt = call.ticket.with_media_gate(|| {
            let state = lock(&call.state);
            if !matches!(state.phase,Phase::Ringing|Phase::Accepting|Phase::Connected)
                || call.cancelled.load(Ordering::SeqCst) || !self.accepting.load(Ordering::SeqCst) || !group.claim.is_live()
                || Instant::now() >= group.deadline { return Err(INVALID); }
            super::super::native_invite_receipts::consume(&self.database,&self.run,&call.history,group.notification_id,&group.fingerprint)
        }).ok_or(INVALID)??;
        if !self.active(&call) || !group.claim.confirm(&receipt) || Instant::now() >= group.deadline { group.claim.cancel(); return Ok(Some(false)); }
        call.invite_consumed.store(true,Ordering::SeqCst);
        Ok(Some(true))
    }
}

#[cfg(test)]
#[path="account_invite_queue_tests.rs"]
pub(super) mod queue_tests;

async fn run_group(weak: Weak<CallService>, group: Arc<Group>) {
    loop {
        let changed = group.changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
        let Some(service) = weak.upgrade() else { group.claim.cancel(); return; };
        if !service.accepting.load(Ordering::SeqCst) || !group.scope.is_open() || group.scope.session_revoked()
            || !service.matches(&group.owner,&group.scope) {
            lock(&service.invites).retire(); lock(&group.state).running = false; return;
        }
        let terminal = match service.consume_invite(&group) {
            Ok(terminal) => terminal,
            Err(_) => Some(false), // actual storage/consumer failure never ACKs true
        };
        if let Some(terminal) = terminal {
            { let mut state = lock(&group.state); if state.terminal.is_none() || !terminal { state.terminal = Some(terminal); } }
            let incoming = lock(&service.invites).pop(&group);
            let Some(incoming) = incoming else { lock(&group.state).running = false; return; };
            let accepted = terminal && group.claim.is_live() && Instant::now() < group.deadline;
            let ticket = incoming.ticket().clipped_to(group.deadline,&group.claim);
            let backend = service.backend.clone(); drop(service);
            // The claim/deadline also reaches the IO actor. Cancelling this
            // wait cannot requeue an unknown write; the original actor observes
            // its dropped completion receiver and owns final close/Join.
            let result = tokio::select! { biased;
                _ = group.scope.session_ended() => { group.claim.cancel(); return; },
                _ = group.claim.cancelled(), if accepted => return,
                result = backend.complete_invite_notification(&group.owner,&group.scope,&ticket,accepted) => result,
            };
            if result.is_err() {
                group.claim.cancel();
                if let Some(service) = weak.upgrade() { service.retire(); }
                lock(&group.state).running = false; return;
            }
            continue;
        }
        drop(service); // no strong CallService -> Join -> CallService cycle
        tokio::select! { biased;
            _ = group.scope.session_ended() => { group.claim.cancel(); return; },
            _ = group.claim.cancelled() => return,
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(group.deadline)) => {},
            _ = changed => {},
        }
    }
}
