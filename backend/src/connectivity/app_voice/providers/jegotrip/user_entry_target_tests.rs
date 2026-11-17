//! Actions-only source handoff DATA; never current SDK auth or native IO.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
fn policy()->n::router_connector::RouterPolicy{let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");n::netstream_runtime::tests::policy(&cert.root,12345)}
#[test]
fn user_entry_target_source_join_keeps_qualified_name_and_original_parent_router_data(){
    let(mut source,who,scope,reply)=auxiliary(b"prefix/UserEntry:sarc -h 42.7 -p 98;");let parent=RpcConnectionScope::fresh();let router=policy();let target=n::user_entry_target::UserEntryTarget::from_bootstrap(&reply,&who,&scope,&parent,&router,None,b"fixture.invalid",b"fixture-cc").unwrap().unwrap();source.close();assert_eq!(target.reference_for(&who,&parent,&router).unwrap().identity(),b"prefix/UserEntry");assert_eq!(target.realm_for(&who,&parent,&router),Some(b"fixture.invalid".as_slice()));assert_eq!(target.cc_for(&who,&parent,&router),Some(b"fixture-cc".as_slice()));assert_eq!(target.original_bootstrap_id(),reply.call_id());assert!(target.reference_for(&who,&parent,&policy()).is_none());assert!(target.reference_for(&owner(),&parent,&router).is_none());parent.close();assert!(target.reference_for(&who,&parent,&router).is_none());
}
#[test]
fn user_entry_target_does_not_guess_identity_adapter_locator_or_tls_as_default_group_route(){
    for bytes in [b"UserEntry".as_slice(),b"prefix/UserEntry@adapter$locator",b"UserEntry:tls -h fixture.invalid -p 443;"]{let(rpc,who,scope,reply)=auxiliary(bytes);let parent=RpcConnectionScope::fresh();assert!(n::user_entry_target::UserEntryTarget::from_bootstrap(&reply,&who,&scope,&parent,&policy(),None,b"fixture.invalid",b"").is_err());assert_eq!(rpc.phase(),ArcPhase::Initial);assert!(scope.is_open());}
}
#[test]
fn user_entry_target_foreign_or_revoked_bootstrap_cannot_mint_a_destination_reference(){
    let(rpc,who,scope,reply)=auxiliary(b"UserEntry:sarc -h 42.7 -p 98;");let parent=RpcConnectionScope::fresh();let router=policy();assert!(n::user_entry_target::UserEntryTarget::from_bootstrap(&reply,&owner(),&scope,&parent,&router,None,b"fixture.invalid",b"").is_err());scope.mark_session_revoked();assert!(n::user_entry_target::UserEntryTarget::from_bootstrap(&reply,&who,&scope,&parent,&router,None,b"fixture.invalid",b"").is_err());assert_eq!(rpc.phase(),ArcPhase::Initial);
}
