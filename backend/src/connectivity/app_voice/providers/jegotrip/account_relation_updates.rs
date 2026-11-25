//! Explicit relation-changing action, distinct from directory READ or IM-send.
//! Synchronous input reservation/fixed deadline precede this original run's
//! first await; UID/selector/token and an active RPC permit never come from HTTP.
use super::*;
use super::super::{group_directory_data::Relation,group_relation_updates::Removal,
    relation_journal::{RelationMessage,RelationWriteView,RelationConfiguration,RelationIntent,IntentAdmission,RelationIntentWait,RelationIntentView}};
/// The existing stored config and relation fingerprint share one SQLite read
/// snapshot before factory construction. A later same-revision blob change
/// cannot replace the policy/credentials of this original runtime.
pub(super) fn load_original_account(c:&rusqlite::Connection,line:&str)->Result<Option<(StoredAccount,Option<Arc<RelationConfiguration>>)>,&'static str>{
    let tx=rusqlite::Transaction::new_unchecked(c,rusqlite::TransactionBehavior::Deferred).map_err(|_|"app_account_relation_storage_failed")?;
    let account=account_store::load(&tx,line)?;
    let configuration=match &account{Some(stored)if (stored.config.profile.relation_updates.is_some()||stored.config.profile.relation_events.is_some()||stored.config.profile.org_relation_sync.is_some()||stored.config.profile.relation_roster.is_some())=>Some(Arc::new(RelationConfiguration::capture(&tx,line,stored.metadata.revision,None)?)),_=>None};
    tx.commit().map_err(|_|"app_account_relation_storage_failed")?;Ok(account.map(|a|(a,configuration)))
}
pub(crate) struct RelationHttpScratch(Arc<super::super::native_rpc_budget::NativeRpcReservation>);
impl std::fmt::Debug for RelationHttpScratch{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("RelationHttpScratch(<redacted>)")}}
impl RelationHttpScratch{pub(crate) fn matches(&self,prepared:&PreparedRelationUpdate)->bool{self.0.belongs_to(&prepared.scope)}}
pub(crate) struct PreparedRelationUpdate{run:Arc<ManagedRun>,owner:AttemptOwner,scope:RpcConnectionScope,message:Arc<RelationMessage>,deadline:Instant,configuration:Arc<RelationConfiguration>,admission:IntentAdmission,_interest:Option<RelationIntentWait>}
impl std::fmt::Debug for PreparedRelationUpdate{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("PreparedRelationUpdate(<redacted>)")}}
fn live_run(run:&ManagedRun)->Result<(),&'static str>{
    run.database.with_conn(|c|{if !super::super::account_runs::runtime_matches(c,&run.run_token)?{return Err("app_account_relation_scope_invalid");}super::super::account_runs::require_live(c,run.run_token.line_id(),run.run_token.revision(),run.run_token.nonce())})
}
impl AccountManager{
    /// Accounting only, before HTTP creates borrowed Relation/property vectors.
    /// The actual immutable input/current identity/nonce admission is separate.
    pub(crate) fn reserve_relation_http_scratch(&self,line:&str,revision:u64,bytes:usize,consent:bool,write_consent:bool)->Result<RelationHttpScratch,&'static str>{
        if !consent||!write_consent{return Err("app_account_relation_consent_required");}
        self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_relation_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_relation_scope_invalid");}
        run.configuration_current()?;live_run(&run)?;
        let(scope,budget)=run.backend.reserve_relation_http_scratch(bytes)?;
        let configuration=run.relation_configuration.as_ref().ok_or("app_account_relation_not_installed")?;
        run.database.with_conn(|c|configuration.require_current(c,line,revision,&scope))?;Ok(RelationHttpScratch(budget))
    }
    pub(crate) fn prepare_relation_update(&self,line:&str,revision:u64,group:Option<&[u8]>,adds:&[Relation<'_>],updates:&[Relation<'_>],removes:&[Removal<'_>],nonce:&str,consent:bool,write_consent:bool)->Result<PreparedRelationUpdate,&'static str>{
        if !consent||!write_consent{return Err("app_account_relation_consent_required");}
        self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_relation_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.relation_updates_enabled(){return Err("app_account_relation_not_installed");}
        run.configuration_current()?;live_run(&run)?;
        let deadline=Instant::now().checked_add(run.backend.relation_update_timeout()).ok_or("app_account_relation_timeout")?;
        let(owner,scope,message)=run.backend.reserve_relation_message(group,adds,updates,removes,nonce)?;
        let configuration=run.relation_configuration.as_ref().cloned().ok_or("app_account_relation_not_installed")?;
        run.database.with_conn(|c|configuration.require_current(c,line,revision,&scope))?;
        let message=Arc::new(message);
        let admission=RelationIntent::capture(run.database.clone(),run.run_token.clone(),owner.clone(),scope.clone(),message.clone(),configuration.clone(),run.im_gate.clone(),deadline)?;
        let interest=match &admission{IntentAdmission::Fresh(intent)=>Some(RelationIntentWait::original(intent.clone())),IntentAdmission::Replay(_)=>None};
        Ok(PreparedRelationUpdate{run,owner,scope,message,deadline,configuration,admission,_interest:interest})
    }
    pub(crate) async fn finish_relation_update(&self,prepared:PreparedRelationUpdate)->Result<RelationWriteView,&'static str>{
        let PreparedRelationUpdate{run,owner,scope,message,deadline,configuration,admission,_interest}=prepared;let slot=self.slot(run.run_token.line_id())?;
        if !lock(&slot.run).as_ref().is_some_and(|current|Arc::ptr_eq(current,&run)){return Err("app_account_relation_scope_invalid");}
        run.configuration_current()?;live_run(&run)?;
        run.database.with_conn(|c|configuration.require_current(c,run.run_token.line_id(),run.run_token.revision(),&scope))?;
        if Instant::now()>=deadline{return Err("app_account_relation_timeout");}
        let intent=match admission{IntentAdmission::Replay(id)=>return run.database.with_conn(|c|super::super::relation_journal::historical_view(c,run.run_token.line_id(),id)),IntentAdmission::Fresh(intent)=>intent};
        let gate=intent.gate();
        let controller=tokio::select!{biased;
            _=gate.retired()=>return Err("app_account_relation_scope_invalid"),
            _=run.im_gate.retired()=>return Err("app_account_relation_scope_invalid"),_=scope.session_ended()=>return Err("app_account_relation_scope_invalid"),
            result=tokio::time::timeout_at(deadline.into(),run.controller.lock())=>result.map_err(|_|"app_account_relation_timeout")?,
        };
        run.configuration_current()?;live_run(&run)?;
        run.database.with_conn(|c|configuration.require_current(c,run.run_token.line_id(),run.run_token.revision(),&scope))?;
        if controller.phase()!=Phase::Hosted||controller.resource_owner().as_ref()!=Some(&owner)||!run.backend.original_scope(&owner).is_some_and(|s|s.matches(&scope))||!run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_relation_scope_invalid");}
        drop(controller);
        let result=Box::pin(run.backend.update_relations(run.database.clone(),run.run_token.clone(),run.im_gate.clone(),owner,scope,message,deadline,configuration,intent)).await?;
        run.configuration_current()?;live_run(&run)?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_relation_scope_invalid");}Ok(result)
    }
    #[cfg(test)]
    pub(crate) fn relation_resources_for_test(&self,line:&str)->Result<(usize,usize),&'static str>{let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("fixture original relation run")?;run.backend.relation_resources_for_test().ok_or("fixture original relation scope")}
    #[cfg(test)]
    pub(crate) async fn relation_controller_pause_for_test(&self,line:&str,ready:tokio::sync::oneshot::Sender<()>,release:tokio::sync::oneshot::Receiver<()>)->Result<(),&'static str>{let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("fixture original relation run")?;let _controller=run.controller.lock().await;ready.send(()).map_err(|_|"fixture controller ready")?;release.await.map_err(|_|"fixture controller release")}
    #[cfg(test)]
    pub(crate) fn relation_retire_parent_for_test(&self,line:&str)->Result<(),&'static str>{let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("fixture original relation run")?;run.im_gate.retire();Ok(())}
    pub(crate) fn org_relation_history(&self,line:&str,limit:usize)->Result<Vec<super::super::org_relation_inbox::OrgRelationView>,&'static str>{self.database.with_conn(|c|super::super::org_relation_inbox::history(c,line,limit))}
    pub(crate) fn relation_event_history(&self,line:&str,limit:usize)->Result<Vec<super::super::group_event_inbox::GroupEventView>,&'static str>{self.database.with_conn(|c|super::super::group_event_inbox::history(c,line,limit))}
    pub(crate) fn relation_intent_history(&self,line:&str,limit:usize)->Result<Vec<RelationIntentView>,&'static str>{self.database.with_conn(|c|super::super::relation_journal::intent_history(c,line,limit))}
    pub(crate) fn relation_write_history(&self,line:&str,limit:usize)->Result<Vec<RelationWriteView>,&'static str>{self.database.with_conn(|c|super::super::relation_journal::history(c,line,limit))}
}
