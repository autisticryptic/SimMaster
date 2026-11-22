//! Original nonUID manual setReadStatus callback lookup, a READ not a WRITE
//! capability. Its private matched UID must match the fixed current P2P span.
use super::*;
use super::super::{user_entry_action::UserEntryAction,user_entry_rpc::{resolve_original,OwnedUserEntryReply},user_entry_data::{RecipientUri,AidKind}};
impl NativeHostingBackend{
    pub(in super::super) async fn read_status_recipient_reply(&self,action:Arc<UserEntryAction>)->Result<OwnedUserEntryReply,&'static str>{
        if RecipientUri::parse(action.recipient()).map_err(|_|"app_account_im_read_status_request_invalid")?.kind()==AidKind::Uid{return Err("app_account_im_read_status_request_invalid");}
        let who=action.owner();let parent=action.parent();let attempt=self.original(who).map_err(|_|"app_account_im_read_status_scope_invalid")?;
        {let state=lock(&attempt.state);if !state.configured||state.retired||state.logout_started||state.unregister_started||!action.live()||!attempt.scope.matches(&parent){return Err("app_account_im_read_status_scope_invalid");}}
        let client=attempt.user_entries.client_target().ok().flatten();
        if let Some(target)=client{return self.resolve_client_user_entry(action.clone(),target).await.map_err(|_|"app_account_im_read_status_recipient_unconfirmed");}
        let common=self.profile.common.as_ref().ok_or("app_account_im_read_status_recipient_not_installed")?;let target=lock(&attempt.user_entry).clone().map_err(|_|"app_account_im_read_status_recipient_not_installed")?.ok_or("app_account_im_read_status_recipient_not_installed")?;let commands=attempt.commands(who,&parent).map_err(|_|"app_account_im_read_status_scope_invalid")?;
        let operation=Box::pin(async{let lease=attempt.user_entries.acquire(who,&parent,action.deadline()).map_err(|_|"app_account_im_read_status_recipient_capacity")?;let auth=commands.original_user_entry_auth(who,&parent).await.map_err(|_|"app_account_im_read_status_scope_invalid")?;let access=borrowed(&common.access);
            resolve_original(action.clone(),auth,target,&lease,&common.router,&access,super::super::router_runtime::RouterTimeouts{connect:common.connect_timeout,handshake:self.profile.handshake_timeout,partial:self.profile.partial_timeout,io:self.profile.io_timeout,startup:common.startup_timeout},common.compress_above).await.map_err(|_|"app_account_im_read_status_recipient_unconfirmed")
        });let gate=action.gate();tokio::select!{biased;_=parent.session_ended()=>Err("app_account_im_read_status_scope_invalid"),_=gate.retired()=>Err("app_account_im_read_status_scope_invalid"),value=tokio::time::timeout_at(action.deadline().into(),operation)=>value.map_err(|_|"app_account_im_read_status_recipient_unconfirmed")?,}
    }
}
