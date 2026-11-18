//! Actions-only original user-read WRITE budget, SQL journal and private result.
use super::*;
fn aux(f:&Fixture)->(Arc<n::user_entry_ownership::UserEntryBook>,n::user_entry_ownership::UserEntryLease){let b=n::user_entry_ownership::UserEntryBook::original(f.owner.clone(),f.scope.clone());let l=b.acquire(&f.owner,&f.scope,Instant::now()+Duration::from_secs(30)).unwrap();(b,l)}
#[test]
fn native_budget_user_read_capacity_failure_cannot_arm_journal_id_or_consume_the_original_write_id(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let(_book,lease)=aux(&f);lease.arm(131072-f.scope.native_rpc_resources().1).unwrap();assert!(f.backend.prepare_user_read_write(p.clone(),a.deadline()).is_err());assert_eq!(state(&f),(0,None,7));drop(lease);let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert_eq!(w.call_id,3);p.cancel();f.backend.retire_user_read_write(w.call_id);assert_eq!(state(&f),(3,Some(w.call_id),7));assert!(f.scope.is_open());
}
#[test]
fn native_budget_user_read_retained_result_never_refunds_resources_before_original_private_result_drop(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(w.call_id));assert!(p.submitted());let before=f.scope.native_rpc_resources();let r=native_reply(&mut f,w.call_id,true,0);assert_eq!(f.scope.native_rpc_resources().0,before.0);assert!(f.scope.native_rpc_resources().1>before.1);assert_eq!(p.resolve(&r).unwrap().state,"confirmed");assert_eq!(f.scope.native_rpc_resources().0,1);drop(r);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert_eq!(state(&f),(1,Some(w.call_id),7));
}
#[test]
fn native_budget_user_read_overfull_matched_result_stays_unknown_and_closes_only_original_scope(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();let w=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(w.call_id));assert!(p.submitted());let(_book,lease)=aux(&f);lease.arm(131072-f.scope.native_rpc_resources().1).unwrap();assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(w.call_id,&[0,1,0,0,0,0]),3).is_err());assert!(f.backend.take_user_read_replies().is_empty());assert_eq!(state(&f),(0,Some(w.call_id),7));assert!(!f.scope.is_open());drop(lease);
}
