use super::*;

fn credentials<'a>() -> GatewayRegistration<'a> {
    GatewayRegistration { authname_prefix: "prefix", channel_id: "channel", delete_int_code: "1",
        sip_number: "number", sip_password: "password" }
}

fn read_string<'a>(input: &mut &'a [u8]) -> &'a str {
    let length = u32::from_be_bytes(input[..4].try_into().unwrap()) as usize;
    let value = std::str::from_utf8(&input[4..4 + length]).unwrap();
    *input = &input[4 + length..];
    value
}

#[test]
fn primitive_prefix_includes_stream_type_then_be16_versions_and_be32_byte_length() {
    let result = encode_update_status("sdk-user", STATUS_REGISTER, &credentials()).unwrap();
    // Independently specified from the native constructor + primitive writers.
    assert_eq!(&result.expose_to_transport()[..9], &[0, 0, 1, 0, 0, 0, 0, 0, 8]);
    assert_eq!(&result.expose_to_transport()[9..17], b"sdk-user");
    assert_eq!(&result.expose_to_transport()[17..21], &[0, 0, 0, 6]);
}

#[test]
fn string_map_uses_length_then_bytes_not_lexical_order() {
    let args = encode_update_status("uri", STATUS_REGISTER, &credentials()).unwrap();
    let mut input = &args.expose_to_transport()[5..];
    assert_eq!(read_string(&mut input), "uri");
    input = &input[4..];
    let mut keys = Vec::new();
    let mut values = Vec::new();
    while !input.is_empty() {
        keys.push(read_string(&mut input));
        values.push(read_string(&mut input));
    }
    assert_eq!(keys, ["SipPwd", "SipFlag", "ChannelId", "SipNumber", "DeleteIntCode", "AuthnamePrefix"]);
    assert_eq!(values, ["password", "4", "channel", "number", "1", "prefix"]);
}

#[test]
fn unregister_is_the_same_rpc_with_string_sipflag_three() {
    let args = encode_update_status("uri", STATUS_UNREGISTER, &credentials()).unwrap();
    let mut input = &args.expose_to_transport()[5..];
    read_string(&mut input); input = &input[4..];
    let mut flag = None;
    while !input.is_empty() {
        let key = read_string(&mut input);
        let value = read_string(&mut input);
        if key == "SipFlag" { flag = Some(value); }
    }
    assert_eq!(flag, Some("3"));
}

#[test]
fn utf8_length_is_bytes_not_unicode_character_count() {
    let args = encode_update_status("甲", STATUS_REGISTER, &credentials()).unwrap();
    assert_eq!(&args.expose_to_transport()[5..9], &[0, 0, 0, 3]);
    assert_eq!(&args.expose_to_transport()[9..12], &[0xe7, 0x94, 0xb2]);
}

#[test]
fn missing_sdk_uri_or_embedded_nul_are_rejected() {
    assert_eq!(encode_update_status("", STATUS_REGISTER, &credentials()).err(), Some("jegotrip_sdk_uri_required"));
    assert_eq!(encode_update_status("a\0b", STATUS_REGISTER, &credentials()).err(), Some("jegotrip_rpc_string_invalid"));
}

#[test]
fn oversized_arguments_and_unknown_operations_fail_closed() {
    let large = "x".repeat(128 * 1024);
    assert_eq!(encode_update_status(&large, STATUS_REGISTER, &credentials()).err(), Some("jegotrip_rpc_arguments_too_large"));
    assert_eq!(encode_update_status("uri", 42, &credentials()).err(), Some("jegotrip_hosting_status_unsupported"));
}

#[test]
fn empty_or_nul_containing_gateway_credentials_are_rejected() {
    let mut credentials = credentials();
    credentials.sip_password = "";
    assert_eq!(encode_update_status("uri", STATUS_REGISTER, &credentials).err(),
        Some("jegotrip_rpc_credential_missing"));
    credentials.sip_password = "password";
    credentials.channel_id = "a\0b";
    assert_eq!(encode_update_status("uri", STATUS_REGISTER, &credentials).err(),
        Some("jegotrip_rpc_string_invalid"));
}

#[test]
fn field_size_and_total_size_limits_are_independently_enforced() {
    let mut credentials = credentials();
    let large = "v".repeat(64 * 1024);
    credentials.sip_password = &large;
    assert!(encode_update_status("uri", STATUS_REGISTER, &credentials).is_ok());
    let over = "v".repeat(64 * 1024 + 1);
    credentials.sip_password = &over;
    assert_eq!(encode_update_status("uri", STATUS_REGISTER, &credentials).err(),
        Some("jegotrip_rpc_arguments_too_large"));
    credentials.sip_password = &large;
    credentials.authname_prefix = &large;
    assert_eq!(encode_update_status("uri", STATUS_REGISTER, &credentials).err(),
        Some("jegotrip_rpc_arguments_too_large"));
}

#[test]
fn update_status_result_requires_zero_rpc_code_and_preserves_false_ack() {
    // Type0, bool true, mandatory empty footer reason string. Native result
    // streams do not repeat the request's major/minor version words.
    let accepted = [0, 1, 0, 0, 0, 0];
    assert!(decode_update_status_result(0, &accepted).unwrap().0);
    let mut rejected = accepted;
    rejected[1] = 0;
    assert!(!decode_update_status_result(0, &rejected).unwrap().0);
    assert_eq!(decode_update_status_result(1, &accepted).err(), Some(CodecError::RemoteFailure));
}

#[test]
fn a_nonempty_terminal_rpc_error_overrides_a_true_hosting_boolean() {
    let bytes = [0, 1, 0, 0, 0, 3, b'e', b'r', b'r'];
    assert_eq!(decode_update_status_result(0, &bytes).err(), Some(CodecError::RemoteFailure));
}

#[test]
fn update_status_footer_is_required_and_extra_bytes_cannot_hide_in_the_result() {
    let accepted = [0, 1, 0, 0, 0, 0];
    for length in 0..accepted.len() {
        assert!(decode_update_status_result(0, &accepted[..length]).is_err());
    }
    let mut extra = accepted.to_vec();
    extra.push(0xff);
    assert!(decode_update_status_result(0, &extra).is_err());
}

#[test]
fn secret_argument_debug_never_contains_values() {
    let args = encode_update_status("sensitive-uri", STATUS_REGISTER, &credentials()).unwrap();
    let debug = format!("{args:?}");
    assert!(debug.contains("redacted"));
    for value in ["sensitive-uri", "prefix", "channel", "number", "password"] {
        assert!(!debug.contains(value));
    }
}
