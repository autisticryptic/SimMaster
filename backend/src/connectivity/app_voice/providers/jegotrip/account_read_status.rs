//! Current account manual Group/P2P read-position API; no raw-index grant.
//! Target IDs select live opaque original consumer spans, never stored rows.
use super::*;
use super::super::im_inbox::{ReadStatusAction,ReadStatusTargetView,ReadStatusView};
impl AccountManager{
    pub async fn im_read_status_targets(&self,line:&str,revision:u64)->Result<Vec<ReadStatusTargetView>,&'static str>{
        self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_im_read_status_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.backend.im_read_status_enabled(){return Err("app_account_im_read_status_not_installed");}run.configuration_current()?;
        let controller=run.controller.lock().await;if controller.phase()!=Phase::Hosted{return Err("app_account_im_read_status_scope_invalid");}let targets=run.read_status_targets.as_ref().ok_or("app_account_im_read_status_not_installed")?;Ok(targets.list())
    }
    pub fn im_read_status_history(&self,line:&str,limit:usize)->Result<Vec<ReadStatusView>,&'static str>{self.database.with_conn(|conn|super::super::im_inbox::read_status_history(conn,line,limit))}
    pub async fn set_im_read_status(&self,line:&str,revision:u64,target_id:i64,expected_index:&str,nonce:&str,consent:bool)->Result<ReadStatusView,&'static str>{
        if !consent{return Err("app_account_im_read_status_consent_required");}self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_im_read_status_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.im_read_status_enabled(){return Err("app_account_im_read_status_not_installed");}run.configuration_current()?;
        let controller=run.controller.lock().await;if controller.phase()!=Phase::Hosted{return Err("app_account_im_read_status_scope_invalid");}
        let owner=controller.resource_owner().ok_or("app_account_im_read_status_scope_invalid")?;let scope=run.backend.original_scope(&owner).ok_or("app_account_im_read_status_scope_invalid")?;
        let targets=run.read_status_targets.as_ref().ok_or("app_account_im_read_status_not_installed")?;let target=targets.get(target_id,expected_index)?;
        let deadline=Instant::now().checked_add(run.backend.read_status_timeout()).ok_or("app_account_im_read_status_timeout")?;
        let action=ReadStatusAction::original(run.database.clone(),run.run_token.clone(),run.im_gate.clone(),owner,scope,target,nonce,deadline)?;
        drop(controller);let result=run.backend.set_read_status(action).await?;
        run.configuration_current()?;if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_im_read_status_scope_invalid");}Ok(result)
    }
}
