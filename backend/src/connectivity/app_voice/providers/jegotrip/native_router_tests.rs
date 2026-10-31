use super::*;

const ROUTER_FIXTURE_ID: i32 = 12001;

fn h(sequence: u16, kind: u8, part: u8) -> RouterHeader { RouterHeader { sequence, word1: 0x1234, word2: 0x5678,
    byte6: 9, packet_kind: kind, fragment_part: part } }
fn wire(sequence: u16, kind: u8, part: u8, body: &[u8]) -> Vec<u8> { packet(h(sequence,kind,part),body).unwrap().expose_to_transport().to_vec() }
fn data(value: PathInput) -> Option<PathMessage> { match value { PathInput::Data { message, .. } => message, _ => panic!("data required") } }

#[test]
fn independent_control_header_and_optional_probe_words_match_native_fixture() {
    let probe = encode_probe(h(0xabcd,2,1), Some(TimingProbe { peer_send_time: 0x0203, peer_sequence: 0x0405 })).unwrap();
    assert_eq!(probe.expose_to_transport(), [0xab,0xcd,0x12,0x34,0x56,0x78,9,0x12,2,3,4,5]);
    let (header,body)=decode_path_header(probe.expose_to_transport()).unwrap();assert_eq!(header.packet_kind,2); assert_eq!(header.fragment_part,1);
    let echo=decode_probe(body).unwrap().unwrap();assert_eq!(echo.peer_send_time,0x0203);assert_eq!(echo.peer_sequence,0x0405);
    assert_eq!(encode_feedback(h(0xabcd,3,1)).unwrap().expose_to_transport(),[0xab,0xcd,0x12,0x34,0x56,0x78,9,0x13]);
    assert!(decode_probe(&[]).unwrap().is_none());
}

#[test]
fn path_controls_never_become_application_data_or_message_acknowledgements() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(matches!(receiver.on_packet(&wire(1,2,1,&[]),now).unwrap(),PathInput::Probe{echo:None,..}));
    assert!(matches!(receiver.on_packet(&wire(2,3,1,&[]),now).unwrap(),PathInput::Feedback{..}));
    assert!(!receiver.has_partial());
}

#[test]
fn malformed_control_kind_part_and_probe_lengths_close_only_this_decoder() {
    for bytes in [vec![0,1,0,0,0,0,0,0x14], vec![0,1,0,0,0,0,0,0x22], wire(1,2,1,&[1]), wire(1,3,1,&[1])] {
        let mut receiver=PathReceiver::default();assert!(receiver.on_packet(&bytes,Instant::now()).is_err());
        assert!(matches!(receiver.on_packet(&wire(1,2,1,&[]),Instant::now()),Err(TransportError::Closed)));
    }
}

#[test]
fn reserved_short_and_oversized_headers_fail_before_payload_allocation() {
    for bytes in [vec![0;7], vec![0;MAX_NETWORK_PACKET_BYTES+1],vec![0,0,0,0,0,0,0,0x18]] { assert!(decode_path_header(&bytes).is_err()); }
    assert!(encode_probe(h(1,1,1),None).is_err()); assert!(encode_feedback(h(1,2,1)).is_err());
}

#[test]
fn native_sequence_bitmap_uses_first_minus512_and_32word_sliding_not_arc_ids() {
    let mut book=SequenceBookkeeping::default();assert_eq!(book.observe(100),ReceiveObservation::FirstOrReset);
    assert_eq!(book.base,Some(100u16.wrapping_sub(512)));assert_eq!(book.head,0);
    assert_eq!(book.observe(100),ReceiveObservation::Duplicate);assert_eq!(book.observe(611),ReceiveObservation::New);assert_eq!(book.head,0);
    assert_eq!(book.observe(612),ReceiveObservation::New);assert_eq!(book.head,1);assert_eq!(book.base,Some(100u16.wrapping_sub(480)));
}

