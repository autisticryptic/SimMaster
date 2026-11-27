//! Explicit owner creation DTO. No invented GroupID/current UID/callback fields.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{group_create,group_management::{PublicProperty,property},group_directory_data::Relation,group_relation_updates::native_type_from_mtc,rpc_stream::{RpcProperty,MAX_FIELD_BYTES}};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMember{uid:String,relation_type:u32,first:String,second:String,#[serde(default)]properties:Vec<ImSendDataField>}
impl Drop for CreateMember{fn drop(&mut self){for s in [&mut self.uid,&mut self.first,&mut self.second]{zeroize::Zeroize::zeroize(s);}}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupCreateRequest{expected_revision:u64,nonce:String,name:String,group_type:u32,expected_owner_uid:String,members:Vec<CreateMember>,#[serde(default)]properties:Vec<group_management::GroupPropertyRequest>,consent:bool,write_consent:bool,#[serde(default)]properties_read_consent:bool}
impl Drop for GroupCreateRequest{fn drop(&mut self){for s in [&mut self.nonce,&mut self.name,&mut self.expected_owner_uid]{zeroize::Zeroize::zeroize(s);}}}
fn size(request:&GroupCreateRequest)->Result<usize,&'static str>{
 const INVALID:&str="app_account_group_create_invalid";
 let text=|v:&str|!v.as_bytes().contains(&0)&&v.len()<=MAX_FIELD_BYTES;
 if request.nonce.len()!=32||!request.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||request.group_type>4||request.name.is_empty()||!text(&request.name)||!group_create::valid_owner(request.expected_owner_uid.as_bytes())||request.members.is_empty()||request.members.len()>128||request.properties.len()>126||request.members.iter().filter(|m|m.uid==request.expected_owner_uid).count()!=1{return Err(INVALID);}
 let mut wire=128usize+request.name.len();let mut metadata=request.members.len()*std::mem::size_of::<Relation<'_>>()+request.properties.len()*std::mem::size_of::<PublicProperty<'_>>();
 for(i,m)in request.members.iter().enumerate(){
  if !group_create::valid_member_label(m.uid.as_bytes(),request.expected_owner_uid.as_bytes())||request.members[..i].iter().any(|old|old.uid==m.uid)||!text(&m.uid)||!text(&m.first)||!text(&m.second)||m.properties.len()>128||!native_type_from_mtc(m.relation_type).is_ok_and(|flags|group_create::valid_member_type(m.uid.as_bytes(),request.expected_owner_uid.as_bytes(),flags)){return Err(INVALID);}
  wire=wire.checked_add(20+m.uid.len()+m.first.len()+m.second.len()).ok_or(INVALID)?;metadata=metadata.checked_add(m.properties.len()*std::mem::size_of::<RpcProperty<'_>>()).ok_or(INVALID)?;
  for(j,p)in m.properties.iter().enumerate(){if p.key.len()>MAX_FIELD_BYTES||p.value.len()>MAX_FIELD_BYTES||p.key.contains(&0)||p.value.contains(&0)||m.properties[..j].iter().any(|old|old.key==p.key){return Err(INVALID);}wire=wire.checked_add(8+p.key.len()+p.value.len()).ok_or(INVALID)?;}
 }
 for(i,p)in request.properties.iter().enumerate(){let p=property(&p.borrowed()).map_err(|_|INVALID)?;if p.key==b"Group.Org.Name"||p.key==b"Group.Org.Type"{return Err(INVALID);}for old in &request.properties[..i]{if property(&old.borrowed()).map_err(|_|INVALID)?.key==p.key{return Err(INVALID);}}wire=wire.checked_add(8+p.key.len()+p.value.len()).ok_or(INVALID)?;}
 if wire>MAX_FIELD_BYTES{return Err(INVALID);}Ok(metadata)
}
pub(super) async fn create_group(State(manager):State<Arc<AccountManager>>,Path(line):Path<String>,request:Result<Json<GroupCreateRequest>,JsonRejection>)->Response{
 let Ok(Json(request))=request else{return failure("app_account_group_create_invalid");};let metadata=match size(&request){Ok(n)=>n,Err(e)=>return failure(e)};
 let scratch=match manager.reserve_group_create_http_scratch(&line,request.expected_revision,metadata,request.consent,request.write_consent){Ok(s)=>s,Err(e)=>return failure(e)};
 let prepared={let members=request.members.iter().map(|m|Relation{label:m.uid.as_bytes(),flags:native_type_from_mtc(m.relation_type).expect("validated MTC enum"),first:m.first.as_bytes(),second:m.second.as_bytes(),properties:m.properties.iter().map(|p|RpcProperty{key:&p.key,value:&p.value}).collect()}).collect::<Vec<_>>();let props=request.properties.iter().map(group_management::GroupPropertyRequest::borrowed).collect::<Vec<_>>();manager.prepare_group_create(&line,request.expected_revision,&request.name,request.group_type,request.expected_owner_uid.as_bytes(),&members,&props,&request.nonce,request.consent,request.write_consent,request.properties_read_consent)}.and_then(|p|if scratch.matches_create(&p){Ok(p)}else{Err("app_account_relation_scope_invalid")});
 drop(request);drop(scratch);match prepared{Ok(p)=>success(StatusCode::OK,manager.finish_group_create(p).await),Err(e)=>failure(e)}
}
