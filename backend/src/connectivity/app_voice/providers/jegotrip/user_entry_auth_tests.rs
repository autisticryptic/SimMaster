//! Actions-only original matched current SDK authentication capture (no IO).
use super::*;
#[path="user_entry_uid_local_tests.rs"]
mod uid_local;
#[path="im_send_identity_tests.rs"]
mod send_identity;
fn current_native()->(ArcRpcSession,AttemptOwner,RpcConnectionScope){
    let who=owner();let mut rpc=ArcRpcSession::new(who.clone());rpc.claim_common_transport().unwrap();rpc.bind_common_identity(b"AccountEntry").unwrap();let parent=rpc.connection_scope();let(id,_)=rpc.prepare_login(inputs(&[]),&[]).unwrap();assert!(matches!(rpc.on_reply(&who,&parent,&reply(id,&login_result(true))).unwrap(),Some(ArcReply::Login(_))));(rpc,who,parent)
}
#[test]
fn user_entry_auth_is_only_current_common_authenticated_sdk_and_allocates_no_rpc_id(){
    let(mut rpc,who,parent)=current_native();let next=rpc.next_id;let count=rpc.pending.len();let auth=rpc.user_entry_auth().unwrap();assert!(auth.matches(&who,&parent));assert_eq!(auth.uid_for(&who,&parent),Some(b"fixture-returned-user".as_slice()));assert_eq!(auth.selector_for(&who,&parent),Some(b"fixture-selected-session".as_slice()));assert_eq!(auth.token_for(&who,&parent),Some(b"fixture-returned-auth-token".as_slice()));assert_eq!(rpc.next_id,next);assert_eq!(rpc.pending.len(),count);assert_eq!(rpc.phase(),ArcPhase::Authenticated);assert!(!format!("{auth:?}").contains("fixture"));rpc.close();assert!(!auth.matches(&who,&parent));
}
#[test]
fn user_entry_auth_bootstrap_prelogin_or_legacy_login_cannot_make_native_auxiliary_auth(){
    let initial=ArcRpcSession::new(owner());assert!(initial.user_entry_auth().is_err());let(rpc,_,_)=authenticated();assert!(rpc.user_entry_auth().is_err());let(mut rpc,_,_)=current_native();rpc.prepare_logout(&[]).unwrap();assert!(rpc.user_entry_auth().is_err());
}
#[test]
fn user_entry_auth_retained_snapshot_refuses_foreign_owner_scope_and_sticky_revoke(){
    let(rpc,who,parent)=current_native();let auth=rpc.user_entry_auth().unwrap();assert!(auth.token_for(&owner(),&parent).is_none());assert!(auth.token_for(&who,&RpcConnectionScope::fresh()).is_none());parent.mark_session_revoked();assert!(parent.is_open());assert!(!auth.matches(&who,&parent));assert!(rpc.user_entry_auth().is_err());
}
