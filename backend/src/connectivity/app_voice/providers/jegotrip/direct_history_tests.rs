//! Actions-only monotonic/native-window semantics, no real time sleeps or SDK.
use super::*;
#[test]
fn completed_history_rotates_strictly_after_180_seconds_without_lifetime_128_limit() {
    let now=Instant::now();let mut history=DirectHistory::new(now);
    for id in 1i32..=600 {history.record(id,id.to_be_bytes().to_vec());}
    assert_eq!(history.count(),600);assert!(history.contains_id(1));
    history.tick(now+WINDOW).unwrap();assert!(history.contains_id(1));
    history.tick(now+WINDOW+Duration::from_millis(1)).unwrap();assert!(history.contains_id(1));
    history.record(601,601i32.to_be_bytes().to_vec());
    history.tick(now+WINDOW*2+Duration::from_millis(2)).unwrap();
    assert!(!history.contains_id(1));assert!(history.contains_id(601));assert_eq!(history.count(),1);
}
#[test]
fn duplicate_from_previous_bucket_refreshes_under_new_original_rpc_id() {
    let now=Instant::now();let mut history=DirectHistory::new(now);history.record(1,vec![0,0xff,7]);
    history.tick(now+WINDOW+Duration::from_millis(1)).unwrap();assert!(history.refresh_duplicate(2,&[0,0xff,7]));
    assert!(!history.contains_id(1));assert!(history.contains_id(2));assert_eq!(history.count(),1);
    history.tick(now+WINDOW*2+Duration::from_millis(2)).unwrap();assert!(history.contains_id(2));
    assert!(history.refresh_duplicate(3,&[0,0xff,7]));assert_eq!(history.bytes(),3);
}
#[test]
fn a_large_clock_gap_rotates_one_native_bucket_not_two_invented_catchups() {
    let now=Instant::now();let mut history=DirectHistory::new(now);history.record(1,vec![1]);
    history.tick(now+Duration::from_secs(1000)).unwrap();assert!(history.contains_id(1));
    history.tick(now+Duration::from_secs(1001)).unwrap();assert!(history.contains_id(1));
    history.tick(now+Duration::from_secs(1181)).unwrap();assert!(!history.contains_id(1));
}
#[test]
fn monotonic_regression_is_rejected_without_resetting_duplicate_identity() {
    let now=Instant::now();let mut history=DirectHistory::new(now);history.record(1,vec![0xff]);
    history.tick(now+Duration::from_secs(1)).unwrap();assert_eq!(history.tick(now).err(),Some(CodecError::InvalidField));
    assert!(history.contains_id(1));assert_eq!(history.count(),1);
}
#[test]
fn opaque_magic_values_are_not_content_or_string_normalized() {
    let now=Instant::now();let mut history=DirectHistory::new(now);
    history.record(1,vec![0,0xff]);history.record(2,vec![0xff,0]);
    assert!(!history.refresh_duplicate(3,&[0xff]));assert!(history.refresh_duplicate(3,&[0,0xff]));assert_eq!(history.count(),2);
    history.clear();assert_eq!(history.count(),0);assert_eq!(history.bytes(),0);
}
