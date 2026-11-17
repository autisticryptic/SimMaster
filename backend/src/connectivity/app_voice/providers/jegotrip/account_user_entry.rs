//! Explicit authenticated current-account recipient lookup. Query consent is
//! not user mark-read/business WRITE consent and never repairs uncertain WRITE.
use super::*;
use super::super::user_entry_action::{UserEntryAction,UserEntryObservation};
impl AccountManager{
    pub async fn lookup_user_entry(&self,line:&str,revision:u64,recipient:&str,consent:bool)->Result<UserEntryObservation,&'static str>{
        if !consent{return Err("app_account_user_entry_consent_required");}self.check_revision(line,revision)?;
        let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_user_entry_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_user_entry_scope_invalid");}
        run.configuration_current()?;
        let controller=run.controller.lock().await;
        if controller.phase()!=Phase::Hosted{return Err("app_account_user_entry_scope_invalid");}
        let owner=controller.resource_owner().ok_or("app_account_user_entry_scope_invalid")?;
        let scope=run.backend.original_scope(&owner).ok_or("app_account_user_entry_scope_invalid")?;
        let deadline=Instant::now().checked_add(run.backend.user_entry_timeout()).ok_or("app_account_user_entry_timeout")?;
        let action=UserEntryAction::original(owner,scope,run.im_gate.clone(),deadline,recipient.as_bytes())?;
        drop(controller);
        let result=run.backend.resolve_user_entry(action).await?;
        run.configuration_current()?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_user_entry_scope_invalid");}
        Ok(result)
    }
}