#[test]
fn native_u16_sequence_wrap_remains_one_path_and_duplicates_are_bookkeeping_only() {
    let mut book=SequenceBookkeeping::default();assert_eq!(book.observe(65534),ReceiveObservation::FirstOrReset);
    for sequence in [65535,0,1] { assert_eq!(book.observe(sequence),ReceiveObservation::New); }
    assert_eq!(book.observe(65535),ReceiveObservation::Duplicate);
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(data(receiver.on_packet(&wire(1,1,1,b"fixture"),now).unwrap()).is_some());
    let duplicate=receiver.on_packet(&wire(1,1,1,b"fixture"),now).unwrap();
    assert!(matches!(&duplicate,PathInput::Data{observation:ReceiveObservation::Duplicate,message:Some(_)}));
    assert_eq!(data(duplicate).unwrap().body.expose_to_transport(),b"fixture");
}

#[test]
fn native_window_reset_clears_original_partial_cache_without_silent_data_rebinding() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(data(receiver.on_packet(&wire(1,1,2,b"old"),now).unwrap()).is_none());assert!(receiver.has_partial());
    assert!(data(receiver.on_packet(&wire(5000,1,1,b"new"),now).unwrap()).is_some());assert!(!receiver.has_partial());
    assert!(data(receiver.on_packet(&wire(5001,1,3,b"new tail"),now).unwrap()).is_none());
}

#[test]
fn adjacent_part3_before_part2_pairs_without_reordering_body_bytes() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(data(receiver.on_packet(&wire(11,1,3,b"second"),now).unwrap()).is_none());
    let message=data(receiver.on_packet(&wire(10,1,2,b"first"),now).unwrap()).unwrap();
    assert_eq!(message.first_header.sequence,10);assert_eq!(message.last_header.unwrap().sequence,11);
    assert_eq!(message.body.expose_to_transport(),b"firstsecond");assert!(!receiver.has_partial());
}

#[test]
fn both_pair_orders_cross_65535_zero_without_global_sequence_reassignment() {
    for order in [[(65535,2,b"first".as_slice()),(0,3,b"second".as_slice())],[(0,3,b"second".as_slice()),(65535,2,b"first".as_slice())]] {
        let mut receiver=PathReceiver::default();let now=Instant::now();
        assert!(data(receiver.on_packet(&wire(order[0].0,1,order[0].1,order[0].2),now).unwrap()).is_none());
        let message=data(receiver.on_packet(&wire(order[1].0,1,order[1].1,order[1].2),now).unwrap()).unwrap();
        assert_eq!(message.body.expose_to_transport(),b"firstsecond");assert_eq!(message.first_header.sequence,65535);assert_eq!(message.last_header.unwrap().sequence,0);
    }
}

#[test]
fn interleaved_adjacent_pairs_stay_separate_and_duplicate_half_does_not_replace_original() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    for (sequence,body) in [(10,b"A".as_slice()),(12,b"B"),(10,b"A")] { assert!(data(receiver.on_packet(&wire(sequence,1,2,body),now).unwrap()).is_none()); }
    assert_eq!(data(receiver.on_packet(&wire(13,1,3,b"b"),now).unwrap()).unwrap().body.expose_to_transport(),b"Bb");
    assert_eq!(data(receiver.on_packet(&wire(11,1,3,b"a"),now).unwrap()).unwrap().body.expose_to_transport(),b"Aa");
}

#[test]
fn mismatching_adjacent_kind_part_or_retained_body_is_terminal_not_a_new_pair() {
    for second in [wire(11,0,3,b"b"),wire(10,1,3,b"b"),wire(10,1,2,b"changed")] {
        let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"a"),now).unwrap();
        assert!(receiver.on_packet(&second,now).is_err());assert!(!receiver.has_partial());
    }
}

#[test]
fn partial_age_window_resource_caps_and_eof_never_emit_unconfirmed_complete_message() {
    let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"a"),now).unwrap();
    assert!(data(receiver.on_packet(&wire(11,1,3,b"b"),now+HALF_AGE).unwrap()).is_none());
    assert!(matches!(receiver.finish_eof(),Err(TransportError::Truncated)));
    let mut receiver=PathReceiver::default();
    for sequence in (0..MAX_CACHED_HALVES as u16*2).step_by(2) { receiver.on_packet(&wire(sequence,1,2,b"a"),now).unwrap(); }
    assert!(receiver.on_packet(&wire(200,1,2,b"a"),now).is_err());
}

