//! Actions-only exact UID-local reference retention, never auxiliary or WRITE IO.
use super::*;
#[test]
fn uid_local_retains_exact_parsed_raw_reference_after_source_drop_without_auxiliary_scope_or_rpc_id(){
    for raw in [b" UserEntry ".as_slice(),b" prefix/UserEntry@{[a$b]}$<locator:{c$d}> ",b"UserEntry:tls -p 00443 -h fixture.invalid"]{
        let(rpc,who,parent)=current_native();let p=router();let(mut source,_,r)=reference(&who,&parent,&p,raw);let r=r.unwrap();
        let before=(rpc.next_id,rpc.pending.len());let bytes=r.retained_bytes();source.close();
        let captured=r.reference_for(&who,&parent,&p).unwrap();assert_eq!(captured.original_factory_input(),raw);assert_eq!(bytes,captured.retained_bytes());
        let a=action(&who,&parent,ImReceiptGate::fresh(),b"7_9");let local=LocalUidResult::original(a.clone(),r,rpc.user_entry_auth().unwrap(),&p).unwrap();assert_eq!(local.observe(&a,&p).unwrap().report,"local_uid");
        assert_eq!((rpc.next_id,rpc.pending.len()),before);assert_eq!(rpc.phase(),ArcPhase::Authenticated);assert!(parent.is_open());
    }
}
#[test]
fn uid_local_exact_snapshot_cannot_be_used_by_foreign_parent_owner_or_router_or_after_revoke(){
    let(_rpc,who,parent)=current_native();let p=router();let(_,_,r)=reference(&who,&parent,&p,b" UserEntry@adapter$locator ");let r=r.unwrap();
    assert!(r.reference_for(&owner(),&parent,&p).is_none());assert!(r.reference_for(&who,&RpcConnectionScope::fresh(),&p).is_none());assert!(r.reference_for(&who,&parent,&router()).is_none());
    parent.mark_session_revoked();assert!(r.reference_for(&who,&parent,&p).is_none());assert!(parent.is_open());
}
#[test]
fn uid_local_all_holders_keep_exact_reference_accounting_and_redacted_observation_without_network_result(){
    let(rpc,who,parent)=current_native();let p=router();let(_,_,r)=reference(&who,&parent,&p,b" prefix/UserEntry@adapter$locator ");let r=r.unwrap();let retained=r.retained_bytes();let other=r.clone();
    let a=action(&who,&parent,ImReceiptGate::fresh(),b"[uid:7_9@fixture.invalid]");let local=LocalUidResult::original(a.clone(),r,rpc.user_entry_auth().unwrap(),&p).unwrap();
    let output=serde_json::to_string(&local.observe(&a,&p).unwrap()).unwrap()+&format!("{local:?}");for secret in ["7_9","adapter","locator","prefix","fixture"]{assert!(!output.contains(secret));}
    drop(local);assert_eq!(other.retained_bytes(),retained);assert_eq!(other.reference_for(&who,&parent,&p).unwrap().original_factory_input(),b" prefix/UserEntry@adapter$locator ");
    parent.close();assert!(other.reference_for(&who,&parent,&p).is_none());
}
#[test]
fn uid_local_retained_adapter_or_locator_snapshot_never_promotes_alias_or_another_actions_result(){
    let(rpc,who,parent)=current_native();let p=router();let(_,_,r)=reference(&who,&parent,&p,b"UserEntry@adapter$locator");let r=r.unwrap();
    let alias=action(&who,&parent,ImReceiptGate::fresh(),b"[username:fixture@fixture.invalid]");assert!(LocalUidResult::original(alias,r.clone(),rpc.user_entry_auth().unwrap(),&p).is_err());
    let gate=ImReceiptGate::fresh();let a=action(&who,&parent,gate.clone(),b"7_9");let local=LocalUidResult::original(a.clone(),r,rpc.user_entry_auth().unwrap(),&p).unwrap();let other=action(&who,&parent,ImReceiptGate::fresh(),b"7_9");assert!(local.observe(&other,&p).is_err());gate.retire();assert!(local.observe(&a,&p).is_err());assert!(parent.is_open());
}
