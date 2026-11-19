//! Original user-requested native IM send chain. It uses the current Common
//! actor and original auxiliary READ, not a new SDK or automatic business IO.
use super::*;
use super::super::{
    im_inbox::{ImSendMessage, ImSendAction, ImSendInput, ImSendWait, ImSendView, ImSendFailure, PreparedImSend},
    user_entry_rpc::{resolve_original, OwnedUserEntryReply},
    user_entry_action::UserEntryAction, account_runs::RunToken,
};
impl NativeHostingBackend {
    pub(in super::super) fn im_send_timeout(&self) -> Duration { self.profile.rpc_timeout }
    pub(in super::super) fn reserve_im_send_input(&self, message: &ImSendMessage, recipient: &[u8], nonce: &str)
        -> Result<(AttemptOwner, RpcConnectionScope, ImSendInput), &'static str> {
        if !self.im_send_enabled() { return Err("app_account_im_send_not_installed"); }
        let attempt = lock(&self.state).slot.clone().ok_or("app_account_im_send_scope_invalid")?;
        let state = lock(&attempt.state);
        if !state.configured || state.retired || state.logout_started || state.unregister_started
            || !attempt.scope.is_open() || attempt.scope.session_revoked() {
            return Err("app_account_im_send_scope_invalid");
        }
        let input = message.reserve_input(&attempt.scope, recipient, nonce)?;
        Ok((attempt.owner.clone(), attempt.scope.clone(), input))
    }
    #[cfg(test)]
    pub(in super::super) fn im_send_resources_for_test(&self)->Option<(usize,usize)>{lock(&self.state).slot.as_ref().map(|a|a.scope.native_rpc_resources())}
    #[cfg(test)]
    pub(in super::super) async fn im_send_actor_barrier_for_test(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Result<(),&'static str>{let attempt=self.original(owner).map_err(|_|"fixture original send owner")?;let commands=attempt.commands(owner,scope).map_err(|_|"fixture original send commands")?;let binding=commands.original_im_binding(owner,scope).await.map_err(|_|"fixture original send actor barrier")?;if !binding.matches(owner,scope){return Err("fixture original send binding");}Ok(())}
    async fn im_send_recipient_reply(&self, action: Arc<UserEntryAction>) -> Result<OwnedUserEntryReply, &'static str> {
        let owner = action.owner(); let scope = action.parent();
        let attempt = self.original(owner).map_err(|_| "app_account_im_send_scope_invalid")?;
        let common = self.profile.common.as_ref().ok_or("app_account_im_send_recipient_not_installed")?;
        let target = lock(&attempt.user_entry).clone().map_err(|_| "app_account_im_send_recipient_not_installed")?
            .ok_or("app_account_im_send_recipient_not_installed")?;
        let commands = attempt.commands(owner, &scope).map_err(|_| "app_account_im_send_scope_invalid")?;
        let operation = Box::pin(async {
            let lease = attempt.user_entries.acquire(owner, &scope, action.deadline()).map_err(|_| "app_account_im_send_capacity")?;
            let auth = commands.original_user_entry_auth(owner, &scope).await.map_err(|_| "app_account_im_send_scope_invalid")?;
            let access = borrowed(&common.access);
            resolve_original(action.clone(), auth, target, &lease, &common.router, &access,
                super::super::router_runtime::RouterTimeouts { connect: common.connect_timeout, handshake: self.profile.handshake_timeout,
                    partial: self.profile.partial_timeout, io: self.profile.io_timeout, startup: common.startup_timeout },
                common.compress_above).await.map_err(|_| "app_account_im_send_recipient_unconfirmed")
        });
        let gate = action.gate();
        tokio::select! { biased;
            _ = scope.session_ended() => Err("app_account_im_send_scope_invalid"),
            _ = gate.retired() => Err("app_account_im_send_scope_invalid"),
            result = tokio::time::timeout_at(action.deadline().into(), operation) =>
                result.map_err(|_| "app_account_im_send_recipient_unconfirmed")?,
        }
    }
    pub(in super::super) async fn send_im_message(&self, database: Arc<crate::platform::db::Database>, run: Arc<RunToken>,
        parent_gate: Arc<super::super::im_inbox::ImReceiptGate>, owner: AttemptOwner, scope: RpcConnectionScope,
        recipient: Zeroizing<Vec<u8>>, message: ImSendMessage, nonce: Zeroizing<String>, deadline: Instant, input: ImSendInput,
    ) -> Result<ImSendView, ImSendFailure> {
        if !self.im_send_enabled() || !parent_gate.live() { return Err(ImSendFailure::before("app_account_im_send_not_installed")); }
        let policy = self.profile.im_send.as_ref().ok_or_else(|| ImSendFailure::before("app_account_im_send_not_installed"))?;
        let attempt = self.original(&owner).map_err(|_| ImSendFailure::before("app_account_im_send_scope_invalid"))?;
        {
            let state = lock(&attempt.state);
            if !state.configured || state.retired || state.logout_started || state.unregister_started || !attempt.scope.matches(&scope) {
                return Err(ImSendFailure::before("app_account_im_send_scope_invalid"));
            }
        }
        let commands = attempt.commands(&owner, &scope).map_err(|_| ImSendFailure::before("app_account_im_send_scope_invalid"))?;
        let preparation = Box::pin(async {
            let binding = commands.original_im_binding(&owner, &scope).await.map_err(|_| "app_account_im_send_scope_invalid")?;
            let identity = commands.original_im_send_identity(&owner, &scope).await.map_err(|_| "app_account_im_send_scope_invalid")?;
            ImSendAction::original_with_input(database, run, parent_gate.clone(), binding, identity, &recipient, message, &nonce, deadline, policy, input)
        });
        let prepared = tokio::select! { biased;
            _ = parent_gate.retired() => return Err(ImSendFailure::before("app_account_im_send_scope_invalid")),
            _ = scope.session_ended() => return Err(ImSendFailure::before("app_account_im_send_scope_invalid")),
            result = tokio::time::timeout_at(deadline.into(), preparation) =>
                result.map_err(|_| ImSendFailure::before("app_account_im_send_timeout"))?.map_err(ImSendFailure::before)?,
        };
        let action = match prepared { PreparedImSend::Replay(view) => return Ok(view), PreparedImSend::Fresh(action) => action };
        let _wait = ImSendWait::original(action.clone());
        let operation = Box::pin(async {
            if let Some(lookup) = action.recipient_action() {
                let reply = self.im_send_recipient_reply(lookup).await?; action.confirm_recipient(reply)?;
            }
            let permit = action.permit(policy)?;
            commands.send_im(&owner, &scope, permit).await.map_err(|_| "app_account_im_send_uncertain")
        });
        let gate = action.gate();
        tokio::select! { biased;
            _ = parent_gate.retired() => Err(ImSendFailure::after("app_account_im_send_uncertain")),
            _ = scope.session_ended() => Err(ImSendFailure::after("app_account_im_send_uncertain")),
            _ = gate.retired() => Err(ImSendFailure::after("app_account_im_send_uncertain")),
            result = tokio::time::timeout_at(deadline.into(), operation) =>
                result.map_err(|_| ImSendFailure::after("app_account_im_send_timeout"))?.map_err(ImSendFailure::after),
        }
    }
}
