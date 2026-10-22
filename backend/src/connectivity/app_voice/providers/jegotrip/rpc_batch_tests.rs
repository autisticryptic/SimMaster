use super::*;

fn payload<'a>() -> BatchPayload<'a> {
    BatchPayload {
        fields: [1, 2, 3, 4, 5, 6],
        requests: BTreeMap::from([(-2, BatchRequest {
            method: b"m", arguments: &[0, 0xff], metadata: vec![RpcProperty { key: b"id", value: b"obj" }],
        })]),
        results: BTreeMap::from([(3, BatchResult { result_code: 0x0001_0002, result_stream: &[0, 0] })]),
    }
}

fn reply_bytes(success: bool) -> Vec<u8> {
    let selected_session_id = b"session-id";
    let bytes = encode_swap_calls(b"user", selected_session_id, &payload()).unwrap();
    // Result stream only has type marker + bool, not the request's versions.
    let mut result = vec![0, u8::from(success)];
    let header_length = 5 + 4 + b"user".len() + 4 + selected_session_id.len();
    result.extend_from_slice(&bytes.expose_to_transport()[header_length..]);
    result.extend_from_slice(&[0; 8]); // main property count0 + terminal empty reason.
    result
}

#[test]
fn swap_calls_bytes_match_independent_six_fields_requests_then_results_layout() {
    let bytes = encode_swap_calls(b"u", b"t", &payload()).unwrap();
    let expected = [
        0, 0, 1, 0, 0, 0, 0, 0, 1, b'u', 0, 0, 0, 1, b't',
        0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3,
        0, 0, 0, 4, 0, 0, 0, 5, 0, 0, 0, 6,
        0, 0, 0, 1, 0xff, 0xff, 0xff, 0xfe,
        0, 0, 0, 1, b'm', 0, 0, 0, 2, 0, 0xff,
        0, 0, 0, 1, 0, 0, 0, 2, b'i', b'd', 0, 0, 0, 3, b'o', b'b', b'j',
        0, 0, 0, 1, 0, 0, 0, 3, 0, 1, 0, 2, 0, 0, 0, 2, 0, 0,
    ];
    assert_eq!(bytes.expose_to_transport(), &expected);
}

