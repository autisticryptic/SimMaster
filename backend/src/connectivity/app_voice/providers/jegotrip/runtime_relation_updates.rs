//! Independent relation WRITE chain on the current original Common actor.
//! No new login, auxiliary lookup, automatic relation retry or route conversion.
use super::*;
use super::super::{account_runs::RunToken,group_directory_data::Relation,group_relation_updates::Removal,
    im_inbox::ImReceiptGate,relation_journal::{RelationMessage,PreparedRelation,RelationWait,RelationWriteView,RelationConfiguration,RelationIntent}};
impl NativeHostingBackend{
    pub(in super::super) fn relation_updates_enabled(&self)->bool{self.profile.relation_updates.is_some()&&self.im_messages_enabled()}
    pub(in super::super) fn group_creation_enabled(&self)->bool{self.profile.group_creation.is_some()&&self.im_messages_enabled()}
    pub(in super::super) fn group_creation_properties_enabled(&self)->bool{self.group_creation_enabled()&&self.profile.group_creation.is_some_and(|p|p.read_properties)}
    pub(in super::super) fn owner_only_installed(&self)->bool{self.profile.group_owner_only&&self.group_creation_enabled()}
    pub(in super::super) fn owner_only_supported(&self,owner:&AttemptOwner)->bool{
        if !self.owner_only_installed(){return false;}let Ok(attempt)=self.original(owner)else{return false;};let s=lock(&attempt.state);
        let available=s.configured&&!s.retired&&!s.logout_started&&!s.unregister_started&&attempt.user_entries.permits_client_read(owner,&attempt.scope)&&lock(&attempt.user_entry_uid_local).as_ref().is_ok_and(Option::is_some)&&lock(&attempt.session_uri).as_ref().is_some_and(|v|super::super::user_entry_data::RecipientUri::parse(v.as_bytes()).is_ok());available
    }
    pub(in super::super) fn reserve_owner_only_message(&self,name:&str,kind:u32,properties:&[super::super::group_management::PublicProperty<'_>],nonce:&str,read_properties:bool)->Result<(AttemptOwner,RpcConnectionScope,RelationMessage),&'static str>{
        const MISSING:&str="app_account_group_owner_only_not_installed";
        if !self.owner_only_installed(){return Err(MISSING);}let policy=self.profile.group_creation.ok_or(MISSING)?;
        let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let s=lock(&attempt.state);
        if !s.configured||s.retired||s.logout_started||s.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let reference=lock(&attempt.user_entry_uid_local).clone().map_err(|_|MISSING)?.ok_or(MISSING)?;let common=self.profile.common.as_ref().ok_or(MISSING)?;
        let source={let uri=lock(&attempt.session_uri);let uri=uri.as_ref().ok_or(MISSING)?;super::super::group_owner_only::OwnerOnlySource::original(&attempt.owner,&attempt.scope,&common.router,reference,attempt.user_entries.clone(),uri.as_bytes()).map_err(|e|if e==super::super::rpc_stream::CodecError::TooLarge{"app_account_relation_capacity"}else{MISSING})?};
        let message=RelationMessage::capture_owner_only(&attempt.scope,&attempt.owner,source,name,kind,properties,nonce,read_properties,policy)?;
        Ok((attempt.owner.clone(),attempt.scope.clone(),message))
    }
    pub(in super::super) fn reserve_creation_scratch(&self,bytes:usize)->Result<(RpcConnectionScope,Arc<super::super::native_rpc_budget::NativeRpcReservation>),&'static str>{
        if !self.group_creation_enabled(){return Err("app_account_group_create_not_installed");}let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let s=lock(&attempt.state);
        if !s.configured||s.retired||s.logout_started||s.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let b=attempt.scope.reserve_native_rpc(bytes.checked_add(64).ok_or("app_account_relation_capacity")?).map_err(|_|"app_account_relation_capacity")?;Ok((attempt.scope.clone(),b))
    }
    pub(in super::super) fn reserve_creation_message(&self,name:&str,kind:u32,owner:&[u8],members:&[Relation<'_>],properties:&[super::super::group_management::PublicProperty<'_>],nonce:&str,read_properties:bool)->Result<(AttemptOwner,RpcConnectionScope,RelationMessage),&'static str>{
        if !self.group_creation_enabled(){return Err("app_account_group_create_not_installed");}let policy=self.profile.group_creation.ok_or("app_account_group_create_not_installed")?;let attempt=lock(&self.state).slot.clone().ok_or("app_account_relation_scope_invalid")?;let s=lock(&attempt.state);
        if !s.configured||s.retired||s.logout_started||s.unregister_started||!attempt.scope.is_open()||attempt.scope.session_revoked(){return Err("app_account_relation_scope_invalid");}
        let message=RelationMessage::capture_creation(&attempt.scope,name,kind,owner,members,properties,nonce,read_properties,policy)?;Ok((attempt.owner.clone(),attempt.scope.clone(),message))
    }
    pub(in super::super) async fn read_created_properties(&self,receipt:Arc<super::super::group_create_read::CommittedCreation>)->Result<super::super::group_create_read::PropertiesReadView,RuntimeError>{
        if !self.group_creation_enabled()||self.profile.group_creation.is_none_or(|p|!p.read_properties)||!receipt.current(){return Err(RuntimeError::Scope);}
        let scope=receipt.scope();let owner=receipt.owner();let attempt=self.original(owner).map_err(|_|RuntimeError::Scope)?;
        {let s=lock(&attempt.state);if !s.configured||s.retired||s.logout_started||s.unregister_started{return Err(RuntimeError::Scope);}}
        let input=scope.reserve_native_rpc(receipt.group().len()+std::mem::size_of::<super::super::rpc_im::ImQuery>()+256).map_err(|_|RuntimeError::Backpressure)?;
        let query=Arc::new(super::super::rpc_im::ImQuery::created_properties(receipt.clone(),input).map_err(|_|RuntimeError::Scope)?);let _wait=super::super::group_create_read::CreationReadWait::original(receipt.clone(),query.clone());
        let gate=receipt.gate();let commands=attempt.commands(owner,&scope).map_err(|_|RuntimeError::Scope)?;let deadline=receipt.deadline();
        let operation=Box::pin(commands.read_im(owner,&scope,query,gate.clone()));let reply=tokio::select!{biased;_=gate.retired()=>return Err(RuntimeError::Cancelled),_=scope.session_ended()=>return Err(RuntimeError::Cancelled),r=tokio::time::timeout_at(deadline.into(),operation)=>r.map_err(|_|RuntimeError::Timeout)??};
        receipt.outcome(&reply).map_err(|_|RuntimeError::Protocol)
    }
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
        let policy=if message.management_operation()==Some(super::super::group_management::GroupWriteOperation::CreateOwnerOnly){if !self.owner_only_supported(&owner){return Err("app_account_group_owner_only_not_installed");}super::super::relation_update_policy::RelationWriteGrant::OwnerOnly(self.profile.group_creation.ok_or("app_account_group_owner_only_not_installed")?)}else if message.management_operation()==Some(super::super::group_management::GroupWriteOperation::Create){if !self.group_creation_enabled(){return Err("app_account_group_create_not_installed");}super::super::relation_update_policy::RelationWriteGrant::from(self.profile.group_creation.ok_or("app_account_group_create_not_installed")?)}else if message.management_operation().is_some(){if !self.group_management_enabled(){return Err("app_account_group_management_not_installed");}super::super::relation_update_policy::RelationWriteGrant::from(self.profile.group_management.ok_or("app_account_group_management_not_installed")?)}else{if !self.relation_updates_enabled(){return Err("app_account_relation_not_installed");}super::super::relation_update_policy::RelationWriteGrant::from(self.profile.relation_updates.ok_or("app_account_relation_not_installed")?)};let attempt=self.original(&owner).map_err(|_|"app_account_relation_scope_invalid")?;
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
