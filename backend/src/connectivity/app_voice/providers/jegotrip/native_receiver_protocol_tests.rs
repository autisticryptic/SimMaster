//! Original-scope native adapter through the actual ARC/direct/batch codecs.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{arc_session::{ArcLoginInputs, ArcPhase}, offline_backend::{OfflineBackend, OfflineEvent},
    rpc_envelope::{decode_request, decode_reply, encode_request, encode_reply}, rpc_stream::{Reader, Writer}};
fn inputs() -> ArcLoginInputs<'static> { ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"original-selector",
    phone_login_token: b"phone-token", application_key: b"application-key", domain_id: b"outbound-domain", application_id: b"application-id", device_info_cc: b"", client_properties: &[] } }
fn result(id: i32, bytes: &[u8]) -> Vec<u8> { let reply = encode_reply(id, 0, bytes).unwrap(); let mut packet = vec![1]; packet.extend_from_slice(reply.expose_to_transport()); packet }
fn login_reply(id: i32) -> Vec<u8> { let mut w = Writer::new(); w.boolean(true).unwrap(); w.bytes(b"original-user").unwrap(); w.bytes(b"session-token").unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); result(id, w.finish().expose_to_transport()) }
fn setup(policy: Arc<NativeReceiverPolicy>, calls: bool) -> Arc<NativeReceiverSetup> {
    let routing = [RpcProperty { key: b"id", value: b"#original-call-control" }, RpcProperty { key: b"domain", value: b"original-control-domain" }];
    NativeReceiverSetup::capture(policy, true, calls.then_some(routing.as_slice())).unwrap()
}
fn prepared(policy: Arc<NativeReceiverPolicy>, calls: bool) -> (AttemptOwner, RpcConnectionScope, OfflineBackend) {
    let original = owner(); let mut backend = OfflineBackend::new(original.clone()); backend.claim_common_transport().unwrap(); backend.bind_common_identity(b"AccountEntry").unwrap();
    let scope = backend.connection_scope(); backend.arm_native_receivers(setup(policy, calls)).unwrap(); (original, scope, backend)
}
fn authed(policy: Arc<NativeReceiverPolicy>, calls: bool) -> (AttemptOwner, RpcConnectionScope, OfflineBackend) {
    let (original, scope, mut backend) = prepared(policy, calls); let login = backend.prepare_login(inputs(), &[]).unwrap();
    let events = backend.on_plaintext_packet(&original, &scope, &login_reply(login.call_id), 0).unwrap(); assert!(matches!(events.as_slice(), [OfflineEvent::Callback(_)]));
    backend.configure_native_receivers(17).unwrap(); (original, scope, backend)
}
fn sms_args(kind: &[u8]) -> Vec<u8> { let mut w = Writer::versioned().unwrap(); w.bytes(kind).unwrap();
    w.properties(&[RpcProperty { key: b"Sip.Message.From", value: b"fixture-sender" }]).unwrap(); w.bytes("fixture 短信".as_bytes()).unwrap(); w.finish().expose_to_transport().to_vec() }
fn request(id: i32, method: &[u8], argument: &[u8], object: &[u8], domain: Option<&[u8]>, token: Option<&[u8]>) -> Vec<u8> {
    let magic = id.to_be_bytes(); let mut props = vec![RpcProperty { key: b"id", value: object }, RpcProperty { key: b"__magic", value: &magic }];
    if let Some(value) = domain { props.push(RpcProperty { key: b"domain", value }); }
    if let Some(value) = token { props.push(RpcProperty { key: b"token", value }); }
    let r = encode_request(id, &props, method, argument).unwrap(); let mut packet = vec![0]; packet.extend_from_slice(r.expose_to_transport()); packet
}
fn batch(outer: &n::arc_protocol::OwnedCallPackets, method: &[u8], argument: &[u8], object: &[u8], domain: Option<&[u8]>) -> Vec<u8> {
    let r = decode_request(&outer.packets[0].expose_to_transport()[1..]).unwrap(); let mut rd = Reader::versioned(r.arguments).unwrap();
    assert_eq!(rd.bytes().unwrap(), b"original-user"); assert_eq!(rd.bytes().unwrap(), b"original-selector");
    let body = n::rpc_batch::read_payload(&mut rd).unwrap(); let mut w = Writer::new(); w.boolean(true).unwrap();
    for word in [55, 1, 1, body.fields[0], body.fields[1], body.fields[2]] { w.u32(word).unwrap(); }
    w.u32(1).unwrap(); w.i32(1).unwrap(); w.bytes(method).unwrap(); w.bytes(argument).unwrap();
    let mut props = vec![RpcProperty { key: b"id", value: object }]; if let Some(value) = domain { props.push(RpcProperty { key: b"domain", value }); }
    w.properties(&props).unwrap(); w.u32(0).unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); result(outer.call_id, w.finish().expose_to_transport())
}
fn direct_result(events: &[OfflineEvent]) -> Vec<u8> { let outgoing = events.iter().find_map(|event| if let OfflineEvent::Outgoing(packets) = event { Some(packets) } else { None }).unwrap();
    decode_reply(&outgoing.packets[0].expose_to_transport()[1..]).unwrap().result_stream.to_vec() }
