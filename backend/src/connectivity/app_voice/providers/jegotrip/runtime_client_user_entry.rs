//! Purpose-private current Client UserEntry READ on the original ARC actor.
//! Endpoint-less factory provenance cannot mint SDK, WRITE or Unknown retry.
use super::*;
use super::super::{user_entry_action::UserEntryAction,user_entry_client::{ClientUserEntryQuery,ClientReadWait},user_entry_client_target::NewClientTarget,user_entry_rpc::{OwnedUserEntryReply,UserEntryError}};
impl NativeHostingBackend{
    pub(in super::super) async fn resolve_client_user_entry(&self,action:Arc<UserEntryAction>,target:Arc<NewClientTarget>)->Result<OwnedUserEntryReply,UserEntryError>{
        let owner=action.owner();let parent=action.parent();
        let attempt=self.original(owner).map_err(|_|UserEntryError::Scope)?;
        {let state=lock(&attempt.state);if !state.configured||state.retired||state.logout_started||state.unregister_started||!action.live()||!attempt.scope.matches(&parent)||!attempt.user_entries.live(){return Err(UserEntryError::Scope);}}
        let common=self.profile.common.as_ref().ok_or(UserEntryError::Configuration)?;
        let query=ClientUserEntryQuery::original(action.clone(),target,&common.router)?;
        let _interest=ClientReadWait::original(query.interest());
        let commands=attempt.commands(owner,&parent).map_err(|_|UserEntryError::Scope)?;
        let routing=borrowed(&self.profile.poll_routing);
        let operation=Box::pin(commands.client_user_entry(query,&routing));
        let gate=action.gate();
        let result=tokio::select!{biased;
            _=parent.session_ended()=>Err(UserEntryError::Scope),
            _=gate.retired()=>Err(UserEntryError::Scope),
            _=attempt.user_entries.retired()=>Err(UserEntryError::Scope),
            result=tokio::time::timeout_at(action.deadline().into(),operation)=>result.map_err(|_|UserEntryError::Timeout)?,
        }?;
        if Instant::now()>=action.deadline(){return Err(UserEntryError::Timeout);}
        {let state=lock(&attempt.state);if !state.configured||state.retired||state.logout_started||state.unregister_started||!action.live()||!attempt.user_entries.live(){return Err(UserEntryError::Scope);}}
        if !self.original(owner).is_ok_and(|current|Arc::ptr_eq(&current,&attempt))||!result.matches_action(&action){return Err(UserEntryError::Scope);}
        Ok(result)
    }
}
