//! Sealed current native sender identity. Original Mtc_UeGetUri comes from
//! ClientI+2080 normalized OWN login URI, not returned mainAccount or caller UID.
//! Construction/UID/URI DATA alone is not an ImSendPermit or a readiness grant.
use super::*;
pub(in super::super) struct ImSendIdentity{owner:AttemptOwner,scope:RpcConnectionScope,uid:Zeroizing<Vec<u8>>,selector:Zeroizing<Vec<u8>>,token:Zeroizing<Vec<u8>>,uri:Zeroizing<Vec<u8>>,application:Zeroizing<Vec<u8>>,domain:Zeroizing<Vec<u8>>,_budget:Arc<super::super::native_rpc_budget::NativeRpcReservation>}
impl fmt::Debug for ImSendIdentity{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendIdentity(<redacted>)")}}
impl ImSendIdentity{
    pub(in super::super) fn retained_bytes(&self)->usize{self.uid.capacity()+self.selector.capacity()+self.token.capacity()+self.uri.capacity()+self.application.capacity()+self.domain.capacity()+std::mem::size_of::<Self>()+32}
    pub(in super::super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{&self.owner==owner&&self.scope.matches(scope)&&self._budget.belongs_to(scope)&&scope.is_open()&&!scope.session_revoked()}
    pub(in super::super) fn authenticates(&self,binding:&super::super::im_delivery::ImBinding)->bool{self.matches(binding.owner(),&binding.scope())&&binding.authenticates(&self.uid,&self.selector)}
    pub(in super::super) fn uri_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,scope).then_some(self.uri.as_slice())}
    pub(in super::super) fn token_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<&[u8]>{self.matches(owner,scope).then_some(self.token.as_slice())}
    pub(in super::super) fn statistics_for<'a>(&'a self,owner:&AttemptOwner,scope:&RpcConnectionScope,country:&'a [u8])->Option<[RpcProperty<'a>;3]>{self.matches(owner,scope).then_some([RpcProperty{key:b"Statistics.AppId",value:&self.application},RpcProperty{key:b"Statistics.DomainId",value:&self.domain},RpcProperty{key:b"Statistics.CountryCode",value:country}])}
}
impl ArcRpcSession{
    pub(in super::super) fn send_identity(&self)->Result<Arc<ImSendIdentity>,CodecError>{
        if !self.common_transport_claimed||self.phase()!=ArcPhase::Authenticated||self.connection.session_revoked(){return Err(CodecError::InvalidField);}
        let auth=self.authentication.as_ref().ok_or(CodecError::InvalidField)?;let uri=self.login_uri.as_ref().ok_or(CodecError::InvalidField)?;validate_bytes(uri,true)?;validate_bytes(&auth.user_id,true)?;validate_bytes(&auth.selected_session_id,true)?;validate_bytes(&auth.token,true)?;
        let application=self.login_application.as_ref().ok_or(CodecError::InvalidField)?;let domain=self.login_domain.as_ref().ok_or(CodecError::InvalidField)?;
        let retained=auth.user_id.len().checked_add(auth.selected_session_id.len()).and_then(|n|n.checked_add(auth.token.len())).and_then(|n|n.checked_add(uri.len())).and_then(|n|n.checked_add(application.len())).and_then(|n|n.checked_add(domain.len())).and_then(|n|n.checked_add(std::mem::size_of::<ImSendIdentity>()+32)).ok_or(CodecError::TooLarge)?;
        // Charge BEFORE allocating/copying and before an actor oneshot can
        // retain this snapshot. The sealed identity owns its last-holder fee.
        let budget=self.connection.reserve_native_rpc(retained)?;
        Ok(Arc::new(ImSendIdentity{owner:self.owner.clone(),scope:self.connection.clone(),uid:Zeroizing::new(auth.user_id.clone()),selector:Zeroizing::new(auth.selected_session_id.clone()),token:Zeroizing::new(auth.token.clone()),uri:Zeroizing::new(uri.to_vec()),application:application.clone(),domain:domain.clone(),_budget:budget}))
    }
}
