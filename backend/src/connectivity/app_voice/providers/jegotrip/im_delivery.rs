//! Owned native IM reception and original reply brands, never SMS/call grants.
//! Only the matched native ARC session supplies the UID/selector identity;
//! notification box/index values never select a domain, endpoint or account.
use super::{arc_protocol::DirectReplyTicket,batch_scheduler::ServerRequestTicket,
    im_data::{self,ImBox,ImMessage,MessageKey},native_receiver::{NativeReceiverBinding,NativeReceiverPolicy},
    rpc_pending::RpcConnectionScope,rpc_stream::{CodecError,RpcProperty,MAX_FIELD_BYTES},session::AttemptOwner};
use ring::digest::{Context,SHA256};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc,time::{Duration,Instant}};
use zeroize::Zeroizing;

#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeImConfig{
    pub notification_timeout_ms:u64,
    /// Independent explicit Info DATA consumption opt-in; never an ACK/WRITE,
    /// SMS/media/SDK grant. Missing/false preserves the original Text behavior.
    #[serde(default,skip_serializing_if="no_info")]
    pub receive_info:bool,
    /// Independent original Group/Buddy directory READ opt-in. No reception
    /// position, user WRITE, SDK readiness or cached relationship grants it.
    #[serde(default,skip_serializing_if="no_directory")]
    pub directory_queries:bool,
}
fn no_info(value:&bool)->bool{!*value}
fn no_directory(value:&bool)->bool{!*value}
impl fmt::Debug for NativeImConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeImConfig(<redacted>)")}}
impl NativeImConfig{
    pub(super) fn timeout(self)->Result<Duration,CodecError>{
        if !(100..=30_000).contains(&self.notification_timeout_ms){return Err(CodecError::InvalidField);}
        Ok(Duration::from_millis(self.notification_timeout_ms))
    }
}
fn digest(parts:&[&[u8]])->[u8;32]{let mut hash=Context::new(&SHA256);for part in parts{hash.update(&(part.len()as u64).to_be_bytes());hash.update(part);}let mut out=[0;32];out.copy_from_slice(hash.finish().as_ref());out}

