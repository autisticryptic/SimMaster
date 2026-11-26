//! Private original setRelations request/result holder. Decoded bool/times,
//! cached rows or a caller-provided UID/token cannot construct this boundary.
use super::{relation_journal::RelationPermit,native_rpc_budget::NativeRpcReservation,
    rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,RpcBytes,RpcProperty,Writer,MAX_FIELD_BYTES,MAX_STREAM_BYTES},
    group_relation_updates,session::AttemptOwner};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
pub(super) struct RelationRpcRequest{permit:Arc<RelationPermit>,id:i32,budget:Arc<NativeRpcReservation>}
impl fmt::Debug for RelationRpcRequest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationRpcRequest(<redacted>)")}}
impl RelationRpcRequest{
    pub(super) fn id(&self)->i32{self.id}
    pub(super) fn permit(&self)->&Arc<RelationPermit>{&self.permit}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.permit.matches(owner,scope)&&self.permit.current(self.id)&&self.budget.belongs_to(scope)}
}
pub(super) fn encode_original(permit:Arc<RelationPermit>,id:i32,owner:&AttemptOwner,scope:&RpcConnectionScope,token:&[u8],deadline:Instant)->Result<(Arc<RelationRpcRequest>,RpcBytes),CodecError>{
    if id<=0||!permit.matches(owner,scope)||permit.token_for(owner,scope)!=Some(token)||Instant::now()>=deadline.min(permit.deadline())||permit.arguments().len()>MAX_FIELD_BYTES||token.len()>MAX_FIELD_BYTES{return Err(CodecError::InvalidField);}
    let method=permit.management_operation().map_or(group_relation_updates::SET_RELATIONS_METHOD,|op|op.method()).as_bytes();
    let size=1usize+4+4+(4+2+4+6)+(4+5+4+token.len())+4+method.len()+4+permit.arguments().len();
    if size>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
    // Envelope and emitted packets coexist briefly; all copies remain charged
    // by this request holder through last actual frame/result Drop.
    let fee=size.checked_mul(3).and_then(|n|n.checked_add(512)).ok_or(CodecError::TooLarge)?;let budget=scope.reserve_native_rpc(fee)?;
    let mut w=Writer::measured(size)?;w.i32(id)?;w.properties(&[RpcProperty{key:b"id",value:b"#Group"},RpcProperty{key:b"token",value:token}])?;w.bytes(method)?;w.bytes(permit.arguments())?;
    let bytes=w.finish();debug_assert_eq!(bytes.expose_to_transport().len(),size);
    permit.arm(id,deadline).map_err(|_|CodecError::InvalidField)?;
    Ok((Arc::new(RelationRpcRequest{permit,id,budget}),bytes))
}
pub(super) struct OwnedRelationReply{request:Arc<RelationRpcRequest>,code:u32,accepted:bool,base_time:Option<i64>,update_time:Option<i64>,raw:Zeroizing<Vec<u8>>}
impl fmt::Debug for OwnedRelationReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedRelationReply(<redacted>)")}}
impl OwnedRelationReply{
    pub(super) fn original(request:Arc<RelationRpcRequest>,owner:&AttemptOwner,scope:&RpcConnectionScope,code:u32,bytes:&[u8])->Result<Self,CodecError>{
        if !request.matches(owner,scope)||Instant::now()>=request.permit.deadline(){return Err(CodecError::InvalidField);}
        let(accepted,base_time,update_time)=if let Some(op)=request.permit.management_operation(){let data=super::group_management::decode(scope,op,code,bytes)?;(data.accepted,data.base,data.update)}else{let data=group_relation_updates::decode_updates(code,bytes)?;(data.accepted,Some(data.base_time),Some(data.update_time))};
        request.budget.retain_reply(bytes.len().checked_add(96).ok_or(CodecError::TooLarge)?)?;
        Ok(Self{request,code,accepted,base_time,update_time,raw:Zeroizing::new(bytes.to_vec())})
    }
    pub(super) fn request(&self)->&Arc<RelationRpcRequest>{&self.request}
    pub(super) fn belongs_to(&self,permit:&RelationPermit)->bool{std::ptr::eq(Arc::as_ptr(&self.request.permit),permit)}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.request.matches(owner,scope)}
    pub(super) fn code(&self)->u32{self.code}
    pub(super) fn accepted(&self)->bool{self.accepted}
    pub(super) fn base_time(&self)->Option<i64>{self.base_time}
    pub(super) fn update_time(&self)->Option<i64>{self.update_time}
    pub(super) fn retained_bytes(&self)->usize{self.raw.len()+96}
}
