//! Current original native Group READ only; no journal resolution/write retry.
use super::*;
use super::super::im_inbox::{ImReadObservation, ImReconciliationAction, ImReconciliationWait};
impl NativeHostingBackend {
    pub(in super::super) fn im_reconciliation_enabled(&self) -> bool { self.profile.common.is_some() && self.im_messages_enabled() }
    pub(in super::super) fn im_reconciliation_timeout(&self) -> Duration { self.profile.rpc_timeout }
    pub(in super::super) async fn reconcile_user_read(&self, action: Arc<ImReconciliationAction>) -> Result<ImReadObservation, &'static str> {
        let _wait = ImReconciliationWait::original(action.clone());
        if !self.im_reconciliation_enabled() { return Err("app_account_im_reconciliation_not_installed"); }
        if !action.live() { return Err("app_account_im_scope_invalid"); }
        let attempt = self.original(action.owner()).map_err(|_| "app_account_im_scope_invalid")?;
        { let state = lock(&attempt.state); if !state.configured || state.retired || state.logout_started || state.unregister_started { return Err("app_account_im_scope_invalid"); } }
        let scope = action.scope();
        let commands = attempt.commands(action.owner(), &scope).map_err(|_| "app_account_im_scope_invalid")?;
        // Heap-own this explicit operation on the same ARC command actor;
        // never add another task/SDK scope or hold account controller locks.
        let operation = Box::pin(async {
            let binding = commands.original_im_binding(action.owner(), &scope).await.map_err(|_| "app_account_im_scope_invalid")?;
            let context = action.bind_current(binding)?;
            let reply = commands.read_im(action.owner(), &scope, context.query(), action.gate()).await
                .map_err(|_| "app_account_im_reconciliation_reply_invalid")?;
            context.observe(&reply)
        });
        let parent = action.parent_gate();
        tokio::select! { biased;
            _ = parent.retired() => Err("app_account_im_scope_invalid"),
            result = tokio::time::timeout_at(action.deadline().into(), operation) => result.map_err(|_| "app_account_im_reconciliation_timeout")?,
        }
    }
}
