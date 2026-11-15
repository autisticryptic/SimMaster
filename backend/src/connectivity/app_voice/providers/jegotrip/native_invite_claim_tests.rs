//! Actions-only immutable consumption claim/capability alias regressions.
use super::*;
#[test]
fn cancelled_invite_claim_alias_cannot_rebind_a_fresh_component_claim(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let old=component_claim(&invite);let ticket=invite.ticket().clipped_to(invite.deadline(),&old);old.cancel();let fresh=component_claim(&invite);
    let rebound=ticket.clipped_to(invite.deadline(),&fresh);assert!(!rebound.positive_is_current());assert!(backend.complete_invite_notification(&rebound,true).is_err());assert!(scope.is_open());
}
#[test]
fn foreign_claim_owner_scope_cannot_complete_same_notification_bits_and_hash(){
    let(owner,scope,mut backend)=context(true);let invite=incoming(&mut backend,&owner,&scope,1,ID);let(foreign,foreign_scope,mut other)=context(true);let other_invite=incoming(&mut other,&foreign,&foreign_scope,1,ID);assert_eq!(invite.fingerprint(),other_invite.fingerprint());let claim=component_claim(&other_invite);
    assert!(backend.complete_invite_notification(&invite.ticket().clipped_to(invite.deadline(),&claim),true).is_err());assert!(scope.is_open());assert!(foreign_scope.is_open());
}
#[test]
fn other_notification_claim_cannot_complete_original_holder_on_same_scope(){
    let(owner,scope,mut backend)=context(true);let first=incoming(&mut backend,&owner,&scope,1,ID);let second=incoming(&mut backend,&owner,&scope,2,ID+1);let claim=component_claim(&second);
    assert!(backend.complete_invite_notification(&first.ticket().clipped_to(first.deadline(),&claim),true).is_err());assert!(scope.is_open());
}
#[test]
fn changed_opaque_values_cannot_redeem_same_notification_id_with_another_fingerprint(){
    let(owner,scope,mut backend)=context(true);let first=incoming(&mut backend,&owner,&scope,1,ID);let changed=take(backend.on_plaintext_packet(&owner,&scope,&packet(2,&args(ID,&props(b"changed",b"opaque-caller")),Some(b"fixture-auth"),b"changed"),2).unwrap());assert_ne!(first.fingerprint(),changed.fingerprint());let claim=component_claim(&changed);
    assert!(backend.complete_invite_notification(&first.ticket().clipped_to(first.deadline(),&claim),true).is_err());assert!(scope.is_open());
}
