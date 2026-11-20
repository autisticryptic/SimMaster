//! Explicit original current Group/Buddy directory READ. Result summaries are
//! DATA, not cached SDK grants, peer-read ACKs, notifications or remote WRITEs.
use super::*;
use super::super::{rpc_im::{ImQuery,ImReplyData},transport_actor::RuntimeError};
use serde::Serialize;
#[derive(Serialize)]
pub struct DirectoryReadView{
    pub operation:&'static str,pub target_kind:&'static str,pub accepted:bool,
    #[serde(skip_serializing_if="Option::is_none")]pub reported:Option<bool>,
    #[serde(skip_serializing_if="Option::is_none")]pub original_time:Option<String>,
    pub first_relations:usize,pub second_relations:usize,pub names:usize,pub properties:usize,
}
impl std::fmt::Debug for DirectoryReadView{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("DirectoryReadView(<redacted>)")}}
fn error(code:RuntimeError)->&'static str{match code{RuntimeError::Timeout=>"app_account_directory_timeout",RuntimeError::Backpressure=>"app_account_directory_capacity",RuntimeError::InvalidConfiguration=>"app_account_directory_not_installed",_=>"app_account_directory_scope_invalid"}}
fn live_run(run:&ManagedRun)->Result<(),&'static str>{run.database.with_conn(|conn|{if !super::super::account_runs::runtime_matches(conn,&run.run_token)?{return Err("app_account_directory_scope_invalid");}super::super::account_runs::require_live(conn,run.run_token.line_id(),run.run_token.revision(),run.run_token.nonce())})}
/// Synchronously reserved original-query context. No Deserialize/caller UID,
/// scope substitution, persistent grant, WRITE permit or notification result.
pub struct PreparedDirectoryRead{run:Arc<ManagedRun>,owner:AttemptOwner,scope:RpcConnectionScope,query:Arc<ImQuery>,deadline:Instant}
impl std::fmt::Debug for PreparedDirectoryRead{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("PreparedDirectoryRead(<redacted>)")}}
impl AccountManager{
    /// Reserve/copy only after current original run checks, before the HTTP or
    /// manager's first await. The HTTP temporary group/since fields can be wiped.
    pub fn prepare_native_directory(&self,line:&str,revision:u64,group:Option<&str>,since:i64,properties:bool,consent:bool)->Result<PreparedDirectoryRead,&'static str>{
        if !consent{return Err("app_account_directory_consent_required");}
        if since< -1||properties&&since!= -1||group.is_some_and(|id|id.len()>508)||properties&&group.is_none(){return Err("app_account_directory_request_invalid");}
        if let Some(id)=group{super::super::group_directory_data::GroupId::parse(id.as_bytes()).map_err(|_|"app_account_directory_request_invalid")?;}
        self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_directory_scope_invalid")?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.directory_queries_enabled(){return Err("app_account_directory_not_installed");}
        run.configuration_current()?;live_run(&run)?;
        let deadline=Instant::now().checked_add(run.backend.directory_query_timeout()).ok_or("app_account_directory_timeout")?;
        let(owner,scope,query)=run.backend.reserve_directory_query(group.map(str::as_bytes),since,properties,deadline).map_err(error)?;
        Ok(PreparedDirectoryRead{run,owner,scope,query,deadline})
    }
    pub async fn finish_native_directory(&self,prepared:PreparedDirectoryRead)->Result<DirectoryReadView,&'static str>{
        let PreparedDirectoryRead{run,owner,scope,query,deadline}=prepared;
        // Check even the same-named line's original run; replacement/config/
        // stop cannot revive this snapshot or move it to another SDK scope.
        let slot=self.slot(run.run_token.line_id())?;
        if !lock(&slot.run).as_ref().is_some_and(|current|Arc::ptr_eq(current,&run)){return Err("app_account_directory_scope_invalid");}
        run.configuration_current()?;live_run(&run)?;
        if Instant::now()>=deadline{return Err("app_account_directory_timeout");}
        let controller=tokio::select!{biased;_ = run.im_gate.retired()=>return Err("app_account_directory_scope_invalid"),_ = scope.session_ended()=>return Err("app_account_directory_scope_invalid"),result=tokio::time::timeout_at(deadline.into(),run.controller.lock())=>result.map_err(|_|"app_account_directory_timeout")?};
        run.configuration_current()?;
        if controller.phase()!=Phase::Hosted||controller.resource_owner().as_ref()!=Some(&owner)||!run.backend.original_scope(&owner).is_some_and(|s|s.matches(&scope))||!run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_directory_scope_invalid");}
        drop(controller);live_run(&run)?;
        if Instant::now()>=deadline{return Err("app_account_directory_timeout");}
        // No controller, DB connection or original IO gate spans this await.
        let operation=Box::pin(run.backend.read_directory(&owner,&scope,query.clone(),run.im_gate.clone()));
        let reply=tokio::select!{biased;_ = run.im_gate.retired()=>return Err("app_account_directory_scope_invalid"),_ = scope.session_ended()=>return Err("app_account_directory_scope_invalid"),result=tokio::time::timeout_at(deadline.into(),operation)=>result.map_err(|_|"app_account_directory_timeout")?.map_err(error)?};
        run.configuration_current()?;live_run(&run)?;
        if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!reply.matches(&owner,&scope)||!Arc::ptr_eq(reply.request().query_arc(),&query)||!Arc::ptr_eq(&reply.request().gate(),&run.im_gate){return Err("app_account_directory_scope_invalid");}
        match(reply.request().query(),reply.result().map_err(|_|"app_account_directory_reply_invalid")?){
            (ImQuery::Relations{group,..},ImReplyData::Relations(value))=>Ok(DirectoryReadView{operation:"relations",target_kind:if group.is_some(){"group"}else{"current_user"},accepted:value.accepted,reported:Some(value.reported),original_time:Some(value.time.to_string()),first_relations:value.first.len(),second_relations:value.second.len(),names:value.names.len(),properties:0}),
            (ImQuery::GroupProperties{..},ImReplyData::GroupProperties(value))=>Ok(DirectoryReadView{operation:"properties",target_kind:"group",accepted:value.accepted,reported:None,original_time:None,first_relations:0,second_relations:0,names:0,properties:value.properties.len()}),
            _=>Err("app_account_directory_reply_invalid"),
        }
    }
    pub async fn read_native_directory(&self,line:&str,revision:u64,group:Option<&str>,since:i64,properties:bool,consent:bool)->Result<DirectoryReadView,&'static str>{
        let prepared=self.prepare_native_directory(line,revision,group,since,properties,consent)?;self.finish_native_directory(prepared).await
    }
}
