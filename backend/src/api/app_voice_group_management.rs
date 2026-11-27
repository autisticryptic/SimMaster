//! Explicit operation-specific Group WRITE DTO, sharing original relation intent.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::group_management::{GroupWriteOperation,PublicProperty,PropertyValue};
#[derive(Deserialize)]
#[serde(untagged)]
pub enum GroupPropertyValue{Text(String),Number(u32)}
impl Drop for GroupPropertyValue{fn drop(&mut self){if let Self::Text(v)=self{zeroize::Zeroize::zeroize(v);}}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupPropertyRequest{key:String,value:GroupPropertyValue}
impl Drop for GroupPropertyRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.key);}}
impl GroupPropertyRequest{pub(super) fn borrowed(&self)->PublicProperty<'_>{PublicProperty{key:&self.key,value:match &self.value{GroupPropertyValue::Text(v)=>PropertyValue::Text(v),GroupPropertyValue::Number(n)=>PropertyValue::Number(*n)}}}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupManagementRequest{expected_revision:u64,group_id:String,operation:GroupWriteOperation,nonce:String,#[serde(default)]properties:Vec<GroupPropertyRequest>,consent:bool,write_consent:bool,#[serde(default)]destructive_consent:bool}
impl Drop for GroupManagementRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.group_id);zeroize::Zeroize::zeroize(&mut self.nonce);}}
pub(super) async fn write_group(State(manager):State<Arc<AccountManager>>,Path(line):Path<String>,request:Result<Json<GroupManagementRequest>,JsonRejection>)->Response{
 let Ok(Json(request))=request else{return failure("app_account_group_management_invalid");};if request.operation==GroupWriteOperation::Create{return failure("app_account_group_management_invalid");}if request.operation==GroupWriteOperation::CreateOwnerOnly{return failure("app_account_group_management_invalid");}
 if request.properties.len()>128||request.group_id.len()>508||request.nonce.len()!=32||!request.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||!crate::connectivity::app_voice::providers::jegotrip::group_relation_updates::valid_relation_group_id(request.group_id.as_bytes()){return failure("app_account_group_management_invalid");}
 if request.operation==GroupWriteOperation::Disband&&(!request.properties.is_empty()||!request.destructive_consent){return failure("app_account_group_management_invalid");}
 if request.operation==GroupWriteOperation::SetProperties&&request.properties.is_empty(){return failure("app_account_group_management_invalid");}
 // Validate bounded borrowed fields before reserving temporary projection headers.
 let mut total=0usize;for p in &request.properties{let v=match crate::connectivity::app_voice::providers::jegotrip::group_management::property(&p.borrowed()){Ok(v)=>v,Err(_)=>return failure("app_account_group_management_invalid")};total=match total.checked_add(v.key.len()+v.value.len()+8){Some(n)if n<=65536=>n,_=>return failure("app_account_group_management_invalid")};}
 let scratch=match manager.reserve_group_management_http_scratch(&line,request.expected_revision,request.properties.len()*std::mem::size_of::<PublicProperty<'_>>(),request.consent,request.write_consent){Ok(v)=>v,Err(e)=>return failure(e)};
 let prepared={let fields=request.properties.iter().map(GroupPropertyRequest::borrowed).collect::<Vec<_>>();manager.prepare_group_management(&line,request.expected_revision,request.group_id.as_bytes(),request.operation,&fields,&request.nonce,request.consent,request.write_consent,request.destructive_consent)}.and_then(|p|if scratch.matches(&p){Ok(p)}else{Err("app_account_relation_scope_invalid")});
 drop(request);drop(scratch);match prepared{Ok(p)=>success(StatusCode::OK,manager.finish_relation_update(p).await),Err(e)=>failure(e)}
}
