//! Explicit relation modification DTO: no current UID/token/endpoint/callback input.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{group_directory_data::Relation,group_relation_updates::{Removal,native_type_from_mtc,valid_relation_group_id},rpc_stream::{RpcProperty,MAX_FIELD_BYTES,MAX_STREAM_BYTES}};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationChangeRequest{label:String,relation_type:u32,#[serde(default)]first:String,#[serde(default)]second:String,#[serde(default)]properties:Vec<ImSendDataField>}
impl Drop for RelationChangeRequest{fn drop(&mut self){for s in [&mut self.label,&mut self.first,&mut self.second]{zeroize::Zeroize::zeroize(s);}}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationRemovalRequest{label:String,#[serde(default)]flag:bool}
impl Drop for RelationRemovalRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.label);}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationUpdateRequest{expected_revision:u64,group_id:Option<String>,nonce:String,#[serde(default)]additions:Vec<RelationChangeRequest>,#[serde(default)]updates:Vec<RelationChangeRequest>,#[serde(default)]removals:Vec<RelationRemovalRequest>,consent:bool,write_consent:bool}
impl Drop for RelationUpdateRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.nonce);if let Some(s)=&mut self.group_id{zeroize::Zeroize::zeroize(s);}}}
// Validate borrowed shape and charge every temporary Relation/property header
// before allocating those vectors. This measurement cannot supply current UID.
fn projection_size(request:&RelationUpdateRequest)->Result<usize,&'static str>{
    const INVALID:&str="app_account_relation_request_invalid";
    if request.nonce.len()!=32||!request.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||request.group_id.as_ref().is_some_and(|g|!valid_relation_group_id(g.as_bytes())){return Err(INVALID);}
    if request.additions.len()>128||request.updates.len()>128||request.removals.len()>128||request.additions.len()+request.updates.len()+request.removals.len()==0{return Err(INVALID);}
    let mut wire=21usize+request.group_id.as_ref().map_or(0,|g|g.len());let mut properties=0usize;
    let mut add=|bytes:usize|->Result<(),&'static str>{wire=wire.checked_add(bytes).ok_or(INVALID)?;if wire>MAX_STREAM_BYTES{return Err(INVALID);}Ok(())};
    for values in [&request.additions,&request.updates]{for(i,p)in values.iter().enumerate(){
        if p.label.is_empty()||p.label.len()>MAX_FIELD_BYTES||p.first.len()>MAX_FIELD_BYTES||p.second.len()>MAX_FIELD_BYTES||p.properties.len()>128||values[..i].iter().any(|old|old.label==p.label){return Err(INVALID);}
        native_type_from_mtc(p.relation_type).map_err(|_|INVALID)?;add(20+p.label.len()+p.first.len()+p.second.len())?;properties=properties.checked_add(p.properties.len()).ok_or(INVALID)?;
        for(j,v)in p.properties.iter().enumerate(){if v.key.len()>MAX_FIELD_BYTES||v.value.len()>MAX_FIELD_BYTES||p.properties[..j].iter().any(|old|old.key==v.key){return Err(INVALID);}add(8+v.key.len()+v.value.len())?;}
    }}
    for(i,p)in request.removals.iter().enumerate(){if p.label.is_empty()||p.label.len()>MAX_FIELD_BYTES||request.removals[..i].iter().any(|old|old.label==p.label){return Err(INVALID);}add(5+p.label.len())?;}
    if request.additions.iter().any(|a|request.updates.iter().any(|u|u.label==a.label)||request.removals.iter().any(|r|r.label==a.label))||request.updates.iter().any(|u|request.removals.iter().any(|r|r.label==u.label)){return Err(INVALID);}
    (request.additions.len()+request.updates.len()).checked_mul(std::mem::size_of::<Relation<'_>>()).and_then(|n|n.checked_add(request.removals.len()*std::mem::size_of::<Removal<'_>>() )).and_then(|n|n.checked_add(properties*std::mem::size_of::<RpcProperty<'_>>())).ok_or(INVALID)
}
fn changes<'a>(fields:&'a[RelationChangeRequest])->Result<Vec<Relation<'a>>,&'static str>{
    if fields.len()>128{return Err("app_account_relation_request_invalid");}
    fields.iter().map(|p|{
        if p.label.is_empty()||p.properties.len()>128{return Err("app_account_relation_request_invalid");}
        let flags=native_type_from_mtc(p.relation_type).map_err(|_|"app_account_relation_request_invalid")?;
        Ok(Relation{label:p.label.as_bytes(),flags,first:p.first.as_bytes(),second:p.second.as_bytes(),properties:p.properties.iter().map(|p|RpcProperty{key:&p.key,value:&p.value}).collect()})
    }).collect()
}
pub(super) async fn update_relations(State(manager):State<Arc<AccountManager>>,Path(line):Path<String>,request:Result<Json<RelationUpdateRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_relation_request_invalid");};
    let metadata=match projection_size(&request){Ok(n)=>n,Err(e)=>return failure(e)};
    let scratch=match manager.reserve_relation_http_scratch(&line,request.expected_revision,metadata,request.consent,request.write_consent){Ok(v)=>v,Err(e)=>return failure(e)};
    let prepared={
        let additions=match changes(&request.additions){Ok(v)=>v,Err(e)=>return failure(e)};
        let updates=match changes(&request.updates){Ok(v)=>v,Err(e)=>return failure(e)};
        let removals=request.removals.iter().map(|r|Removal{label:r.label.as_bytes(),flag:r.flag}).collect::<Vec<_>>();
        manager.prepare_relation_update(&line,request.expected_revision,request.group_id.as_deref().map(str::as_bytes),&additions,&updates,&removals,&request.nonce,request.consent,request.write_consent)
    }.and_then(|p|if scratch.matches(&p){Ok(p)}else{Err("app_account_relation_scope_invalid")});
    // Original typed body/maps and borrowed metadata were charged synchronously.
    // Wipe HTTP temporaries before controller/auth/IO awaits; no input is echoed.
    drop(request);drop(scratch);
    match prepared{Ok(p)=>success(StatusCode::OK,manager.finish_relation_update(p).await),Err(e)=>failure(e)}
}
pub(super) async fn intent_history(State(manager):State<Arc<AccountManager>>,Path(line):Path<String>,query:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(query))=query else{return failure("app_account_relation_request_invalid");};success(StatusCode::OK,manager.relation_intent_history(&line,query.limit))
}
pub(super) async fn relation_history(State(manager):State<Arc<AccountManager>>,Path(line):Path<String>,query:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(query))=query else{return failure("app_account_relation_request_invalid");};success(StatusCode::OK,manager.relation_write_history(&line,query.limit))
}