#[test]
fn expired_halves_are_not_retained_by_full_data_probe_or_feedback_traffic() {
    for final_packet in [wire(11,1,1,b"whole"),wire(11,2,1,&[]),wire(11,3,1,&[])] {
        let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"old private half"),now).unwrap();
        assert!(receiver.has_partial());receiver.on_packet(&final_packet,now+HALF_AGE).unwrap();
        assert!(!receiver.has_partial());assert_eq!(receiver.cached_bytes,0);assert!(receiver.finish_eof().is_ok());
    }
}

#[test]
fn clock_rollback_and_combined_body_overflow_close_and_clear_retention() {
    let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"a"),now).unwrap();
    assert!(receiver.on_packet(&wire(11,1,3,b"b"),now-Duration::from_millis(1)).is_err());assert!(!receiver.has_partial());
    let mut receiver=PathReceiver::default();let large=vec![0;MAX_ROUTED_BODY_BYTES];receiver.on_packet(&wire(10,1,2,&large),now).unwrap();
    assert!(receiver.on_packet(&wire(11,1,3,&[0]),now).is_err());assert!(!receiver.has_partial());
}

fn control_route(payload: &[u8]) -> Vec<u8> {
    let route = super::super::router_packets::RouterRouting { tag: 0, flags: 0, path_words: &[],
        address1: super::super::router_packets::RouterAddress { mask: 0, label: 0, bytes: &[0, 0] },
        address2: super::super::router_packets::RouterAddress { mask: 0, label: 0, bytes: &[0, 0] } };
    let body = super::super::router_packets::encode_routed_body(&route, payload).unwrap();
    wire(40, 1, 1, body.expose_to_transport())
}
fn control_reply(call: i32, code: u32, accepted: bool, router: Option<u32>, node: u32, reason: &[u8]) -> Vec<u8> {
    let mut result = super::super::rpc_stream::Writer::new(); result.boolean(accepted).unwrap();
    if let Some(router) = router { result.u32(router).unwrap(); }
    result.u32(node).unwrap(); result.bytes(reason).unwrap(); let result = result.finish();
    let reply = super::super::rpc_envelope::encode_reply(call, code, result.expose_to_transport()).unwrap();
    let common = super::super::transport_packets::encode_call_packets(super::super::transport_packets::CallKind::Reply,
        reply.expose_to_transport(), super::super::transport_packets::TLS_FRAGMENT_THRESHOLD).unwrap();
    control_route(common[0].expose_to_transport())
}
fn control_owner() -> super::super::session::AttemptOwner {
    let mut session = super::super::session::HostingSession::new("fixture-router-line".into());
    session.import(super::super::tests::imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn tracker(owner: &super::super::session::AttemptOwner, scope: &super::super::rpc_pending::RpcConnectionScope,
    id: i32, now: Instant) -> RouterConnectTracker {
    RouterConnectTracker::for_original(owner.clone(), scope.clone(), id, 0x11223344, &[], 1460, 10, Duration::from_secs(5), now).unwrap()
}
fn timing() -> super::super::packet_transport::RouterTiming { super::super::packet_transport::RouterTiming { word1: 1, word2: 1, byte6: 0 } }

#[test]
fn router_connect_requests_match_native_agent_metadata_versions_words_and_fresh_control_route() {
    let owner = control_owner(); let now = Instant::now();
    for id in [ROUTER_FIXTURE_ID, -1] {
        let scope = super::super::rpc_pending::RpcConnectionScope::fresh();
        let mut tracker = tracker(&owner, &scope, id, now); let request = tracker.prepare(&owner, &scope, timing(), now).unwrap().unwrap();
        let packets = request.packets_for(&owner, &scope).unwrap(); assert_eq!(packets.len(), 1);
        let (header, body) = decode_path_header(packets[0].expose_to_transport()).unwrap(); assert_eq!(header.packet_kind, 1);
        assert_eq!(&body[..12], &[0; 12]); let route = super::super::router_packets::decode_routed_body(body).unwrap();
        let request = super::super::rpc_envelope::decode_request(&route.payload[1..]).unwrap();
        assert_eq!(request.call_id, 1); assert_eq!(request.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"Router");
        assert_eq!(request.metadata.iter().find(|p| p.key == b"arcVer").unwrap().value, b"16777216");
        assert_eq!(request.method, if id > 0 { b"clientConnect1.RouterClient.Common".as_slice() } else { b"clientConnect3.RouterClient.Common".as_slice() });
        let mut expected = vec![0, 0, 1, 0, 0, 0, 0, 0, 0, 0x11, 0x22, 0x33, 0x44]; if id > 0 { expected.extend_from_slice(&(id as u32).to_be_bytes()); }
        assert_eq!(request.arguments, expected);
        assert_eq!(tracker.prepare(&owner, &scope, timing(), now).err(), Some(RouterConnectError::Pending));
    }
}

#[test]
fn router_connect_configuration_requires_nonzero_original_ri_nonce_and_complete_access_pair() {
    let owner = control_owner(); let scope = super::super::rpc_pending::RpcConnectionScope::fresh(); let now = Instant::now();
    for id in [0, -1000] { assert!(RouterConnectTracker::for_original(owner.clone(), scope.clone(), id, 1, &[], 1460, 1, Duration::from_secs(1), now).is_err()); }
    for access in [vec![super::super::rpc_stream::RpcProperty { key: b"accessKey", value: b"fixture" }],
        vec![super::super::rpc_stream::RpcProperty { key: b"pwd", value: b"fixture" }, super::super::rpc_stream::RpcProperty { key: b"token", value: b"fixture" }]] {
        assert!(RouterConnectTracker::for_original(owner.clone(), scope.clone(), 1, 1, &access, 1460, 1, Duration::from_secs(1), now).is_err());
    }
    assert!(RouterConnectTracker::for_original(owner.clone(), scope.clone(), 1, 0, &[], 1460, 1, Duration::from_secs(1), now).is_err());
}

#[test]
fn actual_endpoint_tls_router_connect_assigns_only_logical_ids_after_matched_reply() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup, endpoint_raw_wire, server_input, server_output, server_plaintext};
    use std::io::Write;
    let cert = Certificates::generate("DNS:fixture.invalid");
    for id in [ROUTER_FIXTURE_ID, -1] {
        let (protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
        let mut tracker = tracker(&owner, &scope, id, now); let request = tracker.prepare(&owner, &scope, timing(), now).unwrap().unwrap();
        let wire = engine.encrypt_packets(&owner, &scope, request.packets_for(&owner, &scope).unwrap(), now).unwrap().unwrap();
        server_input(&mut server, &endpoint_raw_wire(&wire, &owner, &scope));
        let plain = server_plaintext(&mut server); let packets = super::super::packet_stream::PacketStreamDecoder::default().on_bytes(&plain).unwrap();
        let (_, body) = decode_path_header(packets[0].expose_to_transport()).unwrap(); let route = super::super::router_packets::decode_routed_body(body).unwrap();
        assert_eq!(super::super::rpc_envelope::decode_request(&route.payload[1..]).unwrap().call_id, 1);
        assert_eq!(tracker.phase(), RouterConnectPhase::Pending);
        let reply = control_reply(1, 0, true, (id < 0).then_some(ROUTER_FIXTURE_ID as u32), 0x55667788, b"");
        server.writer().write_all(super::super::packet_stream::encode_packet_stream_frame(&reply).unwrap().expose_to_transport()).unwrap();
        let cipher = server_output(&mut server); let progress = engine.on_wire_bytes(&owner, &scope, &cipher, now).unwrap().unwrap();
        let assignment = tracker.on_packets(&owner, &scope, progress.plaintext, now).unwrap().unwrap();
        assert_eq!(assignment.ids_for(&owner, &scope), Some((ROUTER_FIXTURE_ID as u32, 0x55667788))); assert_eq!(tracker.phase(), RouterConnectPhase::Connected);
        assert_eq!(protocol.phase(), super::super::arc_session::ArcPhase::Initial);
        assert!(assignment.ids_for(&control_owner(), &scope).is_none()); engine.close(); assert!(assignment.ids_for(&owner, &scope).is_none());
    }
}

#[test]
fn router_connect_rejected_zero_or_nonzero_outer_results_never_assign_or_grant_sdk() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup, server_output}; use std::io::Write;
    let cert = Certificates::generate("DNS:fixture.invalid");
    for reply in [control_reply(1, 0, false, None, 7, b""), control_reply(1, 0, true, None, 0, b""),
        control_reply(1, 0, true, None, 7, b"fixture rejection"), control_reply(1, 1, true, None, 7, b"")] {
        let (protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
        let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now); tracker.prepare(&owner, &scope, timing(), now).unwrap();
        server.writer().write_all(super::super::packet_stream::encode_packet_stream_frame(&reply).unwrap().expose_to_transport()).unwrap();
        let cipher = server_output(&mut server); let input = engine.on_wire_bytes(&owner, &scope, &cipher, now).unwrap().unwrap().plaintext;
        assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Rejected));
        assert_eq!(tracker.phase(), RouterConnectPhase::Closed); assert_eq!(protocol.phase(), super::super::arc_session::ArcPhase::Closed);
    }
}

