//! Pure original DC5638 class1 reply DATA, never native retry/WRITE authority.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::im_send_data::decode_user_send_refused_versions;
fn refusal(versions:&[i16])->Vec<u8>{let mut out=vec![0];out.extend_from_slice(&(versions.len()as i16).to_be_bytes());for v in versions{out.extend_from_slice(&v.to_be_bytes());}out}
#[test]
fn original_version_refusal_is_short_count_short_values_not_bool_two_i64_or_footer(){
    assert_eq!(decode_user_send_refused_versions(65537,&[0,0,1,0,1]).unwrap(),vec![1]);
    assert_eq!(decode_user_send_refused_versions(65536,&refusal(&[3,1,0])).unwrap(),vec![3,1,0]);
    assert_eq!(decode_user_send_refused_versions(65537,&[0,0,0]).unwrap(),Vec::<i16>::new());
    assert!(decode_user_send_refused_versions(65537,&result(0,1,2,b"")).is_err());
    assert_eq!(decode_user_send_result(65537,&refusal(&[1])).err(),Some(CodecError::RemoteFailure));
}
#[test]
fn original_version_refusal_complete_stream_rejects_truncation_bad_tag_negative_count_and_trailing(){
    let wire=refusal(&[3,1,0]);for end in 0..wire.len(){assert!(decode_user_send_refused_versions(65537,&wire[..end]).is_err());}
    for bad in [vec![1,0,0],vec![0,255,255],vec![0,0,1,255,255],{let mut v=wire.clone();v.push(0);v}]{assert!(decode_user_send_refused_versions(65537,&bad).is_err());}
}
#[test]
fn original_version_refusal_is_bounded_and_duplicate_or_129_versions_never_mutate_native_cache(){
    let versions=(0..128).collect::<Vec<i16>>();assert_eq!(decode_user_send_refused_versions(65537,&refusal(&versions)).unwrap(),versions);
    assert_eq!(decode_user_send_refused_versions(65537,&refusal(&(0..129).collect::<Vec<i16>>())).err(),Some(CodecError::TooLarge));
    assert!(decode_user_send_refused_versions(65537,&refusal(&[1,1])).is_err());
}
#[test]
fn original_other_error_classes_are_not_relabelled_as_version_refusal_or_native_send_ack(){
    for code in [0,1,65535,131072,393216,u32::MAX]{assert_eq!(decode_user_send_refused_versions(code,&refusal(&[1])).err(),Some(CodecError::RemoteFailure));}
    let mut exception=vec![0];bytes(&mut exception,b"private-location");bytes(&mut exception,b"private-reason");
    assert!(decode_user_send_refused_versions(393216,&exception).is_err());
    assert!(decode_user_send_result(393216,&exception).is_err());
}
