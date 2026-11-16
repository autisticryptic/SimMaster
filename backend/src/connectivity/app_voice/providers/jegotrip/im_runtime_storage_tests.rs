//! Actions-only descriptor/owner checks for the unchanged original IO stack.
//! No socket, SDK/authentication or new background worker is created here.
use super::*;
#[test]
fn common_original_pre_network_engine_is_heap_owned_and_cannot_restore_auth_or_readiness(){
    let prepared=prepare_common(owner(),rpc_policy(),Instant::now()+Duration::from_secs(5)).unwrap();
    let engine:&Box<OfflineBackend>=prepared.backend.as_ref().unwrap();
    assert!(std::mem::size_of::<PreparedCommonRuntime>()<=512,"prepared descriptor must not inline the protocol/receipt engine");
    assert_eq!(engine.phase(),ArcPhase::Initial);let scope=prepared.connection_scope();assert!(engine.connection_scope().matches(&scope));
    assert!(scope.is_open());drop(prepared);assert!(!scope.is_open());
}
#[test]
fn common_engine_box_moves_to_original_actor_without_copy_rebind_or_another_scope(){
    let mut prepared=prepare_common(owner(),rpc_policy(),Instant::now()+Duration::from_secs(5)).unwrap();
    let scope=prepared.connection_scope();let original=prepared.owner.clone();
    let before=std::ptr::from_ref(prepared.backend.as_ref().unwrap().as_ref());
    let backend=prepared.backend.take().unwrap();prepared.transferred=true;drop(prepared);
    let(_command,commands)=mpsc::channel(MAX_COMMANDS);let(events,_receiver)=mpsc::channel(MAX_EVENTS);
    let actor:Actor<crate::connectivity::app_voice::providers::jegotrip::runtime_connector::BoxRuntimeIo>=Actor{
        io:None,owner:original,scope:scope.clone(),backend,tls:None,common:None,native_attempt_end:None,
        commands,events,policy:rpc_policy(),timing:None,start:Instant::now(),pending:std::collections::BTreeMap::new(),
        logout_pending:None,unhosting_pending:None,call_pending:std::collections::BTreeMap::new(),im_pending:std::collections::BTreeMap::new(),
    };
    let engine:&Box<OfflineBackend>=&actor.backend;
    assert_eq!(before,std::ptr::from_ref(engine.as_ref()));assert!(engine.connection_scope().matches(&scope));assert_eq!(engine.phase(),ArcPhase::Initial);
    assert!(scope.is_open());drop(actor);assert!(!scope.is_open());
}
