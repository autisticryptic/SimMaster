//! Actions-only private cancellation provenance, never an IO/readiness grant.
use super::*;
#[test]
fn user_read_queued_retirement_never_claims_io_closure_or_changes_a_later_healthy_close(){
    let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();
    assert!(!p.closed_original_io());p.cancel();assert!(p.retired());assert!(!p.closed_original_io());assert!(f.scope.is_open());assert_eq!(state(&f),(3,None,7));
}
#[test]
fn user_read_possible_or_complete_write_retirement_retains_original_io_closure_provenance(){
    for full in [false,true]{let mut f=Fixture::new(true);let id=message_id(&mut f,1,7);let a=action(&f,id,&"a".repeat(32));let p=checked(&mut f,&a,Some(9),true).unwrap();
        let write=f.backend.prepare_user_read_write(p.clone(),a.deadline()).unwrap();assert!(p.begin_write(write.call_id));if full{assert!(p.submitted());}assert!(!p.closed_original_io());p.cancel();assert!(p.retired());assert!(p.closed_original_io());assert!(!f.scope.is_open());assert_eq!(state(&f),(0,Some(write.call_id),7));p.cancel();assert!(p.closed_original_io());
    }
}
