//! Original System.OrgRelSync DATA inside an IM record, NOT Group.Update's
//! RelationMap or local kMtcImGroupOrgRelSyncNotify varargs. 5BDC24 selects
//! P2P/0_1; 5C06D8+1C00 reads message parameters(+280), UpdateRel -> E734A0
//! (shared raw bytes, not base64) -> E86DA0 -> real 3F6BF8 single Relation.
//! 5C3D20 has the same offline-record branch. Missing UpdateRel uses original
//! 3CA950's type20/two-empty-STRING/empty-properties default, not a forged map.
use super::{group_directory_data::{GroupId,Relation},im_data::{ImBox,ImMessage},rpc_stream::{CodecError,Reader,RpcProperty,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES}};
use serde::{Deserialize,Serialize};
use std::fmt;
pub(super) const KIND:&[u8]=b"System.OrgRelSync";
pub(super) const SYSTEM_BOX:&[u8]=b"P2P/0_1";
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOrgRelationSyncConfig{pub receive:bool}
impl fmt::Debug for NativeOrgRelationSyncConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeOrgRelationSyncConfig(<redacted>)")}}
impl NativeOrgRelationSyncConfig{pub(super) fn validate(self)->Result<(),&'static str>{if self.receive{Ok(())}else{Err("app_account_org_relation_policy_invalid")}}}
pub(super) struct OrgRelationSync<'a>{pub group:GroupId<'a>,pub update_id:&'a[u8],pub base:Option<i64>,pub update:Option<i64>,pub org_deleted:bool,pub relation:Relation<'a>,pub relation_supplied:bool}
impl fmt::Debug for OrgRelationSync<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalOrgRelationSync(<redacted>)")}}
fn signed(value:&[u8])->Result<i64,CodecError>{
    if value.is_empty()||value.len()>20{return Err(CodecError::InvalidField);}let s=std::str::from_utf8(value).map_err(|_|CodecError::InvalidField)?;let v=s.parse::<i64>().map_err(|_|CodecError::InvalidField)?;if v.to_string()!=s{return Err(CodecError::InvalidField);}Ok(v)
}
/// Borrowed premeasurement of map headers. No body copy or decoder allocation.
pub(super) fn message_scratch(raw:&[u8])->Result<usize,CodecError>{
    let mut r=Reader::new(raw)?;r.i64()?;r.i64()?;r.bytes()?;r.bytes()?;r.bytes()?;let mut total=std::mem::size_of::<ImMessage<'_>>();
    for _ in 0..2{let n=r.count(8)?;total=total.checked_add(n.checked_mul(std::mem::size_of::<RpcProperty<'_>>()).ok_or(CodecError::TooLarge)?).ok_or(CodecError::TooLarge)?;for _ in 0..n{r.bytes()?;r.bytes()?;}}r.finish()?;Ok(total)
}
pub(super) fn single_scratch(raw:&[u8])->Result<usize,CodecError>{let mut r=Reader::new(raw)?;r.u32()?;r.bytes()?;r.bytes()?;let n=r.count(8)?;for _ in 0..n{r.bytes()?;r.bytes()?;}r.finish()?;n.checked_mul(std::mem::size_of::<RpcProperty<'_>>()).and_then(|n|n.checked_add(std::mem::size_of::<Relation<'_>>())).ok_or(CodecError::TooLarge)}
fn fields(fields:&[RpcProperty<'_>])->Result<(),CodecError>{
    if fields.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}let mut total=0usize;for(i,p)in fields.iter().enumerate(){if p.key.len()>MAX_FIELD_BYTES||p.value.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}total=total.checked_add(p.key.len()+p.value.len()).ok_or(CodecError::TooLarge)?;if total>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}if fields[..i].iter().any(|v|v.key==p.key){return Err(CodecError::DuplicateKey);}}Ok(())
}
pub(super) fn decode<'a>(box_id:ImBox<'_>,message:&ImMessage<'a>)->Result<Option<OrgRelationSync<'a>>,CodecError>{
    if message.kind!=KIND{return Ok(None);}if box_id.bytes()!=SYSTEM_BOX{return Err(CodecError::InvalidField);}fields(&message.parameters)?;
    let get=|k:&[u8]|message.parameters.iter().find(|p|p.key==k).map(|p|p.value);
    // Required nonempty target/label are conservative local admission. Missing
    // times remain absent DATA, not native fallback zero or cursor positions.
    let group=GroupId::parse(get(b"GroupId").ok_or(CodecError::InvalidField)?)?;
    let update_id=get(b"UpdateId").filter(|v|!v.is_empty()).ok_or(CodecError::InvalidField)?;
    let base=get(b"LastUpdateTime").filter(|v|!v.is_empty()).map(signed).transpose()?;let update=get(b"UpdateTime").filter(|v|!v.is_empty()).map(signed).transpose()?;
    let raw=get(b"UpdateRel");let relation=if let Some(raw)=raw{let mut r=Reader::new(raw)?;let flags=r.u32()?;let first=r.bytes()?;let second=r.bytes()?;let properties=r.properties()?;r.finish()?;Relation{label:update_id,flags,first,second,properties}}else{Relation{label:update_id,flags:20,first:b"",second:b"",properties:Vec::new()}};
    Ok(Some(OrgRelationSync{group,update_id,base,update,org_deleted:get(b"OrgDel")==Some(b"1".as_slice()),relation,relation_supplied:raw.is_some()}))
}
#[cfg(test)]
#[path="org_relation_sync_tests.rs"]
mod tests;
