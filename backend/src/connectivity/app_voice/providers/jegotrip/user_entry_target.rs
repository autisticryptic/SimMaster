//! Matched bootstrap auxiliary reference bound to the pre-existing account and
//! original Router policy. This is routing DATA, never current SDK authority.
use super::{arc_session::OwnedBootstrapReply,legacy_config::ConfigDecodeKey,object_reference::{ObjectReference,ReferenceTarget,parse_object_reference},
    rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,MAX_FIELD_BYTES},router_connector::{RouterPolicy,RouterPolicyIdentity},
    session::AttemptOwner,user_entry_reference::UserEntryReference};
use std::fmt;
use zeroize::Zeroizing;
pub(super) struct UserEntryTarget{owner:AttemptOwner,parent:RpcConnectionScope,router:RouterPolicyIdentity,bootstrap_id:i32,
    reference:ObjectReference,realm:Zeroizing<Vec<u8>>,cc:Zeroizing<Vec<u8>>}
impl fmt::Debug for UserEntryTarget{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryTarget(<redacted>)")}}
impl UserEntryTarget{
    pub(super) fn from_bootstrap(reply:&OwnedBootstrapReply,owner:&AttemptOwner,source:&RpcConnectionScope,parent:&RpcConnectionScope,
        router:&RouterPolicy,key:Option<&ConfigDecodeKey>,realm:&[u8],cc:&[u8])->Result<Option<Box<Self>>,CodecError>{
        if parent.matches(source)||!parent.is_open()||parent.session_revoked()||realm.is_empty(){return Err(CodecError::InvalidField);}
        let Some(original)=UserEntryReference::from_bootstrap(reply,owner,source,key)?else{return Ok(None);};
        let reference=original.reference_for(owner,source).ok_or(CodecError::InvalidField)?;
        let ReferenceTarget::Direct(endpoints)=reference.target()else{return Err(CodecError::InvalidField);};
        if endpoints.is_empty()||endpoints.len()>16||endpoints.iter().any(|e|!matches!(e.scheme(),b"arc"|b"sarc")||e.router_host().is_err()){return Err(CodecError::InvalidField);}
        let bytes=reference.encode_native().map_err(|_|CodecError::InvalidField)?;
        if bytes.expose_to_backend().len().checked_add(realm.len()).and_then(|n|n.checked_add(cc.len())).is_none_or(|n|n>MAX_FIELD_BYTES){return Err(CodecError::TooLarge);}
        let reference=parse_object_reference(bytes.expose_to_backend()).map_err(|_|CodecError::InvalidField)?;
        Ok(Some(Box::new(Self{owner:owner.clone(),parent:parent.clone(),router:router.original_identity(),bootstrap_id:original.original_call_id(),reference,
            realm:Zeroizing::new(realm.to_vec()),cc:Zeroizing::new(cc.to_vec())})))
    }
    fn matches(&self,owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy)->bool{
        &self.owner==owner&&self.parent.matches(parent)&&parent.is_open()&&!parent.session_revoked()&&self.router.matches(router)
    }
    pub(super) fn reference_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy)->Option<&ObjectReference>{self.matches(owner,parent,router).then_some(&self.reference)}
    pub(super) fn realm_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy)->Option<&[u8]>{self.matches(owner,parent,router).then_some(&self.realm)}
    pub(super) fn cc_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope,router:&RouterPolicy)->Option<&[u8]>{self.matches(owner,parent,router).then_some(&self.cc)}
    pub(super) fn retained_bytes(&self)->usize{self.reference.encode_native().map_or(MAX_FIELD_BYTES+1,|b|b.expose_to_backend().len())+self.realm.len()+self.cc.len()}
    pub(super) fn original_bootstrap_id(&self)->i32{self.bootstrap_id}
}
