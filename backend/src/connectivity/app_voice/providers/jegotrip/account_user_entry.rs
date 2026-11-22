//! Explicit authenticated current-account recipient lookup. Query consent is
//! not user mark-read/business WRITE consent and never repairs uncertain WRITE.
use super::*;
use super::super::user_entry_action::{UserEntryAction,UserEntryObservation};
/// Synchronously captured original run/action, never caller-supplied identity.
pub struct PreparedUserEntryLookup{run:Arc<ManagedRun>,action:Arc<UserEntryAction>}
impl std::fmt::Debug for PreparedUserEntryLookup{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("PreparedUserEntryLookup(<redacted>)")}}
impl AccountManager{
    fn current_user_entry_run(&self,run:&Arc<ManagedRun>)->Result<(),&'static str>{
        let slot=self.slot(run.run_token.line_id())?;
        if !lock(&slot.run).as_ref().is_some_and(|current|Arc::ptr_eq(current,run))||!run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_user_entry_scope_invalid");}
        run.configuration_current()?;
        run.database.with_conn(|conn|{
            if !super::super::account_runs::runtime_matches(conn,&run.run_token)?{return Err("app_account_user_entry_scope_invalid");}
            super::super::account_runs::require_live(conn,run.run_token.line_id(),run.run_token.revision(),run.run_token.nonce())
        })
    }
    /// Keep the original advertised deadline, including controller contention.
    /// Reserve original input storage before copying or the first manager await.
    pub fn prepare_user_entry(&self,line:&str,revision:u64,recipient:&str,consent:bool)->Result<PreparedUserEntryLookup,&'static str>{
        if !consent{return Err("app_account_user_entry_consent_required");}self.check_revision(line,revision)?;
        super::super::user_entry_data::RecipientUri::parse(recipient.as_bytes()).map_err(|_|"app_account_user_entry_recipient_invalid")?;
        let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_user_entry_scope_invalid")?;
        self.current_user_entry_run(&run)?;
        let deadline=Instant::now().checked_add(run.backend.user_entry_timeout()).ok_or("app_account_user_entry_timeout")?;
        let action=run.backend.reserve_user_entry_action(recipient.as_bytes(),run.im_gate.clone(),deadline)?;
        Ok(PreparedUserEntryLookup{run,action})
    }
    pub async fn finish_user_entry(&self,prepared:PreparedUserEntryLookup)->Result<UserEntryObservation,&'static str>{
        let PreparedUserEntryLookup{run,action}=prepared;let scope=action.parent();let deadline=action.deadline();
        self.current_user_entry_run(&run)?;
        if Instant::now()>=deadline{return Err("app_account_user_entry_timeout");}
        let controller=tokio::select!{biased;
            _=run.im_gate.retired()=>return Err("app_account_user_entry_scope_invalid"),
            _=scope.session_ended()=>return Err("app_account_user_entry_scope_invalid"),
            result=tokio::time::timeout_at(deadline.into(),run.controller.lock())=>result.map_err(|_|"app_account_user_entry_timeout")?,
        };
        self.current_user_entry_run(&run)?;
        if controller.phase()!=Phase::Hosted||controller.resource_owner().as_ref()!=Some(action.owner())||!run.backend.original_scope(action.owner()).is_some_and(|current|current.matches(&scope))||!action.live(){return Err("app_account_user_entry_scope_invalid");}
        drop(controller);
        let original=action.clone();
        // No controller, DB connection, state mutex or original IO gate crosses
        // the IO await. The same action/input charge stays with private holders.
        let operation=Box::pin(async{run.backend.resolve_user_entry(action).await});
        let result=tokio::select!{biased;
            _=run.im_gate.retired()=>return Err("app_account_user_entry_scope_invalid"),
            _=scope.session_ended()=>return Err("app_account_user_entry_scope_invalid"),
            result=tokio::time::timeout_at(deadline.into(),operation)=>result.map_err(|_|"app_account_user_entry_timeout")??,
        };
        self.current_user_entry_run(&run)?;
        if Instant::now()>=deadline{return Err("app_account_user_entry_timeout");}
        if !original.live()||!Arc::ptr_eq(&original.gate(),&run.im_gate)||!run.backend.original_scope(original.owner()).is_some_and(|current|current.matches(&scope)){return Err("app_account_user_entry_scope_invalid");}
        Ok(result)
    }
    /// Fixture-only original-run poll admission barrier and READ-only cap.
    /// Startup/transport policy remains the actual stored profile; no identity
    /// or handle is exported and no controller is held across the barrier.
    #[cfg(test)]
    pub(crate) async fn test_user_entry_fixture_barrier(&self,line:&str,cap:Option<Duration>)->Result<Duration,&'static str>{
        let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_user_entry_scope_invalid")?;
        self.current_user_entry_run(&run)?;
        let controller=run.controller.lock().await;let owner=controller.resource_owner().ok_or("app_account_user_entry_scope_invalid")?;
        let scope=run.backend.original_scope(&owner).ok_or("app_account_user_entry_scope_invalid")?;drop(controller);
        let timeout=run.backend.user_entry_fixture_barrier(&owner,&scope,cap).await.map_err(|_|"app_account_user_entry_scope_invalid")?;
        self.current_user_entry_run(&run)?;Ok(timeout)
    }
    /// Fixture-only barrier: expose no run/scope/identity and perform no IO.
    /// Lets Actions prove controller contention and SQL revalidation without
    /// racing the original background controller against synthetic fault data.
    #[cfg(test)]
    pub(crate) async fn test_user_entry_controller_barrier(&self,line:&str,entered:tokio::sync::oneshot::Sender<()>,released:tokio::sync::oneshot::Receiver<()>){
        let slot=self.slot(line).unwrap();let run=lock(&slot.run).clone().unwrap();
        let controller=run.controller.lock().await;let _=entered.send(());let _=released.await;drop(controller);
    }
    pub async fn lookup_user_entry(&self,line:&str,revision:u64,recipient:&str,consent:bool)->Result<UserEntryObservation,&'static str>{
        let prepared=self.prepare_user_entry(line,revision,recipient,consent)?;
        self.finish_user_entry(prepared).await
    }
}
