use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    arc_protocol::ArcProtocol, arc_session::{ArcLoginInputs, ArcPhase},
    packet_stream::PacketStreamDecoder, router_packets::{RouterAddress, RouterRouting},
    session::HostingSession, tests::imported,
};

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("line-a".into()); session.import(imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn binding(tag: u32) -> RouterBinding<'static> {
    RouterBinding { packet_kind: 1, routing: RouterRouting { tag, flags: 0, path_words: &[],
        address1: RouterAddress { mask: 0, label: 0x12, bytes: &[0x10, 0x11] },
        address2: RouterAddress { mask: 0, label: 0x34, bytes: &[0x20, 0x21] } } }
}
fn route(mtu: usize, sequence: u16) -> PacketRoute<'static> {
    PacketRoute::Router { transmit: binding(0x12345678), receive: binding(0x12345678), mtu, first_sequence: sequence }
}
fn timing() -> RouterTiming { RouterTiming { word1: 0x4567, word2: 0x89ab, byte6: 0xcd } }
fn direct() -> (ArcProtocol, AttemptOwner, RpcConnectionScope, OwnedPacketTransport) {
    let owner = owner(); let mut protocol = ArcProtocol::new(owner.clone()); let scope = protocol.connection_scope();
    let transport = protocol.take_packet_transport(PacketRoute::DirectTlsCarrier).unwrap(); (protocol, owner, scope, transport)
}

#[test]
fn transport_is_taken_once_from_original_arc_and_invalid_capture_does_not_consume_slot() {
    let original = owner(); let mut protocol = ArcProtocol::new(original); let scope = protocol.connection_scope();
    assert!(protocol.take_packet_transport(route(0, 1)).is_err()); assert!(scope.is_open());
    let transport = protocol.take_packet_transport(PacketRoute::DirectTlsCarrier).unwrap();
    assert!(protocol.take_packet_transport(PacketRoute::DirectTlsCarrier).is_err());
    assert_eq!(protocol.phase(), ArcPhase::Initial);
    drop(transport); assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn direct_wire_fixture_is_be32_carrier_length_and_exact_ff_ff_six() {
    let (protocol, original, scope, mut transport) = direct();
    let wire = transport.encode_tls_bytes(&original, &scope, &[0x16, 3, 3, 0, 2, 0xff, 0], None).unwrap().unwrap();
    let frames = wire.frames_for(&original, &scope).unwrap(); assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].expose_to_transport(), &[0, 0, 0, 10, 0xff, 0xff, 6, 0x16, 3, 3, 0, 2, 0xff, 0]);
    let input = transport.on_bytes(&original, &scope, frames[0].expose_to_transport()).unwrap();
    assert_eq!(input[0].bytes_for_tls(&original, &scope), Some(&[0x16, 3, 3, 0, 2, 0xff, 0][..]));
    assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn split_and_coalesced_stream_delivers_only_opaque_tls_input_not_login_success() {
    let (mut protocol, original, scope, mut transport) = direct();
    protocol.prepare_login(ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid",
        selected_session_id: b"selector", phone_login_token: b"pwd", application_key: b"app-key", domain_id: b"domain",
        application_id: b"app", device_info_cc: b"", client_properties: &[] }, &[]).unwrap();
    let bytes = [0, 0, 0, 5, 0xff, 0xff, 6, 0, 0x80, 0, 0, 0, 4, 0xff, 0xff, 6, 9];
    let mut inputs = vec![];
    for chunk in bytes.chunks(3) { inputs.extend(transport.on_bytes(&original, &scope, chunk).unwrap()); }
    assert_eq!(inputs.len(), 2); assert_eq!(inputs[0].bytes_for_tls(&original, &scope), Some(&[0, 0x80][..]));
    assert_eq!(inputs[1].bytes_for_tls(&original, &scope), Some(&[9][..]));
    assert_eq!(protocol.phase(), ArcPhase::LoginPending);
}

#[test]
fn stale_owner_scope_and_eof_are_rejected_before_original_partial_parser() {
    let (protocol, original, scope, mut transport) = direct();
    transport.on_bytes(&original, &scope, &[0, 0]).unwrap();
    let other = owner(); let other_scope = RpcConnectionScope::fresh();
    assert!(transport.on_bytes(&other, &scope, &[0xff; 12]).unwrap().is_empty());
    assert!(transport.on_bytes(&original, &other_scope, &[0xff; 12]).unwrap().is_empty());
    assert!(!transport.finish_eof(&other, &scope).unwrap());
    assert!(!transport.finish_eof(&original, &other_scope).unwrap());
    let inputs = transport.on_bytes(&original, &scope, &[0, 4, 0xff, 0xff, 6, 9]).unwrap();
    assert_eq!(inputs[0].bytes_for_tls(&original, &scope), Some(&[9][..])); assert_eq!(protocol.phase(), ArcPhase::Initial);
    assert!(inputs[0].bytes_for_tls(&other, &scope).is_none());
}