#[test]
fn native_adapter_cannot_arm_on_legacy_or_authenticate_from_a_configuration_flag() {
    let original = owner(); let mut legacy = OfflineBackend::new(original); assert!(legacy.arm_native_receivers(setup(default_policy(), false)).is_err());
    let (original, scope, mut backend) = prepared(default_policy(), false);
    assert!(backend.configure_native_receivers(17).is_err()); assert_eq!(backend.phase(), ArcPhase::Initial);
    let packet = request(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.Request"), MESSAGE_OBJECT, None, None);
    assert!(backend.on_plaintext_packet(&original, &scope, &packet, 0).is_err()); assert!(!scope.is_open());
}
#[test]
fn matched_login_installs_native_direct_receiver_before_a_coalesced_message_or_callback_configuration() {
    let (original, scope, mut backend) = prepared(default_policy(), false); let login = backend.prepare_login(inputs(), &[]).unwrap();
    backend.on_plaintext_packet(&original, &scope, &login_reply(login.call_id), 0).unwrap();
    let packet = request(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.Request"), MESSAGE_OBJECT, None, None);
    let events = backend.on_plaintext_packet(&original, &scope, &packet, 0).unwrap();
    assert!(matches!(events.as_slice(), [OfflineEvent::DirectMessage(_)]), "no automatic message ACK or readiness");
    backend.configure_native_receivers(17).unwrap(); assert!(backend.configure_native_receivers(17).is_err());
}
#[test]
fn native_direct_message_with_absent_domain_keeps_original_ticket_and_waits_for_consumer_ack() {
    let (original, scope, mut backend) = authed(default_policy(), false);
    let packet = request(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.Request"), MESSAGE_OBJECT, None, None);
    let mut events = backend.on_plaintext_packet(&original, &scope, &packet, 1).unwrap(); assert_eq!(events.len(), 1);
    let OfflineEvent::DirectMessage(message) = events.remove(0) else { panic!("original native receiver"); };
    assert_eq!(message.message().body, "fixture 短信".as_bytes()); assert_eq!(message.owner(), &original);
    assert!(backend.on_plaintext_packet(&original, &scope, &packet, 2).unwrap().is_empty());
    let reply = backend.complete_direct_message(message.reply_ticket(), true).unwrap().unwrap();
    assert_eq!(decode_reply(&reply.packets[0].expose_to_transport()[1..]).unwrap().result_stream, [0, 1, 0, 0, 0, 0]);
    assert!(backend.complete_direct_message(message.reply_ticket(), true).unwrap().is_none());
}
#[test]
fn native_batch_absent_domain_bypasses_only_direct_filter_not_authenticated_outer_scope() {
    let (original, scope, mut backend) = authed(policy(DomainMode::AllowList), false); let outer = backend.prepare_swap(1, &[]).unwrap();
    let packet = batch(&outer, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.Request"), MESSAGE_OBJECT, None);
    assert!(backend.on_plaintext_packet(&owner(), &scope, &[0xff], 1).unwrap().is_empty());
    assert!(backend.on_plaintext_packet(&original, &RpcConnectionScope::fresh(), &[0xff], 1).unwrap().is_empty());
    let events = backend.on_plaintext_packet(&original, &scope, &packet, 2).unwrap();
    let message = events.into_iter().find_map(|event| if let OfflineEvent::Message(message) = event { Some(message) } else { None }).unwrap();
    assert_eq!(message.message().body, "fixture 短信".as_bytes()); assert!(backend.complete_message(message.ticket(), true).unwrap());
    assert!(!backend.complete_message(message.ticket(), true).unwrap());
}
#[test]
fn denied_direct_domain_never_delivers_native_sms_even_with_a_matching_object() {
    for domain in [None, Some(b"other".as_slice()), Some(b"")] {
        let (original, scope, mut backend) = authed(policy(DomainMode::AllowList), false);
        let packet = request(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.Request"), MESSAGE_OBJECT, domain, None);
        assert!(backend.on_plaintext_packet(&original, &scope, &packet, 1).is_err()); assert!(!scope.is_open());
    }
}
#[test]
fn unknown_native_kind_gets_negative_reply_and_does_not_become_sms_or_offline_hint() {
    let (original, scope, mut backend) = authed(default_policy(), false);
    let events = backend.on_plaintext_packet(&original, &scope, &request(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.request"), MESSAGE_OBJECT, None, None), 1).unwrap();
    assert_eq!(events.len(), 1); assert_eq!(direct_result(&events), [0, 0, 0, 0, 0, 0]); assert!(scope.is_open());
}
#[test]
fn native_offline_hint_uses_message_object_and_retains_64bit_position_not_a_sms_body() {
    let (original, scope, mut backend) = authed(default_policy(), false); let mut writer = Writer::versioned().unwrap(); writer.i64(0x1_0000_0010).unwrap();
    let events = backend.on_plaintext_packet(&original, &scope, &request(1, n::rpc_offline::MORE_OFFLINE_METHOD.as_bytes(), writer.finish().expose_to_transport(), MESSAGE_OBJECT, None, None), 1).unwrap();
    assert_eq!(events.len(), 1); let OfflineEvent::OfflineHint(hint) = &events[0] else { panic!("native offline hint"); };
    assert_eq!(hint.position(), 0x1_0000_0010); assert!(hint.matches(&original, &scope));
    assert!(backend.complete_offline_hint(hint, true).unwrap().is_some());
}
#[test]
fn native_call_receiver_activates_original_control_book_before_coalesced_invitation() {
    let (original, scope, mut backend) = prepared(default_policy(), true); let login = backend.prepare_login(inputs(), &[]).unwrap();
    backend.on_plaintext_packet(&original, &scope, &login_reply(login.call_id), 0).unwrap();
    let mut w = Writer::versioned().unwrap(); w.u64(0x1_0000_0001).unwrap(); w.bytes(b"fixture-peer").unwrap(); w.properties(&[]).unwrap();
    let events = backend.on_plaintext_packet(&original, &scope, &request(1, n::rpc_calls::INVITED_METHOD.as_bytes(), w.finish().expose_to_transport(), CALL_OBJECT, None, None), 1).unwrap();
    let call = events.into_iter().find_map(|event| if let OfflineEvent::Call(call) = event { Some(call) } else { None }).unwrap();
    assert_eq!(call.notice().native_id(), n::rpc_calls::NativeCallId::from_bits(0x1_0000_0001));
    assert!(call.ticket().control_ticket().is_some()); assert!(call.ticket().matches(&original, &scope));
    assert!(backend.complete_call(call.ticket(), true, &[]).unwrap().is_some());
    backend.configure_native_receivers(17).unwrap();
}
#[test]
fn native_client_ex_still_requires_current_arc_token_and_never_invents_alive_success() {
    let (original, scope, mut backend) = authed(default_policy(), false);
    let empty = Writer::versioned().unwrap().finish(); let alive = request(1, n::rpc_client_ex::CHECK_ALIVE_METHOD.as_bytes(), empty.expose_to_transport(), MESSAGE_OBJECT, None, None);
    assert_eq!(direct_result(&backend.on_plaintext_packet(&original, &scope, &alive, 1).unwrap()), [0; 10]);
    let mut w = Writer::versioned().unwrap(); w.bytes(b"server-kick-off").unwrap(); let reason = w.finish();
    let stale = request(2, n::rpc_client_ex::NOTIFY_DISCONNECT_METHOD.as_bytes(), reason.expose_to_transport(), MESSAGE_OBJECT, None, Some(b"phone-token"));
    assert_eq!(direct_result(&backend.on_plaintext_packet(&original, &scope, &stale, 2).unwrap()), [0, 0, 0, 0, 0, 0]); assert!(!scope.session_revoked());
    let valid = request(3, n::rpc_client_ex::NOTIFY_DISCONNECT_METHOD.as_bytes(), reason.expose_to_transport(), MESSAGE_OBJECT, None, Some(b"session-token"));
    backend.on_plaintext_packet(&original, &scope, &valid, 3).unwrap(); assert!(scope.session_revoked()); assert!(backend.prepare_swap(4, &[]).is_err());
}
#[test]
fn native_mode_cannot_rebind_after_login_to_legacy_objects_or_a_different_policy() {
    let (original, scope, mut backend) = authed(default_policy(), false);
    let old = n::rpc_incoming::IncomingBinding { object_id: b"#other", domain: b"other", receiver_kind: b"Sip.Request" };
    assert!(backend.configure(18, Some(old)).is_err()); assert!(backend.arm_native_receivers(setup(policy(DomainMode::DenyList), false)).is_err());
    assert!(backend.configure_calls(n::call_binding::CallIncomingBinding { object_id: b"other", domain: b"other" }).is_err());
    backend.prepare_logout(&[]).unwrap(); assert!(backend.configure_native_receivers(18).is_err());
    let packet = request(1, n::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), &sms_args(b"Sip.Request"), MESSAGE_OBJECT, None, None);
    assert!(backend.on_plaintext_packet(&original, &scope, &packet, 4).unwrap().is_empty());
}
