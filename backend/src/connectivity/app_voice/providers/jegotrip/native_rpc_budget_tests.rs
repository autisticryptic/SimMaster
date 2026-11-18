//! Actions-only original resource accounting; no IO or SDK/WRITE authority.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
#[test]
fn native_rpc_budget_original_scope_clones_share_one_total_and_true_restart_is_independent(){
    let scope=RpcConnectionScope::fresh();let clone=scope.clone();let fresh=RpcConnectionScope::fresh();let hold=scope.reserve_native_rpc(123).unwrap();assert_eq!(clone.native_rpc_resources(),(1,123));assert_eq!(fresh.native_rpc_resources(),(0,0));assert!(hold.belongs_to(&clone));assert!(!hold.belongs_to(&fresh));drop(hold);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_rpc_budget_counts_all_pending_and_retained_holders_not_only_actor_maps(){
    let scope=RpcConnectionScope::fresh();let mut holds=Vec::new();for _ in 0..128{holds.push(scope.reserve_native_rpc(1).unwrap());}assert!(scope.reserve_native_rpc(1).is_err());assert_eq!(scope.native_rpc_resources(),(128,128));let retained=holds.pop().unwrap();let copy=retained.clone();drop(retained);assert!(scope.reserve_native_rpc(1).is_err());drop(copy);let replacement=scope.reserve_native_rpc(2).unwrap();assert_eq!(scope.native_rpc_resources(),(128,129));drop(replacement);drop(holds);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_rpc_budget_capacity_failure_and_reply_growth_are_atomic_without_budget_refund(){
    let scope=RpcConnectionScope::fresh();let hold=scope.reserve_native_rpc(65536).unwrap();let second=scope.reserve_native_rpc(65536).unwrap();assert!(hold.retain_reply(1).is_err());assert_eq!(scope.native_rpc_resources(),(2,131072));drop(second);hold.retain_reply(1).unwrap();assert_eq!(scope.native_rpc_resources(),(1,65537));assert!(hold.retain_reply(1).is_err());drop(hold);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_rpc_budget_auxiliary_count_reservation_precedes_one_request_arm_and_one_reply(){
    let scope=RpcConnectionScope::fresh();let hold=scope.reserve_native_rpc(0).unwrap();assert_eq!(scope.native_rpc_resources(),(1,0));assert!(hold.retain_reply(1).is_err());hold.arm_request(9).unwrap();assert!(hold.arm_request(9).is_err());hold.retain_reply(4).unwrap();assert_eq!(scope.native_rpc_resources(),(1,13));drop(hold);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_rpc_budget_closing_or_revoking_never_frees_retained_original_resources_or_allows_rebind(){
    for revoke in [false,true]{let scope=RpcConnectionScope::fresh();let hold=scope.reserve_native_rpc(10).unwrap();if revoke{scope.mark_session_revoked();}else{scope.close();}assert!(scope.reserve_native_rpc(1).is_err());assert_eq!(scope.native_rpc_resources(),(1,10));let fresh=RpcConnectionScope::fresh();assert!(!hold.belongs_to(&fresh));drop(hold);assert_eq!(scope.native_rpc_resources(),(0,0));}
}
#[test]
fn native_rpc_budget_auxiliary_admission_at_full_total_consumes_no_scope_or_monotonic_id(){
    let who=owner();let scope=RpcConnectionScope::fresh();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),scope.clone());let mut holds=Vec::new();for _ in 0..128{holds.push(scope.reserve_native_rpc(1).unwrap());}assert!(book.acquire(&who,&scope,std::time::Instant::now()+std::time::Duration::from_secs(1)).is_err());assert_eq!(book.active(),0);drop(holds);let lease=book.acquire(&who,&scope,std::time::Instant::now()+std::time::Duration::from_secs(1)).unwrap();assert_eq!(lease.id(),1);assert_eq!(scope.native_rpc_resources(),(1,0));drop(lease);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_rpc_budget_auxiliary_reply_keeps_charge_after_owning_io_slot_drops(){
    let who=owner();let scope=RpcConnectionScope::fresh();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),scope.clone());let lease=book.acquire(&who,&scope,std::time::Instant::now()+std::time::Duration::from_secs(1)).unwrap();lease.arm(25).unwrap();let reply=lease.retain_reply(50).unwrap();drop(lease);assert_eq!(book.active(),0);assert_eq!(book.retained_bytes(),0);assert_eq!(scope.native_rpc_resources(),(1,75));drop(reply);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_rpc_budget_private_debug_exposes_no_scope_owner_identity_or_payload(){
    let scope=RpcConnectionScope::fresh();let hold=scope.reserve_native_rpc(17).unwrap();let value=format!("{hold:?}");assert!(!value.contains("17")&&!value.contains("owner")&&!value.contains("scope"));drop(hold);
}