#[test]
fn malformed_original_input_closes_arc_and_invalidates_already_queued_output() {
    let (protocol, original, scope, mut transport) = direct();
    let wire = transport.encode_tls_bytes(&original, &scope, b"opaque", None).unwrap().unwrap();
    assert!(wire.frames_for(&owner(), &scope).is_none());
    assert!(wire.frames_for(&original, &RpcConnectionScope::fresh()).is_none());
    assert_eq!(transport.on_bytes(&original, &scope, &[0, 0, 0, 0]).err(), Some(TransportError::InvalidPacket));
    assert_eq!(protocol.phase(), ArcPhase::Closed); assert!(wire.frames_for(&original, &scope).is_none());
    assert!(transport.on_bytes(&original, &scope, &[0xff; 10]).unwrap().is_empty());
}

#[test]
fn independent_router_frame_fixture_checks_exact_configured_binding_before_tls() {
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let mut transport = protocol.take_packet_transport(route(100, 7)).unwrap();
    let bytes = [0, 0, 0, 24, 0, 7, 0x45, 0x67, 0x89, 0xab, 0xcd, 0x11,
        0x12, 0x34, 0x56, 0x78, 0, 0, 0x12, 0x34, 0x10, 0x11, 0x20, 0x21, 0xff, 0xff, 6, 0x80];
    let input = transport.on_bytes(&original, &scope, &bytes).unwrap();
    assert_eq!(input[0].bytes_for_tls(&original, &scope), Some(&[0x80][..]));
    assert_eq!(protocol.phase(), ArcPhase::Initial);
    assert_eq!(transport.on_bytes(&original, &scope, &bytes).err(), Some(TransportError::InvalidFragment));
    assert!(input[0].bytes_for_tls(&original, &scope).is_none());
}

