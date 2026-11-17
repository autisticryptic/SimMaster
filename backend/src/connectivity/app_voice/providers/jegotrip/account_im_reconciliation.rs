//! Explicit current-account read observation; no new user read/write consent.
use super::*;
use super::super::im_inbox::{ImReadObservation, ImReconciliationAction};
impl AccountManager {
    pub async fn reconcile_im_user_read(&self, line: &str, revision: u64, receipt_id: i64, consent: bool)
        -> Result<ImReadObservation, &'static str> {
        if !consent { return Err("app_account_im_reconciliation_consent_required"); }
        self.check_revision(line, revision)?;
        let slot = self.slot(line)?;
        let run = lock(&slot.run).clone().ok_or("app_account_im_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst) || run.stopping.load(Ordering::SeqCst) || run.finalized.load(Ordering::SeqCst)
            || !run.im_gate.live() || !run.backend.im_reconciliation_enabled() { return Err("app_account_im_reconciliation_not_installed"); }
        run.configuration_current()?;
        let controller = run.controller.lock().await;
        if controller.phase() != Phase::Hosted { return Err("app_account_im_scope_invalid"); }
        let owner = controller.resource_owner().ok_or("app_account_im_scope_invalid")?;
        let scope = run.backend.original_scope(&owner).ok_or("app_account_im_scope_invalid")?;
        let deadline = Instant::now().checked_add(run.backend.im_reconciliation_timeout()).ok_or("app_account_im_reconciliation_timeout")?;
        let action = ImReconciliationAction::original(run.database.clone(), run.run_token.clone(), owner, scope, run.im_gate.clone(), receipt_id, deadline)?;
        drop(controller);
        run.backend.reconcile_user_read(action).await
    }
}
