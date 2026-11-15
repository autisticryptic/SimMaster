//! Actions-only replay rules for a proven consumed holder, not a new
//! consumption deadline or transferable owner/scope/call capability.
use super::*;
#[test]
fn consumed_invite_claim_only_replays_on_a_current_new_original_reply_not_an_expired_alias(){
    let(owner,scope,mut backend)=context(true);let first=incoming(&mut backend,&owner,&scope,1,ID);let expired=Instant::now()-Duration::from_millis(1);
    let claim=ConsumerClaim::fixture_consumed(&owner,&scope,ID,first.fingerprint(),expired);
    let original=first.ticket().clipped_to(expired,&claim);assert!(!original.positive_is_current());
    let extended=original.clipped_to(first.deadline()+Duration::from_secs(60),&claim);assert_eq!(extended.completion_deadline(),expired);assert!(!extended.positive_is_current());assert!(backend.complete_invite_notification(&extended,true).is_err());
    let duplicate=incoming(&mut backend,&owner,&scope,2,ID);let current=duplicate.ticket().clipped_to(duplicate.deadline(),&claim);assert_eq!(current.completion_deadline(),duplicate.deadline());assert!(current.positive_is_current());assert_eq!(direct_result(backend.complete_invite_notification(&current,true).unwrap()),[0,1,0,0,0,0]);
    assert_eq!(direct_result(backend.complete_invite_notification(&original,false).unwrap()),[0;6]);assert!(scope.is_open());
}
#[test]
fn expired_unconsumed_invite_claim_cannot_be_promoted_by_a_new_original_rpc_deadline(){
    let(owner,scope,mut backend)=context(true);let first=incoming(&mut backend,&owner,&scope,1,ID);let claim=ConsumerClaim::new(&owner,&scope,ID,first.fingerprint(),Instant::now()-Duration::from_millis(1));
    let duplicate=incoming(&mut backend,&owner,&scope,2,ID);let ticket=duplicate.ticket().clipped_to(duplicate.deadline(),&claim);assert!(!ticket.positive_is_current());assert!(backend.complete_invite_notification(&ticket,true).is_err());assert_eq!(direct_result(backend.complete_invite_notification(&ticket,false).unwrap()),[0;6]);assert!(scope.is_open());
}
#[test]
fn consumed_invite_batch_replay_keeps_its_new_ticket_deadline_and_original_ack_retirement_guard(){
    let(owner,scope,mut backend)=context(true);let first=incoming(&mut backend,&owner,&scope,1,ID);let claim=ConsumerClaim::fixture_consumed(&owner,&scope,ID,first.fingerprint(),Instant::now()-Duration::from_millis(1));
    let duplicate=batch_incoming(&mut backend,&owner,&scope);let ticket=duplicate.ticket().clipped_to(duplicate.deadline(),&claim);assert!(ticket.positive_is_current());assert!(matches!(backend.complete_invite_notification(&ticket,true).unwrap(),Some(CallCompletion::BatchQueued)));
    assert_eq!(backend.invite_write_guard().0.len(),1);assert_eq!(backend.invite_write_guard().1,Some(duplicate.deadline()));claim.cancel();assert!(!ticket.positive_is_current());assert!(backend.prepare_swap(2,&[]).is_err());assert!(!scope.is_open());
}
