//! Single-use login binding from the matched plan. Fixed bytes are checked again
//! on the owning actor, never constructed from an HTTP id, URI or cached result.
use super::{arc_session::ArcLoginInputs,batch_scheduler::AliasOrigin,im_inbox::ImReceiptGate,native_rpc_budget::NativeRpcReservation,
    rpc_pending::RpcConnectionScope,rpc_stream::CodecError,router_connector::RouterPolicy,runtime_provision::ProvisionedCommonLogin,session::AttemptOwner};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;
fn fingerprint(inputs:&ArcLoginInputs<'_>)->[u8;32]{
    let mut hash=ring::digest::Context::new(&ring::digest::SHA256);
    for value in [inputs.typed_identity,inputs.domain_name,inputs.selected_session_id,inputs.phone_login_token,inputs.application_key,
        inputs.domain_id,inputs.application_id,inputs.device_info_cc]{hash.update(&(value.len()as u64).to_be_bytes());hash.update(value);}
    hash.update(&(inputs.client_properties.len()as u64).to_be_bytes());
    for p in inputs.client_properties{for value in [p.key,p.value]{hash.update(&(value.len()as u64).to_be_bytes());hash.update(value);}}
    let mut out=[0;32];out.copy_from_slice(hash.finish().as_ref());out
}
pub(super) struct AliasLogin{entry:Zeroizing<Vec<u8>>,digest:[u8;32],origin:Arc<AliasOrigin>,router:super::router_connector::RouterPolicyIdentity,_charge:Arc<NativeRpcReservation>}
impl AliasLogin{
    pub(super) fn capture(owner:&AttemptOwner,scope:&RpcConnectionScope,plan:&ProvisionedCommonLogin,router:&RouterPolicy,gate:Arc<ImReceiptGate>,inputs:&ArcLoginInputs<'_>)->Result<Self,CodecError>{
        let entry=plan.entry_for(owner,scope,router).map_err(|_|CodecError::InvalidField)?;
        let charge=scope.reserve_native_rpc(entry.identity().len()+std::mem::size_of::<Self>()+128)?;
        let origin=AliasOrigin::from_plan(owner,scope,router,plan,inputs,gate)?;
        Ok(Self{entry:Zeroizing::new(entry.identity().to_vec()),digest:fingerprint(inputs),origin,router:router.original_identity(),_charge:charge})
    }
    pub(super) fn entry(&self)->&[u8]{&self.entry}
    pub(super) fn current(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,inputs:&ArcLoginInputs<'_>)->bool{
        self.origin.matches(owner,scope)&&self.digest==fingerprint(inputs)&&self._charge.belongs_to(scope)
    }
    pub(super) fn transport_matches(&self,router:&super::router_connector::RouterPolicyIdentity)->bool{self.router.same(router)}
    pub(super) fn into_origin(self)->Arc<AliasOrigin>{self.origin}
}
impl fmt::Debug for AliasLogin{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AliasLogin(<redacted>)")}}
