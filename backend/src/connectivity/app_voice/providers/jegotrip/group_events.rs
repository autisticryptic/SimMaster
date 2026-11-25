//! Original Group.Update inbound DATA, not setRelations reply/IM content or grant.
//! Login listener 5B1B44 -> 5AAEC4/vt16E75E8 -> 563E48 reads metadata
//! Group.GroupId/Group.UpdateTime/Group.LastUpdateTime and 3F6C64 RelationMap.
//! 3F6BF8 is a REAL entry after an exception tail; original MOV/LDR/BLR
//! assembly reads u32,two STRING,properties. Type20 is retained as removal
//! DATA (55E39C), never a native role or user-authorized WRITE operation.
use super::{group_directory_data::{GroupId,Relation},rpc_stream::{CodecError,Reader,RpcProperty,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES}};
use serde::{Deserialize,Serialize};
use std::fmt;
pub(super) const GROUP_UPDATE_KIND:&[u8]=b"Group.Update";
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRelationEventsConfig{pub group_updates:bool}
impl fmt::Debug for NativeRelationEventsConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeRelationEventsConfig(<redacted>)")}}
impl NativeRelationEventsConfig{pub(super) fn validate(self)->Result<(),&'static str>{if self.group_updates{Ok(())}else{Err("app_account_relation_events_policy_invalid")}}}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum RelationEventTarget{Buddy,Group}
impl RelationEventTarget{pub(super) fn name(self)->&'static str{match self{Self::Buddy=>"buddy",Self::Group=>"group"}}}
impl fmt::Debug for RelationEventTarget{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelationEventTarget(<redacted>)")}}
pub(super) struct GroupUpdate<'a>{pub target:RelationEventTarget,pub identifier:&'a[u8],pub base_time:i64,pub update_time:i64,pub relations:Vec<Relation<'a>>}
impl fmt::Debug for GroupUpdate<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalGroupUpdate(<redacted>)")}}
fn time(bytes:&[u8])->Result<i64,CodecError>{
    if bytes.is_empty()||bytes.len()>20{return Err(CodecError::InvalidField);}
    let s=std::str::from_utf8(bytes).map_err(|_|CodecError::InvalidField)?;let value=s.parse::<i64>().map_err(|_|CodecError::InvalidField)?;
    if value.to_string()!=s{return Err(CodecError::InvalidField);}Ok(value)
}
fn metadata<'a>(header:&[RpcProperty<'a>])->Result<(),CodecError>{
    if header.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    for(i,p)in header.iter().enumerate(){if p.key.len()>MAX_FIELD_BYTES||p.value.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}if header[..i].iter().any(|q|q.key==p.key){return Err(CodecError::DuplicateKey);}}
    Ok(())
}
/// Measure borrowed decode headers before allocating new event state/vectors.
pub(super) fn relation_map_scratch(body:&[u8])->Result<usize,CodecError>{
    let mut r=Reader::new(body)?;let count=r.count(20)?;let mut bytes=count.checked_mul(std::mem::size_of::<Relation<'_>>()).ok_or(CodecError::TooLarge)?;
    for _ in 0..count{r.bytes()?;r.u32()?;r.bytes()?;r.bytes()?;let n=r.count(8)?;bytes=bytes.checked_add(n.checked_mul(std::mem::size_of::<RpcProperty<'_>>()).ok_or(CodecError::TooLarge)?).ok_or(CodecError::TooLarge)?;for _ in 0..n{r.bytes()?;r.bytes()?;}}
    r.finish()?;Ok(bytes)
}
pub(super) fn decode_relation_map(body:&[u8])->Result<Vec<Relation<'_>>,CodecError>{
    let mut reader=Reader::new(body)?;let count=reader.count(20)?;let mut relations=Vec::<Relation<'_>>::with_capacity(count);
    for _ in 0..count{let label=reader.bytes()?;if label.is_empty(){return Err(CodecError::InvalidField);}if relations.iter().any(|r|r.label==label){return Err(CodecError::DuplicateKey);}
        relations.push(Relation{label,flags:reader.u32()?,first:reader.bytes()?,second:reader.bytes()?,properties:reader.properties()?});}
    reader.finish()?;Ok(relations)
}
/// The only missing-identifier fallback is the CURRENT original SDK UID,
/// supplied by its sealed inbound binding. Metadata cannot choose another user.
pub(super) fn decode_group_update<'a>(kind:&[u8],header:&[RpcProperty<'a>],body:&'a[u8],current_uid:&'a[u8])->Result<Option<GroupUpdate<'a>>,CodecError>{
    if kind!=GROUP_UPDATE_KIND{return Ok(None);}metadata(header)?;
    let field=|k:&[u8]|header.iter().find(|p|p.key==k).map(|p|p.value);
    if current_uid.is_empty()||current_uid.len()>MAX_FIELD_BYTES{return Err(CodecError::InvalidField);}
    let identifier=field(b"Group.GroupId").unwrap_or(current_uid);
    let target=if identifier==current_uid{RelationEventTarget::Buddy}else if GroupId::parse(identifier).is_ok(){RelationEventTarget::Group}else{return Err(CodecError::InvalidField);};
    let base_time=time(field(b"Group.LastUpdateTime").ok_or(CodecError::InvalidField)?)?;
    let update_time=time(field(b"Group.UpdateTime").ok_or(CodecError::InvalidField)?)?;
    // Conservative local admission, not a proved native/server invariant.
    // The original listener carries both signed values without this comparison.
    if update_time<=base_time{return Err(CodecError::InvalidField);}
    if body.len()>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}let relations=decode_relation_map(body)?;
    Ok(Some(GroupUpdate{target,identifier,base_time,update_time,relations}))
}
#[cfg(test)]
#[path="group_events_tests.rs"]
mod tests;
