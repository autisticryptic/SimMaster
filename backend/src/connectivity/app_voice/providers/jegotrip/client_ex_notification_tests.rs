//! Actions-only recognized SDK notification rejection, not OEM handling success.
use super::*;
fn fields(command: &'static [u8]) -> Vec<RpcProperty<'static>> {
    let (key1, key2): (&[u8], &[u8]) = if command == b"Message" { (b"Notify.MsgIdx", b"Notify.Box") } else { (b"Notify.CallId", b"Notify.Caller") };
    vec![RpcProperty { key: b"Notify.Command", value: command }, RpcProperty { key: key1, value: b"123456789" }, RpcProperty { key: key2, value: b"fixture-private-target" }]
}
fn notify(properties: &[RpcProperty<'_>]) -> Vec<u8> {
    let mut writer = Writer::versioned().unwrap(); writer.u64(0xf123_4567_89ab_cdef).unwrap(); writer.properties(properties).unwrap();
    writer.finish().expose_to_transport().to_vec()
}
#[test]
fn known_message_and_invite_are_explicitly_rejected_without_closing_or_faking_success() {
    for command in [b"Message".as_slice(), b"Invite".as_slice()] {
        let (owner, scope, mut backend) = authenticated(true);
        let events = backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&fields(command)), None, b"known"), 1).unwrap();
        assert_eq!(control_result(&events), [0; 6]); assert_eq!(events.len(), 1);
        assert_eq!(backend.phase(), ArcPhase::Authenticated); assert!(scope.is_open()); assert!(!scope.session_revoked());
    }
}
#[test]
fn negative_hook_reply_cannot_refresh_alive_or_become_an_empty_map_poll_hint() {
    let (owner, scope, mut backend) = authenticated(true);
    let rejected = backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&fields(b"Message")), None, b"message"), 1).unwrap();
    assert_eq!(rejected.len(), 1); assert_eq!(control_result(&rejected), [0; 6]);
    let alive = backend.on_plaintext_packet(&owner, &scope, &request(2, ClientExKind::CheckAlive, &arguments(ClientExKind::CheckAlive, 0, b""), None, b"alive"), 2).unwrap();
    assert_eq!(control_result(&alive), [0; 10]); assert_eq!(alive.len(), 1);
    let empty = backend.on_plaintext_packet(&owner, &scope, &request(3, ClientExKind::NotifyCalls, &arguments(ClientExKind::NotifyCalls, 4, b""), None, b"empty"), 3).unwrap();
    assert_eq!(control_result(&empty), [0, 1, 0, 0, 0, 0]); assert!(empty.iter().any(|e| matches!(e, OfflineEvent::ClientEx(_))));
}
#[test]
fn known_business_values_remain_uninterpreted_and_never_route_or_publish_events() {
    let (owner, scope, mut backend) = authenticated(true);
    let mut values = fields(b"Message"); values[1].value = &[0xff, 0, 7]; values[2].value = b"AccountAlive:stcp -h fixture.invalid -p 1;";
    let events = backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&values), Some(b"not-the-arc-token"), b"opaque"), 1).unwrap();
    assert_eq!(control_result(&events), [0; 6]); assert_eq!(events.len(), 1); assert!(scope.is_open()); assert!(!scope.session_revoked());
}
#[test]
fn unknown_command_aliases_and_additional_fields_still_fail_closed() {
    for command in [b"Unknown".as_slice(), b"message", b"Message.extra", b"Invite\0"] {
        let (owner, scope, mut backend) = authenticated(true);
        let mut values = fields(b"Message"); values[0].value = command;
        assert!(backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&values), None, b"bad"), 1).is_err());
        assert!(!scope.is_open()); assert!(!scope.session_revoked());
    }
    let (owner, scope, mut backend) = authenticated(true); let mut values = fields(b"Invite");
    values.push(RpcProperty { key: b"unknown-oem-hook", value: b"fixture-private" });
    assert!(backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&values), None, b"extra"), 1).is_err());
    assert!(!scope.is_open());
}
#[test]
fn missing_or_cross_command_business_keys_are_not_treated_as_known_shapes() {
    for index in 0..3 {
        let (owner, scope, mut backend) = authenticated(true); let mut values = fields(b"Message"); values.remove(index);
        assert!(backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&values), None, b"missing"), 1).is_err());
    }
    let (owner, scope, mut backend) = authenticated(true); let mut values = fields(b"Message"); values[0].value = b"Invite";
    assert!(backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&values), None, b"cross"), 1).is_err());
}
#[test]
fn malformed_known_notification_versions_truncation_and_trailing_data_are_not_acknowledged() {
    let valid = notify(&fields(b"Message")); let mut cases = (0..valid.len()).map(|n| valid[..n].to_vec()).collect::<Vec<_>>();
    let mut version = valid.clone(); version[4] = 1; cases.push(version);
    let mut trailing = valid; trailing.push(0); cases.push(trailing);
    for bytes in cases {
        let (owner, scope, mut backend) = authenticated(true);
        assert!(backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &bytes, None, b"malformed"), 1).is_err());
        assert!(!scope.session_revoked());
    }
}
#[test]
fn rejected_direct_notification_replays_do_not_emit_new_results_or_notices() {
    let (owner, scope, mut backend) = authenticated(true); let argument = notify(&fields(b"Invite"));
    let packet = request(1, ClientExKind::NotifyCalls, &argument, None, b"same-magic");
    assert_eq!(control_result(&backend.on_plaintext_packet(&owner, &scope, &packet, 1).unwrap()), [0; 6]);
    assert!(backend.on_plaintext_packet(&owner, &scope, &packet, 2).unwrap().is_empty());
    assert!(backend.on_plaintext_packet(&owner, &scope, &request(2, ClientExKind::NotifyCalls, &argument, None, b"same-magic"), 3).unwrap().is_empty());
    assert!(scope.is_open());
}
#[test]
fn authenticated_batch_queues_only_its_negative_result_and_keeps_polling_original_scope() {
    let (owner, scope, mut backend) = authenticated(true); let argument = notify(&fields(b"Message"));
    let first = backend.prepare_swap(0, &[]).unwrap();
    let events = backend.on_plaintext_packet(&owner, &scope, &batch_result(&first.packets[0].expose_to_transport()[1..], ClientExKind::NotifyCalls, &argument, None), 1).unwrap();
    assert!(!events.iter().any(|e| matches!(e, OfflineEvent::ClientEx(_) | OfflineEvent::Message(_) | OfflineEvent::DirectMessage(_) | OfflineEvent::Call(_))));
    let next = backend.prepare_swap(2, &[]).unwrap(); let request = decode_request(&next.packets[0].expose_to_transport()[1..]).unwrap();
    let mut reader = Reader::versioned(request.arguments).unwrap(); assert_eq!(reader.bytes().unwrap(), b"fixture-user"); assert_eq!(reader.bytes().unwrap(), b"fixture-selector");
    let payload = super::super::super::rpc_batch::read_payload(&mut reader).unwrap(); reader.finish().unwrap();
    assert_eq!(payload.results.get(&1).unwrap().result_stream, [0; 6]); assert!(scope.is_open()); assert!(!scope.session_revoked());
}
#[test]
fn unrelated_owner_or_scope_cannot_apply_even_a_known_negative_notification() {
    let (original, scope, mut backend) = authenticated(true); let packet = request(1, ClientExKind::NotifyCalls, &notify(&fields(b"Invite")), None, b"scope");
    assert!(backend.on_plaintext_packet(&owner(), &scope, &packet, 1).unwrap().is_empty());
    assert!(backend.on_plaintext_packet(&original, &RpcConnectionScope::fresh(), &packet, 1).unwrap().is_empty());
    assert_eq!(control_result(&backend.on_plaintext_packet(&original, &scope, &packet, 1).unwrap()), [0; 6]); assert!(scope.is_open());
}
#[test]
fn normal_token_revocation_remains_sticky_after_a_known_notification_rejection() {
    let (owner, scope, mut backend) = authenticated(true);
    backend.on_plaintext_packet(&owner, &scope, &request(1, ClientExKind::NotifyCalls, &notify(&fields(b"Message")), None, b"nack"), 1).unwrap();
    let kick = request(2, ClientExKind::NotifyDisconnect, &arguments(ClientExKind::NotifyDisconnect, 0, b"server-kick-off"), Some(b"fixture-auth"), b"kick");
    let events = backend.on_plaintext_packet(&owner, &scope, &kick, 2).unwrap(); assert_eq!(control_result(&events), [0, 1, 0, 0, 0, 0]);
    assert!(scope.session_revoked()); assert!(backend.prepare_swap(3, &[]).is_err());
}
