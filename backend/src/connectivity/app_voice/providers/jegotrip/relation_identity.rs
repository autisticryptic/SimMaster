//! Sealed current native Group identity; not directory/send/WRITE authority.
use super::*;
pub(in super::super) struct RelationIdentity{owner:AttemptOwner,scope:RpcConnectionScope,uid:Zeroizing<Vec<u8>>,selector:Zeroizing<Vec<u8>>,token:Zeroizing<Vec<u8>>,budget:Arc<super::super::native_rpc_budget::NativeRpcReservation>}
impl fmt::Debug for RelationIdentity{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationIdentity(<redacted>)")}}
impl RelationIdentity{
    pub(in super::super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{&self.owner==owner&&self.scope.matches(scope)&&self.budget.belongs_to(scope)&&scope.is_open()&&!scope.session_revoked()}
    pub(in super::super) fn authenticates(&self,binding:&super::super::im_delivery::ImBinding)->bool{self.matches(binding.owner(),&binding.scope())&&binding.authenticates(&self.uid,&self.selector)}
    pub(in super::super) fn uid_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,scope).then_some(self.uid.as_slice())}
    pub(in super::super) fn token_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,scope).then_some(self.token.as_slice())}
}
impl ArcRpcSession{
    pub(in super::super) fn relation_identity(&self)->Result<Arc<RelationIdentity>,CodecError>{
        if !self.native_receiver_scope(){return Err(CodecError::InvalidField);}
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;
        validate_bytes(&auth.user_id,true)?;validate_bytes(&auth.selected_session_id,true)?;validate_bytes(&auth.token,true)?;
        let size=auth.user_id.len().checked_add(auth.selected_session_id.len()).and_then(|n|n.checked_add(auth.token.len())).and_then(|n|n.checked_add(std::mem::size_of::<RelationIdentity>()+self.owner.line_id().len()+32)).ok_or(CodecError::TooLarge)?;
        let budget=self.connection.reserve_native_rpc(size)?;
        Ok(Arc::new(RelationIdentity{owner:self.owner.clone(),scope:self.connection.clone(),uid:Zeroizing::new(auth.user_id.clone()),selector:Zeroizing::new(auth.selected_session_id.clone()),token:Zeroizing::new(auth.token.clone()),budget}))
    }
}
