//! Actions-only exact controls and original ARC/direct/batch provenance.
use super::*;
use super::super::{
    arc_session::{ArcLoginInputs, ArcPhase},
    offline_backend::{OfflineBackend, OfflineEvent},
    rpc_envelope::{decode_reply, decode_request, encode_reply, encode_request},
    rpc_stream::Writer,
    session::HostingSession,
    tests::imported,
};

#[path = "client_ex_notification_tests.rs"]
mod notification_rejection;

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("fixture-client-ex-line".into());
    session.import(imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn binding() -> IncomingBinding<'static> {
    IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"fixture.kind" }
}
fn login() -> ArcLoginInputs<'static> {
    ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-selector",
        phone_login_token: b"fixture-phone", application_key: b"fixture-key", domain_id: b"fixture-domain", application_id: b"fixture-app",
        device_info_cc: b"", client_properties: &[] }
}
fn login_result() -> Vec<u8> {
    let mut writer = Writer::new(); writer.boolean(true).unwrap(); writer.bytes(b"fixture-user").unwrap(); writer.bytes(b"fixture-auth").unwrap();
    writer.properties(&[]).unwrap(); writer.bytes(b"").unwrap(); writer.finish().expose_to_transport().to_vec()
}
fn reply_packet(id: i32, result: &[u8]) -> Vec<u8> {
    let bytes = encode_reply(id, 0, result).unwrap(); let mut packet = vec![1]; packet.extend_from_slice(bytes.expose_to_transport()); packet
}
fn arguments(kind: ClientExKind, id: u64, reason: &[u8]) -> Vec<u8> {
    let mut writer = Writer::versioned().unwrap();
    match kind {
        ClientExKind::CheckAlive => {},
        ClientExKind::NotifyCalls => { writer.u64(id).unwrap(); writer.properties(&[]).unwrap(); },
        ClientExKind::NotifyDisconnect => writer.bytes(reason).unwrap(),
    }
    writer.finish().expose_to_transport().to_vec()
}
fn method(kind: ClientExKind) -> &'static [u8] {
    match kind { ClientExKind::CheckAlive => CHECK_ALIVE_METHOD.as_bytes(), ClientExKind::NotifyCalls => NOTIFY_CALLS_METHOD.as_bytes(),
        ClientExKind::NotifyDisconnect => NOTIFY_DISCONNECT_METHOD.as_bytes() }
}
fn request(id: i32, kind: ClientExKind, arguments: &[u8], token: Option<&[u8]>, magic: &[u8]) -> Vec<u8> {
    let mut metadata = vec![RpcProperty { key: b"id", value: binding().object_id }, RpcProperty { key: b"domain", value: binding().domain },
        RpcProperty { key: b"__magic", value: magic }];
    if let Some(value) = token { metadata.push(RpcProperty { key: b"token", value }); }
    let bytes = encode_request(id, &metadata, method(kind), arguments).unwrap(); let mut packet = vec![0]; packet.extend_from_slice(bytes.expose_to_transport()); packet
}
fn authenticated(arm: bool) -> (AttemptOwner, RpcConnectionScope, OfflineBackend) {
    let original = owner(); let mut backend = OfflineBackend::new(original.clone()); let scope = backend.connection_scope();
    if arm { backend.arm_client_ex(binding()).unwrap(); }
    let login = backend.prepare_login(login(), &[]).unwrap();
    let events = backend.on_plaintext_packet(&original, &scope, &reply_packet(login.call_id, &login_result()), 0).unwrap();
    assert!(matches!(events.as_slice(), [OfflineEvent::Callback(_)]));
    backend.configure(7, Some(binding())).unwrap();
    if arm { backend.configure_client_ex().unwrap(); }
    (original, scope, backend)
}
fn control_result(events: &[OfflineEvent]) -> Vec<u8> {
    let packets = events.iter().find_map(|event| if let OfflineEvent::Outgoing(packets) = event { Some(packets) } else { None }).unwrap();
    assert_eq!(packets.packets.len(), 1); let packet = packets.packets[0].expose_to_transport(); assert_eq!(packet[0], 1);
    let reply = decode_reply(&packet[1..]).unwrap(); assert_eq!(reply.result_code, 0); reply.result_stream.to_vec()
}
fn batch_result(outer: &[u8], kind: ClientExKind, argument: &[u8], token: Option<&[u8]>) -> Vec<u8> {
    let request = decode_request(outer).unwrap(); let mut reader = Reader::versioned(request.arguments).unwrap();
    assert_eq!(reader.bytes().unwrap(), b"fixture-user"); assert_eq!(reader.bytes().unwrap(), b"fixture-selector");
    let payload = super::super::rpc_batch::read_payload(&mut reader).unwrap(); reader.finish().unwrap();
    let mut writer = Writer::new(); writer.boolean(true).unwrap();
    for word in [99, 1, 1, payload.fields[0], payload.fields[1], payload.fields[2]] { writer.u32(word).unwrap(); }
    writer.u32(1).unwrap(); writer.i32(1).unwrap(); writer.bytes(method(kind)).unwrap(); writer.bytes(argument).unwrap();
    let mut metadata = vec![RpcProperty { key: b"id", value: binding().object_id }, RpcProperty { key: b"domain", value: binding().domain }];
    if let Some(value) = token { metadata.push(RpcProperty { key: b"token", value }); }
    writer.properties(&metadata).unwrap(); writer.u32(0).unwrap(); writer.properties(&[]).unwrap(); writer.bytes(b"").unwrap();
    reply_packet(request.call_id, writer.finish().expose_to_transport())
}

