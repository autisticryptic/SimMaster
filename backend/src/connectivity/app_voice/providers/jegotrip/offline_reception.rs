//! 已 Hosted 的原账号消费器中的离线页调度；只有一页在途。
//! 提示 ACK 只表示接受取件提示，不表示短信消费。位置只能由 SQLite 提交
//! 证明推进；不确定的取件提交/写入失败关闭原 scope，重连从原身份重新核对。
use super::{ManagedRun, HostingController, Phase, InboxScope, AttemptOwner, RpcConnectionScope, lock};
use super::super::{rpc_offline::{IncomingOfflineHint, OwnedOfflinePage}, transport_actor::RuntimeError};
use std::{collections::VecDeque, sync::atomic::Ordering};

pub(super) struct State { cursor: i64, inflight: Option<(i32, i64, u64)>, wanted: bool, highest_hint: i64, hint_epoch: u64, pending_hints: VecDeque<IncomingOfflineHint> }
impl Default for State {
    fn default() -> Self { Self { cursor: -1, inflight: None, wanted: false, highest_hint: -1, hint_epoch: 0, pending_hints: VecDeque::new() } }
}
impl State {
    pub(super) fn hosted(&mut self, run: &ManagedRun) { if run.backend.offline_reception_enabled() { self.wanted = true; } }
    pub(super) fn clear(&mut self) { self.pending_hints.clear(); self.wanted = false; self.inflight = None; }
}
fn fail(run: &ManagedRun, code: &'static str) { if run.accepting.load(Ordering::SeqCst) { *lock(&run.error) = Some(code); run.retire(); } }
fn original(run: &ManagedRun, controller: &HostingController) -> Option<(AttemptOwner, RpcConnectionScope)> {
    if controller.phase() != Phase::Hosted || !run.accepting.load(Ordering::SeqCst) { return None; }
    let owner = controller.resource_owner()?; let scope = run.backend.original_scope(&owner)?;
    Some((owner, scope))
}

pub(super) async fn drive(run: &ManagedRun, controller: &HostingController, state: &mut State) {
    let Some((owner, scope)) = original(run, controller) else { return; };
    if let Some(hint) = state.pending_hints.pop_front() {
        acknowledge(run, controller, state, hint).await;
    }
    if !run.accepting.load(Ordering::SeqCst) || !run.backend.offline_reception_enabled() || !state.wanted || state.inflight.is_some() { return; }
    match run.backend.get_offline(&owner, &scope, state.cursor).await {
        Ok(receipt) if receipt.matches(&owner, &scope) => {
            let Some(id) = receipt.call_id else { fail(run, "app_account_offline_submission_invalid"); return; };
            state.inflight = Some((id, state.cursor, state.hint_epoch)); state.wanted = false;
        }
        Err(RuntimeError::Backpressure) => {}, // 未准入，250ms 定时器保留原位置重试。
        _ => fail(run, "app_account_offline_submission_failed"), // 不重放不确定提交。
    }
}
async fn acknowledge(run: &ManagedRun, controller: &HostingController, state: &mut State, hint: IncomingOfflineHint) {
    let owner = hint.owner().clone();
    let Some(scope) = run.backend.original_scope(&owner) else { return; };
    if !hint.matches(&owner, &scope) { return; }
    let accepted = controller.phase() == Phase::Hosted && run.accepting.load(Ordering::SeqCst) && run.backend.offline_reception_enabled();
    if accepted && hint.position() > state.highest_hint {
        state.highest_hint = hint.position(); state.hint_epoch = state.hint_epoch.saturating_add(1);
        if state.cursor == -1 || hint.position() >= state.cursor { state.wanted = true; }
    }
    match run.backend.complete_offline_hint(&owner, &scope, &hint, accepted).await {
        Ok(receipt) if receipt.submitted_on(&owner, &scope) => {},
        Err(RuntimeError::Backpressure) => {
            if state.pending_hints.len() >= 64 { fail(run, "app_account_offline_hint_backpressure"); }
            else { state.pending_hints.push_back(hint); }
        }
        _ => fail(run, "app_account_offline_hint_reply_failed"),
    }
}
pub(super) async fn hint(run: &ManagedRun, controller: &HostingController, state: &mut State, hint: IncomingOfflineHint) {
    acknowledge(run, controller, state, hint).await;
    drive(run, controller, state).await;
}
pub(super) async fn page(run: &ManagedRun, controller: &HostingController, state: &mut State, page: OwnedOfflinePage) {
    let Some((owner, scope)) = original(run, controller) else { return; };
    let Some((id, position, hint_epoch)) = state.inflight else { fail(run, "app_account_offline_page_unmatched"); return; };
    if !page.matches(&owner, &scope) || id != page.call_id() || position != page.requested_position() {
        fail(run, "app_account_offline_page_unmatched"); return;
    }
    state.inflight = None;
    let notifications = run.notifications.as_ref().map(|planner| planner.capture()).transpose();
    let context = InboxScope::original(&run.line_id, run.revision, &run.inbox_nonce, &owner, &scope)
        .and_then(|context| {
            if let Some(binding) = run.backend.original_native_binding(&owner, &scope) {
                context.with_native_receiver(&binding)
            } else {
                let binding = run.backend.original_incoming_binding(&owner, &scope).ok_or("app_account_offline_binding_missing")?;
                context.with_receiver(binding)
            }
        }).map(|context| context.with_notifications(notifications));
    let context = match context {
        Ok(context) => context,
        Err(code) => { fail(run, code); return; }
    };
    match context.commit_offline(&run.database, &page) {
        Ok(proof) if proof.matches(&owner, &scope) && run.accepting.load(Ordering::SeqCst) => {
            if proof.more() && proof.next_position() <= position { fail(run, "app_account_offline_no_progress"); return; }
            state.cursor = proof.next_position();
            // 仅本页真实 high-water 或本次请求之后的新提示可以继续；旧 hint
            // 比服务器页更高时不重复自旋，也绝不把其位置写进持久 cursor。
            state.wanted = proof.more() || state.hint_epoch != hint_epoch;
            drive(run, controller, state).await;
        }
        Ok(_) => {}, // 原 scope 已退休：允许历史提交，不准入新连接或后续请求。
        Err(code) => fail(run, code),
    }
}