#[test]
fn router_transmit_and_receive_bindings_are_independent_not_assumed_reverse_routes() {
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let mut transport = protocol.take_packet_transport(PacketRoute::Router { transmit: binding(1), receive: binding(2), mtu: 100, first_sequence: 1 }).unwrap();
    let wire = transport.encode_tls_bytes(&original, &scope, &[0x80], Some(timing())).unwrap().unwrap();
    let frame = wire.frames_for(&original, &scope).unwrap()[0].expose_to_transport();
    assert_eq!(transport.on_bytes(&original, &scope, frame).err(), Some(TransportError::UnsupportedRoute));
    assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn router_mtu_split_waits_for_ordered_second_half_before_one_tls_input() {
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let mut transport = protocol.take_packet_transport(route(32, 7)).unwrap();
    let tls = (0..20).collect::<Vec<u8>>();
    let wire = transport.encode_tls_bytes(&original, &scope, &tls, Some(timing())).unwrap().unwrap();
    let frames = wire.frames_for(&original, &scope).unwrap(); assert_eq!(frames.len(), 2);
    let first = PacketStreamDecoder::default().on_bytes(frames[0].expose_to_transport()).unwrap();
    let second = PacketStreamDecoder::default().on_bytes(frames[1].expose_to_transport()).unwrap();
    assert_eq!(first[0].expose_to_transport().len(), 25); assert_eq!(second[0].expose_to_transport().len(), 26);
    assert_eq!(&first[0].expose_to_transport()[..8], &[0, 7, 0x45, 0x67, 0x89, 0xab, 0xcd, 0x21]);
    assert_eq!(&second[0].expose_to_transport()[..8], &[0, 8, 0x45, 0x67, 0x89, 0xab, 0xcd, 0x31]);
    assert!(transport.on_bytes(&original, &scope, frames[0].expose_to_transport()).unwrap().is_empty());
    let input = transport.on_bytes(&original, &scope, frames[1].expose_to_transport()).unwrap();
    assert_eq!(input.len(), 1); assert_eq!(input[0].bytes_for_tls(&original, &scope), Some(tls.as_slice()));
    assert_eq!(protocol.phase(), ArcPhase::Initial);
}

#[test]
fn sequence_exhaustion_and_failed_local_encoding_do_not_reuse_or_consume_router_ids() {
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let mut transport = protocol.take_packet_transport(route(100, u16::MAX)).unwrap();
    assert!(transport.encode_tls_bytes(&original, &scope, &[0], None).is_err());
    assert!(transport.encode_tls_bytes(&original, &scope, &[], Some(timing())).is_err());
    let wire = transport.encode_tls_bytes(&original, &scope, &[0], Some(timing())).unwrap().unwrap();
    let frame = wire.frames_for(&original, &scope).unwrap()[0].expose_to_transport();
    assert_eq!(&frame[4..6], &[0xff, 0xff]);
    assert_eq!(transport.encode_tls_bytes(&original, &scope, &[0], Some(timing())).err(), Some(TransportError::InvalidFragment));
    assert!(scope.is_open());
}

#[test]
fn unknown_carrier_mode_wrong_router_address_and_tiny_control_are_terminal_not_downgrades() {
    for carrier in [&[0xff, 0xff, 5, 1][..], &[0xff, 0xff, 0xff, 1][..], &[0, 1][..], &[2][..]] {
        let (protocol, original, scope, mut transport) = direct();
        let frame = encode_packet_stream_frame(carrier).unwrap();
        assert!(transport.on_bytes(&original, &scope, frame.expose_to_transport()).is_err());
        assert_eq!(protocol.phase(), ArcPhase::Closed);
    }
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let mut transport = protocol.take_packet_transport(route(100, 7)).unwrap();
    let mut bytes = vec![0, 0, 0, 24, 0, 7, 0, 0, 0, 0, 0, 0x11,
        0x12, 0x34, 0x56, 0x78, 0, 0, 0x12, 0x34, 0x10, 0x11, 0x20, 0x22, 0xff, 0xff, 6, 0];
    assert_eq!(transport.on_bytes(&original, &scope, &bytes).err(), Some(TransportError::UnsupportedRoute));
    bytes[0] = 0xff; assert!(transport.on_bytes(&original, &scope, &bytes).unwrap().is_empty());
}

#[test]
fn eof_with_incomplete_stream_or_router_pair_revokes_scope_and_retained_inputs() {
    let (protocol, original, scope, mut transport) = direct();
    let inputs = transport.on_bytes(&original, &scope, &[0, 0, 0, 4, 0xff, 0xff, 6, 9]).unwrap();
    transport.on_bytes(&original, &scope, &[0, 0]).unwrap();
    assert_eq!(transport.finish_eof(&original, &scope), Err(TransportError::Truncated));
    assert_eq!(protocol.phase(), ArcPhase::Closed); assert!(inputs[0].bytes_for_tls(&original, &scope).is_none());
    let original = owner(); let mut protocol = ArcProtocol::new(original.clone()); let scope = protocol.connection_scope();
    let mut transport = protocol.take_packet_transport(route(32, 7)).unwrap();
    let wire = transport.encode_tls_bytes(&original, &scope, &[0; 20], Some(timing())).unwrap().unwrap();
    transport.on_bytes(&original, &scope, wire.frames_for(&original, &scope).unwrap()[0].expose_to_transport()).unwrap();
    assert_eq!(transport.finish_eof(&original, &scope), Err(TransportError::Truncated));
    assert_eq!(protocol.phase(), ArcPhase::Closed);
}

#[test]
fn arc_close_drop_or_protocol_release_invalidates_lower_frames_without_new_scope_rebinding() {
    let (mut protocol, original, scope, mut transport) = direct();
    let wire = transport.encode_tls_bytes(&original, &scope, b"opaque", None).unwrap().unwrap();
    protocol.on_plaintext_packet(&original, &scope, &[4], |_| {}).unwrap();
    assert!(wire.frames_for(&original, &scope).is_none());
    let (protocol, original, scope, mut transport) = direct();
    let wire = transport.encode_tls_bytes(&original, &scope, b"opaque", None).unwrap().unwrap();
    drop(protocol); assert!(wire.frames_for(&original, &scope).is_none());
    let mut new_protocol = ArcProtocol::new(original.clone()); let new_scope = new_protocol.connection_scope();
    let mut new_transport = new_protocol.take_packet_transport(PacketRoute::DirectTlsCarrier).unwrap();
    assert!(wire.frames_for(&original, &new_scope).is_none());
    assert!(new_transport.on_bytes(&original, &scope, &[0xff; 5]).unwrap().is_empty()); assert!(new_scope.is_open());
}

#[test]
fn checked_snapshots_are_not_revocable_io_leases_and_new_access_requires_open_original_scope() {
    let (mut protocol, original, scope, mut transport) = direct();
    let wire = transport.encode_tls_bytes(&original, &scope, b"opaque", None).unwrap().unwrap();
    let inputs = transport.on_bytes(&original, &scope, wire.frames_for(&original, &scope).unwrap()[0].expose_to_transport()).unwrap();
    let prior_wire = wire.frames_for(&original, &scope).unwrap();
    let prior_tls = inputs[0].bytes_for_tls(&original, &scope).unwrap();
    protocol.close();
    // Rust references/copies cannot be revoked by the atomic scope flag. This
    // is an explicit limitation; a future serialized writer/TLS actor must
    // cancel/drain work and check the owner again at final consumption.
    assert!(!prior_wire[0].expose_to_transport().is_empty()); assert_eq!(prior_tls, b"opaque");
    assert!(wire.frames_for(&original, &scope).is_none());
    assert!(inputs[0].bytes_for_tls(&original, &scope).is_none());
    assert!(transport.on_bytes(&original, &scope, &[0xff]).unwrap().is_empty());
}

#[test]
fn debug_never_exposes_retained_tls_body_routing_or_line_identity() {
    let (protocol, original, scope, mut transport) = direct();
    let wire = transport.encode_tls_bytes(&original, &scope, b"fixture-private-tls", None).unwrap().unwrap();
    let frame = wire.frames_for(&original, &scope).unwrap()[0].expose_to_transport();
    let input = transport.on_bytes(&original, &scope, frame).unwrap();
    for value in [format!("{wire:?}"), format!("{:?}", input[0]), format!("{transport:?}"), format!("{protocol:?}"), format!("{:?}", route(32, 7))] {
        assert!(!value.contains("fixture")); assert!(!value.contains("line-a"));
    }
}
