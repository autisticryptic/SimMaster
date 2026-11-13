//! Actions-only passive wire fixtures. No socket, SDK, device or provider IO.
use super::*;
fn field(bytes: &[u8], out: &mut Vec<u8>) { out.extend_from_slice(&(bytes.len() as u32).to_be_bytes()); out.extend_from_slice(bytes); }
fn request(user: &[u8], selector: &[u8], stamp: u64, interval: u32) -> Vec<u8> {
    let mut bytes = vec![0, 0, 1, 0, 0]; field(user, &mut bytes); field(selector, &mut bytes);
    bytes.extend_from_slice(&stamp.to_be_bytes()); bytes.extend_from_slice(&interval.to_be_bytes()); bytes
}
fn arguments<'a>(user: &'a [u8], selector: &'a [u8], stamp: u64, interval: u32) -> RegisterAliveArguments<'a> {
    RegisterAliveArguments { user_id: user, selected_session_id: selector, request_timestamp_ms: stamp, interval_seconds: interval }
}
fn result(accepted: u8, reason: &[u8], properties: Option<&[(&[u8], &[u8])]>) -> Vec<u8> {
    let mut bytes = vec![0, accepted]; field(reason, &mut bytes);
    if let Some(properties) = properties {
        bytes.extend_from_slice(&(properties.len() as u32).to_be_bytes());
        for (key, value) in properties { field(key, &mut bytes); field(value, &mut bytes); }
    }
    bytes
}
#[test]
fn independent_request_fixture_keeps_one_entry_zero_version_and_exact_native_argument_order() {
    let expected = [0, 0, 1, 0, 0, 0, 0, 0, 1, b'u', 0, 0, 0, 1, b's', 0x81, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98];
    let encoded = encode_register_alive(&arguments(b"u", b"s", 0x8123_4567_89ab_cdef, 0xfedc_ba98)).unwrap();
    assert_eq!(encoded.expose_to_transport(), expected); assert_eq!(REGISTER_ALIVE_METHOD, "registerAlive.AccountAlive.Account");
}
#[test]
fn request_decoder_borrows_exact_strings_and_preserves_high_numeric_bits() {
    let bytes = request(b"u", b"s", 0x8123_4567_89ab_cdef, 0xfedc_ba98);
    let parsed = decode_register_alive_arguments(&bytes).unwrap();
    assert_eq!(parsed.user_id, b"u"); assert_eq!(parsed.selected_session_id, b"s");
    assert_eq!(parsed.user_id.as_ptr(), bytes[9..10].as_ptr()); assert_eq!(parsed.selected_session_id.as_ptr(), bytes[14..15].as_ptr());
    assert_eq!(parsed.request_timestamp_ms, 0x8123_4567_89ab_cdef); assert_eq!(parsed.interval_seconds, 0xfedc_ba98);
}
#[test]
fn opaque_argument_bytes_roundtrip_without_utf8_loss_or_uuid_token_substitution() {
    let binary: Vec<u8> = (0..=255).collect(); let selector = [0xff, 0, b's'];
    let encoded = encode_register_alive(&arguments(&binary, &selector, 7, 9)).unwrap();
    let parsed = decode_register_alive_arguments(encoded.expose_to_transport()).unwrap();
    assert_eq!(parsed.user_id, binary); assert_eq!(parsed.selected_session_id, selector);
    assert_eq!(encoded.expose_to_transport(), request(&binary, &selector, 7, 9));
}
#[test]
fn every_truncated_request_and_trailing_byte_is_rejected_without_partial_arguments() {
    let bytes = request(b"fixture-user", b"fixture-selector", 0x1_0000_0001, 60);
    for length in 0..bytes.len() { assert!(decode_register_alive_arguments(&bytes[..length]).is_err(), "prefix {length}"); }
    let mut extra = bytes; extra.push(0); assert_eq!(decode_register_alive_arguments(&extra).err(), Some(CodecError::TrailingData));
}
#[test]
fn request_stream_kind_and_method_version_list_are_not_guessed_or_negotiated() {
    let bytes = request(b"u", b"s", 1, 1);
    for kind in [1, 2, 255] { let mut invalid = bytes.clone(); invalid[0] = kind; assert_eq!(decode_register_alive_arguments(&invalid).err(), Some(CodecError::UnsupportedStream)); }
    for prefix in [[0, 0, 0, 0], [0, 2, 0, 0], [0, 1, 0, 1], [0xff, 0xff, 0, 0]] {
        let mut invalid = bytes.clone(); invalid[1..5].copy_from_slice(&prefix);
        assert_eq!(decode_register_alive_arguments(&invalid).err(), Some(CodecError::UnsupportedVersion));
    }
}
#[test]
fn empty_original_user_or_selector_is_rejected_as_local_input_policy() {
    for (user, selector) in [(b"".as_slice(), b"s".as_slice()), (b"u".as_slice(), b"".as_slice()), (b"".as_slice(), b"".as_slice())] {
        assert_eq!(encode_register_alive(&arguments(user, selector, 0, 0)).err(), Some(CodecError::InvalidField));
        assert_eq!(decode_register_alive_arguments(&request(user, selector, 0, 0)).err(), Some(CodecError::InvalidField));
    }
}
#[test]
fn field_limits_and_forged_length_prefixes_fail_before_field_copy() {
    let oversized = vec![0u8; MAX_FIELD_BYTES + 1];
    for (user, selector) in [(oversized.as_slice(), b"s".as_slice()), (b"u".as_slice(), oversized.as_slice())] {
        assert_eq!(encode_register_alive(&arguments(user, selector, 1, 1)).err(), Some(CodecError::TooLarge));
    }
    for length in [(MAX_FIELD_BYTES + 1) as u32, u32::MAX] {
        let mut first = vec![0, 0, 1, 0, 0]; first.extend_from_slice(&length.to_be_bytes());
        assert_eq!(decode_register_alive_arguments(&first).err(), Some(CodecError::TooLarge));
        let mut second = request(b"u", b"s", 1, 1); second[10..14].copy_from_slice(&length.to_be_bytes());
        assert_eq!(decode_register_alive_arguments(&second).err(), Some(CodecError::TooLarge));
    }
}
#[test]
fn exact_whole_stream_bound_is_shared_by_encoder_and_decoder_without_larger_budget() {
    let user = vec![b'u'; MAX_FIELD_BYTES]; let selector = vec![b's'; MAX_STREAM_BYTES - 25 - user.len()];
    let encoded = encode_register_alive(&arguments(&user, &selector, u64::MAX, u32::MAX)).unwrap();
    assert_eq!(encoded.expose_to_transport().len(), MAX_STREAM_BYTES);
    let decoded = decode_register_alive_arguments(encoded.expose_to_transport()).unwrap(); assert_eq!(decoded.selected_session_id, selector);
    let mut too_much = selector; too_much.push(b's');
    assert_eq!(encode_register_alive(&arguments(&user, &too_much, 1, 1)).err(), Some(CodecError::TooLarge));
    let overlong = request(&user, &too_much, 1, 1); assert_eq!(decode_register_alive_arguments(&overlong).err(), Some(CodecError::TooLarge));
}
#[test]
fn zero_and_max_timestamp_interval_are_wire_bits_not_clock_defaults_or_renewal_actions() {
    for stamp in [0, 1, 0x1_0000_0000, u64::MAX] { for interval in [0, 1, u32::MAX] {
        let encoded = encode_register_alive(&arguments(b"u", b"s", stamp, interval)).unwrap();
        let decoded = decode_register_alive_arguments(encoded.expose_to_transport()).unwrap();
        assert_eq!(decoded.request_timestamp_ms, stamp); assert_eq!(decoded.interval_seconds, interval);
    } }
}
#[test]
fn result_bool_and_footer_keep_false_and_native_nonzero_bool_as_uncorrelated_data() {
    for accepted in [0, 1, 7, 255] {
        let bytes = result(accepted, b"", None); assert_eq!(bytes, [0, accepted, 0, 0, 0, 0]);
        let parsed = decode_register_alive_result(0, &bytes).unwrap();
        assert_eq!(parsed.returned_acceptance(), accepted != 0); assert!(parsed.footer().reason.is_empty()); assert!(parsed.footer().properties.is_empty());
    }
}
#[test]
fn optional_result_properties_remain_opaque_not_tokens_expiry_or_registration_capabilities() {
    let bytes = result(1, b"", Some(&[(b"token", b"fixture-not-a-grant"), (b"expiry", b"forever"), (b"binary", &[0xff, 0])]));
    let parsed = decode_register_alive_result(0, &bytes).unwrap(); assert!(parsed.returned_acceptance());
    assert_eq!(parsed.footer().properties.len(), 3); assert_eq!(parsed.footer().properties[0].value, b"fixture-not-a-grant");
    assert_eq!(parsed.footer().properties[2].value, &[0xff, 0]);
    assert!(decode_register_alive_result(0, &result(1, b"", Some(&[]))).unwrap().footer().properties.is_empty());
}
#[test]
fn result_truncation_trailing_stream_and_repeated_request_version_are_rejected() {
    let bytes = result(1, b"", None);
    for length in 0..bytes.len() { assert!(decode_register_alive_result(0, &bytes[..length]).is_err()); }
    let properties = result(1, b"", Some(&[(b"key", b"value")]));
    // Ending exactly before the optional map is a valid map-absent footer.
    for length in 7..properties.len() { assert!(decode_register_alive_result(0, &properties[..length]).is_err()); }
    let mut trailing = bytes.clone(); trailing.push(0xff); assert!(decode_register_alive_result(0, &trailing).is_err());
    let mut repeated = vec![0, 0, 1, 0, 0]; repeated.extend_from_slice(&bytes[1..]); assert!(decode_register_alive_result(0, &repeated).is_err());
    for kind in [1, 2, 255] { let mut invalid = bytes.clone(); invalid[0] = kind; assert_eq!(decode_register_alive_result(0, &invalid).err(), Some(CodecError::UnsupportedStream)); }
}
#[test]
fn remote_flags_or_error_footer_do_not_retry_or_reflect_private_peer_reason() {
    for code in [1, 0x1_0000, 0x2_0000, u32::MAX] { assert_eq!(decode_register_alive_result(code, &[]).err(), Some(CodecError::RemoteFailure)); }
    let bytes = result(1, b"fixture-private-reason", Some(&[(b"token", b"fixture-private-token")]));
    let error = decode_register_alive_result(0, &bytes).err().unwrap(); assert_eq!(error, CodecError::RemoteFailure);
    assert!(!format!("{error:?}").contains("fixture-private"));
}
#[test]
fn result_map_duplicates_counts_fields_and_whole_stream_are_bounded() {
    assert_eq!(decode_register_alive_result(0, &result(1, b"", Some(&[(b"k", b"v1"), (b"k", b"v2")]))).err(), Some(CodecError::DuplicateKey));
    let mut count = result(1, b"", None); count.extend_from_slice(&129u32.to_be_bytes());
    assert_eq!(decode_register_alive_result(0, &count).err(), Some(CodecError::TooLarge));
    let mut reason = vec![0, 1]; reason.extend_from_slice(&((MAX_FIELD_BYTES + 1) as u32).to_be_bytes());
    assert_eq!(decode_register_alive_result(0, &reason).err(), Some(CodecError::TooLarge));
    let oversized = vec![0; MAX_STREAM_BYTES + 1]; assert_eq!(decode_register_alive_result(0, &oversized).err(), Some(CodecError::TooLarge));
}
#[test]
fn debug_views_redact_borrowed_identity_bytes_reply_properties_and_encoded_body() {
    let arguments = arguments(b"fixture-private-user", b"fixture-private-selector", 123456789, 7654321);
    let encoded = encode_register_alive(&arguments).unwrap();
    let bytes = result(1, b"", Some(&[(b"token", b"fixture-private-token")])); let reply = decode_register_alive_result(0, &bytes).unwrap();
    for view in [format!("{arguments:?}"), format!("{encoded:?}"), format!("{reply:?}"), format!("{:?}", reply.footer())] {
        assert!(!view.contains("fixture-private")); assert!(!view.contains("123456789")); assert!(!view.contains("7654321")); assert!(view.contains("redacted"));
    }
}
#[test]
fn decoded_true_result_cannot_change_check_alive_even_after_matching_sdk_login() {
    use super::super::{arc_session::{ArcLoginInputs, ArcReply, ArcRpcSession, ArcPhase}, rpc_envelope::{encode_reply, RpcRequest}, rpc_incoming::IncomingBinding,
        rpc_stream::RpcProperty, session::HostingSession, tests::imported};
    let mut session = HostingSession::new("fixture-alive-owner".into()); session.import(imported(), 100).unwrap(); let owner = session.begin_login(100).unwrap();
    let mut rpc = ArcRpcSession::new(owner.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_login(ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-selector", phone_login_token: b"fixture-phone-token",
        application_key: b"fixture-appkey", domain_id: b"fixture-domain", application_id: b"fixture-app", device_info_cc: b"", client_properties: &[] }, &[]).unwrap();
    let mut success = Writer::new(); success.boolean(true).unwrap(); success.bytes(b"fixture-user").unwrap(); success.bytes(b"fixture-arc-token").unwrap(); success.properties(&[]).unwrap(); success.bytes(b"").unwrap();
    let body = success.finish(); let reply = encode_reply(id, 0, body.expose_to_transport()).unwrap();
    assert!(matches!(rpc.on_reply(&owner, &scope, reply.expose_to_transport()).unwrap(), Some(ArcReply::Login(_))));
    let incoming = IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"Sip.Request" };
    let client = rpc.client_ex_dispatcher(incoming).unwrap();
    let raw = result(1, b"", Some(&[(b"token", b"fixture-arc-token"), (b"registered", b"true")]));
    assert!(decode_register_alive_result(0, &raw).unwrap().returned_acceptance());
    let check = Writer::versioned().unwrap().finish();
    let request = RpcRequest { call_id: 91, method: super::super::rpc_client_ex::CHECK_ALIVE_METHOD.as_bytes(), arguments: check.expose_to_transport(),
        metadata: vec![RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" }] };
    let checked = client.on_direct(&owner, &scope, &request).unwrap().unwrap();
    assert_eq!(checked.result.expose_to_transport(), [0; 10]); assert!(checked.notice.is_none());
    assert_eq!(rpc.phase(), ArcPhase::Authenticated); assert!(scope.is_open()); assert!(!scope.session_revoked());
}
