//! Original Message IM DATA, distinct from Sip.Request SMS and ClientEx grants.
//!
//! 3F6AD8/3F6A78 read index,time,sender,type,content,attachment map,parameter map.
//! The two map value types differ in native memory but both are BE32-sized
//! byte strings on the type0 stream (E6A740/E695C0). The 328-byte native object
//! is not its wire size. This module has no owner, RPC ticket, storage or IO;
//! decoded data and successful result booleans cannot complete a notification.
use super::rpc_stream::{read_footer,CodecError,Reader,RpcBytes,RpcFooter,RpcProperty,Writer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES};
use std::fmt;

pub(super) const MORE_MESSAGE_KIND:&[u8]=b"Message.MoreMessage";
pub(super) const RECV_METHOD:&str="recv.Message.Message";
pub(super) const SET_READ_METHOD:&str="setRead.Message.Message";
pub(super) const MAX_BOX_BYTES:usize=512;
pub(super) const GAP_BATCH_COUNT:u32=10;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum BoxKind{P2P,Org}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) struct ImBox<'a>{bytes:&'a[u8],kind:BoxKind}
impl fmt::Debug for ImBox<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImBox(<redacted>)")}}
impl<'a> ImBox<'a>{
    /// Original ImNotifyEnd creates only Org/<group> or P2P/<uid>.
    /// 5B1818: group is positive-decimal (+ or -) positive-decimal.
    /// 514618: uid is decimal _ positive-decimal; its first part may be zero.
    /// Never parse these numeric-looking identities as phone numbers/routes.
    pub(super) fn parse(bytes:&'a[u8])->Result<Self,CodecError>{
        if bytes.len()>MAX_BOX_BYTES{return Err(CodecError::TooLarge);}
        let(kind,id)=if let Some(id)=bytes.strip_prefix(b"P2P/"){(BoxKind::P2P,id)}else if let Some(id)=bytes.strip_prefix(b"Org/"){(BoxKind::Org,id)}else{return Err(CodecError::InvalidField);};
        let separator=match kind{BoxKind::P2P=>id.iter().position(|&b|b==b'_'),BoxKind::Org=>id.iter().position(|&b|b==b'-').or_else(||id.iter().position(|&b|b==b'+'))}.ok_or(CodecError::InvalidField)?;
        let(first,tail)=id.split_at(separator);let last=&tail[1..];
        let digits=|part:&[u8]|!part.is_empty()&&part.iter().all(u8::is_ascii_digit);
        if !digits(first)||!digits(last)||last[0]==b'0'||kind==BoxKind::Org&&first[0]==b'0'{return Err(CodecError::InvalidField);}
        Ok(Self{bytes,kind})
    }
    pub(super) fn bytes(self)->&'a[u8]{self.bytes}
    pub(super) fn kind(self)->BoxKind{self.kind}
}

pub(super) struct MessageKey<'a>{pub(super) box_id:ImBox<'a>,pub(super) index:i64}
impl fmt::Debug for MessageKey<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImMessageKey(<redacted>)")}}
impl<'a> MessageKey<'a>{
    /// Exact nonempty Message shape already recognized by the dispatcher.
    /// Signed64 remains distinct from notification ID, UID and offline cursor.
    /// Invalid decimal strings are not converted to native's fallback zero.
    pub(super) fn notification(properties:&[RpcProperty<'a>])->Result<Self,CodecError>{
        validate_properties(properties)?;
        if properties.len()!=3{return Err(CodecError::InvalidField);}
        let get=|key:&[u8]|properties.iter().find(|p|p.key==key).map(|p|p.value);
        if get(b"Notify.Command")!=Some(b"Message".as_slice()){return Err(CodecError::InvalidField);}
        let bytes=get(b"Notify.MsgIdx").ok_or(CodecError::InvalidField)?;
        if bytes.is_empty()||bytes.len()>20{return Err(CodecError::InvalidField);}
        let text=std::str::from_utf8(bytes).map_err(|_|CodecError::InvalidField)?;
        let index=text.parse::<i64>().map_err(|_|CodecError::InvalidField)?;
        if index.to_string()!=text{return Err(CodecError::InvalidField);}
        Ok(Self{box_id:ImBox::parse(get(b"Notify.Box").ok_or(CodecError::InvalidField)?)?,index})
    }
}

pub(super) struct ImMessage<'a>{
    pub(super) index:i64,
    /// Raw original time value; parsing does not infer units or wall-clock trust.
    pub(super) time:i64,
    pub(super) sender_uid:&'a[u8],
    pub(super) kind:&'a[u8],
    pub(super) content:&'a[u8],
    pub(super) attachments:Vec<RpcProperty<'a>>,
    pub(super) parameters:Vec<RpcProperty<'a>>,
}
impl fmt::Debug for ImMessage<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImMessage(<redacted>)")}}
impl<'a> ImMessage<'a>{
    fn read(reader:&mut Reader<'a>)->Result<Self,CodecError>{
        let index=reader.i64()?;let time=reader.i64()?;let sender_uid=reader.bytes()?;
        let kind=reader.bytes()?;let content=reader.bytes()?;
        let attachments=reader.properties()?;let parameters=reader.properties()?;
        Ok(Self{index,time,sender_uid,kind,content,attachments,parameters})
    }
    pub(super) fn decode(bytes:&'a[u8])->Result<Self,CodecError>{
        let mut reader=Reader::new(bytes)?;let message=Self::read(&mut reader)?;reader.finish()?;Ok(message)
    }
    /// Pure byte validation/roundtrip only; not an outbound message operation.
    pub(super) fn encode(&self)->Result<RpcBytes,CodecError>{
        let mut writer=Writer::new();self.write(&mut writer)?;Ok(writer.finish())
    }
    fn write(&self,writer:&mut Writer)->Result<(),CodecError>{
        validate_properties(&self.attachments)?;validate_properties(&self.parameters)?;
        writer.i64(self.index)?;writer.i64(self.time)?;writer.bytes(self.sender_uid)?;
        writer.bytes(self.kind)?;writer.bytes(self.content)?;
        writer.properties(&self.attachments)?;writer.properties(&self.parameters)
    }
}