#[test]
fn router_connect_foreign_tls_brand_is_ignored_before_clock_or_parsing_and_budget_is_fixed() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup, server_output}; use std::io::Write;
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, _engine, _server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now); let request = tracker.prepare(&owner, &scope, timing(), now).unwrap().unwrap();
    let (_other_protocol, other_owner, other_scope, mut other_engine, mut other_server, other_now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let reply = control_reply(1, 0, true, None, 7, b""); other_server.writer().write_all(super::super::packet_stream::encode_packet_stream_frame(&reply).unwrap().expose_to_transport()).unwrap();
    let cipher = server_output(&mut other_server); let input = other_engine.on_wire_bytes(&other_owner, &other_scope, &cipher, other_now).unwrap().unwrap().plaintext;
    assert!(tracker.on_packets(&owner, &scope, input, now - Duration::from_secs(10)).unwrap().is_none()); assert_eq!(tracker.phase(), RouterConnectPhase::Pending);
    assert_eq!(tracker.prepare(&owner, &scope, timing(), now + Duration::from_secs(5)).err(), Some(RouterConnectError::Timeout));
    assert!(request.packets_for(&owner, &scope).is_none());
}

#[test]
fn router_connect_valid_reply_followed_by_bad_router_packet_suppresses_assignment() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup, server_output}; use std::io::Write;
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now); tracker.prepare(&owner, &scope, timing(), now).unwrap();
    let reply = control_reply(1, 0, true, None, 7, b""); let first = super::super::packet_stream::encode_packet_stream_frame(&reply).unwrap();
    let second = super::super::packet_stream::encode_packet_stream_frame(&[0xff]).unwrap();
    let plain = [first.expose_to_transport(), second.expose_to_transport()].concat(); server.writer().write_all(&plain).unwrap();
    let cipher = server_output(&mut server); let input = engine.on_wire_bytes(&owner, &scope, &cipher, now).unwrap().unwrap().plaintext;
    assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Protocol)); assert_eq!(tracker.phase(), RouterConnectPhase::Closed);
}

#[test]
fn debug_is_redacted_for_route_controls_retained_halves_and_reassembled_bytes() {
    let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"fixture-private-route"),now).unwrap();
    assert!(!format!("{receiver:?}").contains("fixture"));
    let value=receiver.on_packet(&wire(11,1,3,b"private"),now).unwrap();assert!(!format!("{value:?}").contains("fixture"));
}
