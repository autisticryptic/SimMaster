//! Owner-only group creation: no owner UID, member record, realm or UserEntry
//! result is accepted from HTTP. All three action consents are explicit.
use super::*;
use super::group_management::GroupPropertyRequest;
use crate::connectivity::app_voice::providers::jegotrip::{group_management::{property,PublicProperty},rpc_stream::{MAX_FIELD_BYTES,MAX_MAP_ENTRIES}};
const INVALID:&str="app_account_group_create_invalid";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnerOnlyRequest {
    expected_revision:u64, nonce:String, name:String, group_type:u32,
    #[serde(default)] properties:Vec<GroupPropertyRequest>,
    consent:bool, write_consent:bool, session_owner_consent:bool,
    #[serde(default)] properties_read_consent:bool,
}
impl Drop for OwnerOnlyRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.nonce);zeroize::Zeroize::zeroize(&mut self.name);}}
impl OwnerOnlyRequest {
    fn scratch_bytes(&self)->Result<usize,&'static str>{
        if self.nonce.len()!=32||!self.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||self.name.is_empty()||self.name.len()>MAX_FIELD_BYTES||self.name.as_bytes().contains(&0)||self.group_type>4||self.properties.len()>MAX_MAP_ENTRIES-2{return Err(INVALID);}
        let mut size=self.name.len()+64;
        for(i,field)in self.properties.iter().enumerate(){let f=property(&field.borrowed()).map_err(|_|INVALID)?;if f.key==b"Group.Org.Name"||f.key==b"Group.Org.Type"{return Err(INVALID);}for old in &self.properties[..i]{if property(&old.borrowed()).map_err(|_|INVALID)?.key==f.key{return Err(INVALID);}}size=size.checked_add(8+f.key.len()+f.value.len()).ok_or(INVALID)?;}
        if size>MAX_FIELD_BYTES{return Err(INVALID);}Ok(self.properties.len()*std::mem::size_of::<PublicProperty<'_>>())
    }
}
pub(super) async fn create(State(manager):State<Arc<AccountManager>>,Path(line):Path<String>,request:Result<Json<OwnerOnlyRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure(INVALID);};let bytes=match request.scratch_bytes(){Ok(n)=>n,Err(e)=>return failure(e)};
    let scratch=match manager.reserve_owner_only_http_scratch(&line,request.expected_revision,bytes,request.consent,request.write_consent,request.session_owner_consent){Ok(v)=>v,Err(e)=>return failure(e)};
    let prepared={let props=request.properties.iter().map(GroupPropertyRequest::borrowed).collect::<Vec<_>>();manager.prepare_owner_only_group(&line,request.expected_revision,&request.name,request.group_type,&props,&request.nonce,request.consent,request.write_consent,request.session_owner_consent,request.properties_read_consent)}.and_then(|p|if scratch.matches_create(&p){Ok(p)}else{Err("app_account_relation_scope_invalid")});
    drop(request);drop(scratch);
    match prepared{Ok(p)=>success(StatusCode::OK,manager.finish_group_create(p).await),Err(e)=>failure(e)}
}
