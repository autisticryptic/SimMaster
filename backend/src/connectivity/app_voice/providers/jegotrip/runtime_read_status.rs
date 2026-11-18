//! Explicit Mtc_ImSetReadStatus on the original owned Group connection.
//! Manual policy does not borrow automatic receipt opt-in or user UDMR rights.
use super::*;
use super::super::im_inbox::{ReadStatusAction,ReadStatusWait,ReadStatusView,ReadStatusPrepared};
impl NativeHostingBackend{
    pub(in super::super) fn read_status_timeout(&self)->Duration{self.profile.rpc_timeout}
    pub(in super::super) async fn set_read_status(&self,action:Arc<ReadStatusAction>)->Result<ReadStatusView,&'static str>{
        let _wait=ReadStatusWait::original(action.clone());if !self.im_read_status_enabled()||!action.live(){return Err("app_account_im_read_status_not_installed");}
        let attempt=self.original(action.owner()).map_err(|_|"app_account_im_read_status_scope_invalid")?;
        {let state=lock(&attempt.state);if state.retired||!state.configured||state.logout_started||state.unregister_started||!attempt.scope.matches(&action.scope()){return Err("app_account_im_read_status_scope_invalid");}}
        let policy=self.profile.im_read_status.ok_or("app_account_im_read_status_not_installed")?;let commands=attempt.commands(action.owner(),&action.scope()).map_err(|_|"app_account_im_read_status_scope_invalid")?;
        let operation=Box::pin(async{
            let binding=commands.original_im_binding(action.owner(),&action.scope()).await.map_err(|_|if action.recipient_action().is_some(){"app_account_im_read_status_recipient_target_stale"}else{"app_account_im_read_status_scope_invalid"})?;
            if !Arc::ptr_eq(&binding,&action.binding()){return Err("app_account_im_read_status_scope_invalid");}
            if let Some(replay)=action.recipient_preflight()?{return Ok(replay);}
            // The two public paths share one immutable in-flight nonce intent.
            // A default action cannot race a pending alias and rebind its nonce.
            let _resolution=action.claim_recipient_resolution()?;
            if let Some(recipient)=action.recipient_action(){let matched=self.read_status_recipient_reply(recipient).await.map_err(|e|if e=="app_account_im_read_status_scope_invalid"{"app_account_im_read_status_recipient_target_stale"}else{e})?;action.confirm_recipient(matched)?;}
            let prepared=super::super::im_inbox::begin_read_status(action.database(),action.clone(),&policy).map_err(|e|if action.recipient_action().is_some()&&matches!(e,"app_account_im_scope_invalid"|"app_account_im_read_status_scope_invalid"){"app_account_im_read_status_recipient_target_stale"}else{e})?;
            let permit=match prepared{ReadStatusPrepared::Replay(view)=>return Ok(view),ReadStatusPrepared::Fresh(p)=>p};
            let reply=commands.write_received(action.owner(),&action.scope(),permit.clone()).await.map_err(|_|"app_account_im_read_status_uncertain")?;
            if !reply.belongs_to(&permit){return Err("app_account_im_read_status_uncertain");}permit.manual_result_for(&action)
        });
        // Retired READ/lookup before admission is known not to have emitted
        // this WRITE; after admission never relabel the same close as no-IO.
        let closed_code=||if action.write_admission_observed(){"app_account_im_read_status_uncertain"}else if action.recipient_action().is_some(){"app_account_im_read_status_recipient_target_stale"}else{"app_account_im_read_status_scope_invalid"};
        let scope=action.scope();let gate=action.gate();let parent_gate=action.parent_gate();tokio::select!{biased;
            _=parent_gate.retired()=>Err(closed_code()),
            _=scope.session_ended()=>Err(closed_code()),
            _=gate.retired()=>Err(closed_code()),
            result=tokio::time::timeout_at(action.deadline().into(),operation)=>result.map_err(|_|if action.recipient_action().is_some()&&!action.write_admission_observed(){"app_account_im_read_status_recipient_unconfirmed"}else{"app_account_im_read_status_timeout"})?,
        }
    }
}
