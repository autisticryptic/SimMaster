//! Matched original user mark-read reply, distinct from received-position/read
//! results. No public bool/row/input can construct or resolve this capability.
use super::{im_inbox::UserReadPermit,im_user_read_data,rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,MAX_STREAM_BYTES},session::AttemptOwner};
use std::{fmt,sync::Arc};use zeroize::Zeroizing;
pub(super) struct UserReadRpcRequest{permit:Arc<UserReadPermit>,id:i32}
impl fmt::Debug for UserReadRpcRequest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalUserReadRpcRequest(<redacted>)")}}
impl UserReadRpcRequest{
    pub(super) fn original(permit:Arc<UserReadPermit>,id:i32)->Result<Arc<Self>,CodecError>{if !permit.current(id){return Err(CodecError::InvalidField);}Ok(Arc::new(Self{permit,id}))}
    pub(super) fn id(&self)->i32{self.id}
    pub(super) fn permit(&self)->&Arc<UserReadPermit>{&self.permit}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.permit.matches(owner,scope)&&self.permit.current(self.id)}
}
pub struct OwnedUserReadReply{request:Arc<UserReadRpcRequest>,code:u32,bytes:Zeroizing<Vec<u8>>}
impl fmt::Debug for OwnedUserReadReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedUserReadReply(<redacted>)")}}
impl OwnedUserReadReply{
    pub(super) fn original(request:Arc<UserReadRpcRequest>,code:u32,bytes:&[u8])->Result<Self,CodecError>{
        if !request.permit.current(request.id)||bytes.len()>MAX_STREAM_BYTES{return Err(CodecError::InvalidField);}
        if code==0{im_user_read_data::decode_mark_read_result(code,bytes)?;}
        Ok(Self{request,code,bytes:Zeroizing::new(bytes.to_vec())})
    }
    pub(super) fn request(&self)->&Arc<UserReadRpcRequest>{&self.request}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.request.matches(owner,scope)}
    pub(super) fn belongs_to(&self,p:&UserReadPermit)->bool{std::ptr::eq(Arc::as_ptr(&self.request.permit),p)&&p.current(self.request.id)}
    pub(super) fn accepted(&self)->Result<bool,CodecError>{if self.code!=0{return Ok(false);}Ok(im_user_read_data::decode_mark_read_result(self.code,&self.bytes)?.accepted)}
    pub(super) fn code(&self)->u32{self.code}
    pub(super) fn retained_bytes(&self)->usize{self.bytes.len()+self.request.permit.label().len()+96}
}
