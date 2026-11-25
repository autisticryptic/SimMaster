//! Explicit roster materialization uses the original directory READ route and
//! fixed admission deadline. No implicit READ on events, no WRITE permission.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{relation_roster::{RosterRead,RosterReadView},rpc_im::ImQuery};
pub struct PreparedRosterRead{directory:PreparedDirectoryRead}
impl std::fmt::Debug for PreparedRosterRead{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("PreparedRosterRead(<redacted>)")}}
impl AccountManager{
    pub fn prepare_relation_roster(&self,line:&str,revision:u64,group:Option<&str>,since:i64,consent:bool)->Result<PreparedRosterRead,&'static str>{
        let directory=self.prepare_native_directory(line,revision,group,since,false,consent)?;
        if !directory.run.backend.relation_roster_enabled()||directory.run.relation_configuration.is_none(){return Err("app_account_relation_roster_not_installed");}Ok(PreparedRosterRead{directory})
    }
    pub async fn finish_relation_roster(&self,prepared:PreparedRosterRead)->Result<RosterReadView,&'static str>{
        let PreparedDirectoryRead{run,owner,scope,query,deadline}=prepared.directory;
        if !matches!(query.as_ref(),ImQuery::Relations{..}){return Err("app_account_relation_roster_scope_invalid");}
        let slot=self.slot(run.run_token.line_id())?;if !lock(&slot.run).as_ref().is_some_and(|r|Arc::ptr_eq(r,&run)){return Err("app_account_relation_roster_scope_invalid");}
        run.configuration_current()?;live_run(&run)?;
        let configuration=run.relation_configuration.clone().ok_or("app_account_relation_roster_not_installed")?;
        let controller=tokio::select!{biased;_=run.im_gate.retired()=>return Err("app_account_relation_roster_scope_invalid"),_=scope.session_ended()=>return Err("app_account_relation_roster_scope_invalid"),result=tokio::time::timeout_at(deadline.into(),run.controller.lock())=>result.map_err(|_|"app_account_directory_timeout")?};
        if controller.phase()!=Phase::Hosted||controller.resource_owner().as_ref()!=Some(&owner)||!run.backend.original_scope(&owner).is_some_and(|s|s.matches(&scope))||!run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_relation_roster_scope_invalid");}drop(controller);
        let binding=tokio::select!{biased;_=run.im_gate.retired()=>return Err("app_account_relation_roster_scope_invalid"),_=scope.session_ended()=>return Err("app_account_relation_roster_scope_invalid"),result=tokio::time::timeout_at(deadline.into(),run.backend.roster_binding(&owner,&scope))=>result.map_err(|_|"app_account_directory_timeout")?.map_err(error)?};
        let original=run.database.with_conn(|c|RosterRead::capture(run.database.clone(),c,run.run_token.clone(),binding,run.im_gate.clone(),configuration,query.clone()))?;
        if Instant::now()>=deadline{return Err("app_account_directory_timeout");}
        // Capture the serial before the first directory READ IO. The original
        // account consumer may commit newer events while this response waits.
        let reply=tokio::select!{biased;_=run.im_gate.retired()=>return Err("app_account_relation_roster_scope_invalid"),_=scope.session_ended()=>return Err("app_account_relation_roster_scope_invalid"),result=tokio::time::timeout_at(deadline.into(),Box::pin(run.backend.read_directory(&owner,&scope,query,run.im_gate.clone())))=>result.map_err(|_|"app_account_directory_timeout")?.map_err(error)?};
        run.configuration_current()?;live_run(&run)?;if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||!run.im_gate.live()||Instant::now()>=deadline{return Err("app_account_relation_roster_scope_invalid");}
        let value=original.commit(&run.database,&reply)?;
        run.configuration_current()?;live_run(&run)?;if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||!run.im_gate.live()||Instant::now()>=deadline{return Err("app_account_relation_roster_scope_invalid");}Ok(value)
    }
    pub fn relation_roster_history(&self,line:&str,limit:usize)->Result<Vec<n::relation_roster::RosterView>,&'static str>{self.database.with_conn(|c|n::relation_roster::history(c,line,limit))}
}
