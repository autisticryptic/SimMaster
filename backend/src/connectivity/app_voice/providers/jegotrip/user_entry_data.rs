//! Original UserEntry aid/UID resolver DATA, never routing/auth/write permission.
//! 51EF98 URI parser;59B480/593B38/593AEC pairset writer;59A82C/59A658
//! bool + map<string,pair<string,string>> + whole Common footer.
use super::rpc_stream::{compare_keys,read_footer,CodecError,Reader,RpcBytes,RpcFooter,Writer,MAX_FIELD_BYTES};
use std::{cmp::Ordering,fmt};
pub(super) const AID_TO_UID_METHOD:&str="aidToUid.UserEntry.User";
pub(super) const MAX_RECIPIENT_BYTES:usize=512;
pub(super) const MAX_AIDS:usize=128;
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum AidKind{Uid,Phone,Email,Username}
impl AidKind{pub(super) fn bytes(self)->&'static[u8]{match self{Self::Uid=>b"uid",Self::Phone=>b"phone",Self::Email=>b"email",Self::Username=>b"username"}}}
impl fmt::Debug for AidKind{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AidKind(<redacted>)")}}
#[derive(Clone,Copy)]
pub(super) struct RecipientUri<'a>{kind:AidKind,value:&'a[u8],authority:Option<&'a[u8]>}
impl fmt::Debug for RecipientUri<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RecipientUri(<redacted>)")}}
pub(super) fn valid_uid(value:&[u8])->bool{
    if value.len()<3||value.len()>MAX_RECIPIENT_BYTES{return false;}
    let Some(i)=value.iter().position(|b|*b==b'_')else{return false;};
    i>0&&i+1<value.len()&&value[..i].iter().all(u8::is_ascii_digit)&&(b'1'..=b'9').contains(&value[i+1])&&value[i+1..].iter().all(u8::is_ascii_digit)
}
fn valid_aid(kind:AidKind,value:&[u8])->bool{
    if value.is_empty()||value.len()>MAX_RECIPIENT_BYTES||value.iter().any(u8::is_ascii_control){return false;}
    match kind{
        AidKind::Uid=>valid_uid(value),
        AidKind::Phone=>value.len()>2&&(value[0].is_ascii_digit()||value[0]==b'+')&&value[1..].iter().all(u8::is_ascii_digit),
        AidKind::Username=>value.len()<=64&&!matches!(value[0],b'-'|b'.')&&value.iter().all(|b|b.is_ascii_alphanumeric()||matches!(*b,b'+'|b'_'|b'-'|b'.')),
        AidKind::Email=>value.len()>2&&value.iter().filter(|b|**b==b'@').count()==1&&value.split(|b|*b==b'@').nth(1).is_some_and(|domain|domain.contains(&b'.')),
    }
}
impl<'a> RecipientUri<'a>{
    pub(super) fn parse(bytes:&'a[u8])->Result<Self,CodecError>{
        if bytes.is_empty()||bytes.len()>MAX_RECIPIENT_BYTES||bytes.iter().any(u8::is_ascii_control){return Err(CodecError::InvalidField);}
        if valid_uid(bytes){return Ok(Self{kind:AidKind::Uid,value:bytes,authority:None});}
        if bytes.len()<=8||bytes[0]!=b'['||bytes[bytes.len()-1]!=b']'{return Err(CodecError::InvalidField);}
        let inner=&bytes[1..bytes.len()-1];let colon=inner.iter().position(|b|*b==b':').ok_or(CodecError::InvalidField)?;let kind=match &inner[..colon]{b"uid"=>AidKind::Uid,b"phone"=>AidKind::Phone,b"email"=>AidKind::Email,b"username"=>AidKind::Username,_=>return Err(CodecError::InvalidField)};
        let rest=&inner[colon+1..];let(value,authority)=if let Some(at)=rest.iter().rposition(|b|*b==b'@'){
            let authority=&rest[at+1..];
            // Native requires a dotted authority. Retain as DATA only; the
            // owning resolver later verifies current provider provenance.
            if authority.is_empty()||!authority.contains(&b'.')||authority.iter().any(|b|b.is_ascii_whitespace()||matches!(*b,b'['|b']'|b'{'|b'}'|b':'|b'/'|b'\\')){return Err(CodecError::InvalidField);}
            (&rest[..at],Some(authority))
        }else{if kind!=AidKind::Uid{return Err(CodecError::InvalidField);} (rest,None)};
        if !valid_aid(kind,value){return Err(CodecError::InvalidField);}
        Ok(Self{kind,value,authority})
    }
    pub(super) fn kind(self)->AidKind{self.kind}
    pub(super) fn value(self)->&'a[u8]{self.value}
    pub(super) fn authority(self)->Option<&'a[u8]>{self.authority}
    pub(super) fn aid(self)->Aid<'a>{Aid{kind:self.kind,value:self.value}}
}
#[derive(Clone,Copy)]
pub(super) struct Aid<'a>{kind:AidKind,value:&'a[u8]}
impl fmt::Debug for Aid<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("Aid(<redacted>)")}}
impl<'a>Aid<'a>{
    pub(super) fn original(kind:AidKind,value:&'a[u8])->Result<Self,CodecError>{if !valid_aid(kind,value){return Err(CodecError::InvalidField);}Ok(Self{kind,value})}
    pub(super) fn kind(self)->AidKind{self.kind}
    pub(super) fn value(self)->&'a[u8]{self.value}
}
fn compare(a:&Aid<'_>,b:&Aid<'_>)->Ordering{compare_keys(a.kind.bytes(),b.kind.bytes()).then_with(||compare_keys(a.value,b.value))}
pub(super) fn encode_aid_to_uid(aids:&[Aid<'_>])->Result<RpcBytes,CodecError>{
    if aids.is_empty()||aids.len()>MAX_AIDS{return Err(CodecError::TooLarge);}
    let mut sorted=aids.to_vec();sorted.sort_by(compare);if sorted.windows(2).any(|p|compare(&p[0],&p[1])==Ordering::Equal){return Err(CodecError::DuplicateKey);}
    let mut w=Writer::versioned()?;w.u32(sorted.len()as u32)?;
    for aid in sorted{if !valid_aid(aid.kind,aid.value){return Err(CodecError::InvalidField);}w.bytes(aid.kind.bytes())?;w.bytes(aid.value)?;}Ok(w.finish())
}
pub(super) struct AidUidEntry<'a>{pub(super) uid:&'a[u8],pub(super) aid:Aid<'a>}
impl fmt::Debug for AidUidEntry<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AidUidEntry(<redacted>)")}}
pub(super) struct AidUidResult<'a>{pub(super) accepted:bool,pub(super) entries:Vec<AidUidEntry<'a>>,pub(super) footer:RpcFooter<'a>}
impl fmt::Debug for AidUidResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("AidUidResult(<redacted>)")}}
pub(super) fn decode_aid_to_uid(code:u32,bytes:&[u8])->Result<AidUidResult<'_>,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let count=r.count(12)?;if count>MAX_AIDS{return Err(CodecError::TooLarge);}let mut entries:Vec<AidUidEntry<'_>>=Vec::with_capacity(count);
    // Outer map key is the resolved UID; its value retains the ORIGINAL AID
    // pair. Native56D940 removes matching pending pair(type,value), then puts
    // the key UID into the callback list consumed by5B671C. It is not an
    // AID-keyed map containing a guessed ('uid',value) inner pair.
    for _ in 0..count{let uid=r.bytes()?;let kind=r.bytes()?;let value=r.bytes()?;let kind=match kind{b"uid"=>AidKind::Uid,b"phone"=>AidKind::Phone,b"email"=>AidKind::Email,b"username"=>AidKind::Username,_=>return Err(CodecError::InvalidField)};if uid.len()>MAX_FIELD_BYTES||!valid_uid(uid){return Err(CodecError::InvalidField);}let aid=Aid::original(kind,value)?;if entries.iter().any(|e|e.uid==uid){return Err(CodecError::DuplicateKey);}entries.push(AidUidEntry{uid,aid});}
    let footer=read_footer(&mut r)?;r.finish()?;Ok(AidUidResult{accepted:accepted&&footer.reason.is_empty(),entries,footer})
}
