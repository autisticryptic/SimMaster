//! Alias-only descriptor classification never excludes an original mixed WRITE.
use super::*;
#[test]
fn ue_alias_only_outer_classification_rejects_mixed_original_hosting_and_empty_poll(){
    let mut f=Case::new();let empty=f.swap();assert!(!empty.alias_only());
    let host=f.b.queue_hosting(f.owner.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();
    let wait=f.begin();let mixed=f.swap();assert!(mixed.has_business_work());assert!(!mixed.alias_only());assert!(payload(&mixed).requests.contains_key(&host));
    drop(wait);let retained=f.swap();assert!(!retained.alias_only());assert!(payload(&retained).requests.contains_key(&host));
    let mut own=Case::new();let wait=own.begin();let read=own.swap();assert!(read.alias_only());assert_eq!(read.client_sequences(),[wait.sequence()]);assert!(!read.has_business_work());
}
