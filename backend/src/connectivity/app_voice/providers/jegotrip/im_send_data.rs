//! Original userSend.Message.Message DATA only: no send permission or IO.
//!
//! 3F89A4 writes version-count1/version0, Box, the OUTGOING 3F6E38 struct
//! (type,content,STRING->BYTES attachments,STRING->STRING parameters), then
//! a SEPARATE notification-parameters map. Received index/time/sender fields
//! must never appear here. 5C2FF4/5ABB9C encode Text as {"text":<caller text>};
//! 5AC124 prefixes an explicit Info subtype with "Info." and uses "content".
//! 5C8C38 unconditionally injects account=current original Mtc_UeGetUri.
//!
//! All restrictions below are local DATA/resource policies, not server grants.
//! Integration MUST supply a sealed, SDK-authenticated OWN normalized login
//! URI, never caller identity, mainAccount or phone. Byte validation cannot
//! authenticate its source. This module does not normalize/resolve identities,
//! interpret endpoints/tokens/grants, open attachment paths, or complete R01-R14.
use super::{im_data::ImBox,rpc_stream::{read_footer,CodecError,Reader,RpcBytes,RpcFooter,RpcProperty,Writer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES}};
use std::fmt;
use zeroize::Zeroizing;

pub(super) const USER_SEND_METHOD:&str="userSend.Message.Message";
pub(super) const MAX_INFO_SUBTYPE_BYTES:usize=64;
pub(super) const MAX_ATTACHMENT_BYTES:usize=10240;

/// Info takes an explicit UNPREFIXED subtype, never a guessed native kind.
/// No Serialize/Deserialize or permission-bearing type is exposed here.
pub(super) enum SendKind<'a>{Text,Info(&'a[u8])}
impl fmt::Debug for SendKind<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendKind(<redacted>)")}}

pub(super) struct OutgoingMessage<'a>{
    pub(super) kind:SendKind<'a>,
    /// Caller text/content, UTF-8 without NUL; ordinary newlines are allowed.
    /// Both this source and the resulting JSON field must fit MAX_FIELD_BYTES.
    pub(super) content:&'a[u8],
    /// Explicit borrowed byte attachments only, each 1..=10240 bytes. Keys
    /// are metadata, NOT filesystem paths; even "localfile" never invokes IO.
    pub(super) attachments:&'a[RpcProperty<'a>],
    /// Opaque string DATA. The caller cannot supply the reserved "account".
    pub(super) parameters:&'a[RpcProperty<'a>],
}
impl fmt::Debug for OutgoingMessage<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImOutgoingMessage(<redacted>)")}}

fn string(bytes:&[u8])->Result<&str,CodecError>{
    if bytes.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
    if bytes.contains(&0){return Err(CodecError::InvalidField);}
    std::str::from_utf8(bytes).map_err(|_|CodecError::InvalidField)
}
fn key(bytes:&[u8])->Result<(),CodecError>{
    string(bytes)?;
    if bytes.is_empty()||bytes.iter().any(u8::is_ascii_control){return Err(CodecError::InvalidField);}
    Ok(())
}
/// Conservative ASCII component policy: dots may separate components, but
/// leading/trailing/repeated dots and path/control characters are rejected.
fn component(bytes:&[u8])->Result<(),CodecError>{
    if bytes.len()>MAX_INFO_SUBTYPE_BYTES{return Err(CodecError::TooLarge);}
    if bytes.is_empty()||bytes[0]==b'.'||bytes[bytes.len()-1]==b'.'||bytes.windows(2).any(|p|p==b"..")
        ||!bytes.iter().all(|b|b.is_ascii_alphanumeric()||matches!(*b,b'_'|b'-'|b'.')){return Err(CodecError::InvalidField);}
    Ok(())
}
fn properties(fields:&[RpcProperty<'_>],attachments:bool,notify:bool)->Result<(),CodecError>{
    if fields.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    let mut size=4usize;
    for(index,p)in fields.iter().enumerate(){
        key(p.key)?;
        if notify&&p.key!=b"OnlineMsg"{let name=p.key.strip_prefix(b"Notify.").or_else(||p.key.strip_prefix(b"Statistics.")).ok_or(CodecError::InvalidField)?;component(name)?;}
        if attachments{
            if p.value.len()>MAX_ATTACHMENT_BYTES{return Err(CodecError::TooLarge);}
            if p.value.is_empty(){return Err(CodecError::InvalidField);}
        }else{string(p.value)?;}
        size=size.checked_add(8).and_then(|n|n.checked_add(p.key.len())).and_then(|n|n.checked_add(p.value.len())).ok_or(CodecError::TooLarge)?;
        if size>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}
        if fields[..index].iter().any(|prior|prior.key==p.key){return Err(CodecError::DuplicateKey);}
    }
    Ok(())
}
/// Count serde_json's UTF-8 string escaping BEFORE creating a content-bearing
/// allocation. Reserve once so serialization cannot discard unwiped growth
/// buffers. Serialize only the borrowed str, not a copied Value/String/DTO.
fn json_content(prefix:&[u8],text:&str)->Result<Zeroizing<Vec<u8>>,CodecError>{
    let mut size=prefix.len()+3; // string quotes and closing object brace
    for &b in text.as_bytes(){
        let escaped=match b{b'"'|b'\\'|b'\n'|b'\r'|b'\t'|8|12=>2,0..=31=>6,_=>1};
        size=size.checked_add(escaped).ok_or(CodecError::TooLarge)?;
        if size>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
    }
    let mut out=Zeroizing::new(Vec::with_capacity(MAX_FIELD_BYTES));
    out.extend_from_slice(prefix);
    // In-memory Vec sink only: no filesystem, device, SDK or transport IO.
    serde_json::to_writer(&mut *out,text).map_err(|_|CodecError::InvalidField)?;
    out.push(b'}');
    if out.len()!=size{return Err(CodecError::InvalidField);}
    Ok(out)
}

