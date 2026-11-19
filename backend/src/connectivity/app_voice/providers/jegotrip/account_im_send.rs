//! Explicit current application-account IM business send. Stored identity,
//! import/SDK-ready DATA and received/read receipts are not send grants.
use super::*;
use super::super::im_inbox::{ImSendMessage, ImSendView, ImSendFailure};
impl AccountManager {
    pub async fn send_im_message(&self, line: &str, revision: u64, recipient: &str,
        message: ImSendMessage, nonce: &str, consent: bool, write_consent: bool,
    ) -> Result<ImSendView, &'static str> {
        self.send_im_message_with_admission(line, revision, recipient, message, nonce, consent, write_consent)
            .await.map_err(|error| error.code())
    }
    /// Admission metadata is a finite stage observation. It is neither an ACK
    /// nor a permission to retry. The HTTP/UI still rechecks whole history.
    pub async fn send_im_message_with_admission(&self, line: &str, revision: u64, recipient: &str,
        message: ImSendMessage, nonce: &str, consent: bool, write_consent: bool,
    ) -> Result<ImSendView, ImSendFailure> {
        if !consent || !write_consent { return Err(ImSendFailure::before("app_account_im_send_consent_required")); }
        self.check_revision(line, revision).map_err(ImSendFailure::before)?;
        let slot = self.slot(line).map_err(ImSendFailure::before)?;
        let run = lock(&slot.run).clone().ok_or_else(|| ImSendFailure::before("app_account_im_send_scope_invalid"))?;
        if !run.accepting.load(Ordering::SeqCst) || run.stopping.load(Ordering::SeqCst)
            || run.finalized.load(Ordering::SeqCst) || !run.im_gate.live() || !run.backend.im_send_enabled() {
            return Err(ImSendFailure::before("app_account_im_send_not_installed"));
        }
        run.configuration_current().map_err(ImSendFailure::before)?;
        let deadline = Instant::now().checked_add(run.backend.im_send_timeout())
            .ok_or_else(|| ImSendFailure::before("app_account_im_send_timeout"))?;
        // Synchronous snapshot/reservation of the ORIGINAL backend's slot,
        // before any suspension. This is accounting, not authentication.
        let (reserved_owner, scope, input) = run.backend.reserve_im_send_input(&message, recipient.as_bytes(), nonce)
            .map_err(ImSendFailure::before)?;
        let controller = tokio::select! {
            biased;
            _ = run.im_gate.retired() => return Err(ImSendFailure::before("app_account_im_send_scope_invalid")),
            _ = scope.session_ended() => return Err(ImSendFailure::before("app_account_im_send_scope_invalid")),
            result = tokio::time::timeout_at(deadline.into(), run.controller.lock()) =>
                result.map_err(|_| ImSendFailure::before("app_account_im_send_timeout"))?,
        };
        run.configuration_current().map_err(ImSendFailure::before)?;
        if controller.phase() != Phase::Hosted || !run.accepting.load(Ordering::SeqCst)
            || run.stopping.load(Ordering::SeqCst) || run.finalized.load(Ordering::SeqCst) || !run.im_gate.live() {
            return Err(ImSendFailure::before("app_account_im_send_scope_invalid"));
        }
        let owner = controller.resource_owner().ok_or_else(|| ImSendFailure::before("app_account_im_send_scope_invalid"))?;
        if owner != reserved_owner || !run.backend.original_scope(&owner).is_some_and(|s| s.matches(&scope)) {
            return Err(ImSendFailure::before("app_account_im_send_scope_invalid"));
        }
        drop(controller);
        let result = run.backend.send_im_message(run.database.clone(), run.run_token.clone(), run.im_gate.clone(),
            owner, scope, zeroize::Zeroizing::new(recipient.as_bytes().to_vec()), message,
            zeroize::Zeroizing::new(nonce.to_owned()), deadline, input).await?;
        run.configuration_current().map_err(ImSendFailure::after)?;
        if !run.accepting.load(Ordering::SeqCst) || run.stopping.load(Ordering::SeqCst)
            || run.finalized.load(Ordering::SeqCst) || !run.im_gate.live() {
            return Err(ImSendFailure::after("app_account_im_send_scope_invalid"));
        }
        Ok(result)
    }
    #[cfg(test)]
    pub(crate) fn im_send_resources_for_test(&self,line:&str)->Result<(usize,usize),&'static str>{let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("fixture original send run")?;run.backend.im_send_resources_for_test().ok_or("fixture original send scope")}
    #[cfg(test)]
    pub(crate) async fn im_send_controller_pause_for_test(&self,line:&str,ready:tokio::sync::oneshot::Sender<()>,release:tokio::sync::oneshot::Receiver<()>)->Result<(),&'static str>{let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("fixture original send run")?;let _controller=run.controller.lock().await;ready.send(()).map_err(|_|"fixture controller ready")?;release.await.map_err(|_|"fixture controller release")}
    #[cfg(test)]
    pub(crate) fn im_send_retire_gate_for_test(&self,line:&str)->Result<(),&'static str>{let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("fixture original send run")?;run.im_gate.retire();Ok(())}
    #[cfg(test)]
    pub(crate) async fn im_send_actor_barrier_for_test(&self,line:&str,revision:u64)->Result<(),&'static str>{
        self.check_revision(line,revision)?;let slot=self.slot(line)?;
        let run=lock(&slot.run).clone().ok_or("fixture original send run")?;
        let controller=run.controller.lock().await;
        let owner=controller.resource_owner().ok_or("fixture original send owner")?;
        let scope=run.backend.original_scope(&owner).ok_or("fixture original send scope")?;
        drop(controller);
        run.backend.im_send_actor_barrier_for_test(&owner,&scope).await
    }
    pub fn im_send_history(&self, line: &str, limit: usize) -> Result<Vec<ImSendView>, &'static str> {
        self.database.with_conn(|c| super::super::im_inbox::im_send_history(c, line, limit))
    }
}
