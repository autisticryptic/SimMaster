//! Original matched bootstrap UserEntry reference provenance. This is DATA for
//! an owning qualified transport, not a main-ARC route or another SDK login.
use super::{arc_session::OwnedBootstrapReply,legacy_config::{decode_configuration,ConfigDecodeKey},
    object_reference::{parse_object_reference,ObjectReference},rpc_pending::RpcConnectionScope,
    session::AttemptOwner,rpc_stream::CodecError};
use std::fmt;
pub(super) struct UserEntryReference{owner:AttemptOwner,source:RpcConnectionScope,call_id:i32,reference:ObjectReference}
impl fmt::Debug for UserEntryReference{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryReference(<redacted>)")}}
impl UserEntryReference{
    pub(super) fn from_bootstrap(reply:&OwnedBootstrapReply,owner:&AttemptOwner,source:&RpcConnectionScope,key:Option<&ConfigDecodeKey>)->Result<Option<Self>,CodecError>{
        let material=reply.material_for(owner,source).ok_or(CodecError::InvalidField)?;
        if material.user_entry.is_empty(){return Ok(None);}
        // The Common reference key is original and separate from phone/SDK
        // passwords, Client-property keys, TLS trust or an authority field.
        let decoded=decode_configuration(material.user_entry,key).map_err(|_|CodecError::InvalidField)?;
        let reference=parse_object_reference(decoded.expose_to_backend()).map_err(|_|CodecError::InvalidField)?;
        Ok(Some(Self{owner:owner.clone(),source:source.clone(),call_id:reply.call_id(),reference}))
    }
    pub(super) fn original_call_id(&self)->i32{self.call_id}
    pub(super) fn reference_for(&self,owner:&AttemptOwner,source:&RpcConnectionScope)->Option<&ObjectReference>{
        if &self.owner==owner&&self.source.matches(source)&&source.is_open()&&!source.session_revoked(){Some(&self.reference)}else{None}
    }
}
