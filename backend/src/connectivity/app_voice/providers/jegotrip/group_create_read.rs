//! One property READ after an exact, current, durably confirmed create reply.
//! No constructor from a history view/GroupID/boolean, and no WRITE retry. A
//! property read failure never changes the already committed create outcome.
use super::{relation_journal::CreateCommit,rpc_im::{ImQuery,OwnedImReply,ImReplyData},im_inbox::ImReceiptGate,rpc_pending::RpcConnectionScope,session::AttemptOwner,rpc_stream::CodecError};
use serde::Serialize;
use std::{fmt,sync::{Arc,atomic::{AtomicBool,AtomicI32,Ordering}},time::Instant};
pub(super) struct CommittedCreation{token:CreateCommit,claimed:AtomicBool,request:AtomicI32,retired:AtomicBool}
impl fmt::Debug for CommittedCreation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommittedCreation(<redacted>)")}}
impl CommittedCreation{
 pub(super) fn original(token:CreateCommit)->Result<Arc<Self>,CodecError>{if !token.current(){return Err(CodecError::InvalidField);}Ok(Arc::new(Self{token,claimed:AtomicBool::new(false),request:AtomicI32::new(0),retired:AtomicBool::new(false)}))}
 pub(super) fn owner(&self)->&AttemptOwner{self.token.owner()}
 pub(super) fn scope(&self)->RpcConnectionScope{self.token.scope()}
 pub(super) fn group(&self)->&[u8]{self.token.group()}
 pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.token.gate().clone()}
 pub(super) fn deadline(&self)->Instant{self.token.deadline()}
 pub(super) fn current(&self)->bool{!self.retired.load(Ordering::SeqCst)&&self.token.current()&&!self.retired.load(Ordering::SeqCst)}
 pub(super) fn binds(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,gate:&Arc<ImReceiptGate>)->bool{self.owner()==owner&&self.scope().matches(scope)&&Arc::ptr_eq(self.token.gate(),gate)&&self.current()}
 pub(super) fn claim(&self)->Result<(),CodecError>{if !self.current()||self.claimed.compare_exchange(false,true,Ordering::SeqCst,Ordering::SeqCst).is_err(){return Err(CodecError::InvalidField);}Ok(())}
 // Cloning the already-claimed query must not allocate another RPC ID.
 pub(super) fn arm_read(&self,binding:&Arc<super::im_delivery::ImBinding>,gate:&Arc<ImReceiptGate>,id:i32,end:Instant)->Result<(),CodecError>{
  if id<=0||!self.claimed.load(Ordering::SeqCst)||!Arc::ptr_eq(self.token.binding(),binding)||!self.binds(binding.owner(),&binding.scope(),gate)||Instant::now()>=end||end>self.deadline(){return Err(CodecError::InvalidField);}
  self.request.compare_exchange(0,id,Ordering::SeqCst,Ordering::SeqCst).map(|_|()).map_err(|_|CodecError::InvalidField)
 }
 pub(super) fn outcome(self:&Arc<Self>,reply:&OwnedImReply)->Result<PropertiesReadView,CodecError>{
  if self.request.load(Ordering::SeqCst)!=reply.request().call_id()||!Arc::ptr_eq(self.token.binding(),reply.request().binding())||!self.claimed.load(Ordering::SeqCst)||!self.current()||!reply.matches(self.owner(),&self.scope())||!Arc::ptr_eq(&reply.request().gate(),self.token.gate()){return Err(CodecError::InvalidField);}
  let ImQuery::CreatedProperties{receipt,..}=reply.request().query()else{return Err(CodecError::InvalidField);};if !Arc::ptr_eq(receipt,self){return Err(CodecError::InvalidField);}
  let data=reply.result()?;let ImReplyData::CreatedProperties(data)=data else{return Err(CodecError::InvalidField);};
  if !self.current()||!reply.matches(self.owner(),&self.scope()){return Err(CodecError::InvalidField);}
  Ok(PropertiesReadView{state:if data.accepted{"observed"}else{"rejected"},accepted:Some(data.accepted),properties:Some(data.properties.len())})
 }
}
pub(super) struct CreationReadWait{receipt:Arc<CommittedCreation>,_query:Arc<ImQuery>}
impl CreationReadWait{pub(super) fn original(receipt:Arc<CommittedCreation>,query:Arc<ImQuery>)->Self{Self{receipt,_query:query}}}
impl Drop for CreationReadWait{fn drop(&mut self){self.receipt.retired.store(true,Ordering::SeqCst);}}
#[derive(Serialize)]
pub struct PropertiesReadView{pub state:&'static str,pub accepted:Option<bool>,pub properties:Option<usize>}
impl PropertiesReadView{pub(super) fn no_result(state:&'static str)->Self{Self{state,accepted:None,properties:None}}}
impl fmt::Debug for PropertiesReadView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("PropertiesReadView(<redacted>)")}}
