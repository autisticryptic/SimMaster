//! Explicit current UID relationship check, not a roster or WRITE reconciliation.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::buddy_relation_check;
#[derive(Serialize)]
pub struct BuddyRelationCheckView{pub accepted:bool,pub relation_type:u32}
impl std::fmt::Debug for BuddyRelationCheckView{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("BuddyRelationCheckView(<redacted>)")}}
pub struct PreparedBuddyRelationCheck{run:Arc<ManagedRun>,owner:AttemptOwner,scope:RpcConnectionScope,query:Arc<ImQuery>,deadline:Instant,_interest:buddy_relation_check::CheckWait}
impl std::fmt::Debug for PreparedBuddyRelationCheck{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("PreparedBuddyRelationCheck(<redacted>)")}}
fn configuration(run:&ManagedRun,scope:&RpcConnectionScope)->Result<(),&'static str>{
    run.configuration_current()?;live_run(run)?;
    let original=run.relation_configuration.as_ref().ok_or("app_account_buddy_check_not_installed")?;
    run.database.with_conn(|c|original.require_current(c,run.run_token.line_id(),run.run_token.revision(),scope))
}
impl AccountManager{
    pub fn prepare_buddy_relation_check(&self,line:&str,revision:u64,uid:&str,consent:bool)->Result<PreparedBuddyRelationCheck,&'static str>{
        if !consent{return Err("app_account_buddy_check_consent_required");}buddy_relation_check::validate_uid(uid.as_bytes()).map_err(|_|"app_account_buddy_check_request_invalid")?;
        self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_directory_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.buddy_relation_check_enabled(){return Err("app_account_buddy_check_not_installed");}
        run.configuration_current()?;live_run(&run)?;
        let deadline=Instant::now().checked_add(run.backend.directory_query_timeout()).ok_or("app_account_directory_timeout")?;
        let(owner,scope,query)=run.backend.reserve_buddy_relation_check(uid.as_bytes(),deadline).map_err(error)?;configuration(&run,&scope)?;
        let context=buddy_relation_check::CheckContext::capture(run.database.clone(),run.run_token.clone(),owner.clone(),scope.clone(),run.relation_configuration.as_ref().cloned().ok_or("app_account_buddy_check_not_installed")?,run.im_gate.clone(),deadline).map_err(|_|"app_account_directory_scope_invalid")?;
        let query=query.bind_buddy_context(context.clone()).map_err(|_|"app_account_directory_scope_invalid")?;let interest=buddy_relation_check::CheckWait(context,query.clone());
        Ok(PreparedBuddyRelationCheck{run,owner,scope,query,deadline,_interest:interest})
    }
    pub async fn finish_buddy_relation_check(&self,prepared:PreparedBuddyRelationCheck)->Result<BuddyRelationCheckView,&'static str>{
        let PreparedBuddyRelationCheck{run,owner,scope,query,deadline,_interest}=prepared;
        let slot=self.slot(run.run_token.line_id())?;if !lock(&slot.run).as_ref().is_some_and(|r|Arc::ptr_eq(r,&run)){return Err("app_account_directory_scope_invalid");}configuration(&run,&scope)?;
        if Instant::now()>=deadline{return Err("app_account_directory_timeout");}
        let controller=tokio::select!{biased;_=run.im_gate.retired()=>return Err("app_account_directory_scope_invalid"),_=scope.session_ended()=>return Err("app_account_directory_scope_invalid"),r=tokio::time::timeout_at(deadline.into(),run.controller.lock())=>r.map_err(|_|"app_account_directory_timeout")?};
        configuration(&run,&scope)?;
        if controller.phase()!=Phase::Hosted||controller.resource_owner().as_ref()!=Some(&owner)||!run.backend.original_scope(&owner).is_some_and(|s|s.matches(&scope))||!run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_directory_scope_invalid");}drop(controller);
        if Instant::now()>=deadline{return Err("app_account_directory_timeout");}
        let operation=Box::pin(run.backend.check_buddy_relation(&owner,&scope,query.clone(),run.im_gate.clone()));
        let reply=tokio::select!{biased;_=run.im_gate.retired()=>return Err("app_account_directory_scope_invalid"),_=scope.session_ended()=>return Err("app_account_directory_scope_invalid"),r=tokio::time::timeout_at(deadline.into(),operation)=>r.map_err(|_|"app_account_directory_timeout")?.map_err(error)?};
        configuration(&run,&scope)?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!reply.matches(&owner,&scope)||!Arc::ptr_eq(reply.request().query_arc(),&query)||!Arc::ptr_eq(&reply.request().gate(),&run.im_gate){return Err("app_account_directory_scope_invalid");}
        let value=reply.result().map_err(|_|"app_account_directory_reply_invalid")?;
        let ImReplyData::BuddyCheck(value)=value else{return Err("app_account_directory_reply_invalid");};
        Ok(BuddyRelationCheckView{accepted:value.accepted,relation_type:value.relation_type})
    }
}
