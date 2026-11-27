//! Reviewed createOrganize wire for explicitly supplied original owner/member
//! records or the separately installed empty-list/session-owner case. Mixed AID
//! normalization is unsupported: never guess owner strings or convert input into SDK auth.
use super::{group_directory_data::Relation,group_management::{PublicProperty,property,PropertyValue},
 native_rpc_budget::NativeRpcReservation,rpc_pending::RpcConnectionScope,
 rpc_stream::{CodecError,Reader,RpcBytes,RpcProperty,Writer,compare_keys,read_footer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES},user_entry_data::valid_uid};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;
pub(super) const METHOD:&str="createOrganize.GroupServer.Group";
#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeGroupCreateConfig{pub create:bool,#[serde(default)]pub read_properties:bool}
impl fmt::Debug for NativeGroupCreateConfig{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("NativeGroupCreateConfig(<redacted>)")}}
impl NativeGroupCreateConfig{pub(super) fn validate(self)->Result<(),&'static str>{if self.create{Ok(())}else{Err("app_account_group_create_invalid")}}}
fn field(bytes:&[u8])->Result<(),CodecError>{if bytes.len()>MAX_FIELD_BYTES||bytes.contains(&0){Err(CodecError::InvalidField)}else{Ok(())}}
fn type_value(group_type:u32)->Result<&'static[u8],CodecError>{Ok(property(&PublicProperty{key:"Type",value:PropertyValue::Number(group_type)})?.value)}
pub(crate) fn valid_owner(owner:&[u8])->bool{!owner.is_empty()&&owner.len()<=512&&!owner.iter().any(u8::is_ascii_control)}
pub(crate) fn valid_member_label(label:&[u8],owner:&[u8])->bool{label==owner||valid_uid(label)}
pub(super) const EXPLICIT_OWNER_TYPE:u32=0; // Local explicit-owner subset: MTC 257, never an authorization source.
pub(crate) fn valid_member_type(label:&[u8],owner:&[u8],flags:u32)->bool{matches!(flags,0..=2|10..=16|18)&&(label!=owner||flags==EXPLICIT_OWNER_TYPE)}
pub(super) fn properties_size(name:&str,group_type:u32,props:&[PublicProperty<'_>])->Result<usize,CodecError>{
 field(name.as_bytes())?;let ty=type_value(group_type)?;
 if name.is_empty()||props.len()>MAX_MAP_ENTRIES-2{return Err(CodecError::InvalidField);}
 let mut bytes=5+8+b"Group.Org.Name".len()+name.len()+8+b"Group.Org.Type".len()+ty.len();
 for(i,p)in props.iter().enumerate(){let p=property(p)?;if p.key==b"Group.Org.Name"||p.key==b"Group.Org.Type"{return Err(CodecError::InvalidField);}for old in &props[..i]{if property(old)?.key==p.key{return Err(CodecError::DuplicateKey);}}bytes=bytes.checked_add(8+p.key.len()+p.value.len()).ok_or(CodecError::TooLarge)?;}
 if bytes>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}Ok(bytes)
}
fn encode_properties(name:&str,kind:u32,props:&[PublicProperty<'_>],size:usize)->Result<Zeroizing<Vec<u8>>,CodecError>{
 let mut fields=Vec::with_capacity(props.len()+2);for p in props{fields.push(property(p)?);}fields.push(RpcProperty{key:b"Group.Org.Name",value:name.as_bytes()});fields.push(RpcProperty{key:b"Group.Org.Type",value:type_value(kind)?});let mut w=Writer::measured(size)?;w.properties(&fields)?;Ok(Zeroizing::new(w.finish().expose_to_transport().to_vec()))
}
pub(super) fn size(name:&str,group_type:u32,owner:&[u8],members:&[Relation<'_>],props:&[PublicProperty<'_>])->Result<(usize,usize),CodecError>{
 let properties=properties_size(name,group_type,props)?;
 if !valid_owner(owner)||members.is_empty()||members.len()>MAX_MAP_ENTRIES{return Err(CodecError::InvalidField);}
 // Exact owner is checked against matched native UID later, before any WRITE.
 // Only that explicit expected-owner label may be opaque; other labels are UIDs.
 if members.iter().filter(|m|m.label==owner).count()!=1{return Err(CodecError::InvalidField);}
 let mut map=5usize;
 for(i,m)in members.iter().enumerate(){
  if !valid_member_label(m.label,owner)||!valid_member_type(m.label,owner,m.flags)||members[..i].iter().any(|x|x.label==m.label)||m.properties.len()>MAX_MAP_ENTRIES{return Err(CodecError::InvalidField);}
  for b in [m.label,m.first,m.second]{field(b)?;}
  map=map.checked_add(20+m.label.len()+m.first.len()+m.second.len()).ok_or(CodecError::TooLarge)?;
  for(j,p)in m.properties.iter().enumerate(){field(p.key)?;field(p.value)?;if m.properties[..j].iter().any(|x|x.key==p.key){return Err(CodecError::DuplicateKey);}map=map.checked_add(8+p.key.len()+p.value.len()).ok_or(CodecError::TooLarge)?;}
 }
 if map.checked_add(properties).and_then(|v|v.checked_add(3)).is_none_or(|v|v>MAX_FIELD_BYTES){return Err(CodecError::TooLarge);}Ok((map,properties))
}
pub(super) struct CreateInput{pub(super) owner_only:Option<Arc<super::group_owner_only::OwnerOnlySource>>,owner:Zeroizing<Vec<u8>>,members:Zeroizing<Vec<u8>>,properties:Zeroizing<Vec<u8>>,pub(super) initial_members:usize,pub(super) property_count:usize,pub(super) read_properties:bool,fingerprint:[u8;32],_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for CreateInput{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CreateInput(<redacted>)")}}
impl CreateInput{
 pub(super) fn capture(scope:&RpcConnectionScope,name:&str,group_type:u32,owner:&[u8],members:&[Relation<'_>],props:&[PublicProperty<'_>],read_properties:bool)->Result<Arc<Self>,CodecError>{
  let(a,b)=size(name,group_type,owner,members,props)?;let headers=members.len()*std::mem::size_of::<&Relation<'_>>()+members.iter().map(|m|m.properties.len()).max().unwrap_or(0)*std::mem::size_of::<&RpcProperty<'_>>()+ (props.len()+2)*(std::mem::size_of::<RpcProperty<'_>>()+std::mem::size_of::<&RpcProperty<'_>>());
  let budget=scope.reserve_native_rpc((a+b)*2+owner.len()+std::mem::size_of::<Self>()+headers+512)?;
  let initial_members=members.len();let mut ordered=members.iter().collect::<Vec<_>>();ordered.sort_unstable_by(|a,b|compare_keys(a.label,b.label));let mut w=Writer::measured(a)?;w.u32(ordered.len()as u32)?;
  for m in ordered{w.bytes(m.label)?;w.u32(m.flags)?;w.bytes(m.first)?;w.bytes(m.second)?;w.properties(&m.properties)?;}let members=Zeroizing::new(w.finish().expose_to_transport().to_vec());
  let properties=encode_properties(name,group_type,props,b)?;
  let fingerprint=digest(&[b"original-explicit-owner-group-create-v1",owner,&members,&properties,&[u8::from(read_properties)]]);
  Ok(Arc::new(Self{owner_only:None,owner:Zeroizing::new(owner.to_vec()),initial_members,property_count:props.len()+2,members,properties,read_properties,fingerprint,_budget:budget}))
 }
 pub(super) fn belongs_to(&self,scope:&RpcConnectionScope)->bool{self._budget.belongs_to(scope)}
 pub(super) fn fingerprint(&self)->[u8;32]{self.fingerprint}
 pub(super) fn arguments_size(&self)->usize{3+self.members.len()+self.properties.len()}
 pub(super) fn operation(&self)->super::group_management::GroupWriteOperation{if self.owner_only.is_some(){super::group_management::GroupWriteOperation::CreateOwnerOnly}else{super::group_management::GroupWriteOperation::Create}}
 pub(super) fn capture_owner_only(scope:&RpcConnectionScope,owner:&super::session::AttemptOwner,source:Arc<super::group_owner_only::OwnerOnlySource>,name:&str,kind:u32,props:&[PublicProperty<'_>],read_properties:bool)->Result<Arc<Self>,CodecError>{
  if !source.current(owner,scope){return Err(CodecError::InvalidField);}let size=properties_size(name,kind,props)?;
  let budget=scope.reserve_native_rpc(size*2+std::mem::size_of::<Self>()+(props.len()+2)*(std::mem::size_of::<RpcProperty<'_>>()+std::mem::size_of::<&RpcProperty<'_>>())+256)?;
  let properties=encode_properties(name,kind,props,size)?;let fingerprint=digest(&[b"original-session-owner-only-group-create-v1",&properties,&[u8::from(read_properties)]]);
  Ok(Arc::new(Self{owner_only:Some(source),owner:Zeroizing::new(Vec::new()),members:Zeroizing::new(Vec::new()),properties,initial_members:1,property_count:props.len()+2,read_properties,fingerprint,_budget:budget}))
 }
 pub(super) fn measured_arguments(&self,uid:&[u8])->Result<usize,CodecError>{match &self.owner_only{Some(s)=>s.size(uid,&self.properties),None=>Ok(self.arguments_size())}}
 pub(super) fn arguments(&self,uid:&[u8])->Result<Zeroizing<Vec<u8>>,CodecError>{
  if let Some(source)=&self.owner_only{return source.arguments(uid,&self.properties);}
  if uid!=self.owner.as_slice(){return Err(CodecError::InvalidField);}let mut b=Zeroizing::new(Vec::with_capacity(self.arguments_size()));b.extend_from_slice(&[0,0,1,0,0]);b.extend_from_slice(&self.members[1..]);b.extend_from_slice(&self.properties[1..]);Ok(b)
 }
}
pub(super) fn digest(parts:&[&[u8]])->[u8;32]{let mut h=ring::digest::Context::new(&ring::digest::SHA256);for p in parts{h.update(&(p.len()as u64).to_be_bytes());h.update(p);}let mut out=[0;32];out.copy_from_slice(h.finish().as_ref());out}
pub(super) struct CreatedReply{pub(super) group:Zeroizing<Vec<u8>>,pub(super) relation_type:u32,pub(super) relation_properties:usize,pub(super) fingerprint:[u8;32],pub(super) accepted:bool,pub(super) base:i64,pub(super) update:i64,_budget:Arc<NativeRpcReservation>}
impl fmt::Debug for CreatedReply{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CreatedReply(<redacted>)")}}
pub(super) fn decode(scope:&RpcConnectionScope,code:u32,bytes:&[u8])->Result<Arc<CreatedReply>,CodecError>{
 if code!=0{return Err(CodecError::RemoteFailure);}let mut r=Reader::new(bytes)?;r.boolean()?;let gid=r.bytes()?;if gid.len()>508{return Err(CodecError::InvalidField);}r.u32()?;r.bytes()?;r.bytes()?;let n=r.count(8)?;for _ in 0..n{r.bytes()?;r.bytes()?;}r.i64()?;r.i64()?;r.bytes()?;let footer=if r.is_empty(){0}else{let n=r.count(8)?;for _ in 0..n{r.bytes()?;r.bytes()?;}n};r.finish()?;
 let budget=scope.reserve_native_rpc(gid.len()+(n+footer)*std::mem::size_of::<RpcProperty<'_>>()+std::mem::size_of::<CreatedReply>()+256)?;
 let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let gid=r.bytes()?;let relation_type=r.u32()?;r.bytes()?;r.bytes()?;let properties=r.properties()?;let base=r.i64()?;let update=r.i64()?;let footer=read_footer(&mut r)?;r.finish()?;let accepted=accepted&&footer.reason.is_empty();if accepted{super::group_directory_data::GroupId::parse(gid)?;}
 Ok(Arc::new(CreatedReply{group:Zeroizing::new(if accepted{gid.to_vec()}else{Vec::new()}),relation_type,relation_properties:properties.len(),fingerprint:digest(&[b"original-create-result-v1",bytes]),accepted,base,update,_budget:budget}))
}
pub(super) fn encode_property_read(group:&[u8])->Result<RpcBytes,CodecError>{super::group_directory_data::GroupId::parse(group)?;let mut w=Writer::versioned_measured(17+group.len())?;w.bytes(group)?;w.u32(1)?;w.bytes(b"")?;Ok(w.finish())}
