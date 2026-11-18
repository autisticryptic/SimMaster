//! Explicit non-UID recipient intent for a fixed locally committed IM body.
//! No raw recipient/lookup boolean is a mark-read grant or uncertain-write ACK.
use super::*;
impl AccountManager{
    pub async fn mark_im_user_read_via_recipient(&self,line:&str,revision:u64,message_id:i64,nonce:&str,recipient:&str,consent:bool)->Result<super::super::im_inbox::UserReadView,&'static str>{
        if !consent{return Err("app_account_im_user_read_consent_required");}self.check_revision(line,revision)?;
        let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_im_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.user_read_enabled(){return Err("app_account_im_user_read_not_installed");}
        run.configuration_current()?;let controller=run.controller.lock().await;
        if controller.phase()!=Phase::Hosted{return Err("app_account_im_scope_invalid");}
        let owner=controller.resource_owner().ok_or("app_account_im_scope_invalid")?;
        let scope=run.backend.original_scope(&owner).ok_or("app_account_im_scope_invalid")?;
        let deadline=Instant::now().checked_add(run.backend.user_read_timeout()).ok_or("app_account_im_user_read_operation_timeout")?;
        let action=super::super::im_inbox::UserReadAction::original_with_recipient(run.database.clone(),run.run_token.clone(),run.im_gate.clone(),owner,scope,message_id,nonce,deadline,recipient.as_bytes())?;
        #[cfg(test)]{*lock(&run.user_read_action)=Arc::downgrade(&action);}
        drop(controller);let result=run.backend.mark_user_read(action).await?;
        run.configuration_current()?;if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_im_scope_invalid");}Ok(result)
    }
}
