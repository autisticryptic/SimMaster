//! Original Group property/disband WRITE data. Not membership changes or grants.
//! 40CA8C: setProps version[0]/GroupID/propertymap ->405440 bool/footer.
//! 40EEB8: disbandOrganize version[0]/GroupID ->405DE8 bool/two i64/footer.
//! 171874/5C628C map public MTC property names/enums to exact original strings.
use super::{group_directory_data::GroupId,native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,
 rpc_stream::{CodecError,Reader,RpcBytes,RpcProperty,Writer,read_footer,MAX_MAP_ENTRIES,MAX_FIELD_BYTES,MAX_STREAM_BYTES}};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc};
#[derive(Clone,Copy,Debug,PartialEq,Eq,Deserialize,Serialize)]
#[serde(rename_all="snake_case")]
pub enum GroupWriteOperation{SetProperties,Disband}
impl GroupWriteOperation{
 pub(super) fn name(self)->&'static str{match self{Self::SetProperties=>"set_properties",Self::Disband=>"disband"}}
 pub(super) fn parse(s:&str)->Result<Self,&'static str>{match s{"set_properties"=>Ok(Self::SetProperties),"disband"=>Ok(Self::Disband),_=>Err("app_account_relation_storage_failed")}}
 pub(super) fn method(self)->&'static str{match self{Self::SetProperties=>"setProps.GroupServer.Group",Self::Disband=>"disbandOrganize.GroupServer.Group"}}
}
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeGroupManagementConfig{pub set_properties:bool,pub disband:bool}
impl fmt::Debug for NativeGroupManagementConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeGroupManagementConfig(<redacted>)")}}
impl NativeGroupManagementConfig{
 pub(super) fn validate(self)->Result<(),&'static str>{if self.set_properties||self.disband{Ok(())}else{Err("app_account_group_management_invalid")}}
 pub(super) fn admits(self,op:GroupWriteOperation)->bool{match op{GroupWriteOperation::SetProperties=>self.set_properties,GroupWriteOperation::Disband=>self.disband}}
}
#[derive(Clone,Copy)]pub(crate) enum PropertyValue<'a>{Text(&'a str),Number(u32)}
pub(crate) struct PublicProperty<'a>{pub key:&'a str,pub value:PropertyValue<'a>}
fn text(bytes:&[u8])->Result<(),CodecError>{if bytes.len()>MAX_FIELD_BYTES||bytes.contains(&0){Err(CodecError::InvalidField)}else{Ok(())}}
/// Borrowed conversion: no JSON cloning/vector/wire allocation before budget.
pub(crate) fn property<'a>(p:&PublicProperty<'a>)->Result<RpcProperty<'a>,CodecError>{
 text(p.key.as_bytes())?;if p.key.is_empty(){return Err(CodecError::InvalidField);}
 let map=|values:&'static[&'static[u8]]|->Result<&'static[u8],CodecError>{match p.value{PropertyValue::Number(n)=>values.get(n as usize).copied().ok_or(CodecError::InvalidField),_=>Err(CodecError::InvalidField)}};
 let(key,value)=match p.key{
 "Type"=>(b"Group.Org.Type".as_slice(),map(&[b"DiscussionGroup",b"Group",b"LargeGroup",b"Room",b"LargeRoom"])?),
 "UpdateMethod"=>(b"Group.Org.Notify".as_slice(),map(&[b"Notify",b"Refresh",b"Mixed"])?),
 "Permission"=>(b"Group.Org.JoinAuth".as_slice(),map(&[b"Manager",b"Member",b"Free",b"Pwd"])?),
 "PropertyControl"=>(b"Group.Org.PropsControl".as_slice(),map(&[b"Owner",b"Manager",b"Member"])?),
 key=>{let PropertyValue::Text(v)=p.value else{return Err(CodecError::InvalidField);};text(v.as_bytes())?;(if key=="Name"{b"Group.Org.Name"}else{key.as_bytes()},v.as_bytes())}
 };Ok(RpcProperty{key,value})
}
pub(super) fn size(group:&[u8],op:GroupWriteOperation,properties:&[PublicProperty<'_>])->Result<usize,CodecError>{
 GroupId::parse(group)?;if properties.len()>MAX_MAP_ENTRIES||match op{GroupWriteOperation::SetProperties=>properties.is_empty(),GroupWriteOperation::Disband=>!properties.is_empty()}{return Err(CodecError::InvalidField);}
 let mut size=9+group.len();if op==GroupWriteOperation::SetProperties{size+=4;}
 for(i,p)in properties.iter().enumerate(){let value=property(p)?;for other in &properties[..i]{if property(other)?.key==value.key{return Err(CodecError::DuplicateKey);}}size=size.checked_add(8+value.key.len()+value.value.len()).ok_or(CodecError::TooLarge)?;if size>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}}
 if size>MAX_STREAM_BYTES{return Err(CodecError::TooLarge);}Ok(size)
}
pub(super) fn encode(group:&[u8],op:GroupWriteOperation,properties:&[PublicProperty<'_>])->Result<RpcBytes,CodecError>{
 let size=size(group,op,properties)?;let mut w=Writer::versioned_measured(size)?;w.bytes(group)?;
 if op==GroupWriteOperation::SetProperties{let fields=properties.iter().map(property).collect::<Result<Vec<_>,_>>()?;w.properties(&fields)?;}Ok(w.finish())
}
pub(super) struct ManagementResult{pub accepted:bool,pub base:Option<i64>,pub update:Option<i64>,_budget:Arc<NativeRpcReservation>}
pub(super) fn decode(scope:&RpcConnectionScope,op:GroupWriteOperation,code:u32,bytes:&[u8])->Result<ManagementResult,CodecError>{
 if code!=0{return Err(CodecError::RemoteFailure);}let mut r=Reader::new(bytes)?;r.boolean()?;if op==GroupWriteOperation::Disband{r.i64()?;r.i64()?;}r.bytes()?;let count=if r.is_empty(){0}else{let n=r.count(8)?;for _ in 0..n{r.bytes()?;r.bytes()?;}n};r.finish()?;
 let budget=scope.reserve_native_rpc(128+count*std::mem::size_of::<RpcProperty<'_>>())?;let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let(base,update)=if op==GroupWriteOperation::Disband{(Some(r.i64()?),Some(r.i64()?))}else{(None,None)};let footer=read_footer(&mut r)?;r.finish()?;
 Ok(ManagementResult{accepted:accepted&&footer.reason.is_empty(),base,update,_budget:budget})
}
