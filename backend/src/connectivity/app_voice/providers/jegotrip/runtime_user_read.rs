//! One explicitly consented user mark-read operation on the original SDK.
//! Original binding -> fixed local message -> purpose refresh -> unknown marker
//! -> original write/result. No cached credentials or automatic replay path.
use super::*;
use super::super::im_inbox::{UserReadAction,UserReadWait,UserReadView,BoundUserRead};
impl NativeHostingBackend{
    pub(in super::super) fn user_read_enabled(&self)->bool{self.profile.im_user_read.is_some()&&self.profile.common.is_some()&&self.im_messages_enabled()}
    pub(in super::super) fn user_read_timeout(&self)->Duration{self.profile.rpc_timeout}
    pub(in super::super) async fn mark_user_read(&self,action:Arc<UserReadAction>)->Result<UserReadView,&'static str>{
        let _wait=UserReadWait::original(action.clone());if !self.user_read_enabled()||!action.live(){return Err("app_account_im_user_read_not_installed");}
        let attempt=self.original(action.owner()).map_err(|_|"app_account_im_scope_invalid")?;
        {let state=lock(&attempt.state);if !state.configured||state.retired||state.logout_started||state.unregister_started{return Err("app_account_im_scope_invalid");}}
        let commands=attempt.commands(action.owner(),&action.scope()).map_err(|_|"app_account_im_scope_invalid")?;
        let operation=Box::pin(async{
            let binding=commands.original_im_binding(action.owner(),&action.scope()).await.map_err(|_|"app_account_im_scope_invalid")?;
            let mut context=match action.bind_current(binding)?{BoundUserRead::Replay(view)=>return Ok(view),BoundUserRead::Context(c)=>c};
            if let Some(recipient)=context.recipient_action(){
                let matched=self.user_read_recipient_reply(recipient).await?;
                context=context.confirm_recipient(matched)?;
            }
            // This read is explicitly scoped to this user action and its fixed
            // current native label; it does not install/readsync or move a cursor.
            let r=commands.read_user_target(&action,context.query()).await.map_err(|_|"app_account_im_user_read_target_unconfirmed")?;
            let p=context.admit(&r)?;
            commands.user_read_write(action.owner(),&action.scope(),p).await.map_err(|_|"app_account_im_user_read_uncertain")
        });
        let parent_gate=action.parent_gate();
        tokio::select!{biased;
            _=parent_gate.retired()=>Err("app_account_im_scope_invalid"),
            result=tokio::time::timeout_at(action.deadline().into(),operation)=>result.map_err(|_|"app_account_im_user_read_operation_timeout")?,
        }
    }
}
