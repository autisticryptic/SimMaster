//! Current original account lookup. No controller lock is held across IO and
//! no new SDK login/session is submitted. Stop waits for this owning guard.
use super::*;
use super::super::{user_entry_action::{UserEntryAction,UserEntryObservation},user_entry_rpc::{resolve_original,UserEntryError}};
impl NativeHostingBackend{
    pub(in super::super) fn user_entry_timeout(&self)->Duration{self.profile.rpc_timeout.min(Duration::from_secs(30))}
    pub(in super::super) fn user_entry_supported(&self,owner:&AttemptOwner)->bool{
        let Ok(attempt)=self.original(owner)else{return false;};let state=lock(&attempt.state);
        !state.retired&&!state.logout_started&&!state.unregister_started&&self.profile.common.is_some()&&attempt.user_entries.live()&&(lock(&attempt.user_entry).as_ref().is_ok_and(|target|target.is_some())||attempt.user_entries.client_target().as_ref().is_ok_and(|target|target.is_some()))
    }
    pub(in super::super) fn user_entry_uid_local_supported(&self,owner:&AttemptOwner)->bool{
        let Ok(attempt)=self.original(owner)else{return false;};let state=lock(&attempt.state);
        !state.retired&&!state.logout_started&&!state.unregister_started&&self.profile.common.is_some()&&lock(&attempt.user_entry_uid_local).as_ref().is_ok_and(|reference|reference.is_some())
    }
    /// Capture account intent before controller acquisition or recipient copy.
    /// The existing scope/budget is retained; no SDK, auxiliary ID or IO is made.
    pub(in super::super) fn reserve_user_entry_action(&self,recipient:&[u8],gate:Arc<super::super::im_inbox::ImReceiptGate>,deadline:Instant)->Result<Arc<UserEntryAction>,&'static str>{
        super::super::user_entry_data::RecipientUri::parse(recipient).map_err(|_|"app_account_user_entry_recipient_invalid")?;
        let attempt=lock(&self.state).slot.clone().ok_or("app_account_user_entry_scope_invalid")?;
        let state=lock(&attempt.state);
        if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked()||!gate.live(){return Err("app_account_user_entry_scope_invalid");}
        if Instant::now()>=deadline{return Err("app_account_user_entry_timeout");}
        let input=attempt.scope.reserve_native_rpc(recipient.len()+64).map_err(|_|"app_account_user_entry_capacity")?;
        UserEntryAction::reserved(attempt.owner.clone(),attempt.scope.clone(),gate,deadline,recipient,input)
    }
    pub(in super::super) async fn resolve_user_entry(&self,action:Arc<UserEntryAction>)->Result<UserEntryObservation,&'static str>{
        let owner=action.owner();let parent=action.parent();
        if !action.live(){return Err("app_account_user_entry_scope_invalid");}
        let attempt=self.original(owner).map_err(|_|"app_account_user_entry_scope_invalid")?;
        {let state=lock(&attempt.state);if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.matches(&parent){return Err("app_account_user_entry_scope_invalid");}}
        let common=self.profile.common.as_ref().ok_or("app_account_user_entry_not_installed")?;
        let recipient=super::super::user_entry_data::RecipientUri::parse(action.recipient()).map_err(|_|"app_account_user_entry_recipient_invalid")?;
        let local=recipient.kind()==super::super::user_entry_data::AidKind::Uid;
        let local_reference=if local{lock(&attempt.user_entry_uid_local).clone().map_err(|_|"app_account_user_entry_reference_unsupported")?.ok_or("app_account_user_entry_not_installed").map(Some)?}else{None};
        let client=if local{None}else{attempt.user_entries.client_target().ok().flatten()};
        let target=if local||client.is_some(){None}else{lock(&attempt.user_entry).clone().map_err(|_|"app_account_user_entry_reference_unsupported")?.ok_or("app_account_user_entry_not_installed").map(Some)?};
        let commands=attempt.commands(owner,&parent).map_err(|_|"app_account_user_entry_scope_invalid")?;
        let operation=Box::pin(async{
            if let Some(target)=client{let reply=self.resolve_client_user_entry(action.clone(),target).await.map_err(user_entry_error)?;return reply.observe(&action).map_err(user_entry_error);}
            if let Some(reference)=local_reference{
                // Original569028's native UID branch does not allocate an
                // auxiliary scope/ID, dial, or invoke aidToUid. Actor capture
                // still requires this current original native SDK auth.
                let auth=commands.original_user_entry_auth(owner,&parent).await.map_err(|_|"app_account_user_entry_scope_invalid")?;
                {let state=lock(&attempt.state);if state.retired||state.logout_started||state.unregister_started||!state.configured{return Err("app_account_user_entry_scope_invalid");}}
                let result=super::super::user_entry_local::LocalUidResult::original(action.clone(),reference,auth,&common.router)?;
                return result.observe(&action,&common.router);
            }
            let target=target.ok_or("app_account_user_entry_not_installed")?;
            let lease=attempt.user_entries.acquire(owner,&parent,action.deadline()).map_err(|_|"app_account_user_entry_capacity")?;
            let auth=commands.original_user_entry_auth(owner,&parent).await.map_err(|_|"app_account_user_entry_scope_invalid")?;
            let access=borrowed(&common.access);
            let reply=resolve_original(action.clone(),auth,target,&lease,&common.router,&access,super::super::router_runtime::RouterTimeouts{
                connect:common.connect_timeout,handshake:self.profile.handshake_timeout,partial:self.profile.partial_timeout,io:self.profile.io_timeout,startup:common.startup_timeout},common.compress_above)
                .await.map_err(user_entry_error)?;
            reply.observe(&action).map_err(user_entry_error)
        });
        let gate=action.gate();tokio::select!{biased;
            _=parent.session_ended()=>Err("app_account_user_entry_scope_invalid"),
            _=gate.retired()=>Err("app_account_user_entry_scope_invalid"),
            result=tokio::time::timeout_at(action.deadline().into(),operation)=>result.map_err(|_|"app_account_user_entry_timeout")?,
        }
    }
}
fn user_entry_error(e:UserEntryError)->&'static str{match e{
    UserEntryError::Scope=>"app_account_user_entry_scope_invalid",UserEntryError::Configuration=>"app_account_user_entry_reference_unsupported",
    UserEntryError::Transport=>"app_account_user_entry_transport_failed",UserEntryError::Timeout=>"app_account_user_entry_timeout",
    UserEntryError::Protocol=>"app_account_user_entry_reply_invalid",UserEntryError::Rejected=>"app_account_user_entry_rejected",
    UserEntryError::Ambiguous=>"app_account_user_entry_ambiguous",UserEntryError::Capacity=>"app_account_user_entry_capacity",UserEntryError::Realm=>"app_account_user_entry_realm_mismatch",
}}
