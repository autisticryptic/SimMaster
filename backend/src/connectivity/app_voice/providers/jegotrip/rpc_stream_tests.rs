use super::*;

#[test]
fn new_stream_retains_the_type_marker_before_be_version_words() {
    assert_eq!(Writer::new().finish().expose_to_transport(), &[0]);
    assert_eq!(Writer::versioned().unwrap().finish().expose_to_transport(), &[0, 0, 1, 0, 0]);
    assert!(Reader::versioned(&[0, 0, 1, 0, 0]).unwrap().finish().is_ok());
}

#[test]
fn unsupported_stream_kind_and_version_fail_without_guessing_alternate_formats() {
    assert!(matches!(Reader::new(&[2]), Err(CodecError::UnsupportedStream)));
    assert!(matches!(Reader::versioned(&[0, 0, 2, 0, 0]), Err(CodecError::UnsupportedVersion)));
    assert!(matches!(Reader::versioned(&[0, 0, 1, 0, 1]), Err(CodecError::UnsupportedVersion)));
    assert!(matches!(Reader::new(&[]), Err(CodecError::Truncated)));
}

#[test]
fn byte_strings_preserve_binary_values_and_length_first_map_order() {
    let mut writer = Writer::new();
    writer.properties(&[
        RpcProperty { key: b"aa", value: &[0xff, 0] },
        RpcProperty { key: b"z", value: b"1" },
        RpcProperty { key: b"a", value: b"2" },
    ]).unwrap();
    let bytes = writer.finish();
    let mut reader = Reader::new(bytes.expose_to_transport()).unwrap();
    let fields = reader.properties().unwrap();
    assert_eq!(fields.iter().map(|f| f.key).collect::<Vec<_>>(), vec![b"a".as_slice(), b"z", b"aa"]);
    assert_eq!(fields[2].value, &[0xff, 0]);
    reader.finish().unwrap();
}

#[test]
fn duplicate_metadata_is_rejected_on_both_encode_and_decode() {
    let duplicate = [RpcProperty { key: b"id", value: b"a" }, RpcProperty { key: b"id", value: b"b" }];
    assert_eq!(Writer::new().properties(&duplicate).err(), Some(CodecError::DuplicateKey));
    let mut writer = Writer::new();
    writer.u32(2).unwrap();
    for field in duplicate { writer.bytes(field.key).unwrap(); writer.bytes(field.value).unwrap(); }
    let bytes = writer.finish();
    assert_eq!(Reader::new(bytes.expose_to_transport()).unwrap().properties().err(), Some(CodecError::DuplicateKey));
}

#[test]
fn hostile_lengths_and_counts_are_rejected_before_reserving_unbounded_memory() {
    let mut reader = Reader::new(&[0, 0xff, 0xff, 0xff, 0xff]).unwrap();
    assert_eq!(reader.bytes().err(), Some(CodecError::TooLarge));
    let mut reader = Reader::new(&[0, 0xff, 0xff, 0xff, 0xff]).unwrap();
    assert_eq!(reader.properties().err(), Some(CodecError::TooLarge));
    let mut reader = Reader::new(&[0, 0, 0, 0, 2]).unwrap();
    assert_eq!(reader.properties().err(), Some(CodecError::Truncated));
    let mut reader = Reader::new(&[0, 0, 0, 0, 3, 1, 2]).unwrap();
    assert_eq!(reader.bytes().err(), Some(CodecError::Truncated));
}

#[test]
fn field_and_total_stream_bounds_are_distinct() {
    let large = vec![0; MAX_FIELD_BYTES];
    let mut writer = Writer::new();
    writer.bytes(&large).unwrap();
    assert_eq!(writer.bytes(&large).err(), Some(CodecError::TooLarge));
    assert_eq!(Writer::new().bytes(&vec![0; MAX_FIELD_BYTES + 1]).err(), Some(CodecError::TooLarge));
    assert!(matches!(Reader::new(&vec![0; MAX_STREAM_BYTES + 1]), Err(CodecError::TooLarge)));
    let fields = vec![RpcProperty { key: b"a", value: b"b" }; MAX_MAP_ENTRIES + 1];
    assert_eq!(Writer::new().properties(&fields).err(), Some(CodecError::TooLarge));
}

#[test]
fn native_boolean_is_a_single_byte_and_any_nonzero_value_is_true() {
    for value in [0, 1, 2, 255] {
        let bytes = [0, value];
        let mut reader = Reader::new(&bytes).unwrap();
        assert_eq!(reader.boolean().unwrap(), value != 0);
        reader.finish().unwrap();
    }
}

#[test]
fn terminal_footer_is_consumed_and_never_exposed_in_debug() {
    let mut writer = Writer::new();
    writer.bytes(b"fixture-private-reason").unwrap();
    writer.properties(&[RpcProperty { key: b"token", value: b"fixture-private-token" }]).unwrap();
    let bytes = writer.finish();
    let mut reader = Reader::new(bytes.expose_to_transport()).unwrap();
    let footer = read_footer(&mut reader).unwrap();
    assert_eq!(footer.reason, b"fixture-private-reason");
    assert_eq!(footer.properties[0].value, b"fixture-private-token");
    assert!(!format!("{footer:?}").contains("fixture"));
    assert!(!format!("{bytes:?}").contains("fixture"));
    assert!(!format!("{:?}", footer.properties[0]).contains("fixture"));
    reader.finish().unwrap();
}

#[test]
fn native_be64_preserves_high_bits_and_never_consumes_adjacent_i32_correlation_bytes() {
    let mut writer=Writer::new();writer.u64(0x0123_4567_89ab_cdef).unwrap();writer.i32(37).unwrap();
    let bytes=writer.finish();assert_eq!(bytes.expose_to_transport(),&[0,1,0x23,0x45,0x67,0x89,0xab,0xcd,0xef,0,0,0,37]);
    let mut reader=Reader::new(bytes.expose_to_transport()).unwrap();assert_eq!(reader.u64().unwrap(),0x0123_4567_89ab_cdef);assert_eq!(reader.i32().unwrap(),37);reader.finish().unwrap();
    for length in 1..9{assert_eq!(Reader::new(&bytes.expose_to_transport()[..length]).unwrap().u64().err(),Some(CodecError::Truncated));}
}
