//! Independent relation WRITE chain on the current original Common actor.
//! No new login, auxiliary lookup, automatic relation retry or route conversion.
use super::*;
use super::super::{account_runs::RunToken,group_directory_data::Relation,group_relation_updates::Removal,
    im_inbox::ImReceiptGate,relation_journal::{RelationMessage,RelationAction,PreparedRelation,RelationWait,RelationWriteView,RelationConfiguration}};
impl NativeHostingBackend{
    pub(super) fn relation_updates_enabled(&self)->bool{self.profile.relation_updates.is_some()&&self.im_messages_enabled()}
    pub(super) fn relation_update_timeout(&self)->Duration{self.profile.rpc_timeout}
    #[cfg(test)]
    pub(super) fn relation_resources_for_test(&self)->Option<(usize,usize)>{lock(&self.state).slot.as_ref().map(|a|a.scope.native_rpc_resources())}
    pub(super) fn reserve_relation_http_scratch(&self,bytes:usize)->Result<(RpcConnectionScope,Arc<super::super::native_rpc_budget::NativeRpcReservation>),&'static str>{
        if !self.relation_updates_enabled(){return Err("app_account_relation_not_installed");}
        let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let state=lock(&attempt.state);
        if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let bytes=bytes.checked_add(64).ok_or("app_account_relation_capacity")?;
        let budget=attempt.scope.reserve_native_rpc(bytes).map_err(|_|"app_account_relation_capacity")?;Ok((attempt.scope.clone(),budget))
    }
    pub(super) fn reserve_relation_message(&self,group:Option<&[u8]>,adds:&[Relation<'_>],updates:&[Relation<'_>],removes:&[Removal<'_>],nonce:&str)->Result<(AttemptOwner,RpcConnectionScope,RelationMessage),&'static str>{
        if !self.relation_updates_enabled(){return Err("app_account_relation_not_installed");}
        let policy=self.profile.relation_updates.ok_or("app_account_relation_not_installed")?;
        let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let state=lock(&attempt.state);
        if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let message=RelationMessage::capture(&attempt.scope,group,adds,updates,removes,nonce,policy)?;
        Ok((attempt.owner.clone(),attempt.scope.clone(),message))
    }
    pub(super) async fn update_relations(&self,database:Arc<crate::platform::db::Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,message:RelationMessage,deadline:Instant,configuration:Arc<RelationConfiguration>)->Result<RelationWriteView,&'static str>{
        if !self.relation_updates_enabled()||!parent_gate.live(){return Err("app_account_relation_not_installed");}
        let policy=self.profile.relation_updates.ok_or("app_account_relation_not_installed")?;let attempt=self.original(&owner).map_err(|_|"app_account_relation_scope_invalid")?;
        {
            let state=lock(&attempt.state);
            if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.matches(&scope){return Err("app_account_relation_scope_invalid");}
        }
        let commands=attempt.commands(&owner,&scope).map_err(|_|"app_account_relation_scope_invalid")?;
        let preparation=Box::pin(async{
            let binding=commands.original_im_binding(&owner,&scope).await.map_err(|_|"app_account_relation_scope_invalid")?;
            let identity=commands.original_relation_identity(&owner,&scope).await.map_err(|_|"app_account_relation_scope_invalid")?;
            RelationAction::original_with_configuration(database,run,binding,identity,parent_gate.clone(),message,deadline,policy,configuration)
        });
        let prepared=tokio::select!{biased;
            _=parent_gate.retired()=>return Err("app_account_relation_scope_invalid"),
            _=scope.session_ended()=>return Err("app_account_relation_scope_invalid"),
            result=tokio::time::timeout_at(deadline.into(),preparation)=>result.map_err(|_|"app_account_relation_timeout")??,
        };
        let action=match prepared{PreparedRelation::Replay(view)=>return Ok(view),PreparedRelation::Fresh(action)=>action};let _wait=RelationWait::original(action.clone());let gate=action.gate();
        let operation=Box::pin(async{let permit=action.permit(policy)?;commands.update_relations(&owner,&scope,permit).await.map_err(|_|"app_account_relation_uncertain")});
        tokio::select!{biased;
            _=parent_gate.retired()=>Err("app_account_relation_uncertain"),
            _=scope.session_ended()=>Err("app_account_relation_uncertain"),
            _=gate.retired()=>Err("app_account_relation_uncertain"),
            result=tokio::time::timeout_at(deadline.into(),operation)=>result.map_err(|_|"app_account_relation_timeout")?,
        }
    }
}
