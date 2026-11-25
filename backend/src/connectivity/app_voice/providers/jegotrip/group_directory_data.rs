//! Original Group/Buddy directory READ DATA, not IM positions or SDK/WRITE grants.
//! 40E9B0 writes getRelations2(identifier,signed64 since); 40C590 writes
//! getPropsPrefixs(group identifier, the original one-entry empty-prefix set).
//! Relation record labels/flags/strings/properties are opaque, not routes or UIDs.
use super::rpc_stream::{read_footer,CodecError,Reader,RpcBytes,RpcFooter,RpcProperty,Writer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES};
use std::{fmt,sync::Arc};
use super::{native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope};
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
/// Private wire-shaped INPUT/READ DATA inside this crate, never a runtime grant.
pub(crate) struct Relation<'a>{pub(crate) label:&'a[u8],pub(crate) flags:u32,pub(crate) first:&'a[u8],pub(crate) second:&'a[u8],pub(crate) properties:Vec<RpcProperty<'a>>}
impl fmt::Debug for Relation<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelation(<redacted>)")}}
fn relations<'a>(r:&mut Reader<'a>)->Result<Vec<Relation<'a>>,CodecError>{
    // 3F6C64 count -> label STRING -> 3F6BF8 u32/two STRING/property map.
    let count=r.count(20)?;let mut values=Vec::with_capacity(count);
    for _ in 0..count{let label=r.bytes()?;if values.iter().any(|old:&Relation<'_>|old.label==label){return Err(CodecError::DuplicateKey);}
        values.push(Relation{label,flags:r.u32()?,first:r.bytes()?,second:r.bytes()?,properties:r.properties()?});}
    Ok(values)
}
pub(super) struct RelationsResult<'a>{pub(super) accepted:bool,pub(super) reported:bool,pub(super) first:Vec<Relation<'a>>,pub(super) second:Vec<Relation<'a>>,pub(super) names:Vec<&'a[u8]>,pub(super) time:i64,pub(super) footer:RpcFooter<'a>,_decode_budget:Option<Arc<NativeRpcReservation>>}
impl fmt::Debug for RelationsResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelationsResult(<redacted>)")}}
// The last field releases only after every decoded vector. Drop also prevents
// moving a vector out while silently discarding its original-scope reservation.
impl Drop for RelationsResult<'_>{fn drop(&mut self){}}
pub(super) fn decode_relations(code:u32,bytes:&[u8])->Result<RelationsResult<'_>,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}
    // 405C34 consumes bool, bool, two relation maps, STRING set, signed64 time,
    // then the same complete Common footer. Reported is DATA, not authorization.
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let reported=r.boolean()?;let first=relations(&mut r)?;let second=relations(&mut r)?;
    let count=r.count(4)?;let mut names=Vec::with_capacity(count);for _ in 0..count{let name=r.bytes()?;if names.contains(&name){return Err(CodecError::DuplicateKey);}names.push(name);}
    let time=r.i64()?;let footer=read_footer(&mut r)?;r.finish()?;
    Ok(RelationsResult{accepted:accepted&&footer.reason.is_empty(),reported,first,second,names,time,footer,_decode_budget:None})
}
pub(super) struct PropertiesResult<'a>{pub(super) accepted:bool,pub(super) properties:Vec<RpcProperty<'a>>,pub(super) footer:RpcFooter<'a>,_decode_budget:Option<Arc<NativeRpcReservation>>}
impl fmt::Debug for PropertiesResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalGroupPropertiesResult(<redacted>)")}}
impl Drop for PropertiesResult<'_>{fn drop(&mut self){}}
pub(super) fn decode_properties(code:u32,bytes:&[u8])->Result<PropertiesResult<'_>,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let properties=r.properties()?;let footer=read_footer(&mut r)?;r.finish()?;
    Ok(PropertiesResult{accepted:accepted&&footer.reason.is_empty(),properties,footer,_decode_budget:None})
}
// This pass only borrows stream slices. Duplicate validation remains in the
// original decoder, after the full allocation reservation. Include ignored
// maps, all nested property vectors, names and optional footer properties.
fn add_headers(size:&mut usize,count:usize,width:usize)->Result<(),CodecError>{*size=size.checked_add(count.checked_mul(width).ok_or(CodecError::TooLarge)?).ok_or(CodecError::TooLarge)?;Ok(())}
fn measure_properties(r:&mut Reader<'_>,size:&mut usize)->Result<(),CodecError>{let count=r.count(8)?;add_headers(size,count,std::mem::size_of::<RpcProperty<'_>>())?;for _ in 0..count{r.bytes()?;r.bytes()?;}Ok(())}
fn measure_footer(r:&mut Reader<'_>,size:&mut usize)->Result<(),CodecError>{r.bytes()?;if !r.is_empty(){measure_properties(r,size)?;}Ok(())}
pub(super) fn relations_decode_size(code:u32,bytes:&[u8])->Result<usize,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}let mut r=Reader::new(bytes)?;let mut size=128usize;r.boolean()?;r.boolean()?;
    for _ in 0..2{let count=r.count(20)?;add_headers(&mut size,count,std::mem::size_of::<Relation<'_>>())?;for _ in 0..count{r.bytes()?;r.u32()?;r.bytes()?;r.bytes()?;measure_properties(&mut r,&mut size)?;}}
    let count=r.count(4)?;add_headers(&mut size,count,std::mem::size_of::<&[u8]>())?;for _ in 0..count{r.bytes()?;}r.i64()?;measure_footer(&mut r,&mut size)?;r.finish()?;Ok(size)
}
pub(super) fn properties_decode_size(code:u32,bytes:&[u8])->Result<usize,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}let mut r=Reader::new(bytes)?;let mut size=128usize;r.boolean()?;measure_properties(&mut r,&mut size)?;measure_footer(&mut r,&mut size)?;r.finish()?;Ok(size)
}
pub(super) fn decode_relations_scoped<'a>(scope:&RpcConnectionScope,code:u32,bytes:&'a[u8])->Result<RelationsResult<'a>,CodecError>{
    let budget=scope.reserve_native_rpc(relations_decode_size(code,bytes)?)?;
    let mut value=decode_relations(code,bytes)?;value._decode_budget=Some(budget);Ok(value)
}
pub(super) fn decode_properties_scoped<'a>(scope:&RpcConnectionScope,code:u32,bytes:&'a[u8])->Result<PropertiesResult<'a>,CodecError>{
    let budget=scope.reserve_native_rpc(properties_decode_size(code,bytes)?)?;
    let mut value=decode_properties(code,bytes)?;value._decode_budget=Some(budget);Ok(value)
}
/// Bound result summaries only; no raw property value or reference may become a
/// view/model field, SDK authorization, receipt or Unknown WRITE resolution.
pub(super) fn record_count(value:&RelationsResult<'_>)->Result<usize,CodecError>{
    let count=value.first.len().checked_add(value.second.len()).ok_or(CodecError::TooLarge)?;
    if count>2*MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}Ok(count)
}