/// Original 5C347C (readonly real prologue) copies these three present string
/// properties into notification DATA. Presence includes explicit empty values.
/// 5C38F4 chooses nonempty MtcImDigestKey text, otherwise original Text content.
/// These fields are NOT account authentication, retry/IO or delivery grants.
pub(super) fn default_user_send_notifications<'a>(message:&OutgoingMessage<'a>,digest:Option<&'a[u8]>)->Result<Vec<RpcProperty<'a>>,CodecError>{
    properties(message.parameters,false,false)?;
    if let Some(value)=digest{string(value)?;if !matches!(&message.kind,SendKind::Text){return Err(CodecError::InvalidField);}}
    let mut fields=Vec::with_capacity(4);
    for(source,target)in[(b"userData".as_slice(),b"Notify.ImUserData".as_slice()),(b"OnlineMsg".as_slice(),b"OnlineMsg".as_slice()),(b"imdnId".as_slice(),b"Notify.ImdnId".as_slice())]{if let Some(value)=message.parameters.iter().find(|p|p.key==source).map(|p|p.value){fields.push(RpcProperty{key:target,value});}}
    if matches!(&message.kind,SendKind::Text){fields.push(RpcProperty{key:b"Notify.Text",value:digest.filter(|value|!value.is_empty()).unwrap_or(message.content)});}
    properties(&fields,false,true)?;Ok(fields)
}

