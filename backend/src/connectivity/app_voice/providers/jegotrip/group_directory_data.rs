//! Original Group/Buddy directory READ DATA, not IM positions or SDK/WRITE grants.
//! 40E9B0 writes getRelations2(identifier,signed64 since); 40C590 writes
//! getPropsPrefixs(group identifier, the original one-entry empty-prefix set).
//! Relation record labels/flags/strings/properties are opaque, not routes or UIDs.
use super::rpc_stream::{read_footer,CodecError,Reader,RpcBytes,RpcFooter,RpcProperty,Writer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES};
use std::fmt;
pub(super) const RELATIONS_METHOD:&str="getRelations2.GroupServer.Group";
pub(super) const PROPERTIES_METHOD:&str="getPropsPrefixs.GroupServer.Group";
#[derive(Clone,Copy)]
pub(super) struct GroupId<'a>{bytes:&'a[u8]}
impl fmt::Debug for GroupId<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalGroupId(<redacted>)")}}
impl<'a> GroupId<'a>{
    pub(super) fn parse(bytes:&'a[u8])->Result<Self,CodecError>{
        if bytes.len()>508{return Err(CodecError::TooLarge);}
        // Same original Org pair grammar as ImBox, validated by borrowing.
        // Validation must not copy caller bytes before native input reservation.
        let split=bytes.iter().position(|&b|b==b'-').or_else(||bytes.iter().position(|&b|b==b'+')).ok_or(CodecError::InvalidField)?;
        let(first,tail)=bytes.split_at(split);let second=&tail[1..];
        let positive=|value:&[u8]|!value.is_empty()&&value[0]!=b'0'&&value.iter().all(u8::is_ascii_digit);
        if !positive(first)||!positive(second){return Err(CodecError::InvalidField);}Ok(Self{bytes})
    }
    pub(super) fn bytes(self)->&'a[u8]{self.bytes}
}
/// The original callback selects current SDK UID when GroupRefresh has no group.
/// Runtime must obtain that UID from its matched login, not from caller data.
pub(super) fn encode_relations(identifier:&[u8],since:i64)->Result<RpcBytes,CodecError>{
    if identifier.is_empty()||identifier.len()>MAX_FIELD_BYTES||since< -1{return Err(CodecError::InvalidField);}
    let mut w=Writer::versioned()?;w.bytes(identifier)?;w.i64(since)?;Ok(w.finish())
}
pub(super) fn encode_properties(group:GroupId<'_>)->Result<RpcBytes,CodecError>{
    let mut w=Writer::versioned()?;w.bytes(group.bytes())?;w.u32(1)?;w.bytes(b"")?;Ok(w.finish())
}
pub(super) struct Relation<'a>{pub(super) label:&'a[u8],pub(super) flags:u32,pub(super) first:&'a[u8],pub(super) second:&'a[u8],pub(super) properties:Vec<RpcProperty<'a>>}
impl fmt::Debug for Relation<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelation(<redacted>)")}}
fn relations<'a>(r:&mut Reader<'a>)->Result<Vec<Relation<'a>>,CodecError>{
    // 3F6C64 count -> label STRING -> 3F6BF8 u32/two STRING/property map.
    let count=r.count(20)?;let mut values=Vec::with_capacity(count);
    for _ in 0..count{let label=r.bytes()?;if values.iter().any(|old:&Relation<'_>|old.label==label){return Err(CodecError::DuplicateKey);}
        values.push(Relation{label,flags:r.u32()?,first:r.bytes()?,second:r.bytes()?,properties:r.properties()?});}
    Ok(values)
}
pub(super) struct RelationsResult<'a>{pub(super) accepted:bool,pub(super) reported:bool,pub(super) first:Vec<Relation<'a>>,pub(super) second:Vec<Relation<'a>>,pub(super) names:Vec<&'a[u8]>,pub(super) time:i64,pub(super) footer:RpcFooter<'a>}
impl fmt::Debug for RelationsResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelationsResult(<redacted>)")}}
pub(super) fn decode_relations(code:u32,bytes:&[u8])->Result<RelationsResult<'_>,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}
    // 405C34 consumes bool, bool, two relation maps, STRING set, signed64 time,
    // then the same complete Common footer. Reported is DATA, not authorization.
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let reported=r.boolean()?;let first=relations(&mut r)?;let second=relations(&mut r)?;
    let count=r.count(4)?;let mut names=Vec::with_capacity(count);for _ in 0..count{let name=r.bytes()?;if names.contains(&name){return Err(CodecError::DuplicateKey);}names.push(name);}
    let time=r.i64()?;let footer=read_footer(&mut r)?;r.finish()?;
    Ok(RelationsResult{accepted:accepted&&footer.reason.is_empty(),reported,first,second,names,time,footer})
}
pub(super) struct PropertiesResult<'a>{pub(super) accepted:bool,pub(super) properties:Vec<RpcProperty<'a>>,pub(super) footer:RpcFooter<'a>}
impl fmt::Debug for PropertiesResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalGroupPropertiesResult(<redacted>)")}}
pub(super) fn decode_properties(code:u32,bytes:&[u8])->Result<PropertiesResult<'_>,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let properties=r.properties()?;let footer=read_footer(&mut r)?;r.finish()?;
    Ok(PropertiesResult{accepted:accepted&&footer.reason.is_empty(),properties,footer})
}
/// Bound result summaries only; no raw property value or reference may become a
/// view/model field, SDK authorization, receipt or Unknown WRITE resolution.
pub(super) fn record_count(value:&RelationsResult<'_>)->Result<usize,CodecError>{
    let count=value.first.len().checked_add(value.second.len()).ok_or(CodecError::TooLarge)?;
    if count>2*MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}Ok(count)
}
