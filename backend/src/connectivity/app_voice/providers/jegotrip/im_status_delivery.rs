//! Original Status.Update delivery, distinct from Message.MoreMessage/SMS.
//! The authenticated original receiver and original DIRECT/batch holder brands
//! own it. A status observation never becomes an IM message consumption proof.
use super::{im_delivery::{ImBinding,ImReplySource},im_status::{self,StatusUpdate},
    native_receiver::{ReceiverObject,ReceiverTransport},rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,RpcProperty,Writer},session::AttemptOwner};
use ring::digest::{Context,SHA256};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
fn fingerprint(binding:&ImBinding,label:&[u8],time:i64,body:&[u8])->[u8;32]{let mut d=Context::new(&SHA256);for p in [b"original-im-status-v1\0".as_slice(),binding.identity().as_slice(),label,&time.to_be_bytes(),body]{d.update(&(p.len()as u64).to_be_bytes());d.update(p);}let mut out=[0;32];out.copy_from_slice(d.finish().as_ref());out}
pub(super) struct ImStatusNotice{binding:Arc<ImBinding>,label:Zeroizing<Vec<u8>>,time:i64,body:Zeroizing<Vec<u8>>,fingerprint:[u8;32],deadline:Instant}
impl fmt::Debug for ImStatusNotice{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImStatusNotice(<redacted>)")}}
impl ImStatusNotice{
    fn capture(binding:Arc<ImBinding>,kind:&[u8],header:&[RpcProperty<'_>],body:&[u8],now:Instant)->Result<Option<Self>,CodecError>{
        if !binding.matches(binding.owner(),&binding.scope()){return Ok(None);}
        let Some(status)=im_status::decode_status_update(kind,header,body)?else{return Ok(None);};
        let label=Zeroizing::new(status.entry.label.bytes().to_vec());let mut w=Writer::new();w.properties(&status.entry.properties)?;let body=Zeroizing::new(w.finish().expose_to_transport().to_vec());
        let fingerprint=fingerprint(&binding,&label,status.update_time,&body);let deadline=binding.deadline(now)?;
        Ok(Some(Self{binding,label,time:status.update_time,body,fingerprint,deadline}))
    }
    pub(super) fn binding(&self)->&Arc<ImBinding>{&self.binding}
    pub(super) fn fingerprint(&self)->&[u8;32]{&self.fingerprint}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn bytes(&self)->usize{self.body.len()+self.label.len()+192}
    pub(super) fn status(&self)->Result<StatusUpdate<'_>,CodecError>{
        // The owned native header remains label/time, never Notify.MsgIdx or a
        // caller-selected Box/index. Typed parsing is repeated before SQL.
        im_status::decode_owned_status(&self.label,self.time,&self.body)
    }
    pub(super) fn label(&self)->&[u8]{&self.label}
    pub(super) fn time(&self)->i64{self.time}
    pub(super) fn body(&self)->&[u8]{&self.body}
}
pub(super) fn direct(binding:&Arc<ImBinding>,owner:&AttemptOwner,scope:&RpcConnectionScope,request:&super::rpc_envelope::RpcRequest<'_>)->Result<Option<ImStatusNotice>,CodecError>{
    if !binding.matches(owner,scope)||request.method!=super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(){return Ok(None);}
    binding.policy().validate(ReceiverObject::Message,&request.metadata,ReceiverTransport::Direct)?;
    let message=super::rpc_incoming::decode_online_message(request.arguments)?;ImStatusNotice::capture(binding.clone(),message.receiver_kind,&message.properties,message.body,Instant::now())
}
pub(super) fn batch(binding:&Arc<ImBinding>,incoming:&super::batch_scheduler::IncomingBatchRequest)->Result<Option<ImStatusNotice>,CodecError>{
    if !binding.matches(binding.owner(),&binding.scope())||!incoming.ticket().matches(binding.owner(),&binding.scope()){return Ok(None);}
    let request=incoming.request();if request.method!=super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(){return Ok(None);}
    binding.policy().validate(ReceiverObject::Message,&request.metadata,ReceiverTransport::Batch)?;
    let message=super::rpc_incoming::decode_online_message(request.arguments)?;ImStatusNotice::capture(binding.clone(),message.receiver_kind,&message.properties,message.body,Instant::now())
}
pub struct ImStatusTicket{source:ImReplySource,binding:Arc<ImBinding>,fingerprint:[u8;32],deadline:Instant}
impl fmt::Debug for ImStatusTicket{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalImStatusTicket(<redacted>)")}}
impl ImStatusTicket{
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.binding.matches(owner,scope)&&match &self.source{ImReplySource::Direct(t)=>t.matches(owner,scope),ImReplySource::Batch(t)=>t.matches(owner,scope)}}
    pub(super) fn source(&self)->&ImReplySource{&self.source}
    pub(super) fn binding(&self)->&Arc<ImBinding>{&self.binding}
    pub(super) fn fingerprint(&self)->&[u8;32]{&self.fingerprint}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn captured_copy(&self)->Self{Self{source:match &self.source{ImReplySource::Direct(t)=>ImReplySource::Direct(t.captured_copy()),ImReplySource::Batch(t)=>ImReplySource::Batch(t.captured_copy())},binding:self.binding.clone(),fingerprint:self.fingerprint,deadline:self.deadline}}
}
pub struct IncomingImStatus{notice:ImStatusNotice,ticket:ImStatusTicket}
impl fmt::Debug for IncomingImStatus{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("IncomingImStatus(<redacted>)")}}
impl IncomingImStatus{
    fn capture(notice:ImStatusNotice,source:ImReplySource)->Result<Self,CodecError>{let ticket=ImStatusTicket{source,binding:notice.binding.clone(),fingerprint:notice.fingerprint,deadline:notice.deadline};if !ticket.matches(notice.binding.owner(),&notice.binding.scope()){return Err(CodecError::InvalidField);}Ok(Self{notice,ticket})}
    pub(super) fn direct(notice:ImStatusNotice,ticket:super::arc_protocol::DirectReplyTicket)->Result<Self,CodecError>{Self::capture(notice,ImReplySource::Direct(ticket))}
    pub(super) fn batch(notice:ImStatusNotice,ticket:super::batch_scheduler::ServerRequestTicket)->Result<Self,CodecError>{Self::capture(notice,ImReplySource::Batch(ticket))}
    pub(super) fn owner(&self)->&AttemptOwner{self.notice.binding.owner()}
    pub(super) fn scope(&self)->RpcConnectionScope{self.notice.binding.scope()}
    pub(super) fn notice(&self)->&ImStatusNotice{&self.notice}
    pub(super) fn ticket(&self)->&ImStatusTicket{&self.ticket}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.ticket.matches(owner,scope)}
    pub(super) fn bytes(&self)->usize{self.notice.bytes()+64}
}
