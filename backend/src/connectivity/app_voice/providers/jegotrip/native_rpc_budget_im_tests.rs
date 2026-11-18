//! Actions-only real original ARC protocol IM/auxiliary resource composition.
//! This uses matched auth/binding/query, not a substitute network/SQL ACK.
use super::*;
use n::user_entry_ownership::UserEntryBook;
fn book(f:&Fixture)->Arc<UserEntryBook>{UserEntryBook::original(f.owner.clone(),f.scope.clone())}
fn end()->Instant{Instant::now()+Duration::from_secs(30)}
#[test]
fn native_budget_actual_im_pending_plus_auxiliary_admission_share_exact_total_entries(){
    let mut f=Fixture::new(true);let book=book(&f);let mut reads=Vec::new();for _ in 0..124{reads.push(request(&mut f,refresh()));}let mut leases=Vec::new();for _ in 0..4{let lease=book.acquire(&f.owner,&f.scope,end()).unwrap();lease.arm(1).unwrap();leases.push(lease);}assert_eq!(f.scope.native_rpc_resources().0,128);assert!(f.backend.prepare_im_read(refresh(),f.gate.clone(),end()).is_err());assert!(f.scope.is_open());let removed=reads.pop().unwrap();f.backend.retire_im_read(removed.call_id);assert_eq!(f.scope.native_rpc_resources().0,127);let next=request(&mut f,refresh());assert_eq!(next.call_id,126,"denied shared admission cannot consume original IM ID");assert_eq!(f.scope.native_rpc_resources().0,128);for p in reads{f.backend.retire_im_read(p.call_id);}f.backend.retire_im_read(next.call_id);drop(leases);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert_eq!(f.count(),0);
}
#[test]
fn native_budget_actual_matched_im_reply_retained_by_caller_cannot_be_evicted_to_admit_auxiliary(){
    let mut f=Fixture::new(true);let first=request(&mut f,refresh());reply(&mut f,&first,&refresh_result(1,true),1);let held=f.backend.take_im_replies();let book=book(&f);let mut reads=Vec::new();for _ in 0..127{reads.push(request(&mut f,refresh()));}assert_eq!(f.scope.native_rpc_resources().0,128);assert!(book.acquire(&f.owner,&f.scope,end()).is_err());assert_eq!(book.active(),0);drop(held);let lease=book.acquire(&f.owner,&f.scope,end()).unwrap();assert_eq!(lease.id(),1);assert_eq!(f.scope.native_rpc_resources().0,128);for p in reads{f.backend.retire_im_read(p.call_id);}drop(lease);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_budget_full_auxiliary_bytes_deny_original_im_before_id_then_restore_on_owner_drop(){
    let mut f=Fixture::new(true);let book=book(&f);let lease=book.acquire(&f.owner,&f.scope,end()).unwrap();lease.arm(131072).unwrap();assert!(f.backend.prepare_im_read(refresh(),f.gate.clone(),end()).is_err());assert_eq!(f.scope.native_rpc_resources(),(1,131072));assert!(f.scope.is_open());drop(lease);let p=request(&mut f,refresh());assert_eq!(p.call_id,2);f.backend.retire_im_read(p.call_id);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]
fn native_budget_actual_im_reply_growth_failure_suppresses_result_without_forged_sql_or_ready(){
    let mut f=Fixture::new(true);let p=request(&mut f,refresh());let before=f.scope.native_rpc_resources();let book=book(&f);let lease=book.acquire(&f.owner,&f.scope,end()).unwrap();lease.arm(131072-before.1).unwrap();assert_eq!(f.scope.native_rpc_resources().1,131072);let raw=reply_packet(p.call_id,&refresh_result(1,true));assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&raw,1).is_err());assert!(f.backend.take_im_replies().is_empty());assert!(!f.scope.is_open());assert_eq!(f.count(),0);drop(lease);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
