//! Sealed current inbound relation-event binding and original DIRECT/batch reply.
//! Its identity and permit cannot be reconstructed from HTTP/history/WRITE ACKs.
use super::{arc_session::RelationIdentity,group_events::{self,NativeRelationEventsConfig,GroupUpdate,RelationEventTarget},im_delivery::{ImBinding,ImReplySource},native_receiver::{ReceiverObject,ReceiverTransport},native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,Reader,RpcProperty,MAX_FIELD_BYTES},session::AttemptOwner};
use std::{fmt,sync::Arc,time::Instant};
use zeroize::Zeroizing;
fn hash(parts:&[&[u8]])->[u8;32]{let mut h=ring::digest::Context::new(&ring::digest::SHA256);for p in parts{h.update(&(p.len()as u64).to_be_bytes());h.update(p);}let mut result=[0;32];result.copy_from_slice(h.finish().as_ref());result}
pub(super) struct RelationEventBinding{im:Arc<ImBinding>,identity:Arc<RelationIdentity>,config:NativeRelationEventsConfig,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for RelationEventBinding{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationEventBinding(<redacted>)")}}
impl RelationEventBinding{
    /// Only the original protocol's matched SDK login and installed IM binding
    /// may supply these values. Event opt-in is independent of relation WRITE.
    pub(super) fn original(im:Arc<ImBinding>,identity:Arc<RelationIdentity>,config:NativeRelationEventsConfig)->Result<Arc<Self>,CodecError>{
        config.validate().map_err(|_|CodecError::InvalidField)?;
        if !identity.authenticates(&im){return Err(CodecError::InvalidField);}let budget=im.scope().reserve_native_rpc(std::mem::size_of::<Self>()+64)?;
        Ok(Arc::new(Self{im,identity,config,_budget:budget}))
    }
    pub(super) fn im(&self)->&Arc<ImBinding>{&self.im}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.config.group_updates&&self.im.matches(owner,scope)&&self.identity.authenticates(&self.im)&&self._budget.belongs_to(scope)}
    fn uid(&self)->Result<&[u8],CodecError>{self.identity.uid_for(self.im.owner(),&self.im.scope()).ok_or(CodecError::InvalidField)}
}
pub(super) struct GroupEventNotice{binding:Arc<RelationEventBinding>,identifier:Zeroizing<Vec<u8>>,body:Zeroizing<Vec<u8>>,target:RelationEventTarget,base:i64,update:i64,fingerprint:[u8;32],deadline:Instant,budget:Arc<NativeRpcReservation>}
impl fmt::Debug for GroupEventNotice{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("GroupEventNotice(<redacted>)")}}
impl GroupEventNotice{
    fn capture(binding:Arc<RelationEventBinding>,kind:&[u8],header:&[RpcProperty<'_>],body:&[u8],now:Instant)->Result<Option<Self>,CodecError>{
        if kind!=group_events::GROUP_UPDATE_KIND{return Ok(None);}
        if !binding.matches(binding.im.owner(),&binding.im.scope()){return Err(CodecError::InvalidField);}
        if body.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
        let uid=binding.uid()?;let id_size=header.iter().find(|p|p.key==b"Group.GroupId").map_or(uid.len(),|p|p.value.len());
        let scratch=group_events::relation_map_scratch(body)?;
        let peak=body.len().checked_mul(2).and_then(|n|n.checked_add(id_size+scratch+std::mem::size_of::<Self>()+256)).ok_or(CodecError::TooLarge)?;
        let budget=binding.im.scope().reserve_native_rpc(peak)?;
        let value=group_events::decode_group_update(kind,header,body,uid)?.ok_or(CodecError::InvalidField)?;
        let identifier=Zeroizing::new(value.identifier.to_vec());let target=value.target;let base=value.base_time;let update=value.update_time;
        // The same validated wire bytes remain immutable through durable
        // consumption and reply lastDrop. No READ/WRITE grant is attached.
        let fingerprint=hash(&[b"original-group-update-v1",binding.im.identity(),&identifier,&base.to_be_bytes(),&update.to_be_bytes(),body]);
        let deadline=binding.im.deadline(now)?;drop(value);
        Ok(Some(Self{binding,identifier,body:Zeroizing::new(body.to_vec()),target,base,update,fingerprint,deadline,budget}))
    }
    pub(super) fn binding(&self)->&Arc<RelationEventBinding>{&self.binding}
    pub(super) fn fingerprint(&self)->&[u8;32]{&self.fingerprint}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn bytes(&self)->usize{self.body.len()+self.identifier.len()+256}
    pub(super) fn identifier(&self)->&[u8]{&self.identifier}
    pub(super) fn body(&self)->&[u8]{&self.body}
    pub(super) fn base_time(&self)->i64{self.base}
    pub(super) fn update_time(&self)->i64{self.update}
    pub(super) fn target(&self)->RelationEventTarget{self.target}
    pub(super) fn decoded(&self)->Result<GroupUpdate<'_>,CodecError>{
        if !self.budget.belongs_to(&self.binding.im.scope()){return Err(CodecError::InvalidField);}
        if !self.binding.matches(self.binding.im.owner(),&self.binding.im.scope()){return Err(CodecError::InvalidField);}
        // Target and both timestamps were fixed under the real original UID
        // at capture; only the retained full relation stream is parsed again.
        Ok(GroupUpdate{identifier:&self.identifier,target:self.target,base_time:self.base,update_time:self.update,relations:group_events::decode_relation_map(&self.body)?})
    }
}
pub(super) fn direct(binding:&Arc<RelationEventBinding>,owner:&AttemptOwner,scope:&RpcConnectionScope,request:&super::rpc_envelope::RpcRequest<'_>)->Result<Option<GroupEventNotice>,CodecError>{
    if !binding.matches(owner,scope)||request.method!=super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(){return Ok(None);}
    let Some(_headers)=reserve_headers(binding,request.arguments)?else{return Ok(None);};
    binding.im.policy().validate(ReceiverObject::Message,&request.metadata,ReceiverTransport::Direct)?;
    if request.metadata.iter().find(|p|p.key==b"token").is_some_and(|p|Some(p.value)!=binding.identity.token_for(owner,scope)){return Err(CodecError::InvalidField);}
    let value=super::rpc_incoming::decode_online_message(request.arguments)?;GroupEventNotice::capture(binding.clone(),value.receiver_kind,&value.properties,value.body,Instant::now())
}
pub(super) fn batch(binding:&Arc<RelationEventBinding>,incoming:&super::batch_scheduler::IncomingBatchRequest)->Result<Option<GroupEventNotice>,CodecError>{
    if !binding.matches(binding.im.owner(),&binding.im.scope())||!incoming.ticket().matches(binding.im.owner(),&binding.im.scope()){return Ok(None);}
    let request=incoming.request();if request.method!=super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(){return Ok(None);}
    let Some(_headers)=reserve_headers(binding,request.arguments)?else{return Ok(None);};
    binding.im.policy().validate(ReceiverObject::Message,&request.metadata,ReceiverTransport::Batch)?;
    if request.metadata.iter().find(|p|p.key==b"token").is_some_and(|p|Some(p.value)!=binding.identity.token_for(binding.im.owner(),&binding.im.scope())){return Err(CodecError::InvalidField);}
    let value=super::rpc_incoming::decode_online_message(request.arguments)?;GroupEventNotice::capture(binding.clone(),value.receiver_kind,&value.properties,value.body,Instant::now())
}
/// Inspect only borrowed fields first. Opting into Group.Update must not add
/// token rules to the unrelated IM/Status/SMS dispatchers. Reserve the original
/// online-header vector before decode_online_message allocates it.
fn reserve_headers(binding:&RelationEventBinding,args:&[u8])->Result<Option<Arc<NativeRpcReservation>>,CodecError>{
    let mut reader=Reader::versioned(args)?;if reader.bytes()?!=group_events::GROUP_UPDATE_KIND{return Ok(None);}
    let count=reader.count(8)?;for _ in 0..count{reader.bytes()?;reader.bytes()?;}reader.bytes()?;reader.finish()?;
    let size=count.checked_mul(std::mem::size_of::<RpcProperty<'_>>()).and_then(|n|n.checked_add(64)).ok_or(CodecError::TooLarge)?;
    binding.im.scope().reserve_native_rpc(size).map(Some)
}
pub(super) struct GroupEventTicket{source:ImReplySource,notice:Arc<GroupEventNotice>}
impl fmt::Debug for GroupEventTicket{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("GroupEventTicket(<redacted>)")}}
impl GroupEventTicket{
    pub(super) fn source(&self)->&ImReplySource{&self.source}
    pub(super) fn notice(&self)->&Arc<GroupEventNotice>{&self.notice}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.notice.binding.matches(owner,scope)&&match &self.source{ImReplySource::Direct(t)=>t.matches(owner,scope),ImReplySource::Batch(t)=>t.matches(owner,scope)}}
    pub(super) fn deadline(&self)->Instant{self.notice.deadline}
    pub(super) fn fingerprint(&self)->&[u8;32]{&self.notice.fingerprint}
    pub(super) fn captured_copy(&self)->Self{Self{source:match &self.source{ImReplySource::Direct(t)=>ImReplySource::Direct(t.captured_copy()),ImReplySource::Batch(t)=>ImReplySource::Batch(t.captured_copy())},notice:self.notice.clone()}}
}
pub struct IncomingGroupEvent{ticket:GroupEventTicket}
impl fmt::Debug for IncomingGroupEvent{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("IncomingGroupEvent(<redacted>)")}}
impl IncomingGroupEvent{
    fn capture(notice:GroupEventNotice,source:ImReplySource)->Result<Self,CodecError>{
        let ticket=GroupEventTicket{source,notice:Arc::new(notice)};let binding=&ticket.notice.binding;
        if !ticket.matches(binding.im.owner(),&binding.im.scope()){return Err(CodecError::InvalidField);}Ok(Self{ticket})
    }
    pub(super) fn direct(notice:GroupEventNotice,ticket:super::arc_protocol::DirectReplyTicket)->Result<Self,CodecError>{Self::capture(notice,ImReplySource::Direct(ticket))}
    pub(super) fn batch(notice:GroupEventNotice,ticket:super::batch_scheduler::ServerRequestTicket)->Result<Self,CodecError>{Self::capture(notice,ImReplySource::Batch(ticket))}
    pub(super) fn ticket(&self)->&GroupEventTicket{&self.ticket}
    pub(super) fn notice(&self)->&Arc<GroupEventNotice>{&self.ticket.notice}
    pub(super) fn owner(&self)->&AttemptOwner{self.ticket.notice.binding.im.owner()}
    pub(super) fn scope(&self)->RpcConnectionScope{self.ticket.notice.binding.im.scope()}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.ticket.matches(owner,scope)}
    pub(super) fn bytes(&self)->usize{self.ticket.notice.bytes()+64}
}