pub(super) struct MoreMessage<'a>{pub(super) box_id:ImBox<'a>,pub(super) message:ImMessage<'a>}
impl fmt::Debug for MoreMessage<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImMoreMessage(<redacted>)")}}
/// The surrounding authenticated receiver must still validate its original
/// object/token/scope and retain its original DIRECT/batch reply capability.
pub(super) fn decode_more_message<'a>(kind:&[u8],properties:&[RpcProperty<'a>],body:&'a[u8])->Result<Option<MoreMessage<'a>>,CodecError>{
    if kind!=MORE_MESSAGE_KIND{return Ok(None);}
    validate_properties(properties)?;
    let box_id=ImBox::parse(properties.iter().find(|p|p.key==b"Box").ok_or(CodecError::InvalidField)?.value)?;
    Ok(Some(MoreMessage{box_id,message:ImMessage::decode(body)?}))
}

pub(super) struct RecvResult<'a>{pub(super) accepted:bool,pub(super) messages:Vec<ImMessage<'a>>,pub(super) footer:RpcFooter<'a>}
impl fmt::Debug for RecvResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImRecvResult(<redacted>)")}}
/// 3F7920 -> 3F6B44: bool, BE32 list count, full messages, Common footer.
/// Each record needs at least36 bytes; reject count before vector allocation.
pub(super) fn decode_recv_result(result_code:u32,bytes:&[u8])->Result<RecvResult<'_>,CodecError>{
    if result_code!=0{return Err(CodecError::RemoteFailure);}
    let mut reader=Reader::new(bytes)?;let accepted=reader.boolean()?;let count=reader.count(36)?;
    let mut messages=Vec::with_capacity(count);for _ in 0..count{messages.push(ImMessage::read(&mut reader)?);}
    let footer=read_footer(&mut reader)?;reader.finish()?;
    Ok(RecvResult{accepted:accepted&&footer.reason.is_empty(),messages,footer})
}

/// Exact original gap-receive DATA, not a request/route/authorization or retry.
/// 5B3C84 limits each gap batch to10, 3F847C writes box,signed64 start,u32 count.
pub(super) fn encode_gap_recv(box_id:ImBox<'_>,start:i64,count:u32)->Result<RpcBytes,CodecError>{
    if count==0||count>GAP_BATCH_COUNT||start.checked_add(i64::from(count)-1).is_none(){return Err(CodecError::InvalidField);}
    let mut writer=Writer::versioned()?;writer.bytes(box_id.bytes())?;writer.i64(start)?;writer.u32(count)?;Ok(writer.finish())
}
/// 3F7F74: method version[0], original box, full signed64 read index. Runtime
/// must require current durable contiguous consumption and an explicit policy;
/// merely decoding a message/notification may never submit this server write.
pub(super) fn encode_set_read(box_id:ImBox<'_>,index:i64)->Result<RpcBytes,CodecError>{
    let mut writer=Writer::versioned()?;writer.bytes(box_id.bytes())?;writer.i64(index)?;Ok(writer.finish())
}
pub(super) struct SetReadResult<'a>{pub(super) accepted:bool,pub(super) footer:RpcFooter<'a>}
impl fmt::Debug for SetReadResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSetReadResult(<redacted>)")}}
pub(super) fn decode_set_read_result(result_code:u32,bytes:&[u8])->Result<SetReadResult<'_>,CodecError>{
    if result_code!=0{return Err(CodecError::RemoteFailure);}
    let mut reader=Reader::new(bytes)?;let accepted=reader.boolean()?;let footer=read_footer(&mut reader)?;reader.finish()?;
    Ok(SetReadResult{accepted:accepted&&footer.reason.is_empty(),footer})
}

fn validate_properties(properties:&[RpcProperty<'_>])->Result<(),CodecError>{
    if properties.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    let mut size=0usize;
    for (index,p) in properties.iter().enumerate(){
        if p.key.len()>MAX_FIELD_BYTES||p.value.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
        size=size.checked_add(p.key.len()).and_then(|n|n.checked_add(p.value.len())).ok_or(CodecError::TooLarge)?;
        if size>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        if properties[..index].iter().any(|prior|prior.key==p.key){return Err(CodecError::DuplicateKey);}
    }
    Ok(())
}
