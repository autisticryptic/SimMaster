//! Original569028 admits the current UserEntry Agent before parsing, then puts
//! UID URIs in its local completion set without aidToUid IO. Presence DATA and
//! current SDK auth stay separate; this result never grants WRITE/readiness.
use super::{arc_session::{OwnedBootstrapReply,UserEntryAuth},legacy_config::ConfigDecodeKey,rpc_pending::RpcConnectionScope,
    rpc_stream::CodecError,router_connector::{RouterPolicy,RouterPolicyIdentity},session::AttemptOwner,
    user_entry_reference::UserEntryReference,user_entry_action::{UserEntryAction,UserEntryObservation},user_entry_data::{RecipientUri,AidKind}};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;
pub(super) struct UserEntryLocalReference{owner:AttemptOwner,parent:RpcConnectionScope,router:RouterPolicyIdentity,bootstrap_id:i32,reference:super::object_reference::ObjectReference}
impl fmt::Debug for UserEntryLocalReference{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryLocalReference(<redacted>)")}}
impl UserEntryLocalReference{
    pub(super) fn from_bootstrap(reply:&OwnedBootstrapReply,owner:&AttemptOwner,source:&RpcConnectionScope,parent:&RpcConnectionScope,
        router:&RouterPolicy,key:Option<&ConfigDecodeKey>)->Result<Option<Arc<Self>>,CodecError>{
        if source.matches(parent)||!parent.is_open()||parent.session_revoked(){return Err(CodecError::InvalidField);}
        let Some(reference)=UserEntryReference::from_bootstrap(reply,owner,source,key)?else{return Ok(None);};
        reference.reference_for(owner,source).ok_or(CodecError::InvalidField)?;
        let bootstrap_id=reference.original_call_id();
        let reference=reference.into_reference(owner,source)?;
        // No endpoint, adapter or locator is selected or contacted by the
        // native UID-local branch; the exact parsed/raw snapshot is retained.
        Ok(Some(Arc::new(Self{owner:owner.clone(),parent:parent.clone(),router:router.original_identity(),bootstrap_id,reference})))
    }
    pub(super) fn matches(&self,owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy)->bool{
        &self.owner==owner&&self.parent.matches(parent)&&parent.is_open()&&!parent.session_revoked()&&self.router.matches(router)
    }
    pub(super) fn retained_bytes(&self)->usize{self.reference.retained_bytes()}
    #[cfg(test)]pub(super) fn reference_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy)->Option<&super::object_reference::ObjectReference>{self.matches(owner,parent,router).then_some(&self.reference)}
    #[cfg(test)]pub(super) fn original_bootstrap_id(&self)->i32{self.bootstrap_id}
}
pub(super) struct LocalUidResult{action:Arc<UserEntryAction>,reference:Arc<UserEntryLocalReference>,auth:Box<UserEntryAuth>,uid:Zeroizing<Vec<u8>>}
impl fmt::Debug for LocalUidResult{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("LocalUidResult(<redacted>)")}}
impl LocalUidResult{
    pub(super) fn original(action:Arc<UserEntryAction>,reference:Arc<UserEntryLocalReference>,auth:Box<UserEntryAuth>,router:&RouterPolicy)->Result<Self,&'static str>{
        let parent=action.parent();if !action.live()||!reference.matches(action.owner(),&parent,router)||!auth.matches(action.owner(),&parent){return Err("app_account_user_entry_scope_invalid");}
        let recipient=RecipientUri::parse(action.recipient()).map_err(|_|"app_account_user_entry_recipient_invalid")?;
        if recipient.kind()!=AidKind::Uid{return Err("app_account_user_entry_recipient_invalid");}
        let uid=Zeroizing::new(recipient.value().to_vec());Ok(Self{action,reference,auth,uid})
    }
    pub(super) fn observe(&self,action:&Arc<UserEntryAction>,router:&RouterPolicy)->Result<UserEntryObservation,&'static str>{
        if !Arc::ptr_eq(action,&self.action)||!action.live()||!self.reference.matches(action.owner(),&action.parent(),router)||!self.auth.matches(action.owner(),&action.parent()){
            return Err("app_account_user_entry_scope_invalid");
        }
        Ok(UserEntryObservation{kind:"uid",report:"local_uid",observed_at:chrono::Utc::now().to_rfc3339()})
    }
    // Explicitly local parsed UID DATA only, not an OwnedUserEntryReply,
    // UserReadPermit, matched WRITE acknowledgement or SDK identity proof.
    #[cfg(test)]pub(super) fn uid(&self)->&[u8]{&self.uid}
}