#[test]
fn exact_client_ex_methods_do_not_accept_aliases_prefixes_or_callclientex_methods() {
    for (bytes, expected) in [(CHECK_ALIVE_METHOD.as_bytes(), ClientExKind::CheckAlive), (NOTIFY_CALLS_METHOD.as_bytes(), ClientExKind::NotifyCalls),
        (NOTIFY_DISCONNECT_METHOD.as_bytes(), ClientExKind::NotifyDisconnect)] { assert_eq!(method_kind(bytes), Some(expected)); }
    for bytes in [b"checkAlive".as_slice(), b"CheckAlive.ClientEx.Client", b"notifyCalls.ClientEx.Client.extra", b"notifyDisconnect.ClientEx.Client\0",
        b"onInvited.CallClientEx.CallEx", b"recvOnlineMessage.MessageReceiver.Client"] { assert!(method_kind(bytes).is_none()); }
}
#[test]
fn check_alive_result_is_exact_bool_unsigned32_footer_not_a_login_or_hosting_grant() {
    assert_eq!(encode_result(ClientExKind::CheckAlive, false, 0).unwrap().expose_to_transport(), &[0; 10]);
    assert_eq!(encode_result(ClientExKind::CheckAlive, true, 0x8123_4567).unwrap().expose_to_transport(), &[0, 1, 0x81, 0x23, 0x45, 0x67, 0, 0, 0, 0]);
    for kind in [ClientExKind::NotifyCalls, ClientExKind::NotifyDisconnect] {
        assert_eq!(encode_result(kind, true, 0).unwrap().expose_to_transport(), &[0, 1, 0, 0, 0, 0]);
    }
}
#[test]
fn registered_sdk_without_independent_account_alive_proof_answers_false_and_zero() {
    let (original, scope, mut backend) = authenticated(true);
    let events = backend.on_plaintext_packet(&original, &scope, &request(1, ClientExKind::CheckAlive, &[0, 0, 1, 0, 0], None, b"alive"), 1).unwrap();
    assert_eq!(control_result(&events), vec![0; 10]); assert_eq!(events.len(), 1);
    assert_eq!(backend.phase(), ArcPhase::Authenticated); assert!(!scope.session_revoked()); assert!(scope.is_open());
}
#[test]
fn notify_calls_keeps_full64_notification_id_and_is_neither_sms_nor_call_payload() {
    let (original, scope, mut backend) = authenticated(true);
    let argument = arguments(ClientExKind::NotifyCalls, 0xf123_4567_89ab_cdef, b"");
    assert_eq!(argument, [0, 0, 1, 0, 0, 0xf1, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0, 0, 0, 0]);
    let events = backend.on_plaintext_packet(&original, &scope, &request(1, ClientExKind::NotifyCalls, &argument, None, &[0xff, 0, 1]), 1).unwrap();
    assert_eq!(control_result(&events), [0, 1, 0, 0, 0, 0]); assert_eq!(events.len(), 2);
    let OfflineEvent::ClientEx(notice) = &events[1] else { panic!("expected original poll hint"); };
    assert!(notice.matches(&original, &scope)); assert_eq!(notice.control(), ClientExControl::PollHint { notification_id: 0xf123_4567_89ab_cdef });
    assert!(!format!("{notice:?}").contains("fixture")); assert!(!scope.session_revoked());
}
#[test]
fn direct_magic_duplicates_cannot_generate_another_hint_or_consume_a_pending_sms() {
    let (original, scope, mut backend) = authenticated(true); let argument = arguments(ClientExKind::NotifyCalls, 42, b"");
    let first = request(1, ClientExKind::NotifyCalls, &argument, None, b"binary\0magic");
    assert_eq!(backend.on_plaintext_packet(&original, &scope, &first, 1).unwrap().len(), 2);
    assert!(backend.on_plaintext_packet(&original, &scope, &first, 2).unwrap().is_empty());
    let another = request(2, ClientExKind::NotifyCalls, &argument, None, b"binary\0magic");
    assert!(backend.on_plaintext_packet(&original, &scope, &another, 3).unwrap().is_empty()); assert!(scope.is_open());
}
#[test]
fn wrong_app_phone_or_absent_token_cannot_revoke_the_original_native_session() {
    let (original, scope, mut backend) = authenticated(true);
    for (index, token) in [None, Some(b"fixture-phone".as_slice()), Some(b"fixture-app-token"), Some(b"fixture-auth-extra")].into_iter().enumerate() {
        let id = index as i32 + 1; let events = backend.on_plaintext_packet(&original, &scope,
            &request(id, ClientExKind::NotifyDisconnect, &arguments(ClientExKind::NotifyDisconnect, 0, b"another-device-logined:private"), token, &id.to_be_bytes()), 1).unwrap();
        assert_eq!(control_result(&events), [0, 0, 0, 0, 0, 0]); assert_eq!(events.len(), 1);
        assert!(!scope.session_revoked()); assert!(scope.is_open());
    }
}
#[test]
fn matching_native_token_latches_all_disconnect_reasons_before_reply_or_future_rpc() {
    for (reason, expected) in [(b"another-device-logined:private".as_slice(), DisconnectReason::AnotherDevice),
        (b"same-device-login:private", DisconnectReason::SameDevice), (b"server-kick-off:private", DisconnectReason::ServerKick),
        (&[0xff, 0, 8], DisconnectReason::Other)] {
        let (original, scope, mut backend) = authenticated(true);
        let events = backend.on_plaintext_packet(&original, &scope, &request(1, ClientExKind::NotifyDisconnect,
            &arguments(ClientExKind::NotifyDisconnect, 0, reason), Some(b"fixture-auth"), b"revoke"), 1).unwrap();
        assert!(scope.session_revoked(), "revocation is locked before an actor can write any reply");
        assert_eq!(control_result(&events), [0, 1, 0, 0, 0, 0]); let OfflineEvent::ClientEx(notice) = &events[1] else { panic!("revoked notice"); };
        assert!(notice.is_revocation_for(&original, &scope)); assert_eq!(notice.control(), ClientExControl::Revoked { reason: expected });
        assert!(!notice.matches(&original, &scope)); assert!(!notice.is_revocation_for(&owner(), &scope));
        assert!(backend.prepare_swap(2, &[]).is_err()); assert!(backend.prepare_logout(&[]).is_err());
        scope.close(); assert!(scope.session_revoked()); assert!(notice.is_revocation_for(&original, &scope));
        assert!(!RpcConnectionScope::fresh().session_revoked());
    }
}
#[test]
fn unrelated_owner_or_scope_is_ignored_before_control_parsing_or_revocation() {
    let (original, scope, mut backend) = authenticated(true);
    assert!(backend.on_plaintext_packet(&owner(), &scope, &[0xff], 1).unwrap().is_empty());
    assert!(backend.on_plaintext_packet(&original, &RpcConnectionScope::fresh(), &[0xff], 1).unwrap().is_empty());
    assert!(scope.is_open()); assert!(!scope.session_revoked());
}
#[test]
fn old_explicit_receiver_binding_and_disabled_client_ex_still_refuse_control_methods() {
    let (original, scope, mut backend) = authenticated(false);
    let packet = request(1, ClientExKind::CheckAlive, &arguments(ClientExKind::CheckAlive, 0, b""), None, b"disabled");
    assert!(backend.on_plaintext_packet(&original, &scope, &packet, 1).is_err()); assert!(!scope.is_open()); assert!(!scope.session_revoked());
}
#[test]
fn wrong_control_object_or_domain_does_not_guess_a_native_default_or_revoke() {
    for (key, value) in [(b"id".as_slice(), b"MessageReceiver".as_slice()), (b"domain", b"foreign-domain"), (b"domain", b"")] {
        let (original, scope, mut backend) = authenticated(true);
        let argument = arguments(ClientExKind::NotifyDisconnect, 0, b"server-kick-off");
        let mut metadata = vec![RpcProperty { key: b"id", value: binding().object_id }, RpcProperty { key: b"domain", value: binding().domain },
            RpcProperty { key: b"token", value: b"fixture-auth" }, RpcProperty { key: b"__magic", value: b"wrong-binding" }];
        metadata.iter_mut().find(|property| property.key == key).unwrap().value = value;
        let bytes = encode_request(1, &metadata, method(ClientExKind::NotifyDisconnect), &argument).unwrap();
        let mut packet = vec![0]; packet.extend_from_slice(bytes.expose_to_transport());
        assert!(backend.on_plaintext_packet(&original, &scope, &packet, 1).is_err()); assert!(!scope.session_revoked());
    }
}
#[test]
fn malformed_versions_truncation_and_trailing_bytes_never_latch_a_disconnect() {
    let valid = arguments(ClientExKind::NotifyDisconnect, 0, b"server-kick-off");
    let mut cases = (0..valid.len()).map(|size| valid[..size].to_vec()).collect::<Vec<_>>();
    let mut version = valid.clone(); version[4] = 1; cases.push(version);
    let mut trailing = valid; trailing.push(0); cases.push(trailing);
    for argument in cases {
        let (original, scope, mut backend) = authenticated(true);
        assert!(backend.on_plaintext_packet(&original, &scope, &request(1, ClientExKind::NotifyDisconnect, &argument, Some(b"fixture-auth"), b"malformed"), 1).is_err());
        assert!(!scope.session_revoked());
    }
}
#[test]
fn nonempty_notification_hook_map_is_rejected_not_swallowed_as_a_successful_hint() {
    let (original, scope, mut backend) = authenticated(true);
    let mut writer = Writer::versioned().unwrap(); writer.u64(9).unwrap();
    writer.properties(&[RpcProperty { key: b"unknown-oem-hook", value: b"fixture-private-data" }]).unwrap();
    let packet = request(1, ClientExKind::NotifyCalls, writer.finish().expose_to_transport(), None, b"hook");
    assert!(backend.on_plaintext_packet(&original, &scope, &packet, 1).is_err()); assert!(!scope.is_open()); assert!(!scope.session_revoked());
}
#[test]
fn original_batch_hint_queues_only_its_control_result_and_is_not_replayed() {
    let (original, scope, mut backend) = authenticated(true); let argument = arguments(ClientExKind::NotifyCalls, u64::MAX, b"");
    for index in 0..2 {
        let outer = backend.prepare_swap(index, &[]).unwrap(); let packet = outer.packets[0].expose_to_transport();
        if index == 1 {
            let request = decode_request(&packet[1..]).unwrap(); let mut reader = Reader::versioned(request.arguments).unwrap();
            reader.bytes().unwrap(); reader.bytes().unwrap(); let payload = super::super::rpc_batch::read_payload(&mut reader).unwrap();
            assert_eq!(payload.results.get(&1).unwrap().result_stream, &[0, 1, 0, 0, 0, 0]);
        }
        let result = batch_result(&packet[1..], ClientExKind::NotifyCalls, &argument, None);
        let events = backend.on_plaintext_packet(&original, &scope, &result, index + 1).unwrap();
        let notices = events.iter().filter(|event| matches!(event, OfflineEvent::ClientEx(_))).count();
        assert_eq!(notices, usize::from(index == 0));
        assert!(!events.iter().any(|event| matches!(event, OfflineEvent::Message(_) | OfflineEvent::DirectMessage(_) | OfflineEvent::Call(_))));
    }
    assert!(!scope.session_revoked()); assert!(scope.is_open());
}
#[test]
fn batch_disconnect_latches_before_result_queue_and_prevents_a_new_ack_poll_or_owner() {
    let (original, scope, mut backend) = authenticated(true); let outer = backend.prepare_swap(0, &[]).unwrap();
    let argument = arguments(ClientExKind::NotifyDisconnect, 0, b"same-device-login:fixture");
    let result = batch_result(&outer.packets[0].expose_to_transport()[1..], ClientExKind::NotifyDisconnect, &argument, Some(b"fixture-auth"));
    let events = backend.on_plaintext_packet(&original, &scope, &result, 1).unwrap();
    assert!(scope.session_revoked()); assert!(events.iter().any(|event| matches!(event, OfflineEvent::ClientEx(notice) if notice.is_revocation_for(&original, &scope))));
    assert!(backend.prepare_swap(2, &[]).is_err()); assert!(backend.prepare_logout(&[]).is_err());
    backend.close(); assert!(scope.session_revoked()); assert!(!scope.is_open());
}
#[test]
fn prearmed_binding_activates_only_on_matched_login_and_catches_coalesced_direct_kick() {
    let original = owner(); let mut backend = OfflineBackend::new(original.clone()); let scope = backend.connection_scope();
    backend.arm_client_ex(binding()).unwrap(); let login = backend.prepare_login(login(), &[]).unwrap();
    assert!(backend.on_plaintext_packet(&original, &RpcConnectionScope::fresh(), &[0xff], 0).unwrap().is_empty());
    let accepted = backend.on_plaintext_packet(&original, &scope, &reply_packet(login.call_id, &login_result()), 0).unwrap();
    assert_eq!(accepted.len(), 1); // No scheduler/consumer/hosting was configured.
    let events = backend.on_plaintext_packet(&original, &scope, &request(1, ClientExKind::NotifyDisconnect,
        &arguments(ClientExKind::NotifyDisconnect, 0, b"server-kick-off"), Some(b"fixture-auth"), b"coalesced"), 0).unwrap();
    assert_eq!(control_result(&events), [0, 1, 0, 0, 0, 0]); assert!(scope.session_revoked());
}
#[test]
fn prearmed_binding_cannot_be_rebound_during_later_receiver_configuration() {
    let original = owner(); let mut backend = OfflineBackend::new(original.clone()); let scope = backend.connection_scope();
    backend.arm_client_ex(binding()).unwrap(); let login = backend.prepare_login(login(), &[]).unwrap();
    backend.on_plaintext_packet(&original, &scope, &reply_packet(login.call_id, &login_result()), 0).unwrap();
    assert!(backend.configure(7, Some(IncomingBinding { object_id: b"#different", ..binding() })).is_err());
    assert!(backend.configure(7, Some(binding())).is_ok()); assert!(backend.configure_client_ex().is_ok());
    assert!(backend.configure_client_ex().is_err()); assert!(scope.is_open());
}
#[test]
fn client_ex_private_option_defaults_off_roundtrips_and_requires_original_receiver_without_native_defaults() {
    let cert = super::super::tls_engine::tests::Certificates::generate("DNS:fixture.invalid");
    let mut config = super::super::account_test_support::config(&cert.root);
    assert!(!config.profile.client_ex);
    let old = config.encode_private().unwrap(); assert!(!std::str::from_utf8(&old).unwrap().contains("client_ex"));
    config.profile.client_ex = true; let encoded = config.encode_private().unwrap();
    let decoded = super::super::account_config::AccountConfig::decode_private(&encoded).unwrap();
    assert!(decoded.profile.client_ex); assert!(decoded.runtime_profile().is_ok());
    assert_eq!(decoded.profile.incoming.as_ref().unwrap().object_id, b"#fixture-client");
    config.profile.incoming = None; assert!(config.runtime_profile().is_err());
}