#[test]
fn signed_sequence_order_preserves_native_integer_map_semantics() {
    let mut payload = payload();
    payload.requests.insert(1, BatchRequest { method: b"p", arguments: &[], metadata: vec![] });
    let bytes = encode_swap_calls(b"u", b"t", &payload).unwrap();
    let mut reader = Reader::versioned(bytes.expose_to_transport()).unwrap();
    reader.bytes().unwrap(); reader.bytes().unwrap();
    let decoded = read_payload(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_eq!(decoded.requests.keys().copied().collect::<Vec<_>>(), vec![-2, 1]);
    assert_eq!(&bytes.expose_to_transport()[43..47], &(-2i32).to_be_bytes());
}

#[test]
fn batch_reply_preserves_nested_result_codes_and_raw_request_bytes() {
    let bytes = reply_bytes(true);
    let reply = decode_swap_calls_reply(0, &bytes).unwrap();
    assert!(reply.success);
    assert_eq!(reply.payload.fields, [1, 2, 3, 4, 5, 6]);
    assert_eq!(reply.payload.requests[&-2].arguments, &[0, 0xff]);
    assert_eq!(reply.payload.results[&3].result_code, 0x0001_0002);
    assert_eq!(reply.payload.results[&3].result_stream, &[0, 0]);
    assert!(reply.properties.is_empty());
    assert!(reply.footer.reason.is_empty());
}

#[test]
fn outer_failure_does_not_authorize_nested_results_or_success() {
    let bytes = reply_bytes(false);
    assert!(!decode_swap_calls_reply(0, &bytes).unwrap().success);
    assert_eq!(decode_swap_calls_reply(0x0002_0001, &reply_bytes(true)).err(), Some(CodecError::RemoteFailure));
}

#[test]
fn terminal_rpc_error_blocks_nested_batch_even_with_true_outer_boolean() {
    let mut bytes = reply_bytes(true);
    bytes.truncate(bytes.len() - 4);
    bytes.extend_from_slice(&[0, 0, 0, 3, b'e', b'r', b'r']);
    assert_eq!(decode_swap_calls_reply(0, &bytes).err(), Some(CodecError::RemoteFailure));
}

#[test]
fn batch_duplicate_sequence_numbers_and_hostile_counts_are_rejected() {
    let mut bytes = vec![0, 1];
    bytes.extend_from_slice(&[0; 24]);
    bytes.extend_from_slice(&[0; 4]); // no requests
    bytes.extend_from_slice(&2u32.to_be_bytes());
    for _ in 0..2 {
        bytes.extend_from_slice(&[0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
    bytes.extend_from_slice(&[0; 8]);
    assert_eq!(decode_swap_calls_reply(0, &bytes).err(), Some(CodecError::DuplicateKey));
    bytes[26..30].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(decode_swap_calls_reply(0, &bytes).err(), Some(CodecError::TooLarge));
}

#[test]
fn reply_requires_mandatory_footer_and_rejects_truncation_or_extra_data() {
    let bytes = reply_bytes(true);
    for length in 0..bytes.len() { assert!(decode_swap_calls_reply(0, &bytes[..length]).is_err()); }
    let mut extra = bytes;
    extra.extend_from_slice(&[0, 0, 0, 0, 1]);
    assert_eq!(decode_swap_calls_reply(0, &extra).err(), Some(CodecError::TrailingData));
}

#[test]
fn batch_encode_requires_session_identity_and_has_local_entry_bounds() {
    assert_eq!(encode_swap_calls(b"", b"t", &payload()).err(), Some(CodecError::InvalidField));
    assert_eq!(encode_swap_calls(b"u", b"", &payload()).err(), Some(CodecError::InvalidField));
    let mut payload = payload();
    payload.requests.get_mut(&-2).unwrap().method = b"";
    assert_eq!(encode_swap_calls(b"u", b"t", &payload).err(), Some(CodecError::InvalidField));
    payload.requests.clear();
    for sequence in 0..=MAX_MAP_ENTRIES as i32 {
        payload.results.insert(sequence, BatchResult { result_code: 0, result_stream: &[] });
    }
    assert_eq!(encode_swap_calls(b"u", b"t", &payload).err(), Some(CodecError::TooLarge));
}

#[test]
fn batch_debug_does_not_reveal_session_selector_method_metadata_or_contents() {
    let mut payload = payload();
    payload.requests.get_mut(&-2).unwrap().method = b"fixture-private-method";
    payload.requests.get_mut(&-2).unwrap().arguments = b"fixture-private-message";
    let bytes = encode_swap_calls(b"fixture-private-user", b"fixture-private-session-id", &payload).unwrap();
    assert!(!format!("{payload:?}").contains("fixture"));
    assert!(!format!("{bytes:?}").contains("fixture"));
    assert!(!format!("{:?}", payload.requests[&-2]).contains("fixture"));
    assert!(!format!("{:?}", payload.results[&3]).contains("fixture"));
}

#[test]
fn swap_arguments_use_the_selected_session_id_not_the_returned_auth_token() {
    let bytes = encode_swap_calls(b"returned-user", b"selected-session", &payload()).unwrap();
    let mut reader = Reader::versioned(bytes.expose_to_transport()).unwrap();
    assert_eq!(reader.bytes().unwrap(), b"returned-user");
    assert_eq!(reader.bytes().unwrap(), b"selected-session");
    assert_eq!(read_payload(&mut reader).unwrap().fields, payload().fields);
    reader.finish().unwrap();
    assert!(!bytes.expose_to_transport().windows(b"returned-auth-token".len())
        .any(|value| value == b"returned-auth-token"));
}
