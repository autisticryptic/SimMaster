//! Original Group/Buddy relation-change codec DATA, not a WRITE/SDK capability.
//!
//! 40E478 writes version list[0], original identifier STRING, two independent
//! STRING->Relation maps and STRING->BOOL map. 3F6F10/3F6E98 preserve each label,
//! u32 type/two STRING/property map. 405AC0 reads bool/two signed64/footer.
//! Buddy add/update select GroupRelationInfo+8/+56; remove+104 has false by
//! default (6244B0). No unknown-outcome retry or synthetic UID is performed here.
use super::{group_directory_data::Relation,
    rpc_stream::{compare_keys,read_footer,CodecError,Reader,RpcBytes,RpcFooter,Writer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES}};
use std::fmt;

pub(super) const SET_RELATIONS_METHOD:&str="setRelations.GroupServer.Group";
#[derive(Clone,Copy)]
pub(crate) struct Removal<'a>{pub label:&'a[u8],pub flag:bool}
impl fmt::Debug for Removal<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelationRemoval(<redacted>)")}}

pub(crate) fn valid_relation_group_id(bytes:&[u8])->bool{super::group_directory_data::GroupId::parse(bytes).is_ok()}
/// Native MTC API enum->wire type mapping (5B19B8), not authority or a role grant.
pub(crate) fn native_type_from_mtc(value:u32)->Result<u32,CodecError>{
    match value{16|13|14|15|11|12|18=>Ok(value),257=>Ok(0),258=>Ok(1),259=>Ok(2),4097=>Ok(10),_=>Err(CodecError::InvalidField)}
}
fn add(total:&mut usize,n:usize)->Result<(),CodecError>{
    *total=total.checked_add(n).ok_or(CodecError::TooLarge)?;
    if *total>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}Ok(())
}
fn field(total:&mut usize,value:&[u8])->Result<(),CodecError>{
    if value.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
    add(total,4)?;add(total,value.len())
}
fn relation_map_size(total:&mut usize,values:&[Relation<'_>])->Result<(),CodecError>{
    if values.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}add(total,4)?;
    for(i,v)in values.iter().enumerate(){
        if values[..i].iter().any(|p|p.label==v.label){return Err(CodecError::DuplicateKey);}
        field(total,v.label)?;add(total,4)?;field(total,v.first)?;field(total,v.second)?;
        if v.properties.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}add(total,4)?;
        for(j,p)in v.properties.iter().enumerate(){
            if v.properties[..j].iter().any(|before|before.key==p.key){return Err(CodecError::DuplicateKey);}
            field(total,p.key)?;field(total,p.value)?;
        }
    }Ok(())
}
/// Validate/measure borrowed request DATA before copying user values or allocating
/// a secret-bearing writer. Empty/cross-map data does not mint a WRITE permit.
pub(super) fn encoded_size(identifier:&[u8],additions:&[Relation<'_>],updates:&[Relation<'_>],removals:&[Removal<'_>])->Result<usize,CodecError>{
    if identifier.is_empty(){return Err(CodecError::InvalidField);}
    let mut total=5;field(&mut total,identifier)?;relation_map_size(&mut total,additions)?;relation_map_size(&mut total,updates)?;
    if removals.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}add(&mut total,4)?;
    for(i,r)in removals.iter().enumerate(){if removals[..i].iter().any(|p|p.label==r.label){return Err(CodecError::DuplicateKey);}field(&mut total,r.label)?;add(&mut total,1)?;}
    Ok(total)
}
fn write_relations(writer:&mut Writer,values:&[Relation<'_>])->Result<(),CodecError>{
    let mut ordered=values.iter().collect::<Vec<_>>();ordered.sort_by(|a,b|compare_keys(a.label,b.label));writer.u32(ordered.len() as u32)?;
    for v in ordered{writer.bytes(v.label)?;writer.u32(v.flags)?;writer.bytes(v.first)?;writer.bytes(v.second)?;writer.properties(&v.properties)?;}Ok(())
}
pub(super) fn encode_updates(identifier:&[u8],additions:&[Relation<'_>],updates:&[Relation<'_>],removals:&[Removal<'_>])->Result<RpcBytes,CodecError>{
    let capacity=encoded_size(identifier,additions,updates,removals)?;let mut writer=Writer::versioned_measured(capacity)?;
    writer.bytes(identifier)?;write_relations(&mut writer,additions)?;write_relations(&mut writer,updates)?;
    let mut ordered=removals.iter().collect::<Vec<_>>();ordered.sort_by(|a,b|compare_keys(a.label,b.label));writer.u32(ordered.len() as u32)?;
    for r in ordered{writer.bytes(r.label)?;writer.boolean(r.flag)?;}
    let bytes=writer.finish();debug_assert_eq!(bytes.expose_to_transport().len(),capacity);Ok(bytes)
}
/// Require nonoverlapping buckets for a high-level user action. The low-level
/// original codec preserves the native independent maps; server precedence is
/// not guessed, and READ snapshots cannot authorize a conflicting write batch.
pub(super) fn reject_conflicting_changes(additions:&[Relation<'_>],updates:&[Relation<'_>],removals:&[Removal<'_>])->Result<(),CodecError>{
    if additions.iter().any(|a|updates.iter().any(|u|u.label==a.label)||removals.iter().any(|r|r.label==a.label))||updates.iter().any(|u|removals.iter().any(|r|r.label==u.label)){return Err(CodecError::InvalidField);}Ok(())
}
pub(super) struct UpdateResult<'a>{pub accepted:bool,pub base_time:i64,pub update_time:i64,pub footer:RpcFooter<'a>}
impl fmt::Debug for UpdateResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalRelationUpdateResult(<redacted>)")}}
pub(super) fn decode_updates(code:u32,bytes:&[u8])->Result<UpdateResult<'_>,CodecError>{
    if code!=0{return Err(CodecError::RemoteFailure);}
    let mut reader=Reader::new(bytes)?;let accepted=reader.boolean()?;let base_time=reader.i64()?;let update_time=reader.i64()?;let footer=read_footer(&mut reader)?;reader.finish()?;
    Ok(UpdateResult{accepted:accepted&&footer.reason.is_empty(),base_time,update_time,footer})
}
#[cfg(test)]
#[path="group_relation_updates_tests.rs"]
mod tests;
