//! Original checkRelation.GroupServer.Group: explicit canonical UID read only.
//! 40DA70 writes version[0]/one STRING; 4057F4 reads bool/u32/footer;
//! 565378 exposes the raw u32 as RelationType, not an online/WRITE grant.
use super::{native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,
    rpc_stream::{read_footer,CodecError,Reader,RpcBytes,RpcFooter,RpcProperty,Writer},user_entry_data::valid_uid};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc};
pub(super) const METHOD:&str="checkRelation.GroupServer.Group";
/// Captured from the original account run; never deserialized or inferred from
/// a relation result. SQL checks are outside the nonblocking physical IO gate.
pub(super) struct CheckContext{database:Arc<crate::platform::db::Database>,run:Arc<super::account_runs::RunToken>,owner:super::session::AttemptOwner,scope:RpcConnectionScope,configuration:Arc<super::relation_journal::RelationConfiguration>,gate:Arc<super::im_inbox::ImReceiptGate>,deadline:std::time::Instant,retired:std::sync::atomic::AtomicBool}
impl fmt::Debug for CheckContext{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("BuddyCheckContext(<redacted>)")}}
impl CheckContext{
    pub(super) fn capture(database:Arc<crate::platform::db::Database>,run:Arc<super::account_runs::RunToken>,owner:super::session::AttemptOwner,scope:RpcConnectionScope,configuration:Arc<super::relation_journal::RelationConfiguration>,gate:Arc<super::im_inbox::ImReceiptGate>,deadline:std::time::Instant)->Result<Arc<Self>,CodecError>{
        let value=Arc::new(Self{database,run,owner,scope,configuration,gate,deadline,retired:std::sync::atomic::AtomicBool::new(false)});if !value.current(&value.scope){return Err(CodecError::InvalidField);}Ok(value)
    }
    pub(super) fn current(&self,scope:&RpcConnectionScope)->bool{
        let live=||!self.retired.load(std::sync::atomic::Ordering::SeqCst)&&self.scope.matches(scope)&&scope.is_open()&&!scope.session_revoked()&&self.gate.live()&&std::time::Instant::now()<self.deadline&&self.owner.line_id()==self.run.line_id();
        live()&&self.database.with_conn(|c|{if !super::account_runs::runtime_matches(c,&self.run)?{return Err("app_account_directory_scope_invalid");}super::account_runs::require_live(c,self.run.line_id(),self.run.revision(),self.run.nonce())?;self.configuration.require_current(c,self.run.line_id(),self.run.revision(),scope)}).is_ok()&&live()
    }
    pub(super) fn binds(&self,owner:&super::session::AttemptOwner,scope:&RpcConnectionScope,gate:&Arc<super::im_inbox::ImReceiptGate>,deadline:std::time::Instant)->bool{self.owner==*owner&&self.scope.matches(scope)&&Arc::ptr_eq(&self.gate,gate)&&self.deadline==deadline&&self.current(scope)}
}
pub(super) struct CheckWait(pub(super) Arc<CheckContext>,pub(super) Arc<super::rpc_im::ImQuery>);
impl Drop for CheckWait{fn drop(&mut self){self.0.retired.store(true,std::sync::atomic::Ordering::SeqCst);}}
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBuddyRelationCheckConfig{pub check:bool}
impl fmt::Debug for NativeBuddyRelationCheckConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeBuddyRelationCheckConfig(<redacted>)")}}
impl NativeBuddyRelationCheckConfig{pub(super) fn validate(self)->Result<(),CodecError>{if self.check{Ok(())}else{Err(CodecError::InvalidField)}}}
// Only the already-normalized UID form is supported here. Phone/AID/URI
// resolution is a distinct original operation; never guess or strip authority.
pub(super) fn validate_uid(uid:&[u8])->Result<(),CodecError>{if valid_uid(uid){Ok(())}else{Err(CodecError::InvalidField)}}
pub(super) fn input_size(uid:&[u8])->Result<usize,CodecError>{validate_uid(uid)?;Ok(uid.len()+std::mem::size_of::<CheckContext>()+std::mem::size_of::<super::rpc_im::ImQuery>()+256)}
pub(super) fn encode(uid:&[u8])->Result<RpcBytes,CodecError>{validate_uid(uid)?;let mut w=Writer::versioned_measured(9+uid.len())?;w.bytes(uid)?;Ok(w.finish())}
pub(super) struct CheckResult<'a>{pub(super) accepted:bool,pub(super) relation_type:u32,pub(super) footer:RpcFooter<'a>,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for CheckResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("BuddyRelationCheckResult(<redacted>)")}}
impl Drop for CheckResult<'_>{fn drop(&mut self){}}
pub(super) fn decode_size(code:u32,bytes:&[u8])->Result<usize,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}let mut r=Reader::new(bytes)?;r.boolean()?;r.u32()?;r.bytes()?;
    let count=if r.is_empty(){0}else{let n=r.count(8)?;for _ in 0..n{r.bytes()?;r.bytes()?;}n};r.finish()?;
    count.checked_mul(std::mem::size_of::<RpcProperty<'_>>()).and_then(|n|n.checked_add(128)).ok_or(CodecError::TooLarge)
}
pub(super) fn decode<'a>(scope:&RpcConnectionScope,code:u32,bytes:&'a[u8])->Result<CheckResult<'a>,CodecError>{
    let budget=scope.reserve_native_rpc(decode_size(code,bytes)?)?;
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let relation_type=r.u32()?;let footer=read_footer(&mut r)?;r.finish()?;
    Ok(CheckResult{accepted:accepted&&footer.reason.is_empty(),relation_type,footer,_budget:budget})
}
