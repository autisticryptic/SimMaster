use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    arc_session::ArcReply, controller::Callback, rpc_envelope::decode_request,
    session::HostingSession, tests::imported, transport_packets::CallAssembler,
};
use std::collections::BTreeMap;

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    session.begin_login(100).unwrap()
}

fn inputs() -> ArcLoginInputs<'static> {
    ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid",
        selected_session_id: b"fixture-selector", phone_login_token: b"fixture-phone",
        application_key: b"fixture-key", domain_id: b"fixture-domain", application_id: b"fixture-app",
        device_info_cc: b"", client_properties: &[] }
}

fn reply(id: i32) -> Vec<u8> {
    let mut result = vec![0, 1];
    for value in [b"fixture-user".as_slice(), b"fixture-auth".as_slice()] {
        result.extend_from_slice(&(value.len() as u32).to_be_bytes()); result.extend_from_slice(value);
    }
    result.extend_from_slice(&[0; 8]);
    let mut bytes = vec![0]; bytes.extend_from_slice(&id.to_be_bytes()); bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&(result.len() as u32).to_be_bytes()); bytes.extend_from_slice(&result);
    bytes
}

#[test]
fn owned_protocol_composes_real_login_envelope_fragments_and_original_callback() {
    let owner = owner(); let mut protocol = ArcProtocol::new(owner.clone()); let scope = protocol.connection_scope();
    let login = protocol.prepare_login(inputs(), &[RpcProperty { key: b"id", value: b"#fixture" }]).unwrap();
    let mut peer = CallAssembler::default();
    let request = peer.on_packet(login.packets[0].expose_to_transport()).unwrap().unwrap();
    let envelope = decode_request(request.payload.expose_to_transport()).unwrap();
    assert_eq!(envelope.call_id, login.call_id);
    assert_eq!(envelope.metadata.iter().find(|field| field.key == b"pwd").unwrap().value, b"fixture-phone");
    let packets = encode_call_packets(CallKind::Reply, &reply(login.call_id), 8).unwrap();
    let mut callback = None;
    for packet in &packets {
        protocol.on_plaintext_packet(&owner, &scope, packet.expose_to_transport(), |event| {
            if let ProtocolEvent::Reply(ArcReply::Login(result)) = event { callback = Some(result); }
        }).unwrap();
    }
    let callback = callback.unwrap();
    assert_eq!(callback.owner, owner);
    assert_eq!(callback.callback, Callback::SdkLogin { success: true });
    assert_eq!(protocol.phase(), ArcPhase::Authenticated);
    let payload = BatchPayload { fields: [0; 6], requests: BTreeMap::new(), results: BTreeMap::new() };
    assert!(protocol.prepare_swap_calls(&payload, &[]).is_ok());
}

#[test]
fn stale_owner_or_receive_scope_is_ignored_before_partial_packet_parsing() {
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let login = protocol.prepare_login(inputs(), &[]).unwrap();
    let packets = encode_call_packets(CallKind::Reply, &reply(login.call_id), 8).unwrap();
    protocol.on_plaintext_packet(&original, &scope, packets[0].expose_to_transport(), |_| panic!("incomplete")).unwrap();
    let other_scope = ArcProtocol::new(original.clone()).connection_scope();
    assert!(!protocol.on_plaintext_packet(&original, &other_scope, &[0xff], |_| panic!("stale")).unwrap());
    let other_owner = owner();
    assert!(!protocol.on_plaintext_packet(&other_owner, &scope, &[0xff], |_| panic!("stale")).unwrap());
    let mut delivered = false;
    for packet in &packets[1..] {
        protocol.on_plaintext_packet(&original, &scope, packet.expose_to_transport(), |_| delivered = true).unwrap();
    }
    assert!(delivered);
    assert_eq!(protocol.phase(), ArcPhase::Authenticated);
}

#[test]
fn malformed_compressed_or_unimplemented_server_call_never_authenticates() {
    for packet in [&[0x11, 0][..], &[0, 0][..], &[1, 0][..]] {
        let owner = owner(); let mut protocol = ArcProtocol::new(owner.clone()); let scope = protocol.connection_scope();
        protocol.prepare_login(inputs(), &[]).unwrap();
        assert!(protocol.on_plaintext_packet(&owner, &scope, packet, |_| panic!("malformed")).is_err());
        assert_eq!(protocol.phase(), ArcPhase::Closed);
    }
}

