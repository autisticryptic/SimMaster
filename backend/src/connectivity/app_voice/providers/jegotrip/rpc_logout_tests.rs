use super::*;
use crate::connectivity::app_voice::providers::jegotrip::rpc_stream::{RpcProperty, Writer};

#[test]
fn native_logout_arguments_have_type_version_count_then_two_original_strings_only() {
    let encoded=encode_logout_session(b"user",b"selector").unwrap();
    assert_eq!(encoded.expose_to_transport(),[b"\0\0\x01\0\0\0\0\0\x04user\0\0\0\x08selector".as_slice()].concat());
    let mut reader=Reader::versioned(encoded.expose_to_transport()).unwrap();assert_eq!(reader.bytes().unwrap(),b"user");assert_eq!(reader.bytes().unwrap(),b"selector");reader.finish().unwrap();
}

#[test]
fn empty_bad_control_or_unbounded_original_logout_strings_fail_before_submission() {
    for (user,selector) in [(b"".as_slice(),b"selector".as_slice()),(b"user",b""),(b"u\0",b"selector"),(b"user",b"bad\n")] {
        assert!(encode_logout_session(user,selector).is_err());
    }
    assert!(encode_logout_session(b"user",&vec![0;128]).is_err());assert!(encode_logout_session(&vec![b'u';MAX_FIELD_BYTES+1],b"selector").is_err());
}

#[test]
fn native_logout_result_zero_true_and_false_are_distinct_no_version_words_or_new_credentials() {
    assert!(decode_logout_result(0,b"\0\x01\0\0\0\0").unwrap().accepted);
    assert!(!decode_logout_result(0,b"\0\0\0\0\0\0").unwrap().accepted);
    assert!(decode_logout_result(0,b"\0\x02\0\0\0\0").unwrap().accepted);
}

#[test]
fn failed_external_result_or_nonempty_footer_never_becomes_accepted_logout() {
    assert_eq!(decode_logout_result(1,b"\0\x01\0\0\0\0").err(),Some(CodecError::RemoteFailure));
    assert_eq!(decode_logout_result(0,b"\0\x01\0\0\0\x03bad").err(),Some(CodecError::RemoteFailure));
}

#[test]
fn truncated_and_trailing_logout_result_cannot_hide_failure_in_next_method_data() {
    let valid=b"\0\x01\0\0\0\0";
    for end in 0..valid.len() { assert!(decode_logout_result(0,&valid[..end]).is_err()); }
    let extra=[valid.as_slice(),b"\0"].concat();assert!(decode_logout_result(0,&extra).is_err());
}

#[test]
fn opaque_logout_footer_properties_are_preserved_without_authorization_or_renewal_guessing() {
    let mut writer=Writer::new();writer.boolean(true).unwrap();writer.bytes(b"").unwrap();writer.properties(&[RpcProperty{key:b"private",value:b"fixture-data"}]).unwrap();let bytes=writer.finish();
    let result=decode_logout_result(0,bytes.expose_to_transport()).unwrap();assert!(result.accepted);assert_eq!(result.footer.properties[0].value,b"fixture-data");
    assert!(!format!("{result:?}").contains("fixture"));
}

#[test]
fn logout_selected_id_is_exact_not_phone_or_returned_auth_material_substitution() {
    let bytes=encode_logout_session(b"returned-user",b"original-selected").unwrap();let mut reader=Reader::versioned(bytes.expose_to_transport()).unwrap();
    assert_eq!(reader.bytes().unwrap(),b"returned-user");assert_eq!(reader.bytes().unwrap(),b"original-selected");reader.finish().unwrap();
    assert!(!format!("{bytes:?}").contains("original"));
}
