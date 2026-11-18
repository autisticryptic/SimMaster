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
            let binding=commands.original_im_binding(action.owner(),&action.scope()).await.map_err(|_|"app_account_im_read_status_scope_invalid")?;
            if !Arc::ptr_eq(&binding,&action.binding()){return Err("app_account_im_read_status_scope_invalid");}
            let prepared=super::super::im_inbox::begin_read_status(action.database(),action.clone(),&policy)?;
            let permit=match prepared{ReadStatusPrepared::Replay(view)=>return Ok(view),ReadStatusPrepared::Fresh(p)=>p};
            let reply=commands.write_received(action.owner(),&action.scope(),permit.clone()).await.map_err(|_|"app_account_im_read_status_uncertain")?;
            if !reply.belongs_to(&permit){return Err("app_account_im_read_status_uncertain");}permit.manual_result_for(&action)
        });
        let scope=action.scope();let gate=action.gate();let parent_gate=action.parent_gate();tokio::select!{biased;
            _=parent_gate.retired()=>Err("app_account_im_read_status_scope_invalid"),
            _=scope.session_ended()=>Err("app_account_im_read_status_scope_invalid"),
            _=gate.retired()=>Err("app_account_im_read_status_scope_invalid"),
            result=tokio::time::timeout_at(action.deadline().into(),operation)=>result.map_err(|_|"app_account_im_read_status_timeout")?,
        }
    }
}
