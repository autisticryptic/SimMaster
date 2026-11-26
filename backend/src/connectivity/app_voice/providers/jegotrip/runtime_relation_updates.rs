//! Independent relation WRITE chain on the current original Common actor.
//! No new login, auxiliary lookup, automatic relation retry or route conversion.
use super::*;
use super::super::{account_runs::RunToken,group_directory_data::Relation,group_relation_updates::Removal,
    im_inbox::ImReceiptGate,relation_journal::{RelationMessage,PreparedRelation,RelationWait,RelationWriteView,RelationConfiguration,RelationIntent}};
impl NativeHostingBackend{
    pub(in super::super) fn relation_updates_enabled(&self)->bool{self.profile.relation_updates.is_some()&&self.im_messages_enabled()}
    pub(in super::super) fn group_management_enabled(&self)->bool{self.profile.group_management.is_some()&&self.im_messages_enabled()}
    pub(in super::super) fn relation_update_timeout(&self)->Duration{self.profile.rpc_timeout}
    #[cfg(test)]
    pub(in super::super) fn relation_resources_for_test(&self)->Option<(usize,usize)>{lock(&self.state).slot.as_ref().map(|a|a.scope.native_rpc_resources())}
    pub(in super::super) fn reserve_relation_http_scratch(&self,bytes:usize)->Result<(RpcConnectionScope,Arc<super::super::native_rpc_budget::NativeRpcReservation>),&'static str>{
        if !self.relation_updates_enabled(){return Err("app_account_relation_not_installed");}
        let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let state=lock(&attempt.state);
        if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let bytes=bytes.checked_add(64).ok_or("app_account_relation_capacity")?;
        let budget=attempt.scope.reserve_native_rpc(bytes).map_err(|_|"app_account_relation_capacity")?;Ok((attempt.scope.clone(),budget))
    }
    pub(in super::super) fn reserve_management_scratch(&self,bytes:usize)->Result<(RpcConnectionScope,Arc<super::super::native_rpc_budget::NativeRpcReservation>),&'static str>{
        if !self.group_management_enabled(){return Err("app_account_group_management_not_installed");}let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let s=lock(&attempt.state);
        if !s.configured||s.retired||s.logout_started||s.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let b=attempt.scope.reserve_native_rpc(bytes.checked_add(64).ok_or("app_account_relation_capacity")?).map_err(|_|"app_account_relation_capacity")?;Ok((attempt.scope.clone(),b))
    }
    pub(in super::super) fn reserve_management_message(&self,group:&[u8],op:super::super::group_management::GroupWriteOperation,properties:&[super::super::group_management::PublicProperty<'_>],nonce:&str)->Result<(AttemptOwner,RpcConnectionScope,RelationMessage),&'static str>{
        if !self.group_management_enabled(){return Err("app_account_group_management_not_installed");}let policy=self.profile.group_management.ok_or("app_account_group_management_not_installed")?;let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let s=lock(&attempt.state);
        if !s.configured||s.retired||s.logout_started||s.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let message=RelationMessage::capture_management(&attempt.scope,group,op,properties,nonce,policy)?;Ok((attempt.owner.clone(),attempt.scope.clone(),message))
    }
    pub(in super::super) fn reserve_relation_message(&self,group:Option<&[u8]>,adds:&[Relation<'_>],updates:&[Relation<'_>],removes:&[Removal<'_>],nonce:&str)->Result<(AttemptOwner,RpcConnectionScope,RelationMessage),&'static str>{
        if !self.relation_updates_enabled(){return Err("app_account_relation_not_installed");}
        let policy=self.profile.relation_updates.ok_or("app_account_relation_not_installed")?;
        let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let state=lock(&attempt.state);
        if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let message=RelationMessage::capture(&attempt.scope,group,adds,updates,removes,nonce,policy)?;
        Ok((attempt.owner.clone(),attempt.scope.clone(),message))
    }
    pub(in super::super) async fn update_relations(&self,database:Arc<crate::platform::db::Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,message:Arc<RelationMessage>,deadline:Instant,configuration:Arc<RelationConfiguration>,intent:Arc<RelationIntent>)->Result<RelationWriteView,&'static str>{
        if !parent_gate.live(){return Err("app_account_relation_not_installed");}
        if !intent.belongs_to(&database,&run,&configuration,&parent_gate,&owner,&scope,&message,deadline){return Err("app_account_relation_scope_invalid");}
        let policy=if message.management_operation().is_some(){if !self.group_management_enabled(){return Err("app_account_group_management_not_installed");}super::super::relation_update_policy::RelationWriteGrant::from(self.profile.group_management.ok_or("app_account_group_management_not_installed")?)}else{if !self.relation_updates_enabled(){return Err("app_account_relation_not_installed");}super::super::relation_update_policy::RelationWriteGrant::from(self.profile.relation_updates.ok_or("app_account_relation_not_installed")?)};let attempt=self.original(&owner).map_err(|_|"app_account_relation_scope_invalid")?;
        {
            let state=lock(&attempt.state);
            if !state.configured||state.retired||state.logout_started||state.unregister_started||!attempt.scope.matches(&scope){return Err("app_account_relation_scope_invalid");}
        }
        let commands=attempt.commands(&owner,&scope).map_err(|_|"app_account_relation_scope_invalid")?;
        let intent_gate=intent.gate();
        let preparation=Box::pin(async{
            let binding=commands.original_im_binding(&owner,&scope).await.map_err(|_|"app_account_relation_scope_invalid")?;
            let identity=commands.original_relation_identity(&owner,&scope).await.map_err(|_|"app_account_relation_scope_invalid")?;
            intent.bind(binding,identity,policy)
        });
        let prepared=tokio::select!{biased;
            _=intent_gate.retired()=>return Err("app_account_relation_scope_invalid"),
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