#[test]
fn ping_pong_and_release_are_transport_events_not_hosting_or_login_success() {
    let owner = owner(); let mut protocol = ArcProtocol::new(owner.clone()); let scope = protocol.connection_scope();
    protocol.prepare_login(inputs(), &[]).unwrap();
    for kind in [CallKind::Ping, CallKind::Pong] {
        let mut control = None;
        protocol.on_plaintext_packet(&owner, &scope, &[kind as u8], |event| {
            if let ProtocolEvent::Control(kind) = event { control = Some(kind); } else { panic!("control expected"); }
        }).unwrap();
        assert_eq!(control, Some(kind));
        assert_eq!(protocol.phase(), ArcPhase::LoginPending);
    }
    protocol.on_plaintext_packet(&owner, &scope, &[4], |_| {}).unwrap();
    assert_eq!(protocol.phase(), ArcPhase::Closed);
    assert!(!protocol.on_plaintext_packet(&owner, &scope, &[0xff], |_| panic!("closed")).unwrap());
}

#[test]
fn login_retirement_clears_partial_plaintext_and_closes_the_bound_hosting_tracker() {
    let owner = owner(); let mut protocol = ArcProtocol::new(owner.clone()); let scope = protocol.connection_scope();
    let mut tracker = protocol.take_hosting_tracker().unwrap();
    let login = protocol.prepare_login(inputs(), &[]).unwrap();
    let packets = encode_call_packets(CallKind::Reply, &reply(login.call_id), 8).unwrap();
    assert!(!protocol.on_plaintext_packet(&owner, &scope, packets[0].expose_to_transport(), |_| panic!("partial")).unwrap());
    assert!(format!("{:?}", protocol.assembler).contains("partial: true"));
    protocol.retire_request(login.call_id);
    assert_eq!(protocol.phase(), ArcPhase::Closed);
    assert_eq!(protocol.assembler.on_packet(&[2]).err(), Some(TransportError::Closed));
    assert!(!format!("{:?}", protocol.assembler).contains("partial: true"));
    let registration = super::super::session::GatewayRegistration {
        authname_prefix: "prefix", channel_id: "channel", delete_int_code: "1", sip_number: "number", sip_password: "password",
    };
    assert!(tracker.prepare_batch_update_status(owner.clone(), "uri", super::super::session::STATUS_REGISTER, &registration).is_err());
    assert!(!protocol.on_plaintext_packet(&owner, &scope, packets[1].expose_to_transport(), |_| panic!("retired")).unwrap());
}

#[test]
fn failed_login_closes_rpc_and_fragment_assembler_before_delivering_failure() {
    let owner = owner(); let mut protocol = ArcProtocol::new(owner.clone()); let scope = protocol.connection_scope();
    let login = protocol.prepare_login(inputs(), &[]).unwrap();
    let mut bad = reply(login.call_id); bad[14] = 0;
    let packets = encode_call_packets(CallKind::Reply, &bad, 8).unwrap();
    let mut failed = false;
    for packet in packets {
        protocol.on_plaintext_packet(&owner, &scope, packet.expose_to_transport(), |event| {
            assert!(matches!(event, ProtocolEvent::Reply(ArcReply::Failure(_)))); failed = true;
        }).unwrap();
    }
    assert!(failed);
    assert_eq!(protocol.phase(), ArcPhase::Closed);
    assert_eq!(protocol.assembler.on_packet(&[2]).err(), Some(TransportError::Closed));
}

