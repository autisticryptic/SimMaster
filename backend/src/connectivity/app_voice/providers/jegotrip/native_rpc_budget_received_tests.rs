//! Actions-only received-position WRITE reserves shared resources before arm.
use super::*;
fn aux(f:&Fixture)->(Arc<n::user_entry_ownership::UserEntryBook>,n::user_entry_ownership::UserEntryLease){let b=n::user_entry_ownership::UserEntryBook::original(f.owner.clone(),f.scope.clone());let l=b.acquire(&f.owner,&f.scope,Instant::now()+Duration::from_secs(30)).unwrap();(b,l)}
#[test]
fn native_budget_received_capacity_refusal_is_no_io_before_original_write_id_or_permit_arm(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let(_book,lease)=aux(&f);let bytes=f.scope.native_rpc_resources().1;lease.arm(131072-bytes).unwrap();assert!(f.backend.prepare_received_write(p.clone(),Instant::now()+Duration::from_secs(30)).is_err());assert_eq!(state(&f),(0,None,7));drop(lease);let packet=write(&mut f,&p);assert_eq!(packet.call_id,4);assert_eq!(state(&f),(0,Some(4),7));p.cancel();f.backend.retire_received_write(packet.call_id);assert!(f.scope.is_open());
}
#[test]
fn native_budget_received_result_retained_outside_actor_keeps_charge_until_final_private_holder_drop(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let before=f.scope.native_rpc_resources();let r=result(&mut f,packet.call_id,true,0);assert_eq!(f.scope.native_rpc_resources().0,before.0);assert!(f.scope.native_rpc_resources().1>before.1);p.resolve_original(&r).unwrap();assert_eq!(state(&f),(1,Some(packet.call_id),7));assert_eq!(f.scope.native_rpc_resources().0,1);drop(r);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert!(f.scope.is_open());
}
#[test]
fn native_budget_received_matched_reply_over_shared_byte_bound_keeps_unknown_not_confirmed(){
    let mut f=Fixture::new(true);let s=span(&mut f,0,7,1);let p=admit(&f,&s);let packet=write(&mut f,&p);assert!(p.begin_write(packet.call_id));assert!(p.submitted());let(_book,lease)=aux(&f);lease.arm(131072-f.scope.native_rpc_resources().1).unwrap();assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(packet.call_id,&bool_result(true)),1).is_err());assert!(f.backend.take_received_replies().is_empty());assert_eq!(state(&f),(0,Some(packet.call_id),7));assert!(!f.scope.is_open());drop(lease);
}
