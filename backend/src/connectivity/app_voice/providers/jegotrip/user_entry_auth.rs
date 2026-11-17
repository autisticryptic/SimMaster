//! Sealed current original native SDK authentication for a UserEntry RPC.
//! Not deserializable, not App phone/SIP credentials, not a new SDK login.
use super::*;
pub(in super::super) struct UserEntryAuth{owner:AttemptOwner,parent:RpcConnectionScope,uid:Zeroizing<Vec<u8>>,selector:Zeroizing<Vec<u8>>,token:Zeroizing<Vec<u8>>}
impl fmt::Debug for UserEntryAuth{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserEntryAuth(<redacted>)")}}
impl UserEntryAuth{
    pub(in super::super) fn matches(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->bool{&self.owner==owner&&self.parent.matches(parent)&&parent.is_open()&&!parent.session_revoked()}
    pub(in super::super) fn parent(&self)->RpcConnectionScope{self.parent.clone()}
    pub(in super::super) fn token_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,parent).then_some(self.token.as_slice())}
    pub(in super::super) fn uid_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,parent).then_some(self.uid.as_slice())}
    pub(in super::super) fn selector_for(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,parent).then_some(self.selector.as_slice())}
}
impl ArcRpcSession{
    pub(in super::super) fn user_entry_auth(&self)->Result<Box<UserEntryAuth>,CodecError>{
        if !self.common_transport_claimed||self.phase()!=ArcPhase::Authenticated||self.connection.session_revoked(){return Err(CodecError::InvalidField);}
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;
        validate_bytes(&auth.user_id,true)?;validate_bytes(&auth.selected_session_id,true)?;validate_bytes(&auth.token,true)?;
        Ok(Box::new(UserEntryAuth{owner:self.owner.clone(),parent:self.connection.clone(),uid:Zeroizing::new(auth.user_id.clone()),selector:Zeroizing::new(auth.selected_session_id.clone()),token:Zeroizing::new(auth.token.clone())}))
    }
}
