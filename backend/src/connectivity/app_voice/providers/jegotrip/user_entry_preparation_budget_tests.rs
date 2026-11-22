//! Actions-only original action/input ownership; not a network READ or grant.
use super::*;
use super::super::super::super::user_entry_action::UserEntryAction;
use super::super::super::super::im_inbox::ImReceiptGate;
fn action(parent:RpcConnectionScope)->(Arc<UserEntryAction>,usize){
    let recipient=b"[phone:+12345@account-domain.invalid]";let bytes=recipient.len()+64;
    let input=parent.reserve_native_rpc(bytes).unwrap();
    (UserEntryAction::reserved(owner(),parent,ImReceiptGate::fresh(),std::time::Instant::now()+std::time::Duration::from_secs(1),recipient,input).unwrap(),bytes)
}
#[test]fn user_entry_prepare_input_reservation_is_original_and_survives_until_last_action_drop(){
    let parent=RpcConnectionScope::fresh();let(a,bytes)=action(parent.clone());let last=a.clone();assert_eq!(parent.native_rpc_resources(),(1,bytes));drop(a);assert_eq!(parent.native_rpc_resources(),(1,bytes));parent.close();assert_eq!(parent.native_rpc_resources(),(1,bytes));assert!(!last.live());drop(last);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]fn user_entry_prepare_foreign_budget_does_not_grant_action_or_copy_recipient(){
    let original=RpcConnectionScope::fresh();let foreign=RpcConnectionScope::fresh();let input=original.reserve_native_rpc(100).unwrap();let held=input.clone();assert!(UserEntryAction::reserved(owner(),foreign.clone(),ImReceiptGate::fresh(),std::time::Instant::now()+std::time::Duration::from_secs(1),b"7_9",input).is_err());assert_eq!(foreign.native_rpc_resources(),(0,0));assert_eq!(original.native_rpc_resources(),(1,100));drop(held);assert_eq!(original.native_rpc_resources(),(0,0));
}
#[test]fn user_entry_prepare_reserved_input_cannot_renew_expired_deadline(){
    let parent=RpcConnectionScope::fresh();let input=parent.reserve_native_rpc(100).unwrap();assert!(UserEntryAction::reserved(owner(),parent.clone(),ImReceiptGate::fresh(),std::time::Instant::now(),b"7_9",input).is_err());assert_eq!(parent.native_rpc_resources(),(0,0));assert!(parent.is_open());
}
#[test]fn user_entry_prepare_reserved_input_refuses_bad_recipient_without_new_scope(){
    let parent=RpcConnectionScope::fresh();let input=parent.reserve_native_rpc(100).unwrap();assert_eq!(UserEntryAction::reserved(owner(),parent.clone(),ImReceiptGate::fresh(),std::time::Instant::now()+std::time::Duration::from_secs(1),b"[phone:+12345@foreign]",input).err(),Some("app_account_user_entry_recipient_invalid"));assert_eq!(parent.native_rpc_resources(),(0,0));assert!(parent.is_open());
}
#[test]fn user_entry_prepare_input_gate_retirement_is_sticky_and_never_resets_budget(){
    let parent=RpcConnectionScope::fresh();let(a,bytes)=action(parent.clone());a.gate().retire();assert!(!a.live());assert_eq!(parent.native_rpc_resources(),(1,bytes));assert!(parent.is_open());drop(a);assert_eq!(parent.native_rpc_resources(),(0,0));
}
#[test]fn user_entry_prepare_input_capture_allocates_no_sdk_rpc_id_or_auxiliary_scope(){
    let(rpc,who,parent)=current_native();let id=rpc.next_id;let count=rpc.pending.len();let input=parent.reserve_native_rpc(67).unwrap();let a=UserEntryAction::reserved(who,parent.clone(),ImReceiptGate::fresh(),std::time::Instant::now()+std::time::Duration::from_secs(1),b"7_9",input).unwrap();assert_eq!(rpc.next_id,id);assert_eq!(rpc.pending.len(),count);assert!(a.live());assert_eq!(a.recipient(),b"7_9");assert!(!format!("{a:?}").contains("7_9"));assert_eq!(parent.native_rpc_resources(),(1,67));drop(a);assert_eq!(parent.native_rpc_resources(),(0,0));
}
