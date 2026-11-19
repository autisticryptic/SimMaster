//! Private original native userSend RPC request/result. Raw bool, message ID,
//! received/user-read reply or row cannot construct this ownership boundary.
use super::{im_inbox::ImSendPermit,native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,session::AttemptOwner,rpc_stream::{CodecError,MAX_STREAM_BYTES},im_send_data};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
pub(super) struct ImSendRpcRequest{permit:Arc<ImSendPermit>,id:i32,budget:Arc<NativeRpcReservation>}
impl fmt::Debug for ImSendRpcRequest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendRpcRequest(<redacted>)")}}
impl ImSendRpcRequest{
    pub(super) fn original(permit:Arc<ImSendPermit>,id:i32,budget:Arc<NativeRpcReservation>)->Result<Arc<Self>,CodecError>{if id<=0||!permit.current(id)||!budget.belongs_to(&permit.binding().scope()){return Err(CodecError::InvalidField);}Ok(Arc::new(Self{permit,id,budget}))}
    pub(super) fn id(&self)->i32{self.id}
    pub(super) fn permit(&self)->&Arc<ImSendPermit>{&self.permit}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.permit.matches(owner,scope)&&self.permit.current(self.id)}
}
pub(super) struct OwnedImSendReply{request:Arc<ImSendRpcRequest>,code:u32,accepted:bool,index:Option<i64>,time:Option<i64>,raw:Zeroizing<Vec<u8>>}
impl fmt::Debug for OwnedImSendReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedImSendReply(<redacted>)")}}
impl OwnedImSendReply{
    pub(super) fn original(request:Arc<ImSendRpcRequest>,code:u32,bytes:&[u8])->Result<Self,CodecError>{if bytes.len()>MAX_STREAM_BYTES||!request.matches(request.permit.binding().owner(),&request.permit.binding().scope())||Instant::now()>=request.permit.deadline(){return Err(CodecError::InvalidField);}let(accepted,index,time)=if code==0{let result=im_send_data::decode_user_send_result(0,bytes)?;(result.accepted,Some(result.msg_id),Some(result.time))}else if code>>16==1{im_send_data::decode_user_send_refused_versions(code,bytes)?;(false,None,None)}else{return Err(CodecError::RemoteFailure);};request.budget.retain_reply(bytes.len().checked_add(96).ok_or(CodecError::TooLarge)?)?;Ok(Self{request,code,accepted,index,time,raw:Zeroizing::new(bytes.to_vec())})}
    pub(super) fn request(&self)->&Arc<ImSendRpcRequest>{&self.request}
    pub(super) fn belongs_to(&self,permit:&ImSendPermit)->bool{std::ptr::eq(Arc::as_ptr(&self.request.permit),permit)}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.request.matches(owner,scope)}
    pub(super) fn code(&self)->u32{self.code}
    pub(super) fn accepted(&self)->bool{self.accepted}
    pub(super) fn message_index(&self)->Option<i64>{self.index}
    pub(super) fn time(&self)->Option<i64>{self.time}
    pub(super) fn retained_bytes(&self)->usize{self.raw.len()+96}
}
