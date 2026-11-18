//! Private original received-position WRITE request/reply. A matched read,
//! caller bool, status row or serialized input cannot construct this receipt.
use super::{im_inbox::ReceivedWritePermit,im_data,rpc_im::ImQuery,rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError,MAX_STREAM_BYTES},session::AttemptOwner};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;
pub(super) struct ReceivedRpcRequest{permit:Arc<ReceivedWritePermit>,query:Arc<ImQuery>,id:i32,budget:Arc<super::native_rpc_budget::NativeRpcReservation>}
impl fmt::Debug for ReceivedRpcRequest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalReceivedRpcRequest(<redacted>)")}}
impl ReceivedRpcRequest{
    /// Only the original ARC pending WRITE allocation calls this constructor;
    /// arming binds one original ID and deadline on the durable permit first.
    pub(super) fn original(permit:Arc<ReceivedWritePermit>,id:i32,budget:Arc<super::native_rpc_budget::NativeRpcReservation>)->Result<Arc<Self>,CodecError>{
        if !permit.request_is_current(id)||!budget.belongs_to(&permit.binding().scope()){return Err(CodecError::InvalidField);}
        Ok(Arc::new(Self{query:permit.query(),permit,id,budget}))
    }
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        self.permit.is_original_for(owner,scope)&&self.permit.request_is_current(self.id)
    }
    pub(super) fn id(&self)->i32{self.id}
    pub(super) fn permit(&self)->&Arc<ReceivedWritePermit>{&self.permit}
    pub(super) fn query(&self)->&Arc<ImQuery>{&self.query}
}
pub struct OwnedReceivedReply{request:Arc<ReceivedRpcRequest>,code:u32,bytes:Zeroizing<Vec<u8>>}
impl fmt::Debug for OwnedReceivedReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedReceivedReply(<redacted>)")}}
impl OwnedReceivedReply{
    pub(super) fn original(request:Arc<ReceivedRpcRequest>,code:u32,bytes:&[u8])->Result<Self,CodecError>{
        if !request.permit.request_is_current(request.id)||bytes.len()>MAX_STREAM_BYTES{return Err(CodecError::InvalidField);}
        if code==0{im_data::decode_set_read_result(code,bytes)?;}
        // A matched native nonzero/version-error result is a negative, not a
        // license to replay the write under a new version or request ID.
        request.budget.retain_reply(bytes.len())?;
        Ok(Self{request,code,bytes:Zeroizing::new(bytes.to_vec())})
    }
    pub(super) fn request(&self)->&Arc<ReceivedRpcRequest>{&self.request}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.request.matches(owner,scope)}
    pub(super) fn belongs_to(&self,permit:&ReceivedWritePermit)->bool{
        std::ptr::eq(Arc::as_ptr(&self.request.permit),permit)&&Arc::ptr_eq(&self.request.query,&permit.query())
            &&permit.request_is_current(self.request.id)
    }
    pub(super) fn accepted(&self)->Result<bool,CodecError>{
        if self.code!=0{return Ok(false);}Ok(im_data::decode_set_read_result(self.code,&self.bytes)?.accepted)
    }
    pub(super) fn result_code(&self)->u32{self.code}
    pub(super) fn retained_bytes(&self)->usize{self.bytes.len()+self.request.query.retained_bytes()+64}
}
