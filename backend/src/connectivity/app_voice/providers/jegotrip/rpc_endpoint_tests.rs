//! Actions-only Endpoint byte fixtures. No runtime, registration, socket, peer,
//! trust mutation, SDK execution or media-readiness transition is exercised.
use super::*;
use super::super::rpc_stream::{MAX_FIELD_BYTES, MAX_MAP_ENTRIES, MAX_STREAM_BYTES};

// Independent byte oracles: no production Writer/Reader or endpoint encoder
// is used to assemble responses, counts, strings, maps or footer properties.
fn string(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
    bytes.extend_from_slice(value);
}
fn map(bytes: &mut Vec<u8>, properties: &[RpcProperty<'_>]) {
    bytes.extend_from_slice(&(properties.len() as u32).to_be_bytes());
    for property in properties {
        string(bytes, property.key);
        string(bytes, property.value);
    }
}
fn result(boolean: u8, features: &[&[u8]], properties: &[RpcProperty<'_>], reason: &[u8],
    footer: Option<&[RpcProperty<'_>]>) -> Vec<u8> {
    let mut bytes = vec![0, boolean];
    bytes.extend_from_slice(&(features.len() as u32).to_be_bytes());
    for feature in features { string(&mut bytes, feature); }
    map(&mut bytes, properties);
    string(&mut bytes, reason);
    if let Some(properties) = footer { map(&mut bytes, properties); }
    bytes
}
fn request(content: &[u8], features: &[&[u8]], properties: &[RpcProperty<'_>])
    -> Result<RpcBytes, CodecError> {
    encode_endpoint_connect(EndpointConnectInputs { content, features, properties })
}

#[test]
fn request_and_result_match_independent_native_content_set_and_binary_map_oracles() {
    let properties = [
        RpcProperty { key: &[0xff, 0], value: &[0xfe, 0, 7] },
        RpcProperty { key: b"z", value: &[0, 0xff] },
    ];
    let expected_request = [
        0, 0, 1, 0, 0, // type-0 and one supported method version, zero
        0, 0, 0, 3, 0xff, 0, 7, // binary content
        0, 0, 0, 2, // set count, not a map
        0, 0, 0, 1, b'b', 0, 0, 0, 2, b'a', b'a', // length-first
        0, 0, 0, 2,
        0, 0, 0, 1, b'z', 0, 0, 0, 2, 0, 0xff,
        0, 0, 0, 2, 0xff, 0, 0, 0, 0, 3, 0xfe, 0, 7,
    ];
    let encoded = request(&[0xff, 0, 7], &[b"aa", b"b"], &properties).unwrap();
    assert_eq!(CONNECT_ENDPOINT_METHOD, "connect.Endpoint.Endpoint");
    assert_eq!(encoded.expose_to_transport(), expected_request);
    let expected_result = [
        0, 1, // type-0, bool, NO request version prefix
        0, 0, 0, 2,
        0, 0, 0, 1, b'b', 0, 0, 0, 2, b'a', b'a',
        0, 0, 0, 2,
        0, 0, 0, 1, b'z', 0, 0, 0, 2, 0, 0xff,
        0, 0, 0, 2, 0xff, 0, 0, 0, 0, 3, 0xfe, 0, 7,
        0, 0, 0, 0, // mandatory Common reason string
    ];
    let decoded = decode_endpoint_connect_result(0, &expected_result).unwrap();
    assert_eq!(decoded.features, vec![b"b".as_slice(), b"aa".as_slice()]);
    assert_eq!(decoded.properties, vec![properties[1], properties[0]]);
    assert!(decoded.footer.reason.is_empty());
    assert!(decoded.footer.properties.is_empty());
}

#[test]
fn binary_features_and_host_named_properties_are_scopeless_data_not_authority() {
    let features: &[&[u8]] = &[&[0xff, 0], &[0]];
    let properties = [
        RpcProperty { key: b"Client.Host", value: &[0xff, 0, 7] },
        RpcProperty { key: b"ArcPort", value: b"not-a-port" },
        RpcProperty { key: b"content", value: b"fixture-opaque-content" },
    ];
    let encoded = request(b"fixture-original-content", features, &properties).unwrap();
    // Explicit supplied content is encoded first, without extraction from the map.
    let mut prefix = vec![0, 0, 1, 0, 0];
    string(&mut prefix, b"fixture-original-content");
    assert!(encoded.expose_to_transport().starts_with(&prefix));
    let mut expected = prefix;
    expected.extend_from_slice(&[0, 0, 0, 2]);
    string(&mut expected, &[0]);
    string(&mut expected, &[0xff, 0]);
    map(&mut expected, &[properties[1], properties[2], properties[0]]);
    assert_eq!(encoded.expose_to_transport(), expected);
    let footer = [RpcProperty { key: &[0, 0xfe], value: &[0xff, 0] }];
    let raw = result(0xfe, features, &properties, b"", Some(&footer));
    let decoded = decode_endpoint_connect_result(0, &raw).unwrap();
    // Native nonzero bool compatibility; values remain borrowed, uninterpreted bytes.
    assert_eq!(decoded.features, features);
    assert_eq!(decoded.properties, properties);
    assert_eq!(decoded.footer.properties, footer);
}

#[test]
fn no_default_features_and_nonempty_content_and_feature_policies_are_paired() {
    assert_eq!(request(b"x", &[], &[]).unwrap().expose_to_transport(),
        &[0, 0, 1, 0, 0, 0, 0, 0, 1, b'x', 0, 0, 0, 0, 0, 0, 0, 0]);
    let empty = [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let decoded = decode_endpoint_connect_result(0, &empty).unwrap();
    assert!(decoded.features.is_empty());
    assert!(decoded.properties.is_empty());
    assert_eq!(request(b"", &[], &[]).err(), Some(CodecError::InvalidField));
    assert_eq!(request(b"x", &[b""], &[]).err(), Some(CodecError::InvalidField));
    assert_eq!(decode_endpoint_connect_result(0, &result(1, &[b""], &[], b"", None)).err(),
        Some(CodecError::InvalidField));
}

#[test]
fn duplicate_and_colliding_feature_bytes_are_rejected_not_set_inserted() {
    let one = [0xff, 0];
    let other_allocation = [0xff, 0];
    let features: &[&[u8]] = &[&one, b"x", &other_allocation];
    assert_eq!(request(b"x", features, &[]).err(), Some(CodecError::DuplicateKey));
    assert_eq!(decode_endpoint_connect_result(0, &result(1, features, &[], b"", None)).err(),
        Some(CodecError::DuplicateKey));
    // Equal lengths alone are not a collision, nor is a shared prefix.
    let distinct: &[&[u8]] = &[b"aa", b"ab", b"a"];
    assert!(request(b"x", distinct, &[]).is_ok());
    let raw = result(1, distinct, &[], b"", None);
    assert_eq!(decode_endpoint_connect_result(0, &raw).unwrap().features, distinct);
}

#[test]
fn duplicate_primary_and_footer_maps_fail_without_normalizing_binary_keys() {
    let duplicate = [
        RpcProperty { key: &[0xff, 0], value: b"first" },
        RpcProperty { key: b"x", value: b"middle" },
        RpcProperty { key: &[0xff, 0], value: b"second" },
    ];
    assert_eq!(request(b"x", &[], &duplicate).err(), Some(CodecError::DuplicateKey));
    assert_eq!(decode_endpoint_connect_result(0, &result(1, &[], &duplicate, b"", None)).err(),
        Some(CodecError::DuplicateKey));
    assert_eq!(decode_endpoint_connect_result(0, &result(1, &[], &[], b"", Some(&duplicate))).err(),
        Some(CodecError::DuplicateKey));
}

#[test]
fn feature_counts_are_bounded_before_allocation_and_minimum_string_prefixes_required() {
    let storage: Vec<_> = (0..MAX_MAP_ENTRIES).map(|index| [index as u8]).collect();
    let features: Vec<&[u8]> = storage.iter().map(|value| value.as_slice()).collect();
    assert!(request(b"x", &features, &[]).is_ok());
    let raw = result(1, &features, &[], b"", None);
    assert_eq!(decode_endpoint_connect_result(0, &raw).unwrap().features, features);
    let excessive = vec![b"x".as_slice(); MAX_MAP_ENTRIES + 1];
    assert_eq!(request(b"x", &excessive, &[]).err(), Some(CodecError::TooLarge));
    for count in [MAX_MAP_ENTRIES as u32 + 1, u32::MAX] {
        let mut raw = vec![0, 1];
        raw.extend_from_slice(&count.to_be_bytes());
        assert_eq!(decode_endpoint_connect_result(0, &raw).err(), Some(CodecError::TooLarge));
    }
    // count(4), not count(8): one single-byte string plus both empty maps/footer is valid.
    assert!(decode_endpoint_connect_result(0, &result(1, &[b"x"], &[], b"", None)).is_ok());
    assert_eq!(decode_endpoint_connect_result(0, &[0, 1, 0, 0, 0, 2, 0, 0, 0, 1]).err(),
        Some(CodecError::Truncated));
}

#[test]
fn all_content_feature_property_and_footer_strings_enforce_the_field_bound() {
    let boundary = vec![0xff; MAX_FIELD_BYTES];
    let excessive = vec![0xff; MAX_FIELD_BYTES + 1];
    assert!(request(&boundary, &[], &[]).is_ok());
    assert_eq!(request(&excessive, &[], &[]).err(), Some(CodecError::TooLarge));
    assert!(request(b"x", &[&boundary], &[]).is_ok());
    assert_eq!(request(b"x", &[&excessive], &[]).err(), Some(CodecError::TooLarge));
    assert!(decode_endpoint_connect_result(0, &result(1, &[&boundary], &[], b"", None)).is_ok());
    assert_eq!(decode_endpoint_connect_result(0, &result(1, &[&excessive], &[], b"", None)).err(),
        Some(CodecError::TooLarge));
    for value in [false, true] {
        let property = RpcProperty {
            key: if value { b"k" } else { &boundary },
            value: if value { &boundary } else { b"v" },
        };
        assert!(request(b"x", &[], &[property]).is_ok());
        assert!(decode_endpoint_connect_result(0, &result(1, &[], &[property], b"", None)).is_ok());
        assert!(decode_endpoint_connect_result(0, &result(1, &[], &[], b"", Some(&[property]))).is_ok());
        let property = RpcProperty {
            key: if value { b"k" } else { &excessive },
            value: if value { &excessive } else { b"v" },
        };
        assert_eq!(request(b"x", &[], &[property]).err(), Some(CodecError::TooLarge));
        assert_eq!(decode_endpoint_connect_result(0, &result(1, &[], &[property], b"", None)).err(),
            Some(CodecError::TooLarge));
        assert_eq!(decode_endpoint_connect_result(0, &result(1, &[], &[], b"", Some(&[property]))).err(),
            Some(CodecError::TooLarge));
    }
    assert_eq!(decode_endpoint_connect_result(0, &result(1, &[], &[], &excessive, None)).err(),
        Some(CodecError::TooLarge));
    // Declared excessive length is rejected even without allocating the payload.
    let mut raw = vec![0, 1, 0, 0, 0, 1];
    raw.extend_from_slice(&((MAX_FIELD_BYTES + 1) as u32).to_be_bytes());
    assert_eq!(decode_endpoint_connect_result(0, &raw).err(), Some(CodecError::TooLarge));
}

#[test]
fn aggregate_stream_limit_is_inclusive_for_requests_and_results() {
    let content = vec![0xff; MAX_FIELD_BYTES];
    let feature = vec![0; MAX_FIELD_BYTES - 21];
    let encoded = request(&content, &[&feature], &[]).unwrap();
    assert_eq!(encoded.expose_to_transport().len(), MAX_STREAM_BYTES);
    let larger_feature = vec![0; feature.len() + 1];
    assert_eq!(request(&content, &[&larger_feature], &[]).err(), Some(CodecError::TooLarge));
    let feature = vec![0; MAX_FIELD_BYTES - 22];
    let raw = result(1, &[&content, &feature], &[], b"", None);
    assert_eq!(raw.len(), MAX_STREAM_BYTES);
    assert_eq!(decode_endpoint_connect_result(0, &raw).unwrap().features.len(), 2);
    let mut excessive = raw;
    excessive.push(0);
    assert_eq!(decode_endpoint_connect_result(0, &excessive).err(), Some(CodecError::TooLarge));
}

#[test]
fn primary_and_footer_map_counts_are_bounded_and_truncated_counts_rejected() {
    let keys: Vec<_> = (0..MAX_MAP_ENTRIES).map(|index| [index as u8]).collect();
    let properties: Vec<_> = keys.iter().map(|key| RpcProperty { key: key.as_slice(), value: b"" }).collect();
    assert!(request(b"x", &[], &properties).is_ok());
    let raw = result(1, &[], &properties, b"", Some(&properties));
    let decoded = decode_endpoint_connect_result(0, &raw).unwrap();
    assert_eq!(decoded.properties.len(), MAX_MAP_ENTRIES);
    assert_eq!(decoded.footer.properties.len(), MAX_MAP_ENTRIES);
    let excessive = vec![RpcProperty { key: b"x", value: b"" }; MAX_MAP_ENTRIES + 1];
    assert_eq!(request(b"x", &[], &excessive).err(), Some(CodecError::TooLarge));
    for count in [MAX_MAP_ENTRIES as u32 + 1, u32::MAX] {
        let mut primary = vec![0, 1, 0, 0, 0, 0];
        primary.extend_from_slice(&count.to_be_bytes());
        assert_eq!(decode_endpoint_connect_result(0, &primary).err(), Some(CodecError::TooLarge));
        let mut footer = result(1, &[], &[], b"", None);
        footer.extend_from_slice(&count.to_be_bytes());
        assert_eq!(decode_endpoint_connect_result(0, &footer).err(), Some(CodecError::TooLarge));
    }
    let mut primary = vec![0, 1, 0, 0, 0, 0, 0, 0, 0, 1];
    assert_eq!(decode_endpoint_connect_result(0, &primary).err(), Some(CodecError::Truncated));
    primary.extend_from_slice(&[0; 7]);
    assert_eq!(decode_endpoint_connect_result(0, &primary).err(), Some(CodecError::Truncated));
    let mut footer = result(1, &[], &[], b"", None);
    footer.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
    assert_eq!(decode_endpoint_connect_result(0, &footer).err(), Some(CodecError::Truncated));
}

#[test]
fn truncation_trailing_data_unsupported_stream_and_request_prefix_never_yield_results() {
    let properties = [RpcProperty { key: b"k", value: &[0xff, 0] }];
    let raw = result(1, &[b"x", b"ab"], &properties, b"", None);
    for length in 0..raw.len() {
        assert!(decode_endpoint_connect_result(0, &raw[..length]).is_err(), "prefix {length}");
    }
    let mut wrong_kind = raw.clone();
    wrong_kind[0] = 1;
    assert_eq!(decode_endpoint_connect_result(0, &wrong_kind).err(), Some(CodecError::UnsupportedStream));
    let mut trailing = result(1, &[b"x"], &[], b"", Some(&[]));
    trailing.push(0xff);
    assert_eq!(decode_endpoint_connect_result(0, &trailing).err(), Some(CodecError::TrailingData));
    let mut incomplete_footer = raw.clone();
    incomplete_footer.push(0xff);
    assert_eq!(decode_endpoint_connect_result(0, &incomplete_footer).err(), Some(CodecError::Truncated));
    let mut version_prefixed = vec![0, 0, 1, 0, 0];
    version_prefixed.extend_from_slice(&raw[1..]);
    assert!(decode_endpoint_connect_result(0, &version_prefixed).is_err());
    let encoded = request(b"x", &[b"ab", b"x"], &properties).unwrap();
    assert!(decode_endpoint_connect_result(0, encoded.expose_to_transport()).is_err());
}

#[test]
fn external_code_boolean_and_mandatory_common_error_are_distinct_from_media_success() {
    let raw = result(1, &[], &[], b"", None);
    for code in [1, 0x1_0000, u32::MAX] {
        assert_eq!(decode_endpoint_connect_result(code, &raw).err(), Some(CodecError::RemoteFailure));
        assert_eq!(decode_endpoint_connect_result(code, &[]).err(), Some(CodecError::RemoteFailure));
    }
    for boolean in [1, 7, 0xff] {
        assert!(decode_endpoint_connect_result(0, &result(boolean, &[], &[], b"", None)).is_ok());
    }
    assert_eq!(decode_endpoint_connect_result(0, &result(0, &[], &[], b"", None)).err(),
        Some(CodecError::RemoteFailure));
    for reason in [b"fixture-rejected".as_slice(), &[0xff, 0]] {
        assert_eq!(decode_endpoint_connect_result(0, &result(1, &[], &[], reason, None)).err(),
            Some(CodecError::RemoteFailure));
    }
    assert_eq!(decode_endpoint_connect_result(0, &raw[..raw.len() - 4]).err(), Some(CodecError::Truncated));
    assert!(decode_endpoint_connect_result(0, &result(1, &[], &[], b"", Some(&[]))).is_ok());
    // Malformed false responses are still fully parsed, not returned as partial data.
    assert_eq!(decode_endpoint_connect_result(0, &[0, 0]).err(), Some(CodecError::Truncated));
}

#[test]
fn inputs_results_properties_and_footer_debug_are_fully_redacted() {
    let features: &[&[u8]] = &[b"fixture-private-feature"];
    let properties = [RpcProperty { key: b"fixture-private-key", value: b"fixture-private-value" }];
    let inputs = EndpointConnectInputs { content: b"fixture-private-content", features, properties: &properties };
    assert_eq!(format!("{inputs:?}"), "EndpointConnectInputs(<redacted>)");
    assert_eq!(format!("{inputs:#?}"), "EndpointConnectInputs(<redacted>)");
    let encoded = encode_endpoint_connect(inputs).unwrap();
    assert!(!format!("{encoded:?}").contains("fixture"));
    let raw = result(1, features, &properties, b"", Some(&properties));
    let decoded = decode_endpoint_connect_result(0, &raw).unwrap();
    assert_eq!(format!("{decoded:?}"), "EndpointConnectResult(<redacted>)");
    assert_eq!(format!("{decoded:#?}"), "EndpointConnectResult(<redacted>)");
    assert_eq!(format!("{:?}", decoded.properties[0]), "RpcProperty(<redacted>)");
    assert_eq!(format!("{:?}", decoded.footer), "RpcFooter(<redacted>)");
}
