use super::*;

fn result(success: bool, entry: &[u8], alive: &[u8], user: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0, u8::from(success)];
    bytes.extend_from_slice(&[0; 4]);
    for value in [entry, alive, user, b"".as_slice()] {
        bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
        bytes.extend_from_slice(value);
    }
    bytes
}

#[test]
fn bootstrap_request_matches_independent_native_type_version_and_key_layout() {
    let bytes = encode_get_app_configs(b"k").unwrap();
    assert_eq!(bytes.expose_to_transport(), &[0, 0, 1, 0, 0, 0, 0, 0, 1, b'k']);
    assert_eq!(encode_get_app_configs(b"").err(), Some(CodecError::InvalidField));
    assert_eq!(encode_get_app_configs(&vec![0; super::super::rpc_stream::MAX_FIELD_BYTES + 1]).err(),
        Some(CodecError::TooLarge));
}

#[test]
fn bootstrap_results_keep_all_object_references_without_granting_channel_readiness() {
    let bytes = result(true, b"entry", b"", b"\xff\x00");
    let reply = decode_get_app_configs_result(0, &bytes).unwrap();
    assert_eq!(reply.account_entry, b"entry");
    assert!(reply.account_alive.is_empty());
    assert_eq!(reply.user_entry, b"\xff\x00");
    assert!(reply.properties.is_empty());
    assert!(reply.footer.reason.is_empty());
}

#[test]
fn bootstrap_requires_external_success_true_bool_and_nonempty_account_entry() {
    let bytes = result(true, b"entry", b"alive", b"user");
    assert_eq!(decode_get_app_configs_result(1, &bytes).err(), Some(CodecError::RemoteFailure));
    assert_eq!(decode_get_app_configs_result(0, &result(false, b"entry", b"", b"")).err(),
        Some(CodecError::RemoteFailure));
    assert_eq!(decode_get_app_configs_result(0, &result(true, b"", b"alive", b"user")).err(),
        Some(CodecError::InvalidField));
    let mut bytes = bytes; bytes[1] = 0xfe;
    assert!(decode_get_app_configs_result(0, &bytes).is_ok());
}

#[test]
fn bootstrap_terminal_error_is_not_success_and_optional_footer_stays_opaque() {
    let mut bytes = result(true, b"entry", b"", b"");
    bytes.truncate(bytes.len() - 4);
    bytes.extend_from_slice(&[0, 0, 0, 1, b'e']);
    assert_eq!(decode_get_app_configs_result(0, &bytes).err(), Some(CodecError::RemoteFailure));
    let mut bytes = result(true, b"entry", b"", b"");
    bytes.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, b'p', 0, 0, 0, 2, 0xff, 0]);
    let reply = decode_get_app_configs_result(0, &bytes).unwrap();
    assert_eq!(reply.footer.properties[0].value, &[0xff, 0]);
}

#[test]
fn bootstrap_truncated_duplicate_and_trailing_data_fail_closed() {
    let bytes = result(true, b"entry", b"", b"");
    for length in 0..bytes.len() { assert!(decode_get_app_configs_result(0, &bytes[..length]).is_err()); }
    let mut extra = bytes; extra.extend_from_slice(&[0, 0, 0, 0, 1]);
    assert_eq!(decode_get_app_configs_result(0, &extra).err(), Some(CodecError::TrailingData));
    let mut duplicate = vec![0, 1, 0, 0, 0, 2];
    for _ in 0..2 { duplicate.extend_from_slice(&[0, 0, 0, 1, b'p', 0, 0, 0, 0]); }
    assert_eq!(decode_get_app_configs_result(0, &duplicate).err(), Some(CodecError::DuplicateKey));
}

#[test]
fn bootstrap_debug_does_not_reveal_app_key_or_returned_deployment() {
    let bytes = result(true, b"fixture-private-entry", b"fixture-private-alive", b"fixture-private-user");
    let reply = decode_get_app_configs_result(0, &bytes).unwrap();
    assert!(!format!("{reply:?}").contains("fixture"));
    assert!(!format!("{:?}", encode_get_app_configs(b"fixture-private-key").unwrap()).contains("fixture"));
}
