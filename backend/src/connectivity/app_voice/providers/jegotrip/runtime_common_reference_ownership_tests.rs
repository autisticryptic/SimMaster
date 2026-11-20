//! Actions-only Common bootstrap handoff, exact reference DATA and unchanged aggregate quotas.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
fn policy()->n::router_connector::RouterPolicy{let cert=Certificates::generate("IP:127.0.0.1");n::netstream_runtime::tests::policy(&cert.root,12345)}
fn common_source(who:&AttemptOwner,entry:&[u8],user:&[u8])->(OwnedBootstrapReply,Source){
    let mut rpc=ArcRpcSession::new(who.clone());let scope=rpc.connection_scope();let(id,_)=rpc.prepare_bootstrap(b"fixture-appkey",&[]).unwrap();let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&properties()).unwrap();w.bytes(entry).unwrap();w.bytes(b"").unwrap();w.bytes(user).unwrap();w.bytes(b"").unwrap();let response=encode_reply(id,0,w.finish().expose_to_transport()).unwrap();
    let Some(ArcReply::Bootstrap(reply))=rpc.on_reply(who,&scope,response.expose_to_transport()).unwrap()else{panic!("owned bootstrap")};(reply,Source{scope,_rpc:rpc})
}
const COMMON:&[u8]=b" prefix/AccountEntry:sarc -p 00098 -h 42.7 ";
#[test]
fn common_source_handoff_keeps_exact_account_reference_and_original_endpoint_order_after_source_close(){
    let raw=b" prefix/AccountEntry:sarc -p 00098 -h 42.7;arc -h 9.2 -p 00099 ";let who=owner();let(reply,source)=common_source(&who,raw,b"");let parent=RpcConnectionScope::fresh();let p=policy();
    let plan=ProvisionedCommonLogin::from_reply(&reply,&who,&source,&parent,&p,None,None).unwrap();source.close();let r=plan.entry_for(&who,&parent,&p).unwrap();assert_eq!(r.original_factory_input(),raw);assert_eq!(r.identity(),b"prefix/AccountEntry");
    let n::object_reference::ReferenceTarget::Direct(e)=r.target()else{panic!("direct")};assert_eq!(e.len(),2);assert_eq!((e[0].host(),e[0].port()),(b"42.7".as_slice(),98));assert_eq!((e[1].host(),e[1].port()),(b"9.2".as_slice(),99));
    assert_eq!(plan.bootstrap_call_id(),reply.call_id());assert!(plan.original_source_scope().matches(&source));assert!(plan.entry_for(&who,&parent,&policy()).is_err());assert!(plan.entry_for(&owner(),&parent,&p).is_err());parent.close();assert!(plan.entry_for(&who,&parent,&p).is_err());
}
#[test]
fn common_qualified_user_entry_snapshot_keeps_exact_raw_name_and_independent_uid_local_copy(){
    let raw=b" prefix/UserEntry:sarc -p 00098 -h 42.7 ";let who=owner();let(reply,source)=common_source(&who,COMMON,raw);let parent=RpcConnectionScope::fresh();let p=policy();let plan=ProvisionedCommonLogin::from_reply(&reply,&who,&source,&parent,&p,None,None).unwrap();source.close();
    let target=plan.user_entry_for(&who,&parent,&p).unwrap().unwrap();assert_eq!(target.reference_for(&who,&parent,&p).unwrap().original_factory_input(),raw);
    let local=plan.user_entry_uid_local_for(&who,&parent,&p).unwrap().unwrap();assert_eq!(local.reference_for(&who,&parent,&p).unwrap().original_factory_input(),raw);assert!(target.retained_bytes()>raw.len());assert!(local.retained_bytes()>raw.len());
    assert!(!format!("{plan:?}{target:?}{local:?}").contains("prefix"));assert_eq!(source._rpc.phase(),n::arc_session::ArcPhase::Closed);assert!(parent.is_open());
}
#[test]
fn common_uid_local_metadata_retains_identity_adapter_locator_or_tls_without_enabling_auxiliary_route(){
    let p=policy();for raw in [b" UserEntry ".as_slice(),b"prefix/UserEntry@adapter$locator",b"UserEntry:tls -p 00443 -h fixture.invalid"]{
        let who=owner();let(reply,source)=common_source(&who,COMMON,raw);let parent=RpcConnectionScope::fresh();let plan=ProvisionedCommonLogin::from_reply(&reply,&who,&source,&parent,&p,None,None).unwrap();source.close();
        let local=plan.user_entry_uid_local_for(&who,&parent,&p).unwrap().unwrap();assert_eq!(local.reference_for(&who,&parent,&p).unwrap().original_factory_input(),raw);
        assert_eq!(plan.user_entry_for(&who,&parent,&p).err(),Some(ProvisionError::UnsupportedReference));assert!(plan.entry_for(&who,&parent,&p).is_ok());assert!(parent.is_open());
    }
}
#[test]
fn common_optional_local_snapshot_quota_failure_does_not_change_main_bootstrap_or_claim_sdk_ready(){
    let raw=vec![b'u';44000];let who=owner();let(reply,source)=common_source(&who,COMMON,&raw);let parent=RpcConnectionScope::fresh();let p=policy();let plan=ProvisionedCommonLogin::from_reply(&reply,&who,&source,&parent,&p,None,None).unwrap();
    assert_eq!(plan.user_entry_uid_local_for(&who,&parent,&p).err(),Some(ProvisionError::ResourceLimit));assert_eq!(plan.user_entry_for(&who,&parent,&p).err(),Some(ProvisionError::UnsupportedReference));
    assert_eq!(plan.entry_for(&who,&parent,&p).unwrap().original_factory_input(),COMMON);assert_eq!(source._rpc.phase(),n::arc_session::ArcPhase::Initial);assert!(parent.is_open());
}
#[test]
fn common_combined_local_and_auxiliary_reference_storage_cannot_exceed_original_128k_budget(){
    let mut raw=vec![b'u';25000];raw.extend_from_slice(b":sarc -h 42.7 -p 98;");let who=owner();let(reply,source)=common_source(&who,COMMON,&raw);let parent=RpcConnectionScope::fresh();let p=policy();let plan=ProvisionedCommonLogin::from_reply(&reply,&who,&source,&parent,&p,None,None).unwrap();
    let local=plan.user_entry_uid_local_for(&who,&parent,&p).unwrap().unwrap();assert!(local.retained_bytes()<128*1024);assert_eq!(local.reference_for(&who,&parent,&p).unwrap().original_factory_input(),raw);
    assert_eq!(plan.user_entry_for(&who,&parent,&p).err(),Some(ProvisionError::ResourceLimit));assert!(plan.entry_for(&who,&parent,&p).is_ok());assert_eq!(source._rpc.phase(),n::arc_session::ArcPhase::Initial);
}
#[test]
fn common_exact_account_reference_still_refuses_unsupported_targets_and_bad_source_scope(){
    let p=policy();for raw in [b" AccountEntry@adapter$locator ".as_slice(),b"AccountEntry:tls -h fixture.invalid -p 443;"]{let who=owner();let(reply,source)=common_source(&who,raw,b"");assert_eq!(ProvisionedCommonLogin::from_reply(&reply,&who,&source,&RpcConnectionScope::fresh(),&p,None,None).err(),Some(ProvisionError::UnsupportedReference));}
    let who=owner();let(reply,source)=common_source(&who,COMMON,b"");assert_eq!(ProvisionedCommonLogin::from_reply(&reply,&owner(),&source,&RpcConnectionScope::fresh(),&p,None,None).err(),Some(ProvisionError::Scope));source.close();assert_eq!(ProvisionedCommonLogin::from_reply(&reply,&who,&source,&RpcConnectionScope::fresh(),&p,None,None).err(),Some(ProvisionError::Scope));
}
