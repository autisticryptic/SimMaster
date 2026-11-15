//! Typed original native #Group IM operation DATA and reply ownership.
//! The producer is the original ARC session; caller data never supplies UID,
//! ARC token, metadata, endpoint, result boolean or a replacement scope.
use super::{im_data::{self,ImBox},im_delivery::ImBinding,im_inbox::ImReceiptGate,im_status,
    rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,RpcBytes},session::AttemptOwner};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
pub(super) const GROUP_OBJECT:&[u8]=b"#Group";
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum ImOperation{Refresh,Receive,SetReceived}
pub(super) enum ImQuery{
    Refresh{labels:Vec<Zeroizing<Vec<u8>>>,since:i64},
    Receive{box_id:Zeroizing<Vec<u8>>,start:i64,count:u32},
    SetReceived{box_id:Zeroizing<Vec<u8>>,index:i64},
}
impl fmt::Debug for ImQuery{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImQuery(<redacted>)")}}
impl ImQuery{
    pub(super) fn refresh(labels:&[im_status::StatusLabel<'_>],since:i64)->Result<Self,CodecError>{
        // Validate original set shape without minting a credential or IO.
        im_status::encode_refresh(b"shape-only",labels,since)?;
        Ok(Self::Refresh{labels:labels.iter().map(|l|Zeroizing::new(l.bytes().to_vec())).collect(),since})
    }
    pub(super) fn receive(box_id:ImBox<'_>,start:i64,count:u32)->Result<Self,CodecError>{
        im_data::encode_gap_recv(box_id,start,count)?;if start<0{return Err(CodecError::InvalidField);}
        Ok(Self::Receive{box_id:Zeroizing::new(box_id.bytes().to_vec()),start,count})
    }
    pub(super) fn set_received(box_id:ImBox<'_>,index:i64)->Result<Self,CodecError>{
        if index<0{return Err(CodecError::InvalidField);}im_data::encode_set_read(box_id,index)?;
        Ok(Self::SetReceived{box_id:Zeroizing::new(box_id.bytes().to_vec()),index})
    }
    pub(super) fn operation(&self)->ImOperation{match self{Self::Refresh{..}=>ImOperation::Refresh,Self::Receive{..}=>ImOperation::Receive,Self::SetReceived{..}=>ImOperation::SetReceived}}
    pub(super) fn method(&self)->&'static str{match self{Self::Refresh{..}=>im_status::REFRESH_METHOD,Self::Receive{..}=>im_data::RECV_METHOD,Self::SetReceived{..}=>im_data::SET_READ_METHOD}}
    pub(super) fn arguments(&self,current_uid:&[u8])->Result<RpcBytes,CodecError>{match self{
        Self::Refresh{labels,since}=>{let labels=labels.iter().map(|l|im_status::StatusLabel::parse(l)).collect::<Result<Vec<_>,_>>()?;im_status::encode_refresh(current_uid,&labels,*since)},
        Self::Receive{box_id,start,count}=>im_data::encode_gap_recv(ImBox::parse(box_id)?,*start,*count),
        Self::SetReceived{box_id,index}=>im_data::encode_set_read(ImBox::parse(box_id)?,*index),
    }}
    pub(super) fn retained_bytes(&self)->usize{match self{Self::Refresh{labels,..}=>labels.iter().map(|l|l.len()+4).sum::<usize>()+16,Self::Receive{box_id,..}|Self::SetReceived{box_id,..}=>box_id.len()+24}}
}
/// A cancellation gate belongs to the parent account run, while binding is the
/// one authenticated original native receiver. It is never serialized.
pub(super) struct ImRequest{binding:Arc<ImBinding>,query:Arc<ImQuery>,gate:Arc<ImReceiptGate>,deadline:Instant,call_id:i32}
impl fmt::Debug for ImRequest{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalImRequest(<redacted>)")}}
impl ImRequest{
    pub(super) fn original(binding:Arc<ImBinding>,query:Arc<ImQuery>,gate:Arc<ImReceiptGate>,deadline:Instant,call_id:i32)->Result<Arc<Self>,CodecError>{
        if call_id<=0||Instant::now()>=deadline||!gate.live()||!binding.matches(binding.owner(),&binding.scope()){return Err(CodecError::InvalidField);}
        Ok(Arc::new(Self{binding,query,gate,deadline,call_id}))
    }
    pub(super) fn active(&self)->bool{self.gate.live()&&Instant::now()<self.deadline&&self.binding.matches(self.binding.owner(),&self.binding.scope())}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.active()&&self.binding.matches(owner,scope)}
    pub(super) fn call_id(&self)->i32{self.call_id}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn binding(&self)->&Arc<ImBinding>{&self.binding}
    pub(super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(super) fn query(&self)->&ImQuery{&self.query}
    pub(super) fn query_arc(&self)->&Arc<ImQuery>{&self.query}
    pub(super) fn decode<'a>(&self,code:u32,bytes:&'a[u8])->Result<ImReplyData<'a>,CodecError>{
        if !self.active(){return Err(CodecError::InvalidField);}
        match self.query.as_ref(){
            ImQuery::Refresh{..}=>im_status::decode_refresh(code,bytes).map(ImReplyData::Refresh),
            ImQuery::Receive{start,count,..}=>{
                let result=im_data::decode_recv_result(code,bytes)?;let end=start.checked_add(i64::from(*count)-1).ok_or(CodecError::InvalidField)?;
                if result.messages.len()>*count as usize{return Err(CodecError::InvalidField);}
                for(i,m)in result.messages.iter().enumerate(){if m.index<*start||m.index>end||result.messages[..i].iter().any(|p|p.index==m.index){return Err(CodecError::InvalidField);}}
                Ok(ImReplyData::Receive(result))
            },
            ImQuery::SetReceived{..}=>im_data::decode_set_read_result(code,bytes).map(ImReplyData::SetReceived),
        }
    }
}
pub(super) enum ImReplyData<'a>{Refresh(im_status::RefreshResult<'a>),Receive(im_data::RecvResult<'a>),SetReceived(im_data::SetReadResult<'a>)}
impl fmt::Debug for ImReplyData<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImReplyData(<redacted>)")}}
pub struct OwnedImReply{request:Arc<ImRequest>,code:u32,bytes:Zeroizing<Vec<u8>>}
impl fmt::Debug for OwnedImReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedImReply(<redacted>)")}}
impl OwnedImReply{
    pub(super) fn original(request:Arc<ImRequest>,code:u32,bytes:&[u8])->Result<Self,CodecError>{request.decode(code,bytes)?;Ok(Self{request,code,bytes:Zeroizing::new(bytes.to_vec())})}
    pub(super) fn request(&self)->&Arc<ImRequest>{&self.request}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.request.matches(owner,scope)}
    pub(super) fn result(&self)->Result<ImReplyData<'_>,CodecError>{self.request.decode(self.code,&self.bytes)}
    pub(super) fn retained_bytes(&self)->usize{self.bytes.len()+self.request.query().retained_bytes()+64}
}
