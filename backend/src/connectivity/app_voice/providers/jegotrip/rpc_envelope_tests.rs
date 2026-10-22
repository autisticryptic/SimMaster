use super::*;

#[test]
fn reply_encoder_independent_fixture_preserves_code_and_nested_terminal_stream() {
    let bytes = encode_reply(7, 0, &[0, 1, 0, 0, 0, 0]).unwrap();
    assert_eq!(bytes.expose_to_transport(), &[0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0, 6, 0, 1, 0, 0, 0, 0]);
    let reply = decode_reply(bytes.expose_to_transport()).unwrap();
    assert_eq!(reply.call_id, 7); assert_eq!(reply.result_code, 0); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]);
    assert_eq!(encode_reply(0, 0, b"result").err(), Some(CodecError::InvalidCallId));
    assert_eq!(encode_reply(1, 2, &vec![0; super::super::rpc_stream::MAX_FIELD_BYTES + 1]).err(), Some(CodecError::TooLarge));
    let bytes = encode_reply(1, 2, &[0, 0xff]).unwrap();
    assert_eq!(decode_reply(bytes.expose_to_transport()).unwrap().result_code, 2);
}

use crate::connectivity::app_voice::providers::jegotrip::{
    rpc_arguments::{encode_update_status, UPDATE_STATUS_METHOD},
    session::{GatewayRegistration, STATUS_REGISTER},
};

#[test]
fn request_bytes_match_independent_type_id_count_method_and_raw_stream_layout() {
    let bytes = encode_request(42, &[RpcProperty { key: b"id", value: b"obj" }], b"m", &[0, 0xff]).unwrap();
    let expected = [
        0, 0, 0, 0, 42, 0, 0, 0, 1,
        0, 0, 0, 2, b'i', b'd', 0, 0, 0, 3, b'o', b'b', b'j',
        0, 0, 0, 1, b'm', 0, 0, 0, 2, 0, 0xff,
    ];
    assert_eq!(bytes.expose_to_transport(), &expected);
    let request = decode_request(&expected).unwrap();
    assert_eq!(request.call_id, 42);
    assert_eq!(request.method, b"m");
    assert_eq!(request.arguments, &[0, 0xff]);
    assert_eq!(request.metadata[0], RpcProperty { key: b"id", value: b"obj" });
}

#[test]
fn request_metadata_keeps_binary_magic_and_uses_native_key_order() {
    let fields = [
        RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &[0, 0xff, 0, 1] },
        RpcProperty { key: b"id", value: b"fixture-object" },
    ];
    let bytes = encode_request(1, &fields, b"method", &[0]).unwrap();
    let request = decode_request(bytes.expose_to_transport()).unwrap();
    assert_eq!(request.metadata.iter().map(|f| f.key).collect::<Vec<_>>(),
        vec![b"id".as_slice(), b"domain", b"__magic"]);
    assert_eq!(request.metadata[2].value, &[0, 0xff, 0, 1]);
    assert!(!format!("{request:?}").contains("fixture"));
}

#[test]
fn reply_code_and_nested_bytes_are_not_mistaken_for_success() {
    let bytes = [0, 0, 0, 0, 7, 0, 2, 0, 3, 0, 0, 0, 2, 0, 0xff];
    let reply = decode_reply(&bytes).unwrap();
    assert_eq!(reply.call_id, 7);
    assert_eq!(reply.result_code, 0x0002_0003);
    assert_eq!(reply.result_stream, &[0, 0xff]);
}

#[test]
fn request_and_reply_truncation_and_trailing_data_fail_explicitly() {
    let request = encode_request(1, &[], b"m", &[0]).unwrap();
    for length in 0..request.expose_to_transport().len() {
        assert!(decode_request(&request.expose_to_transport()[..length]).is_err());
    }
    let reply = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0];
    for length in 0..reply.len() { assert!(decode_reply(&reply[..length]).is_err()); }
    let mut extra = request.expose_to_transport().to_vec();
    extra.push(0);
    assert_eq!(decode_request(&extra).err(), Some(CodecError::TrailingData));
    let mut extra = reply.to_vec(); extra.push(0);
    assert_eq!(decode_reply(&extra).err(), Some(CodecError::TrailingData));
}

#[test]
fn invalid_ids_empty_method_and_oversized_lengths_fail_closed() {
    for id in [0, -1, i32::MIN] {
        assert_eq!(encode_request(id, &[], b"m", &[]).err(), Some(CodecError::InvalidCallId));
        let mut reply = vec![0]; reply.extend_from_slice(&id.to_be_bytes()); reply.extend_from_slice(&[0; 8]);
        assert_eq!(decode_reply(&reply).err(), Some(CodecError::InvalidCallId));
    }
    assert_eq!(encode_request(1, &[], b"", &[]).err(), Some(CodecError::InvalidField));
    let oversized = [0, 0, 0, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(decode_reply(&oversized).err(), Some(CodecError::TooLarge));
}

#[test]
fn hosting_argument_stream_is_nested_with_its_type_marker_intact() {
    let credentials = GatewayRegistration { authname_prefix: "prefix", channel_id: "channel",
        delete_int_code: "1", sip_number: "number", sip_password: "fixture-secret-password" };
    let arguments = encode_update_status("actual-sdk-uri", STATUS_REGISTER, &credentials).unwrap();
    let bytes = encode_request(9, &[RpcProperty { key: b"id", value: b"#SipGateway" }],
        UPDATE_STATUS_METHOD.as_bytes(), arguments.expose_to_transport()).unwrap();
    let request = decode_request(bytes.expose_to_transport()).unwrap();
    assert_eq!(request.arguments, arguments.expose_to_transport());
    assert_eq!(&request.arguments[..5], &[0, 0, 1, 0, 0]);
    assert!(!format!("{bytes:?}").contains("fixture-secret-password"));
}