/// Validate captured input with the original parent input bounds BEFORE a
/// journal/replay lookup. Newly derived duplicates are checked at permit encode,
/// not used to make a previously valid immutable legacy nonce unqueryable.
/// This pure DATA function never returns a send request or permission.
pub(super) fn validate_user_send_capture(message:&OutgoingMessage<'_>,notifications:Option<&[RpcProperty<'_>]>)->Result<(),CodecError>{
    let text_default=[RpcProperty{key:b"Notify.Text",value:message.content}];
    let capture:&[RpcProperty<'_>]=match notifications{Some(fields)=>fields,None=>match &message.kind{SendKind::Text=>text_default.as_slice(),SendKind::Info(_)=>&[]}};
    encode_user_send(ImBox::parse(b"P2P/0_1")?,message,b"[uid:0_1@data.invalid]",Some(capture))?;Ok(())
}

/// Exact request DATA, not a runnable RPC, identity attestation or send permit.
/// Explicit Some(fields) remains an EXACT override, including Some(&[]) which
/// suppresses defaults. None follows original present metadata and Text digest
/// rules; original Statistics.* stays in the same notification DATA map.
pub(super) fn encode_user_send(box_id:ImBox<'_>,message:&OutgoingMessage<'_>,account_uri:&[u8],notifications:Option<&[RpcProperty<'_>]>)->Result<RpcBytes,CodecError>{encode_user_send_with_digest(box_id,message,account_uri,notifications,None)}
pub(super) fn encode_user_send_with_digest(box_id:ImBox<'_>,message:&OutgoingMessage<'_>,account_uri:&[u8],notifications:Option<&[RpcProperty<'_>]>,digest:Option<&[u8]>)->Result<RpcBytes,CodecError>{
    // Only byte bounds/UTF-8/control checks, not proof of SDK ownership. Keep
    // the supplied normalized URI exact; do not infer it from any other field.
    string(account_uri)?;
    if account_uri.is_empty()||account_uri.iter().any(u8::is_ascii_control){return Err(CodecError::InvalidField);}
    let text=string(message.content)?;
    properties(message.attachments,true,false)?;properties(message.parameters,false,false)?;
    if message.parameters.iter().any(|p|p.key==b"account"){return Err(CodecError::DuplicateKey);}
    if message.parameters.len()>=MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    if let Some(fields)=notifications{properties(fields,false,true)?;}
    let mut kind=Zeroizing::new(Vec::with_capacity(5+MAX_INFO_SUBTYPE_BYTES));
    let prefix=match &message.kind{
        SendKind::Text=>{kind.extend_from_slice(b"Text");b"{\"text\":".as_slice()},
        SendKind::Info(subtype)=>{
            component(subtype)?;
            if subtype.starts_with(b"Info."){return Err(CodecError::InvalidField);}
            kind.extend_from_slice(b"Info.");kind.extend_from_slice(subtype);b"{\"content\":".as_slice()
        },
    };
    let content=json_content(prefix,text)?;
    // These vectors contain borrowed references only, not copies of secrets.
    let mut parameters=Vec::with_capacity(message.parameters.len()+1);
    parameters.extend_from_slice(message.parameters);parameters.push(RpcProperty{key:b"account",value:account_uri});
    if notifications.is_some()&&digest.is_some(){return Err(CodecError::InvalidField);}
    let defaults=if notifications.is_none(){default_user_send_notifications(message,digest)?}else{Vec::new()};
    let notify=notifications.unwrap_or(&defaults);
    let mut writer=Writer::versioned()?;writer.bytes(box_id.bytes())?;
    writer.bytes(&kind)?;writer.bytes(&content)?;
    // Both native map value types share the type0 length-prefixed wire shape.
    // Writer.properties preserves length-first then byte-order native keys.
    writer.properties(message.attachments)?;writer.properties(&parameters)?;writer.properties(notify)?;
    Ok(writer.finish())
}

/// Original DC58C0 -> DC5638 consumes the class1 reply before the generated
/// 3F8DA8 version branch. It is type0 + BE16 signed count + that many signed16
/// method versions, NOT a userSend bool/two-i64/footer result. These values
/// remain bounded DATA: no native cache mutation or automatic WRITE retry.
pub(super) fn decode_user_send_refused_versions(code:u32,bytes:&[u8])->Result<Vec<i16>,CodecError>{
    if code>>16!=1{return Err(CodecError::RemoteFailure);}
    let mut reader=Reader::new(bytes)?;
    let count=reader.u16()? as i16;
    if count<0{return Err(CodecError::InvalidField);}
    if count as usize>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    let mut versions=Vec::with_capacity(count as usize);
    for _ in 0..count{let version=reader.u16()? as i16;if version<0||versions.contains(&version){return Err(CodecError::InvalidField);}versions.push(version);}
    reader.finish()?;Ok(versions)
}
pub(super) struct UserSendResult<'a>{
    /// Parsed bool AND empty error reason only; NEVER readiness or a grant.
    pub(super) accepted:bool,
    pub(super) msg_id:i64,
    /// Raw signed64 native DATA; no inferred units or wall-clock trust.
    pub(super) time:i64,
    pub(super) footer:RpcFooter<'a>,
}
impl fmt::Debug for UserSendResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImUserSendResult(<redacted>)")}}
/// Original1AF89C/5BE7FC/5CA7CC: bool, signed64 msg_id, signed64 time,
/// Common footer and strict whole-stream consumption. False/error reason is
/// a known negative DATA result, still fully parsed; properties stay opaque.
pub(super) fn decode_user_send_result(result_code:u32,bytes:&[u8])->Result<UserSendResult<'_>,CodecError>{
    if result_code!=0{return Err(CodecError::RemoteFailure);}
    let mut reader=Reader::new(bytes)?;let accepted=reader.boolean()?;
    let msg_id=reader.i64()?;let time=reader.i64()?;
    let footer=read_footer(&mut reader)?;reader.finish()?;
    Ok(UserSendResult{accepted:accepted&&footer.reason.is_empty(),msg_id,time,footer})
}
