//! Current original account lookup. No controller lock is held across IO and
//! no new SDK login/session is submitted. Stop waits for this owning guard.
use super::*;
use super::super::{user_entry_action::{UserEntryAction,UserEntryObservation},user_entry_rpc::{resolve_original,UserEntryError}};
impl NativeHostingBackend{
    pub(in super::super) fn user_entry_timeout(&self)->Duration{self.profile.rpc_timeout.min(Duration::from_secs(30))}
    pub(in super::super) fn user_entry_supported(&self,owner:&AttemptOwner)->bool{
        let Ok(attempt)=self.original(owner)else{return false;};let state=lock(&attempt.state);
        !state.retired&&!state.logout_started&&!state.unregister_started&&self.profile.common.is_some()&&attempt.user_entries.live()&&lock(&attempt.user_entry).as_ref().is_ok_and(|target|target.is_some())
    }
    pub(in super::super) async fn resolve_user_entry(&self,action:Arc<UserEntryAction>)->Result<UserEntryObservation,&'static str>{
        let owner=action.owner();let parent=action.parent();
        if !action.live(){return Err("app_account_user_entry_scope_invalid");}
        let attempt=self.original(owner).map_err(|_|"app_account_user_entry_scope_invalid")?;
        {let state=lock(&attempt.state);if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.matches(&parent){return Err("app_account_user_entry_scope_invalid");}}
        let common=self.profile.common.as_ref().ok_or("app_account_user_entry_not_installed")?;
        let target=lock(&attempt.user_entry).clone().map_err(|_|"app_account_user_entry_reference_unsupported")?.ok_or("app_account_user_entry_not_installed")?;
        let commands=attempt.commands(owner,&parent).map_err(|_|"app_account_user_entry_scope_invalid")?;
        let operation=Box::pin(async{
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
