//! Actions-only matched original bootstrap UserEntry reference ownership tests.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::user_entry_reference::UserEntryReference;
fn auxiliary(reference:&[u8])->(ArcRpcSession,AttemptOwner,RpcConnectionScope,OwnedBootstrapReply){
    let original=owner();let mut rpc=ArcRpcSession::new(original.clone());let scope=rpc.connection_scope();let(id,_)=rpc.prepare_bootstrap(b"fixture-appkey",&[]).unwrap();let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&[]).unwrap();w.bytes(b"AccountEntry:tcp -h fixture.invalid -p 443;").unwrap();w.bytes(b"").unwrap();w.bytes(reference).unwrap();w.bytes(b"").unwrap();let raw=encode_reply(id,0,w.finish().expose_to_transport()).unwrap();let result=match rpc.on_reply(&original,&scope,raw.expose_to_transport()).unwrap().unwrap(){ArcReply::Bootstrap(reply)=>reply,_=>panic!("matched original bootstrap")};(rpc,original,scope,result)
}
#[test]
fn user_entry_reference_comes_from_matched_auxiliary_field_not_account_entry_or_group_default(){
    let(rpc,who,scope,reply)=auxiliary(b"UserEntry:arc -h 9.1 -p 8888;");let entry=UserEntryReference::from_bootstrap(&reply,&who,&scope,None).unwrap().unwrap();assert_eq!(entry.original_call_id(),reply.call_id());assert_eq!(entry.reference_for(&who,&scope).unwrap().identity(),b"UserEntry");assert_eq!(rpc.phase(),ArcPhase::Initial);
}
#[test]
fn absent_user_entry_is_absent_not_an_invented_auxiliary_route(){
    let(_rpc,who,scope,reply)=auxiliary(b"");assert!(UserEntryReference::from_bootstrap(&reply,&who,&scope,None).unwrap().is_none());
}
#[test]
fn user_entry_wrong_owner_scope_or_retired_source_refuses_data_before_decode(){
    let(mut rpc,who,scope,reply)=auxiliary(b"ENC(unclosed");assert!(UserEntryReference::from_bootstrap(&reply,&owner(),&scope,None).is_err());assert!(UserEntryReference::from_bootstrap(&reply,&who,&RpcConnectionScope::fresh(),None).is_err());rpc.close();assert!(UserEntryReference::from_bootstrap(&reply,&who,&scope,None).is_err());
}
#[test]
fn user_entry_common_reference_key_is_required_and_never_substituted_with_client_or_sdk_key(){
    let secret=b"fixture-reference-key";let reference=fixture_envelope(b"UserEntry:arc -h 9.1 -p 8888;",secret,[7;8]);let(_rpc,who,scope,reply)=auxiliary(&reference);assert!(UserEntryReference::from_bootstrap(&reply,&who,&scope,None).is_err());let key=ConfigDecodeKey::supplied(secret).unwrap();let entry=UserEntryReference::from_bootstrap(&reply,&who,&scope,Some(&key)).unwrap().unwrap();assert_eq!(entry.reference_for(&who,&scope).unwrap().identity(),b"UserEntry");
}
#[test]
fn user_entry_qualified_reference_is_preserved_and_source_close_revokes_access_without_new_authentication(){
    let(mut rpc,who,scope,reply)=auxiliary(b"prefix/UserEntry@adapter$locator");let entry=UserEntryReference::from_bootstrap(&reply,&who,&scope,None).unwrap().unwrap();assert_eq!(entry.reference_for(&who,&scope).unwrap().identity(),b"prefix/UserEntry");assert!(entry.reference_for(&who,&scope).unwrap().direct_raw_tcp().is_err());assert!(!format!("{entry:?}").contains("prefix"));rpc.close();assert!(entry.reference_for(&who,&scope).is_none());
}
