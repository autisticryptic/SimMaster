//! Actions-only；独立字节夹具，不执行 Android SDK 或真实服务。
use super::*;
fn bytes(value: &[u8], output: &mut Vec<u8>) { output.extend_from_slice(&(value.len() as u32).to_be_bytes()); output.extend_from_slice(value); }
fn envelope(kind: &[u8], body: &[u8]) -> Vec<u8> {
    let mut result = vec![0]; bytes(kind, &mut result); result.extend_from_slice(&0u32.to_be_bytes()); bytes(body, &mut result); result
}
fn page(records: &[(i64, &[u8])], position: i64) -> Vec<u8> {
    let mut result = vec![0, 1]; result.extend_from_slice(&(records.len() as u32).to_be_bytes());
    for (position, raw) in records { result.extend_from_slice(&position.to_be_bytes()); bytes(raw, &mut result); }
    result.extend_from_slice(&position.to_be_bytes()); bytes(b"", &mut result); result
}
#[test]
fn independent_request_fixture_keeps_negative_initial_position_and_original_limit() {
    let mut expected = vec![0, 0, 1, 0, 0]; bytes(b"u", &mut expected); bytes(b"s", &mut expected);
    expected.extend_from_slice(&(-1i64).to_be_bytes()); expected.extend_from_slice(&100u32.to_be_bytes());
    assert_eq!(encode_get_offline(b"u", b"s", -1).unwrap().expose_to_transport(), expected);
    for position in [-2, i64::MAX] { assert_eq!(encode_get_offline(b"u", b"s", position).err(), Some(CodecError::InvalidField)); }
    assert!(encode_get_offline(b"", b"s", 0).is_err());
}
#[test]
fn hint_is_versioned_signed64_not_sms_body_or_a_32bit_rpc_identifier() {
    let position = 0x1234_5678_90ab_i64;
    let mut fixture = vec![0, 0, 1, 0, 0]; fixture.extend_from_slice(&position.to_be_bytes());
    assert_eq!(decode_more_offline(&fixture).unwrap(), position);
    for length in 0..fixture.len() { assert!(decode_more_offline(&fixture[..length]).is_err()); }
    fixture.push(0); assert_eq!(decode_more_offline(&fixture).err(), Some(CodecError::TrailingData));
}
#[test]
fn independent_collection_keeps_position64_blob_and_separate_server_position_footer() {
    let first = envelope(b"kind", &[0xff, 0, 7]); let second = envelope(b"other", b"text");
    let raw = page(&[(0x1_0000_0002, &second), (0x1_0000_0001, &first)], 0x1_0000_0009);
    let result = decode_get_offline(0, &raw).unwrap();
    assert_eq!(result.records[0].position, 0x1_0000_0001); assert_eq!(result.records[0].envelope, first);
    assert_eq!(result.records[1].position, 0x1_0000_0002); assert_eq!(result.server_position, 0x1_0000_0009);
    assert!(result.footer.reason.is_empty()); assert!(result.footer.properties.is_empty());
    assert!(!format!("{result:?}").contains("text"));
}
#[test]
fn nested_message_is_type0_kind_map_body_without_repeating_method_version() {
    let raw = envelope(b"kind", &[0, 0xff]); let result = decode_offline_message(&raw).unwrap();
    assert_eq!(result.receiver_kind, b"kind"); assert_eq!(result.body, &[0, 0xff]);
    assert!(result.properties.is_empty()); assert!(!format!("{result:?}").contains("kind"));
    let mut versioned = vec![0, 0, 1, 0, 0]; versioned.extend_from_slice(&raw[1..]);
    assert!(decode_offline_message(&versioned).is_err());
}
#[test]
fn empty_collection_and_native_nonzero_bool_keep_the_original_success_semantics() {
    let mut raw = page(&[], -1); raw[1] = 7;
    let result = decode_get_offline(0, &raw).unwrap(); assert!(result.records.is_empty()); assert_eq!(result.server_position, -1);
    raw[1] = 0; assert_eq!(decode_get_offline(0, &raw).err(), Some(CodecError::RemoteFailure));
    assert_eq!(decode_get_offline(1, &[0xff]).err(), Some(CodecError::RemoteFailure));
}
#[test]
fn truncated_trailing_duplicate_and_impossible_positions_fail_without_partial_delivery() {
    let record = envelope(b"kind", b"message"); let raw = page(&[(2, &record)], 2);
    for length in 0..raw.len() { assert!(decode_get_offline(0, &raw[..length]).is_err(), "length {length}"); }
    let mut trailing = raw.clone(); trailing.extend_from_slice(&[1, 2, 3]); assert!(decode_get_offline(0, &trailing).is_err());
    assert_eq!(decode_get_offline(0, &page(&[(2, &record), (2, &record)], 2)).err(), Some(CodecError::DuplicateKey));
    for position in [-1, i64::MAX] { assert_eq!(decode_get_offline(0, &page(&[(position, &record)], 4)).err(), Some(CodecError::InvalidField)); }
    assert_eq!(decode_get_offline(0, &page(&[(2, &record)], 1)).err(), Some(CodecError::InvalidField));
}
#[test]
fn oversized_count_blob_and_empty_envelope_are_bounded_before_consumption() {
    let mut raw = vec![0, 1]; raw.extend_from_slice(&101u32.to_be_bytes()); raw.extend_from_slice(&vec![0; 101 * 12 + 12]);
    assert_eq!(decode_get_offline(0, &raw).err(), Some(CodecError::TooLarge));
    assert_eq!(decode_get_offline(0, &page(&[(0, b"")], 0)).err(), Some(CodecError::InvalidField));
    let large = vec![0; super::super::rpc_stream::MAX_FIELD_BYTES + 1];
    assert_eq!(decode_get_offline(0, &page(&[(0, &large)], 0)).err(), Some(CodecError::TooLarge));
}
#[test]
fn type2_json_is_not_guessed_as_a_sequential_message_and_raw_page_remains_available() {
    let json = b"\x02{\"body\":\"not-a-native-message\"}";
    assert_eq!(decode_offline_message(json).err(), Some(CodecError::UnsupportedStream));
    let raw = page(&[(0, json)], 0); let result = decode_get_offline(0, &raw).unwrap();
    assert_eq!(result.records[0].envelope, json); // 由消费器隔离保存，不能提前跳过。
}
#[test]
fn outer_common_error_footer_is_not_an_ack_or_authentication_token() {
    let mut raw = page(&[], -1); raw.truncate(raw.len() - 4); bytes(b"private-reason", &mut raw);
    assert_eq!(decode_get_offline(0, &raw).err(), Some(CodecError::RemoteFailure));
}
