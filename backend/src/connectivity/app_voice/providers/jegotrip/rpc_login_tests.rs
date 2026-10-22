use super::*;

fn login_result(success: bool, user_id: &[u8], token: &[u8]) -> Vec<u8> {
    // Result streams have a type marker then fields; versions only belong to
    // method argument streams, as confirmed by native result readers.
    let mut bytes = vec![0, u8::from(success)];
    for value in [user_id, token] {
        bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
        bytes.extend_from_slice(value);
    }
    bytes.extend_from_slice(&[0; 8]); // main property map0 and empty footer string
    bytes
}

#[test]
fn login_arguments_preserve_two_native_strings_and_deployment_property_map() {
    let args = encode_login_session(b"i", b"s", &[RpcProperty { key: b"__appKey", value: b"k" }]).unwrap();
    assert_eq!(args.expose_to_transport(), &[
        0, 0, 1, 0, 0, 0, 0, 0, 1, b'i', 0, 0, 0, 1, b's',
        0, 0, 0, 1, 0, 0, 0, 8, b'_', b'_', b'a', b'p', b'p', b'K', b'e', b'y',
        0, 0, 0, 1, b'k',
    ]);
}

#[test]
fn login_result_requires_zero_code_true_boolean_and_nonempty_returned_id_and_token() {
    let bytes = login_result(true, b"actual-id", b"actual-session-token");
    let result = decode_login_session_result(0, &bytes).unwrap();
    assert_eq!(result.user_id, b"actual-id");
    assert_eq!(result.session_token, b"actual-session-token");
    assert_eq!(decode_login_session_result(1, &bytes).err(), Some(CodecError::RemoteFailure));
    assert_eq!(decode_login_session_result(0, &login_result(false, b"id", b"token")).err(), Some(CodecError::RemoteFailure));
    for (id, token) in [(b"".as_slice(), b"token".as_slice()), (b"id".as_slice(), b"".as_slice())] {
        assert_eq!(decode_login_session_result(0, &login_result(true, id, token)).err(), Some(CodecError::InvalidField));
    }
}

#[test]
fn terminal_rpc_error_prevents_use_of_returned_token_even_when_boolean_is_true() {
    let mut bytes = login_result(true, b"id", b"token");
    bytes.truncate(bytes.len() - 4);
    bytes.extend_from_slice(&[0, 0, 0, 3, b'e', b'r', b'r']);
    assert_eq!(decode_login_session_result(0, &bytes).err(), Some(CodecError::RemoteFailure));
}

#[test]
fn login_debug_redacts_returned_session_material_and_footer() {
    let bytes = login_result(true, b"fixture-secret-id", b"fixture-secret-token");
    let result = decode_login_session_result(0, &bytes).unwrap();
    assert!(!format!("{result:?}").contains("fixture"));
    let args = encode_login_session(b"fixture-secret-identity", b"fixture-native-auth", &[]).unwrap();
    assert!(!format!("{args:?}").contains("fixture"));
}

#[test]
fn login_result_truncation_wrong_stream_and_trailing_bytes_fail_closed() {
    let bytes = login_result(true, b"id", b"token");
    for length in 0..bytes.len() { assert!(decode_login_session_result(0, &bytes[..length]).is_err()); }
    let mut wrong_type = bytes.clone(); wrong_type[0] = 2;
    assert_eq!(decode_login_session_result(0, &wrong_type).err(), Some(CodecError::UnsupportedStream));
    let mut extra = bytes; extra.extend_from_slice(&[0, 0, 0, 0, 0xff]);
    assert_eq!(decode_login_session_result(0, &extra).err(), Some(CodecError::TrailingData));
}

#[test]
fn login_argument_builder_never_fills_missing_native_auth_or_deployment_identity() {
    assert_eq!(encode_login_session(b"", b"native-session-argument", &[]).err(), Some(CodecError::InvalidField));
    assert_eq!(encode_login_session(b"native-identity", b"", &[]).err(), Some(CodecError::InvalidField));
    let duplicate = [RpcProperty { key: b"DeviceId", value: b"a" }, RpcProperty { key: b"DeviceId", value: b"b" }];
    assert_eq!(encode_login_session(b"i", b"s", &duplicate).err(), Some(CodecError::DuplicateKey));
}