#[test]
fn direct_frontier_does_not_overtake_pending_original_delivery_and_closes_with_scope() {
    use super::super::long_reception_test_support as fixture;
    let original=owner();let mut protocol=fixture::authenticated(original.clone());let scope=protocol.connection_scope();
    let first=fixture::incoming(&mut protocol,&original,1,b"first");let second=fixture::incoming(&mut protocol,&original,2,b"second");
    protocol.complete_incoming(second.reply_ticket(),true).unwrap().unwrap();
    assert!(protocol.direct_replay_frontier().is_none(),"uncompleted id1 must block a later response frontier");
    protocol.complete_incoming(first.reply_ticket(),true).unwrap().unwrap();let proof=protocol.direct_replay_frontier().unwrap();
    assert_eq!(proof.through(),2);assert!(proof.matches(&original,&scope));assert!(!proof.matches(&owner(),&scope));
    assert!(!proof.matches(&original,&RpcConnectionScope::fresh()));scope.close();assert!(!proof.matches(&original,&scope));
    assert!(protocol.direct_replay_frontier().is_none());assert_eq!(format!("{proof:?}"),"DirectReplayFrontier(<redacted>)");
}
#[test]
fn original_direct_ids_remain_nonreusable_after_history_window_rotation() {
    use super::super::long_reception_test_support as fixture;
    let original=owner();let mut protocol=fixture::authenticated(original.clone());let scope=protocol.connection_scope();
    let first=fixture::incoming(&mut protocol,&original,1,b"first");protocol.complete_incoming(first.reply_ticket(),true).unwrap();
    let now=Instant::now();protocol.on_plaintext_packet_at(&original,&scope,&[CallKind::Pong as u8],now+std::time::Duration::from_secs(181),|_|{}).unwrap();
    protocol.on_plaintext_packet_at(&original,&scope,&[CallKind::Pong as u8],now+std::time::Duration::from_secs(362),|_|{}).unwrap();
    let wire=fixture::wire(1,&[0,1],b"must-not-reenter");let packet=encode_call_packets(CallKind::Request,&wire,TLS_FRAGMENT_THRESHOLD).unwrap();
    assert!(protocol.on_plaintext_packet_at(&original,&scope,packet[0].expose_to_transport(),now+std::time::Duration::from_secs(363),|_|panic!("old RPC id replay")).is_err());
    assert_eq!(protocol.phase(),ArcPhase::Closed);
}
#[test]
fn duplicate_magic_under_new_rpc_id_is_suppressed_and_that_id_cannot_be_reused() {
    use super::super::long_reception_test_support as fixture;
    let original=owner();let mut protocol=fixture::authenticated(original.clone());let scope=protocol.connection_scope();
    let first=fixture::incoming(&mut protocol,&original,1,b"first");protocol.complete_incoming(first.reply_ticket(),true).unwrap();
    let wire=fixture::wire(2,&1i32.to_be_bytes(),b"duplicate-magic");let packet=encode_call_packets(CallKind::Request,&wire,TLS_FRAGMENT_THRESHOLD).unwrap();
    assert!(!protocol.on_plaintext_packet(&original,&scope,packet[0].expose_to_transport(),|_|panic!("magic replay")).unwrap());
    assert_eq!(protocol.direct_last_id,2);assert_eq!(protocol.direct_seen.count(),1);
    let wire=fixture::wire(2,&[9,9],b"cannot-reuse");let packet=encode_call_packets(CallKind::Request,&wire,TLS_FRAGMENT_THRESHOLD).unwrap();
    assert!(!protocol.on_plaintext_packet(&original,&scope,packet[0].expose_to_transport(),|_|panic!("same RPC id")).unwrap());
    assert!(scope.is_open());
}
#[test]
fn old_scope_clock_input_cannot_rotate_a_current_direct_history() {
    use super::super::long_reception_test_support as fixture;
    let original=owner();let mut protocol=fixture::authenticated(original.clone());let scope=protocol.connection_scope();
    let first=fixture::incoming(&mut protocol,&original,1,b"first");protocol.complete_incoming(first.reply_ticket(),true).unwrap();
    let now=Instant::now()+std::time::Duration::from_secs(1000);
    assert!(!protocol.on_plaintext_packet_at(&owner(),&scope,&[CallKind::Pong as u8],now,|_|panic!("foreign")).unwrap());
    assert!(!protocol.on_plaintext_packet_at(&original,&RpcConnectionScope::fresh(),&[CallKind::Pong as u8],now,|_|panic!("foreign")).unwrap());
    assert!(protocol.direct_seen.contains_id(1));assert!(scope.is_open());
}
#[test]
fn pending_direct_replies_are_never_expired_to_make_room_in_rotating_history() {
    use super::super::long_reception_test_support as fixture;
    let original=owner();let mut protocol=fixture::authenticated(original.clone());let scope=protocol.connection_scope();
    let first=fixture::incoming(&mut protocol,&original,1,b"pending");let now=Instant::now();
    for seconds in [181,362,543]{protocol.on_plaintext_packet_at(&original,&scope,&[CallKind::Pong as u8],now+std::time::Duration::from_secs(seconds),|_|{}).unwrap();}
    assert_eq!(protocol.direct_pending.len(),1);assert!(protocol.direct_replay_frontier().is_none());
    assert!(protocol.complete_incoming(first.reply_ticket(),true).unwrap().is_some());assert_eq!(protocol.direct_pending.len(),0);
}

#[test]
fn submission_encoding_has_no_tls_socket_or_channel_readiness_side_effect() {
    let protocol = ArcProtocol::new(owner());
    assert_eq!(protocol.phase(), ArcPhase::Initial);
    assert!(!format!("{protocol:?}").contains("fixture"));
    let mut protocol = protocol;
    let call = protocol.prepare_login(inputs(), &[]).unwrap();
    assert_eq!(call.packets[0].expose_to_transport()[0], CallKind::Request as u8);
    assert!(!format!("{call:?}").contains("fixture"));
    assert_eq!(protocol.phase(), ArcPhase::LoginPending);
}