pub(super) struct ImBinding{native:NativeReceiverBinding,owner:AttemptOwner,scope:RpcConnectionScope,identity:[u8;32],receipt_epoch:Zeroizing<String>,config:NativeImConfig}
impl fmt::Debug for ImBinding{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImBinding(<redacted>)")}}
impl ImBinding{
    /// Called only by ArcRpcSession after its original matched native login.
    /// This hash is a private message-dedup context, not an auth/trust proof.
    pub(super) fn authenticated(owner:AttemptOwner,scope:RpcConnectionScope,policy:Arc<NativeReceiverPolicy>,uid:&[u8],selector:&[u8])->Result<Arc<Self>,CodecError>{
        if uid.is_empty()||selector.is_empty()||uid.len()>MAX_FIELD_BYTES||selector.len()>127{return Err(CodecError::InvalidField);}
        let config=policy.im_messages().ok_or(CodecError::InvalidField)?;config.timeout()?;
        let native=NativeReceiverBinding::original(owner.clone(),scope.clone(),policy)?;
        let identity=digest(&[b"jegotrip-native-im-context-v1\0",uid,selector]);
        // A private local SQL association namespace for this ORIGINAL login
        // binding. It never changes any App/SDK UUID, selector or wire field.
        // A new physical login cannot redeem an old scope's notification row.
        let receipt_epoch=Zeroizing::new(super::account_runtime::local_nonce().map_err(|_|CodecError::InvalidField)?);
        Ok(Arc::new(Self{native,owner,scope,identity,receipt_epoch,config}))
    }
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.native.matches(owner,scope)&&&self.owner==owner&&self.scope.matches(scope)}
    pub(super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(super) fn identity(&self)->&[u8;32]{&self.identity}
    pub(super) fn authenticates(&self,uid:&[u8],selector:&[u8])->bool{self.identity==digest(&[b"jegotrip-native-im-context-v1\0",uid,selector])}
    pub(super) fn receipt_epoch(&self)->&str{self.receipt_epoch.as_str()}
    pub(super) fn deadline(&self,now:Instant)->Result<Instant,CodecError>{now.checked_add(self.config.timeout()?).ok_or(CodecError::InvalidField)}
    pub(super) fn policy(&self)->&Arc<NativeReceiverPolicy>{self.native.policy()}
    pub(super) fn receives_info(&self)->bool{self.config.receive_info}
    pub(super) fn directory_queries(&self)->bool{self.config.directory_queries}
}

pub(super) struct ImKey{box_id:Zeroizing<Vec<u8>>,index:i64,digest:[u8;32]}
impl fmt::Debug for ImKey{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OwnedImKey(<redacted>)")}}
impl ImKey{
    pub(super) fn capture(binding:&ImBinding,box_id:ImBox<'_>,index:i64)->Arc<Self>{
        Arc::new(Self{box_id:Zeroizing::new(box_id.bytes().to_vec()),index,digest:digest(&[b"jegotrip-native-im-key-v1\0",binding.identity(),box_id.bytes(),&index.to_be_bytes()])})
    }
    pub(super) fn box_id(&self)->ImBox<'_>{ImBox::parse(&self.box_id).expect("captured original IM box")}
    pub(super) fn index(&self)->i64{self.index}
    pub(super) fn digest(&self)->&[u8;32]{&self.digest}
    pub(super) fn message_fingerprint(&self,raw:&[u8])->[u8;32]{digest(&[b"jegotrip-native-im-message-v1\0",self.digest(),raw])}
    pub(super) fn bytes(&self)->usize{self.box_id.len()+40}
}

pub(super) enum ImPayload{Message{raw:Zeroizing<Vec<u8>>,fingerprint:[u8;32]},Notification{notification_id:u64}}
pub(super) struct ImNotice{binding:Arc<ImBinding>,key:Arc<ImKey>,payload:ImPayload,deadline:Instant,_org_budget:Option<Arc<super::native_rpc_budget::NativeRpcReservation>>}
impl fmt::Debug for ImNotice{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImNotice(<redacted>)")}}
impl ImNotice{
    pub(super) fn more(binding:Arc<ImBinding>,kind:&[u8],properties:&[RpcProperty<'_>],body:&[u8],now:Instant)->Result<Option<Self>,CodecError>{
        if !binding.matches(binding.owner(),&binding.scope()){return Ok(None);}
        if body.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
        let org_budget=if binding.policy().org_relation_sync()&&kind==im_data::MORE_MESSAGE_KIND{
            let headers=super::org_relation_sync::message_scratch(body)?;let peak=body.len().checked_mul(2).and_then(|n|n.checked_add(headers+std::mem::size_of::<Self>()+256)).ok_or(CodecError::TooLarge)?;
            Some(binding.scope().reserve_native_rpc(peak)?)
        }else{None};
        let Some(decoded)=im_data::decode_more_message(kind,properties,body)? else{return Ok(None);};
        let key=ImKey::capture(&binding,decoded.box_id,decoded.message.index);
        let canonical=if org_budget.is_some(){decoded.message.encode_measured(body.len())?}else{decoded.message.encode()?};let raw=Zeroizing::new(canonical.expose_to_transport().to_vec());
        let fingerprint=key.message_fingerprint(&raw);
        let deadline=now.checked_add(binding.config.timeout()?).ok_or(CodecError::InvalidField)?;
        Ok(Some(Self{binding,key,payload:ImPayload::Message{raw,fingerprint},deadline,_org_budget:org_budget}))
    }
    pub(super) fn notification(binding:Arc<ImBinding>,notification_id:u64,properties:&[RpcProperty<'_>],now:Instant)->Result<Self,CodecError>{
        if !binding.matches(binding.owner(),&binding.scope())||notification_id==0||notification_id==i64::MAX as u64{return Err(CodecError::InvalidField);}
        let decoded=MessageKey::notification(properties)?;let key=ImKey::capture(&binding,decoded.box_id,decoded.index);
        let deadline=now.checked_add(binding.config.timeout()?).ok_or(CodecError::InvalidField)?;
        Ok(Self{binding,key,payload:ImPayload::Notification{notification_id},deadline,_org_budget:None})
    }
    pub(super) fn binding(&self)->&Arc<ImBinding>{&self.binding}
    pub(super) fn key(&self)->&Arc<ImKey>{&self.key}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn notification_id(&self)->Option<u64>{match &self.payload{ImPayload::Notification{notification_id}=>Some(*notification_id),_=>None}}
    pub(super) fn message(&self)->Result<Option<ImMessage<'_>>,CodecError>{match &self.payload{ImPayload::Message{raw,..}=>ImMessage::decode(raw).map(Some),_=>Ok(None)}}
    pub(super) fn raw(&self)->Option<&[u8]>{match &self.payload{ImPayload::Message{raw,..}=>Some(raw),_=>None}}
    pub(super) fn message_fingerprint(&self)->Option<&[u8;32]>{match &self.payload{ImPayload::Message{fingerprint,..}=>Some(fingerprint),_=>None}}
    pub(super) fn fingerprint(&self)->[u8;32]{match &self.payload{ImPayload::Message{fingerprint,..}=>*fingerprint,ImPayload::Notification{notification_id}=>digest(&[b"jegotrip-native-im-notification-v1\0",self.key.digest(),&notification_id.to_be_bytes()])}}
    pub(super) fn bytes(&self)->usize{self.key.bytes()+160+self.raw().map_or(0,|raw|raw.len())}
}

/// Original authenticated native object checks precede the IM content parser.
/// Unsupported kinds remain available to the unchanged Sip.Request dispatcher.
pub(super) fn direct_message(binding:&Arc<ImBinding>,owner:&AttemptOwner,scope:&RpcConnectionScope,request:&super::rpc_envelope::RpcRequest<'_>)->Result<Option<ImNotice>,CodecError>{
    if !binding.matches(owner,scope)||request.method!=super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(){return Ok(None);}
    binding.policy().validate(super::native_receiver::ReceiverObject::Message,&request.metadata,super::native_receiver::ReceiverTransport::Direct)?;
    let message=super::rpc_incoming::decode_online_message(request.arguments)?;
    ImNotice::more(binding.clone(),message.receiver_kind,&message.properties,message.body,Instant::now())
}
pub(super) fn batch_message(binding:&Arc<ImBinding>,incoming:&super::batch_scheduler::IncomingBatchRequest)->Result<Option<ImNotice>,CodecError>{
    if !binding.matches(binding.owner(),&binding.scope())||!incoming.ticket().matches(binding.owner(),&binding.scope()){return Ok(None);}
    let request=incoming.request();if request.method!=super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(){return Ok(None);}
    binding.policy().validate(super::native_receiver::ReceiverObject::Message,&request.metadata,super::native_receiver::ReceiverTransport::Batch)?;
    let message=super::rpc_incoming::decode_online_message(request.arguments)?;
    ImNotice::more(binding.clone(),message.receiver_kind,&message.properties,message.body,Instant::now())
}

pub(super) enum ImReplySource{Direct(DirectReplyTicket),Batch(ServerRequestTicket)}
pub struct ImReplyTicket{source:ImReplySource,binding:Arc<ImBinding>,key:Arc<ImKey>,fingerprint:[u8;32],message:bool,deadline:Instant,completion_deadline:Instant}
impl fmt::Debug for ImReplyTicket{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImReplyTicket(<redacted>)")}}
impl ImReplyTicket{
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{
        self.binding.matches(owner,scope)&&match &self.source{ImReplySource::Direct(ticket)=>ticket.matches(owner,scope),ImReplySource::Batch(ticket)=>ticket.matches(owner,scope)}
    }
    pub(super) fn source(&self)->&ImReplySource{&self.source}
    pub(super) fn binding(&self)->&Arc<ImBinding>{&self.binding}
    pub(super) fn key(&self)->&Arc<ImKey>{&self.key}
    pub(super) fn fingerprint(&self)->&[u8;32]{&self.fingerprint}
    pub(super) fn is_message(&self)->bool{self.message}
    pub(super) fn deadline(&self)->Instant{self.deadline}
    pub(super) fn completion_deadline(&self)->Instant{self.completion_deadline}
    pub(super) fn clipped_to(&self,deadline:Instant)->Self{let mut ticket=self.captured_copy();ticket.completion_deadline=ticket.completion_deadline.min(deadline);ticket}
    pub(super) fn captured_copy(&self)->Self{
        let source=match &self.source{ImReplySource::Direct(ticket)=>ImReplySource::Direct(ticket.captured_copy()),ImReplySource::Batch(ticket)=>ImReplySource::Batch(ticket.captured_copy())};
        Self{source,binding:self.binding.clone(),key:self.key.clone(),fingerprint:self.fingerprint,message:self.message,deadline:self.deadline,completion_deadline:self.completion_deadline}
    }
}
pub struct IncomingImDelivery{notice:ImNotice,ticket:ImReplyTicket}
impl fmt::Debug for IncomingImDelivery{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("IncomingImDelivery(<redacted>)")}}
impl IncomingImDelivery{
    pub(super) fn direct(notice:ImNotice,ticket:DirectReplyTicket)->Result<Self,CodecError>{Self::capture(notice,ImReplySource::Direct(ticket))}
    pub(super) fn batch(notice:ImNotice,ticket:ServerRequestTicket)->Result<Self,CodecError>{Self::capture(notice,ImReplySource::Batch(ticket))}
    fn capture(notice:ImNotice,source:ImReplySource)->Result<Self,CodecError>{
        let ticket=ImReplyTicket{source,binding:notice.binding.clone(),key:notice.key.clone(),fingerprint:notice.fingerprint(),message:notice.raw().is_some(),deadline:notice.deadline,completion_deadline:notice.deadline};
        if !ticket.matches(notice.binding.owner(),&notice.binding.scope()){return Err(CodecError::InvalidField);}
        Ok(Self{notice,ticket})
    }
    pub(super) fn owner(&self)->&AttemptOwner{self.notice.binding.owner()}
    pub(super) fn scope(&self)->RpcConnectionScope{self.notice.binding.scope()}
    pub(super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.ticket.matches(owner,scope)}
    pub(super) fn notice(&self)->&ImNotice{&self.notice}
    pub(super) fn ticket(&self)->&ImReplyTicket{&self.ticket}
    pub(super) fn bytes(&self)->usize{self.notice.bytes()+64}
}
