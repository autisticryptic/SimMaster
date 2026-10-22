use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{rpc_stream::Writer, session::HostingSession, tests::imported};

fn owner() -> AttemptOwner {
    let mut s = HostingSession::new("line-a".into()); s.import(imported(), 100).unwrap(); s.begin_login(100).unwrap()
}
fn arguments(kind: &[u8], properties: &[RpcProperty<'_>], body: &[u8]) -> RpcBytes {
    let mut w = Writer::versioned().unwrap(); w.bytes(kind).unwrap(); w.properties(properties).unwrap(); w.bytes(body).unwrap(); w.finish()
}
fn dispatcher(owner: AttemptOwner, scope: RpcConnectionScope) -> IncomingDispatcher {
    IncomingDispatcher::for_arc_session(owner, scope, IncomingBinding { object_id: b"#fixture-incoming", domain: b"fixture-domain",
        receiver_kind: b"fixture.registered.message-kind" }).unwrap()
}

#[test]
fn online_message_independent_fixture_uses_type_version_kind_properties_and_raw_stream() {
    let bytes = [0, 0, 1, 0, 0, 0, 0, 0, 1, b'k', 0, 0, 0, 0, 0, 0, 0, 2, 0xff, 0];
    let message = decode_online_message(&bytes).unwrap();
    assert_eq!(message.receiver_kind, b"k"); assert!(message.properties.is_empty()); assert_eq!(message.body, &[0xff, 0]);
    assert_eq!(encode_online_message_result(true).unwrap().expose_to_transport(), &[0, 1, 0, 0, 0, 0]);
    assert_eq!(encode_online_message_result(false).unwrap().expose_to_transport(), &[0, 0, 0, 0, 0, 0]);
}

#[test]
fn incoming_parameters_preserve_optional_sender_time_and_unknown_binary_properties() {
    let properties = [RpcProperty { key: b"Sip.Message.From", value: &[0xff, 0] },
        RpcProperty { key: b"Sip.Message.Time", value: b"fixture-opaque-time" },
        RpcProperty { key: b"unknown", value: &[0, 0xff] }];
    let bytes = arguments(b"fixture-kind", &properties, &[0xff, 0, 1]);
    let message = decode_online_message(bytes.expose_to_transport()).unwrap();
    assert_eq!(message.properties.len(), 3);
    for property in properties {
        assert_eq!(message.properties.iter().find(|p| p.key == property.key).unwrap().value, property.value);
    }
    assert_eq!(message.body, &[0xff, 0, 1]);
    assert!(!format!("{message:?}").contains("fixture"));
}

#[test]
fn truncated_trailing_duplicate_wrong_version_and_oversized_fields_are_not_delivered() {
    let bytes = arguments(b"kind", &[], b"body"); let bytes = bytes.expose_to_transport();
    for length in 0..bytes.len() { assert!(decode_online_message(&bytes[..length]).is_err()); }
    let mut trailing = bytes.to_vec(); trailing.push(0); assert_eq!(decode_online_message(&trailing).err(), Some(CodecError::TrailingData));
    let mut version = bytes.to_vec(); version[2] = 2; assert_eq!(decode_online_message(&version).err(), Some(CodecError::UnsupportedVersion));
    let mut duplicate = vec![0, 0, 1, 0, 0, 0, 0, 0, 1, b'k', 0, 0, 0, 2];
    for _ in 0..2 { duplicate.extend_from_slice(&[0, 0, 0, 1, b'p', 0, 0, 0, 0]); }
    assert_eq!(decode_online_message(&duplicate).err(), Some(CodecError::DuplicateKey));
    let large = [0, 0, 1, 0, 0, 0, 1, 0, 1]; assert_eq!(decode_online_message(&large).err(), Some(CodecError::TooLarge));
}

#[test]
fn exact_configured_receiver_and_original_object_domain_are_mandatory() {
    let owner = owner(); let scope = RpcConnectionScope::fresh(); let dispatcher = dispatcher(owner.clone(), scope.clone());
    let bytes = arguments(b"fixture.registered.message-kind", &[RpcProperty { key: b"Sip.Message.From", value: b"sender" }], b"body");
    let metadata = vec![RpcProperty { key: b"id", value: b"#fixture-incoming" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &[0, 0xff] }];
    let request = RpcRequest { call_id: 1, metadata, method: ONLINE_MESSAGE_METHOD.as_bytes(), arguments: bytes.expose_to_transport() };
    let message = dispatcher.on_direct_request(&owner, &scope, &request).unwrap().unwrap();
    assert_eq!(message.body, b"body"); assert_eq!(message.from, Some(b"sender".to_vec())); assert!(message.time.is_none());
    let mut wrong = request; wrong.metadata[0].value = b"#other";
    assert_eq!(dispatcher.on_direct_request(&owner, &scope, &wrong).err(), Some(CodecError::InvalidField));
    wrong.metadata[0].value = b"#fixture-incoming"; wrong.metadata[1].value = b"other-domain";
    assert_eq!(dispatcher.on_direct_request(&owner, &scope, &wrong).err(), Some(CodecError::InvalidField));
}

#[test]
fn missing_receiver_configuration_or_wildcard_is_not_guessed_from_gateway_name() {
    for kind in [b"".as_slice(), b"fixture.*", b"ENC(unresolved)"] {
        assert!(IncomingDispatcher::for_arc_session(owner(), RpcConnectionScope::fresh(), IncomingBinding {
            object_id: b"#fixture", domain: b"fixture-domain", receiver_kind: kind }).is_err());
    }
    let original = owner(); let scope = RpcConnectionScope::fresh(); let d = dispatcher(original.clone(), scope.clone());
    let bytes = arguments(b"other-kind", &[], b"body");
    let request = RpcRequest { call_id: 1, metadata: vec![RpcProperty { key: b"id", value: b"#fixture-incoming" },
        RpcProperty { key: b"domain", value: b"fixture-domain" }], method: ONLINE_MESSAGE_METHOD.as_bytes(), arguments: bytes.expose_to_transport() };
    assert!(d.on_direct_request(&original, &scope, &request).unwrap().is_none());
}

#[test]
fn wrong_scope_unsupported_outbound_method_and_closed_binding_are_checked_before_parsing() {
    let original = owner(); let scope = RpcConnectionScope::fresh(); let d = dispatcher(original.clone(), scope.clone());
    let mut request = RpcRequest { call_id: 1, metadata: vec![], method: ONLINE_MESSAGE_METHOD.as_bytes(), arguments: &[0xff] };
    assert!(d.on_direct_request(&owner(), &scope, &request).unwrap().is_none());
    assert!(d.on_direct_request(&original, &RpcConnectionScope::fresh(), &request).unwrap().is_none());
    request.method = super::super::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes();
    assert!(d.on_direct_request(&original, &scope, &request).unwrap().is_none());
    request.method = b"receiveSipMESSAGE.SipGatewayPub.SipGateway";
    assert!(d.on_direct_request(&original, &scope, &request).unwrap().is_none());
    scope.close(); request.method = ONLINE_MESSAGE_METHOD.as_bytes();
    assert!(d.on_direct_request(&original, &scope, &request).unwrap().is_none());
}

#[test]
fn configured_binding_debug_and_body_never_become_credentials_or_channel_success() {
    let binding = IncomingBinding { object_id: b"fixture-secret-object", domain: b"fixture-secret-domain", receiver_kind: b"fixture-kind" };
    assert!(!format!("{binding:?}").contains("fixture"));
    let dispatcher = IncomingDispatcher::for_arc_session(owner(), RpcConnectionScope::fresh(), binding).unwrap();
    assert!(!format!("{dispatcher:?}").contains("fixture"));
    let empty = arguments(b"kind", &[], b""); assert!(decode_online_message(empty.expose_to_transport()).unwrap().body.is_empty());
}
