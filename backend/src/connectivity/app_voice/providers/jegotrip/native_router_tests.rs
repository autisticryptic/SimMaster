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

fn resolution_reference(host: &str, port: u16) -> super::super::object_reference::ObjectReference {
    super::super::object_reference::parse_object_reference(format!("AccountEntry:sarc -h {host} -p {port};").as_bytes()).unwrap()
}
fn resolution_endpoint(reference: &super::super::object_reference::ObjectReference) -> &ReferenceEndpoint {
    let super::super::object_reference::ReferenceTarget::Direct(endpoints) = reference.target() else { panic!("endpoint required") };
    &endpoints[0]
}
fn resolution_result(accepted: bool, hosts: &[&[u8]], reason: &[u8]) -> Vec<u8> {
    let mut writer = super::super::rpc_stream::Writer::new();
    writer.boolean(accepted).unwrap(); writer.u32(hosts.len() as u32).unwrap();
    for host in hosts { writer.bytes(host).unwrap(); }
    writer.bytes(reason).unwrap(); writer.finish().expose_to_transport().to_vec()
}
fn resolution_reply(call_id: i32, code: u32, result: &[u8]) -> Vec<u8> {
    let reply = super::super::rpc_envelope::encode_reply(call_id, code, result).unwrap();
    let packets = super::super::transport_packets::encode_call_packets(super::super::transport_packets::CallKind::Reply,
        reply.expose_to_transport(), super::super::transport_packets::TLS_FRAGMENT_THRESHOLD).unwrap();
    assert_eq!(packets.len(), 1); control_route(packets[0].expose_to_transport())
}
fn endpoint_peer_group(owner: &super::super::session::AttemptOwner, scope: &super::super::rpc_pending::RpcConnectionScope,
    engine: &mut super::super::tls_engine::EndpointTlsEngine, server: &mut tokio_rustls::rustls::ServerConnection,
    packets: &[&[u8]], now: Instant) -> super::super::tls_engine::VerifiedEndpointPackets {
    use std::io::Write;
    for packet in packets {
        server.writer().write_all(super::super::packet_stream::encode_packet_stream_frame(packet).unwrap().expose_to_transport()).unwrap();
    }
    let cipher = super::super::tls_engine::tests::server_output(server);
    engine.on_wire_bytes(owner, scope, &cipher, now).unwrap().unwrap().plaintext
}
fn finish_fixture_connect(tracker: &mut RouterConnectTracker, owner: &super::super::session::AttemptOwner,
    scope: &super::super::rpc_pending::RpcConnectionScope, engine: &mut super::super::tls_engine::EndpointTlsEngine,
    server: &mut tokio_rustls::rustls::ServerConnection, now: Instant) -> RouterAssignment {
    tracker.prepare(owner, scope, timing(), now).unwrap().unwrap();
    let reply = control_reply(1, 0, true, None, 0x55667788, b"");
    let input = endpoint_peer_group(owner, scope, engine, server, &[&reply], now);
    let assignment = tracker.on_packets(owner, scope, input, now).unwrap();
    assert!(assignment.is_some()); assignment.unwrap()
}

#[test]
fn router_resolve_native_request_retains_original_endpoint_ids_sequence_and_access() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    use super::super::rpc_stream::RpcProperty;
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let access = [RpcProperty { key: b"accessKey", value: b"fixture-key" }, RpcProperty { key: b"accessAuth", value: b"fixture-auth" }];
    let mut tracker = RouterConnectTracker::for_original(owner.clone(), scope.clone(), ROUTER_FIXTURE_ID, 7,
        &access, 1460, 10, Duration::from_secs(5), now).unwrap();
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    let reference = resolution_reference("arc@AccountEntry", 98); let endpoint = resolution_endpoint(&reference);
    let deadline = now + Duration::from_secs(4);
    let prepared = tracker.prepare_resolve(&owner, &scope, endpoint, timing(), deadline, now).unwrap().unwrap();
    assert_eq!(prepared.call_id(), 2); assert_eq!(tracker.next_deadline(), Some(deadline));
    let packets = prepared.packets_for(&owner, &scope).unwrap(); assert_eq!(packets.len(), 1);
    let (header, body) = decode_path_header(packets[0].expose_to_transport()).unwrap();
    assert_eq!(header.sequence, 11); assert_eq!(header.packet_kind, 1); assert_eq!(&body[..12], &[0; 12]);
    let route = super::super::router_packets::decode_routed_body(body).unwrap();
    let request = super::super::rpc_envelope::decode_request(&route.payload[1..]).unwrap();
    assert_eq!(request.call_id, 2); assert_eq!(request.method, b"clientResolveHosts.RouterClient.Common");
    // Independent type0 + version-count1/version0 + BE32 string length12.
    let mut expected = vec![0, 0, 1, 0, 0, 0, 0, 0, 12]; expected.extend_from_slice(b"AccountEntry");
    assert_eq!(request.arguments, expected);
    assert_eq!(request.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"Router");
    for field in access { assert_eq!(request.metadata.iter().find(|p| p.key == field.key).unwrap().value, field.value); }
    assert_eq!(tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(10), now).err(), Some(RouterConnectError::Pending));
    assert_eq!(tracker.next_deadline(), Some(deadline));
    assert_eq!(protocol.phase(), super::super::arc_session::ArcPhase::Initial);
    assert!(!format!("{prepared:?}").contains("AccountEntry"));
    tracker.close(); assert!(prepared.packets_for(&owner, &scope).is_none()); assert!(tracker.access.is_empty());
}

#[test]
fn router_resolve_tls_reply_captures_query_port_scheme_and_scope_without_granting_sdk() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    let reference = resolution_reference("arc@AccountEntry", 98); let endpoint = resolution_endpoint(&reference);
    tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(4), now).unwrap();
    let result = resolution_result(true, &[b"0x12.010", b"2.3", b"65535.0xfffffffe", b"2.3"], b"");
    let reply = resolution_reply(2, 0, &result);
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply], now);
    assert!(tracker.on_packets(&owner, &scope, input, now).unwrap().is_none());
    assert!(tracker.take_resolution(&control_owner(), &scope).is_none());
    assert_eq!(tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(4), now).err(), Some(RouterConnectError::Pending));
    let resolved = tracker.take_resolution(&owner, &scope).unwrap();
    assert_eq!(resolved.call_id(), 2);
    let hosts = resolved.hosts_for(&owner, &scope, endpoint).unwrap();
    assert_eq!(hosts.iter().map(|h| (h.network(), h.node())).collect::<Vec<_>>(), vec![(18, 8), (2, 3), (65535, u32::MAX - 1), (2, 3)]);
    for other in [resolution_reference("arc@UserEntry", 98), resolution_reference("arc@AccountEntry", 99),
        super::super::object_reference::parse_object_reference(b"id:arc -h arc@AccountEntry -p 98;").unwrap()] {
        assert!(resolved.hosts_for(&owner, &scope, resolution_endpoint(&other)).is_none());
    }
    assert!(resolved.hosts_for(&control_owner(), &scope, endpoint).is_none());
    assert!(resolved.hosts_for(&owner, &super::super::rpc_pending::RpcConnectionScope::fresh(), endpoint).is_none());
    assert_eq!(tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(4), now).unwrap().unwrap().call_id(), 3);
    assert_eq!(protocol.phase(), super::super::arc_session::ArcPhase::Initial);
    assert!(!format!("{resolved:?}").contains("AccountEntry"));
    engine.close(); assert!(resolved.hosts_for(&owner, &scope, endpoint).is_none());
}

#[test]
fn router_resolve_negative_empty_and_nonzero_outer_results_close_original_scope() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    for (code, result) in [(0, resolution_result(false, &[b"1.2"], b"")), (0, resolution_result(true, &[], b"")),
        (0, resolution_result(true, &[b"1.2"], b"fixture-rejection")), (1, resolution_result(true, &[b"1.2"], b""))] {
        let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
        let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
        finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
        let reference = resolution_reference("arc@AccountEntry", 98);
        tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), now + Duration::from_secs(4), now).unwrap();
        let reply = resolution_reply(2, code, &result);
        let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply], now);
        assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Rejected));
        assert!(!scope.is_open()); assert!(tracker.pending_resolve.is_none()); assert!(tracker.take_resolution(&owner, &scope).is_none());
    }
}

#[test]
fn router_resolve_malformed_hosts_counts_footer_and_trailing_stream_are_terminal() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let mut invalid = [b"".as_slice(), b"arc@Other", b"fixture.invalid", b"1.2.3", b"65536.2", b"1.4294967295", b"1.4294967296"]
        .iter().map(|host| resolution_result(true, &[*host], b"")).collect::<Vec<_>>();
    invalid.push(resolution_result(true, &vec![b"1.2".as_slice(); 33], b""));
    let mut missing = resolution_result(true, &[b"1.2"], b""); missing.truncate(missing.len() - 4); invalid.push(missing);
    let mut trailing = resolution_result(true, &[b"1.2"], b""); trailing.push(1); invalid.push(trailing);
    invalid.push(vec![0, 1, 0xff, 0xff, 0xff, 0xff]);
    for result in invalid {
        let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
        let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
        finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
        let reference = resolution_reference("arc@AccountEntry", 98);
        tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), now + Duration::from_secs(4), now).unwrap();
        let reply = resolution_reply(2, 0, &result);
        let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply], now);
        assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Protocol));
        assert_eq!(tracker.phase(), RouterConnectPhase::Closed); assert!(tracker.take_resolution(&owner, &scope).is_none());
    }
}

#[test]
fn router_resolve_foreign_tls_brand_and_unmatched_ids_cannot_complete_original_query() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    let reference = resolution_reference("arc@AccountEntry", 98);
    tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), now + Duration::from_secs(4), now).unwrap();
    let (_other_protocol, other_owner, other_scope, mut other_engine, mut other_server, other_now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let reply = resolution_reply(2, 0, &resolution_result(true, &[b"1.2"], b""));
    let input = endpoint_peer_group(&other_owner, &other_scope, &mut other_engine, &mut other_server, &[&reply], other_now);
    assert!(tracker.on_packets(&owner, &scope, input, now - Duration::from_secs(1)).unwrap().is_none());
    assert_eq!(tracker.last_now, now); assert!(tracker.pending_resolve.is_some());
    let unmatched = resolution_reply(999, 0, &resolution_result(true, &[b"1.2"], b""));
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&unmatched], now);
    assert!(tracker.on_packets(&owner, &scope, input, now).unwrap().is_none());
    assert!(tracker.take_resolution(&owner, &scope).is_none()); assert!(tracker.pending_resolve.is_some());
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply], now);
    tracker.on_packets(&owner, &scope, input, now).unwrap(); assert!(tracker.take_resolution(&owner, &scope).is_some());
}

#[test]
fn router_resolve_whole_tls_group_must_pass_before_result_is_published() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    for conflicting in [false, true] {
        let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
        let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
        finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
        let reference = resolution_reference("arc@AccountEntry", 98);
        tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), now + Duration::from_secs(4), now).unwrap();
        let reply = resolution_reply(2, 0, &resolution_result(true, &[b"1.2"], b""));
        let bad = if conflicting { resolution_reply(2, 0, &resolution_result(true, &[b"9.9"], b"")) } else { vec![0xff] };
        let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply, &bad], now);
        assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Protocol));
        assert!(tracker.take_resolution(&owner, &scope).is_none()); assert!(!scope.is_open());
    }
}

#[test]
fn router_resolve_identical_duplicate_reply_is_validated_and_published_once() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    let reference = resolution_reference("arc@AccountEntry", 98);
    tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), now + Duration::from_secs(4), now).unwrap();
    let reply = resolution_reply(2, 0, &resolution_result(true, &[b"1.2"], b""));
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply, &reply], now);
    tracker.on_packets(&owner, &scope, input, now).unwrap();
    assert!(tracker.take_resolution(&owner, &scope).is_some()); assert!(tracker.take_resolution(&owner, &scope).is_none());
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply], now);
    tracker.on_packets(&owner, &scope, input, now).unwrap();
    assert!(tracker.take_resolution(&owner, &scope).is_none()); assert!(scope.is_open());
}

#[test]
fn router_resolve_partial_and_probe_traffic_do_not_renew_absolute_budget() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    let reference = resolution_reference("arc@AccountEntry", 98); let deadline = now + Duration::from_secs(4);
    let request = tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), deadline, now).unwrap().unwrap();
    let reply = super::super::rpc_envelope::encode_reply(2, 0, &resolution_result(true, &[b"1.2", b"3.4"], b"")).unwrap();
    let fragments = super::super::transport_packets::encode_call_packets(super::super::transport_packets::CallKind::Reply, reply.expose_to_transport(), 8).unwrap();
    assert!(fragments.len() > 1);
    let partial = control_route(fragments[0].expose_to_transport());
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&partial], now + Duration::from_secs(1));
    tracker.on_packets(&owner, &scope, input, now + Duration::from_secs(1)).unwrap();
    let probe = wire(41, 2, 1, &[]);
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&probe], now + Duration::from_secs(3));
    tracker.on_packets(&owner, &scope, input, now + Duration::from_secs(3)).unwrap();
    assert_eq!(tracker.next_deadline(), Some(deadline)); assert!(tracker.take_resolution(&owner, &scope).is_none());
    assert_eq!(tracker.tick(&owner, &scope, deadline).err(), Some(RouterConnectError::Timeout));
    assert!(request.packets_for(&owner, &scope).is_none()); assert!(tracker.next_deadline().is_none());
}

#[test]
fn router_resolve_rejects_invalid_deadlines_and_non_query_endpoints_before_allocating_ids() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    let reference = resolution_reference("arc@AccountEntry", 98); let endpoint = resolution_endpoint(&reference);
    assert_eq!(tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(1), now).err(), Some(RouterConnectError::Pending));
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    for deadline in [now, now - Duration::from_secs(1), now + Duration::from_secs(31)] {
        assert_eq!(tracker.prepare_resolve(&owner, &scope, endpoint, timing(), deadline, now).err(), Some(RouterConnectError::Configuration));
    }
    let numeric = resolution_reference("1.2", 98);
    assert_eq!(tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&numeric), timing(), now + Duration::from_secs(1), now).err(), Some(RouterConnectError::Configuration));
    assert_eq!(tracker.next_control_id, Some(2)); assert!(scope.is_open());
    assert!(tracker.prepare_resolve(&control_owner(), &scope, endpoint, timing(), now, now - Duration::from_secs(1)).unwrap().is_none());
    assert_eq!(tracker.last_now, now);
}

#[test]
fn router_control_id_exhaustion_and_path_sequence_wrap_do_not_reuse_rpc_ids() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    tracker.next_control_id = Some(i32::MAX); tracker.sequence = u16::MAX;
    let reference = resolution_reference("arc@AccountEntry", 98); let endpoint = resolution_endpoint(&reference);
    let prepared = tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(4), now).unwrap().unwrap();
    assert_eq!(prepared.call_id(), i32::MAX); assert_eq!(tracker.sequence, 0); assert!(tracker.next_control_id.is_none());
    let reply = resolution_reply(i32::MAX, 0, &resolution_result(true, &[b"1.2"], b""));
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply], now);
    tracker.on_packets(&owner, &scope, input, now).unwrap(); tracker.take_resolution(&owner, &scope).unwrap();
    assert_eq!(tracker.prepare_resolve(&owner, &scope, endpoint, timing(), now + Duration::from_secs(4), now).err(), Some(RouterConnectError::Configuration));
    assert_eq!(tracker.sequence, 0);
}

#[test]
fn router_resolve_preserves_same_group_data_under_original_scope_without_common_delivery() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
    let reference = resolution_reference("arc@AccountEntry", 98);
    tracker.prepare_resolve(&owner, &scope, resolution_endpoint(&reference), timing(), now + Duration::from_secs(4), now).unwrap();
    let reply = resolution_reply(2, 0, &resolution_result(true, &[b"1.2"], b""));
    let mut data = control_route(b"opaque-unverified-channel"); data[7] = 0x10;
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&reply, &data], now);
    tracker.on_packets(&owner, &scope, input, now).unwrap();
    assert!(tracker.take_resolution(&owner, &scope).is_some());
    assert!(tracker.take_data(&control_owner(), &scope).is_none());
    let captured = tracker.take_data(&owner, &scope).unwrap(); assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].message_for(&owner, &scope).unwrap().first_header.packet_kind, 0);
    assert_eq!(protocol.phase(), super::super::arc_session::ArcPhase::Initial);
    assert!(!format!("{:?}", captured[0]).contains("opaque"));
    tracker.close(); assert!(captured[0].message_for(&owner, &scope).is_none());
}

#[test]
fn router_connect_unmatched_call_id_cannot_assign_before_original_reply() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
    tracker.prepare(&owner, &scope, timing(), now).unwrap().unwrap();
    let unrelated = control_reply(2, 0, true, None, 0x55667788, b"");
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&unrelated], now);
    assert!(tracker.on_packets(&owner, &scope, input, now).unwrap().is_none());
    assert_eq!(tracker.phase(), RouterConnectPhase::Pending);
    assert!(tracker.take_resolution(&owner, &scope).is_none());
    assert_eq!(protocol.phase(), super::super::arc_session::ArcPhase::Initial);
    let original = control_reply(1, 0, true, None, 0x55667788, b"");
    let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&original], now);
    assert_eq!(tracker.on_packets(&owner, &scope, input, now).unwrap().unwrap().ids_for(&owner, &scope),
        Some((ROUTER_FIXTURE_ID as u32, 0x55667788)));
}

#[test]
fn router_data_retention_limit_and_later_bad_packet_revoke_queued_data() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    for exhaust in [false, true] {
        let (_protocol, owner, scope, mut engine, mut server, now) = endpoint_setup(&cert, &tokio_rustls::rustls::version::TLS13);
        let mut tracker = tracker(&owner, &scope, ROUTER_FIXTURE_ID, now);
        finish_fixture_connect(&mut tracker, &owner, &scope, &mut engine, &mut server, now);
        let mut data = control_route(b"opaque-channel"); data[7] = 0x10;
        if exhaust {
            for _ in 0..64 {
                let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&data], now);
                tracker.on_packets(&owner, &scope, input, now).unwrap();
            }
            let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&data], now);
            assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Protocol));
        } else {
            let input = endpoint_peer_group(&owner, &scope, &mut engine, &mut server, &[&data, &[0xff]], now);
            assert_eq!(tracker.on_packets(&owner, &scope, input, now).err(), Some(RouterConnectError::Protocol));
        }
        assert!(tracker.data.is_empty()); assert_eq!(tracker.data_bytes, 0); assert!(tracker.take_data(&owner, &scope).is_none());
    }
}

fn mixed_width_data_fixture(port: u16, payload: &[u8]) -> Vec<u8> {
    // Independent source mask3: BE16 network/aux, BE32 node/BE16 aux,
    // BE16 source port. Destination mask0: its BE16 port, then IPv4 metadata.
    let mut body = vec![0,0,0,0,0,0x0c,0,0, 0,42,0xff,0xff, 0x11,0x22,0x33,0x44,0xff,0xff, 0,98];
    body.extend_from_slice(&port.to_be_bytes()); body.extend_from_slice(&[192,0,2,1]); body.extend_from_slice(payload);
    wire(41,0,1,&body)
}

#[test]
fn actual_endpoint_tls_mixed_width_data_activates_only_original_pending_listen_not_sdk() {
    use super::super::{tls_engine::tests::{Certificates, endpoint_setup}, router_ports::ListenProtocol};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol,owner,scope,mut engine,mut server,now) = endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    let ports = tracker.create_ports(&owner,&scope,12000,now).unwrap().unwrap();
    let lease = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    let assignment = finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    assert!(lease.address_for(&owner,&scope).is_none()); // connect alone cannot register
    let raw = mixed_width_data_fixture(12000,b"not-yet-channel");
    let input = endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&raw],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured = tracker.take_data(&owner,&scope).unwrap();
    assert_eq!(captured.len(),1); let delivery = ports.on_data(&owner,&scope,&assignment,&captured[0],now).unwrap().unwrap();
    let (route,payload) = delivery.for_lease(&owner,&scope,&lease).unwrap(); assert_eq!(payload,b"not-yet-channel");
    assert_eq!((route.source.network,route.source.node,route.source.port),(42,0x11223344,98));
    assert_eq!((route.destination.network,route.destination.node,route.destination.port),(12001,0x55667788,12000));
    assert_eq!(protocol.phase(),super::super::arc_session::ArcPhase::Initial);
    assert!(delivery.for_lease(&control_owner(),&scope,&lease).is_none());
    scope.close(); assert!(delivery.for_lease(&owner,&scope,&lease).is_none());
}

#[test]
fn retained_tls_data_predating_port_reuse_cannot_rebind_or_refresh_host_twice() {
    use super::super::{tls_engine::tests::{Certificates, endpoint_setup}, router_ports::ListenProtocol};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now) = endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    let ports = tracker.create_ports(&owner,&scope,12000,now).unwrap().unwrap();
    let old = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    let assignment = finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let raw = mixed_width_data_fixture(12000,b"old-channel");
    let input = endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&raw],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured = tracker.take_data(&owner,&scope).unwrap();
    old.release(&owner,&scope);
    let new = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    assert!(ports.on_data(&owner,&scope,&assignment,&captured[0],now).unwrap().is_none());
    assert!(ports.on_data(&owner,&scope,&assignment,&captured[0],now+Duration::from_secs(6)).unwrap().is_none());
    assert!(ports.needs_host_refresh(&owner,&scope,now+Duration::from_secs(6)));
    assert!(new.address_for(&owner,&scope).is_some()); // local binding only; no old payload
}

#[test]
fn a_second_clock_with_same_owner_scope_cannot_admit_another_trackers_tls_data() {
    use super::super::{tls_engine::tests::{Certificates, endpoint_setup}, router_ports::{PortClock, RouterPorts, ListenProtocol}};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now) = endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    let ports = RouterPorts::for_original(owner.clone(),scope.clone(),PortClock::for_original(owner.clone(),scope.clone()),12000,now).unwrap();
    let lease = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    let assignment = finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let raw = mixed_width_data_fixture(12000,b"wrong-book");
    let input = endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&raw],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured = tracker.take_data(&owner,&scope).unwrap();
    assert!(ports.on_data(&owner,&scope,&assignment,&captured[0],now-Duration::from_secs(1)).unwrap().is_none());
    assert!(lease.address_for(&owner,&scope).is_none()); assert!(scope.is_open());
}

#[test]
fn later_malformed_route_in_tls_group_prevents_port_registration_and_all_delivery() {
    use super::super::{tls_engine::tests::{Certificates, endpoint_setup}, router_ports::ListenProtocol};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now) = endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    let ports = tracker.create_ports(&owner,&scope,12000,now).unwrap().unwrap();
    let lease = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let raw = mixed_width_data_fixture(12000,b"channel"); let mut bad = raw.clone(); bad[12] = 4;
    let input = endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&raw,&bad],now);
    assert_eq!(tracker.on_packets(&owner,&scope,input,now).err(),Some(RouterConnectError::Protocol));
    assert!(tracker.take_data(&owner,&scope).is_none()); assert!(lease.address_for(&owner,&scope).is_none()); assert!(!scope.is_open());
}

#[test]
fn conflicting_connect_reply_in_same_tls_group_cannot_publish_assignment_for_ports() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now) = endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner,&scope,ROUTER_FIXTURE_ID,now); tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let first = control_reply(1,0,true,None,7,b""); let conflict = control_reply(1,0,true,None,8,b"");
    let input = endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&first,&conflict],now);
    assert_eq!(tracker.on_packets(&owner,&scope,input,now).err(),Some(RouterConnectError::Protocol));
    assert!(!scope.is_open()); assert_eq!(tracker.phase(),RouterConnectPhase::Closed);
}

#[test]
fn identical_connect_reply_in_same_tls_group_is_fully_parsed_but_assigns_only_once() {
    use super::super::tls_engine::tests::{Certificates, endpoint_setup};
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (protocol,owner,scope,mut engine,mut server,now) = endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker = tracker(&owner,&scope,ROUTER_FIXTURE_ID,now); tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let first = control_reply(1,0,true,None,7,b"");
    let input = endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&first,&first],now);
    assert_eq!(tracker.on_packets(&owner,&scope,input,now).unwrap().unwrap().ids_for(&owner,&scope),Some((12001,7)));
    assert_eq!(protocol.phase(),super::super::arc_session::ArcPhase::Initial);
}

fn node_control_fixture(payload: &[u8]) -> Vec<u8> {
    let mut body = vec![0;12]; body.extend_from_slice(payload); wire(42,0,0,&body)
}

#[test]
fn native_node_type_zero_is_unfragmented_and_does_not_relax_old_strict_header_codec() {
    let bytes = node_control_fixture(&[0]); let (header,_) = decode_path_header(&bytes).unwrap();
    assert_eq!((header.packet_kind,header.fragment_part),(0,0));
    let mut receiver = PathReceiver::default(); let message = data(receiver.on_packet(&bytes,Instant::now()).unwrap()).unwrap();
    assert_eq!(message.first_header.fragment_part,0); assert!(!receiver.has_partial());
    assert!(super::super::router_packets::decode_header(&bytes).is_err());
    for kind in [1,2,3] { let mut bad=bytes.clone(); bad[7]=kind; assert!(decode_path_header(&bad).is_err()); }
}

#[test]
fn node_probe_matches_native_type_zero_byte_payload_and_keeps_rpc_allocator_separate() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    assert_eq!(tracker.prepare_node_probe(&owner,&scope,timing(),now).err(),Some(RouterConnectError::Pending));
    finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let request=tracker.prepare_node_probe(&owner,&scope,timing(),now).unwrap().unwrap();
    let mut expected=vec![0,11,0,1,0,1,0,0]; expected.extend_from_slice(&[0;12]); expected.push(0);
    assert_eq!(request.packets_for(&owner,&scope).unwrap()[0].expose_to_transport(),expected);
    assert_eq!(tracker.next_control_id,Some(2)); assert_eq!(tracker.sequence,12);
    assert_eq!(protocol.phase(),super::super::arc_session::ArcPhase::Initial);
    tracker.close(); assert!(request.packets_for(&owner,&scope).is_none());
}

#[test]
fn real_endpoint_tls_node_probe_produces_original_reply_and_alive_never_ping_pongs() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let request=node_control_fixture(&[0]); let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&request],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured=tracker.take_data(&owner,&scope).unwrap();
    let reply=tracker.prepare_node_reply(&owner,&scope,&captured[0],timing(),now).unwrap().unwrap();
    let bytes=reply.packets_for(&owner,&scope).unwrap()[0].expose_to_transport(); assert_eq!(bytes[7],0);
    assert_eq!(&bytes[8..20],&[0;12]); assert_eq!(&bytes[20..],&[1]);
    let alive=node_control_fixture(&[1]); let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&alive],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured=tracker.take_data(&owner,&scope).unwrap();
    assert!(tracker.prepare_node_reply(&owner,&scope,&captured[0],timing(),now).unwrap().is_none());
}

#[test]
fn pre_assignment_node_controls_are_ignored_without_port_grant_or_deadline_renewal() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now); tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let probe=node_control_fixture(&[0]); let later=now+Duration::from_secs(4);
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&probe],later);
    assert!(tracker.on_packets(&owner,&scope,input,later).unwrap().is_none());
    assert!(tracker.take_data(&owner,&scope).unwrap().is_empty()); assert!(tracker.assigned_ids.is_none());
    assert_eq!(tracker.next_deadline(),Some(now+Duration::from_secs(5)));
    assert_eq!(tracker.tick(&owner,&scope,now+Duration::from_secs(5)).err(),Some(RouterConnectError::Timeout));
}

#[test]
fn node_report_levels_are_applied_after_whole_group_and_echoed_as_destination_label() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let report=node_control_fixture(&[4,0,2,3,4,5,0,0,0,0,0,0,0,0]);
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&report],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); assert_eq!(tracker.node_levels,[2,3,4,5]);
    let request=tracker.prepare_node_probe(&owner,&scope,timing(),now).unwrap().unwrap();
    let raw=request.packets_for(&owner,&scope).unwrap()[0].expose_to_transport();
    assert_eq!(raw[14],0); assert_eq!(raw[15],2); // source/destination labels
    let bad=node_control_fixture(&[4,0,2]); let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&report,&bad],now);
    assert_eq!(tracker.on_packets(&owner,&scope,input,now).err(),Some(RouterConnectError::Protocol));
    assert_eq!(tracker.node_levels,[0;4]); assert!(request.packets_for(&owner,&scope).is_none());
}

#[test]
fn normal_port_zero_data_is_not_reinterpreted_as_node_control() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let mut normal=node_control_fixture(&[0]); normal[7]=0x10;
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&normal],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured=tracker.take_data(&owner,&scope).unwrap();
    assert!(captured[0].node_control_for(&owner,&scope).is_none());
    assert!(tracker.prepare_node_reply(&owner,&scope,&captured[0],timing(),now).unwrap().is_none());
}

#[test]
fn stale_or_foreign_node_probe_cannot_generate_a_reply_on_this_path() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);
    finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let raw=node_control_fixture(&[0]); let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&raw],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap(); let captured=tracker.take_data(&owner,&scope).unwrap();
    assert!(tracker.prepare_node_reply(&control_owner(),&scope,&captured[0],timing(),now-Duration::from_secs(1)).unwrap().is_none());
    assert!(tracker.prepare_node_reply(&owner,&scope,&captured[0],timing(),now+Duration::from_secs(6)).unwrap().is_none());
    assert_eq!(tracker.sequence,11); assert!(scope.is_open());
}

#[test]
fn original_timing_probe_queues_one_type_one_feedback_without_local_node_or_rpc_id() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let probe=wire(42,2,1,&[]);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&probe],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();assert_eq!(tracker.pending_feedback,1);assert!(tracker.assigned_ids.is_none());
    let feedback=tracker.prepare_feedback(&owner,&scope,timing(),now).unwrap().unwrap();
    assert_eq!(feedback.packets_for(&owner,&scope).unwrap()[0].expose_to_transport(),[0,11,0,1,0,1,0,0x13]);
    assert!(tracker.prepare_feedback(&owner,&scope,timing(),now).unwrap().is_none());assert_eq!(tracker.next_control_id,Some(2));
}

#[test]
fn later_bad_tls_group_packet_revokes_timing_feedback_instead_of_sending_partial_success() {
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let probe=wire(42,2,1,&[]);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&probe,&[0xff]],now);
    assert_eq!(tracker.on_packets(&owner,&scope,input,now).err(),Some(RouterConnectError::Protocol));
    assert_eq!(tracker.pending_feedback,0);assert!(tracker.prepare_feedback(&owner,&scope,timing(),now).unwrap().is_none());
}

fn peer_reply_fixture(call_id:i32,dynamic:bool,node:u32,first_word:u16)->Vec<u8>{
    let mut result=super::super::rpc_stream::Writer::new();result.boolean(true).unwrap();
    if dynamic{result.u32(12001).unwrap();}result.u32(node).unwrap();
    for word in [first_word,2,3,4,5,6,7,8]{result.u16(word).unwrap();}
    result.bytes(b"opaque-alternate-hint").unwrap();result.bytes(b"").unwrap();
    resolution_reply(call_id,0,result.finish().expose_to_transport())
}

#[test]
fn peer_connect_two_follows_original_tracker_ids_and_captures_endpoint_before_negotiation(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,ROUTER_FIXTURE_ID,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let endpoint=resolution_reference("42.7",98);let endpoint=resolution_endpoint(&endpoint);
    let target=tracker.capture_peer_target(&owner,&scope,endpoint,0,now).unwrap().unwrap();
    let request=tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(5),now).unwrap().unwrap();assert_eq!(request.call_id(),2);
    let (_,body)=decode_path_header(request.packets_for(&owner,&scope).unwrap()[0].expose_to_transport()).unwrap();
    let routed=super::super::router_packets::decode_routed_body(body).unwrap();let decoded=super::super::rpc_envelope::decode_request(&routed.payload[1..]).unwrap();
    assert_eq!(decoded.method,b"clientConnect2.RouterClient.Common");assert_eq!(&decoded.arguments[5..13],&[0x55,0x66,0x77,0x88,0x11,0x22,0x33,0x44]);
    let reply=peer_reply_fixture(2,false,0x55667788,1);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reply],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();let binding=tracker.take_peer_binding(&owner,&scope,&target).unwrap();
    assert_eq!(binding.call_id(),2);assert_eq!(binding.reply_for(&owner,&scope,&tracker.port_clock,&target).unwrap().network_words,[1,2,3,4]);
    assert_eq!(protocol.phase(),super::super::arc_session::ArcPhase::Initial);assert_eq!(tracker.next_control_id,Some(3));
}

#[test]
fn peer_connect_four_preserves_actual_local_ids_after_original_dynamic_connect_three(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,-1,now);tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let connected=control_reply(1,0,true,Some(12001),0x55667788,b"");let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&connected],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();let r=resolution_reference("42",98);
    let target=tracker.capture_peer_target(&owner,&scope,resolution_endpoint(&r),0,now).unwrap().unwrap();
    let request=tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(5),now).unwrap().unwrap();
    let (_,body)=decode_path_header(request.packets_for(&owner,&scope).unwrap()[0].expose_to_transport()).unwrap();
    let route=super::super::router_packets::decode_routed_body(body).unwrap();let rpc=super::super::rpc_envelope::decode_request(&route.payload[1..]).unwrap();
    assert_eq!(rpc.method,b"clientConnect4.RouterClient.Common");assert_eq!(rpc.arguments.len(),21);assert_eq!(&rpc.arguments[13..],&[0,0,0,42,0,0,0,0]);
    let reply=peer_reply_fixture(2,true,0x55667788,1);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reply],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();assert!(tracker.take_peer_binding(&owner,&scope,&target).is_some());
}

#[test]
fn logical_query_result_is_consumed_only_by_original_peer_target_and_next_rpc_id(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let r=resolution_reference("arc@AccountEntry",98);let endpoint=resolution_endpoint(&r);
    tracker.prepare_resolve(&owner,&scope,endpoint,timing(),now+Duration::from_secs(5),now).unwrap();
    let reply=resolution_reply(2,0,&resolution_result(true,&[b"42.7"],b""));let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reply],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();let target=tracker.capture_peer_target(&owner,&scope,endpoint,0,now).unwrap().unwrap();
    assert_eq!(target.ids_for(&owner,&scope,&tracker.port_clock),Some((42,7,98)));assert!(tracker.resolution.is_none());
    assert_eq!(tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(5),now).unwrap().unwrap().call_id(),3);
}

#[test]
fn peer_binding_cannot_be_taken_with_new_equal_target_and_foreign_clock_cannot_prepare(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let r=resolution_reference("42.7",98);let endpoint=resolution_endpoint(&r);
    let foreign_clock=super::super::router_ports::PortClock::for_original(owner.clone(),scope.clone());
    let foreign=super::super::router_peer::PeerTarget::capture(&owner,&scope,&foreign_clock,endpoint,None,0).unwrap();
    assert!(tracker.prepare_peer(&owner,&scope,&foreign,timing(),now,now-Duration::from_secs(1)).unwrap().is_none());assert_eq!(tracker.last_now,now);
    let target=tracker.capture_peer_target(&owner,&scope,endpoint,0,now).unwrap().unwrap();
    tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(5),now).unwrap();
    let reply=peer_reply_fixture(2,false,0x55667788,1);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reply],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();let replacement=tracker.capture_peer_target(&owner,&scope,endpoint,0,now).unwrap().unwrap();
    assert!(tracker.take_peer_binding(&owner,&scope,&replacement).is_none());assert!(tracker.take_peer_binding(&owner,&scope,&target).is_some());
}

#[test]
fn peer_reply_conflict_changed_local_id_or_later_bad_packet_revokes_whole_group(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    for bad in [peer_reply_fixture(2,false,0x55667788,9),peer_reply_fixture(2,false,0x55667789,1),vec![0xff]]{
        let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
        let mut tracker=tracker(&owner,&scope,12001,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
        let r=resolution_reference("42.7",98);let target=tracker.capture_peer_target(&owner,&scope,resolution_endpoint(&r),0,now).unwrap().unwrap();
        tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(5),now).unwrap();let reply=peer_reply_fixture(2,false,0x55667788,1);
        let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reply,&bad],now);
        assert_eq!(tracker.on_packets(&owner,&scope,input,now).err(),Some(RouterConnectError::Protocol));assert!(tracker.take_peer_binding(&owner,&scope,&target).is_none());
    }
}

#[test]
fn peer_deadline_is_not_renewed_by_node_or_unmatched_rpc_traffic(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let r=resolution_reference("42",98);let target=tracker.capture_peer_target(&owner,&scope,resolution_endpoint(&r),0,now).unwrap().unwrap();
    let deadline=now+Duration::from_secs(5);let request=tracker.prepare_peer(&owner,&scope,&target,timing(),deadline,now).unwrap().unwrap();
    let unrelated=peer_reply_fixture(99,false,0x55667788,1);let pulse=node_control_fixture(&[1]);let later=now+Duration::from_secs(4);
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&unrelated,&pulse],later);tracker.on_packets(&owner,&scope,input,later).unwrap();
    assert_eq!(tracker.next_deadline(),Some(deadline));assert!(tracker.take_peer_binding(&owner,&scope,&target).is_none());
    assert_eq!(tracker.tick(&owner,&scope,deadline).err(),Some(RouterConnectError::Timeout));assert!(request.packets_for(&owner,&scope).is_none());
}

#[test]
fn normal_router_outbound_pair_wraps_wire_sequence_without_reusing_rpc_ids(){
    let frames=encode_path_packets(h(u16::MAX,0,1),b"abcdefghij",18).unwrap();assert_eq!(frames.len(),2);
    assert_eq!(frames[0].expose_to_transport(),[0xff,0xff,0x12,0x34,0x56,0x78,9,0x20,b'a',b'b',b'c',b'd',b'e']);
    assert_eq!(frames[1].expose_to_transport(),[0,0,0x12,0x34,0x56,0x78,9,0x30,b'f',b'g',b'h',b'i',b'j']);
    let mut receiver=PathReceiver::default();let now=Instant::now();assert!(data(receiver.on_packet(frames[1].expose_to_transport(),now).unwrap()).is_none());
    assert_eq!(data(receiver.on_packet(frames[0].expose_to_transport(),now).unwrap()).unwrap().body.expose_to_transport(),b"abcdefghij");
}

#[test]
fn prepared_channel_datagram_needs_original_live_port_peer_and_generation(){
    use super::super::{tls_engine::tests::{Certificates,endpoint_setup},router_ports::ListenProtocol};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);let ports=tracker.create_ports(&owner,&scope,12000,now).unwrap().unwrap();
    let lease=ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    let assignment=finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let r=resolution_reference("42.7",98);let target=tracker.capture_peer_target(&owner,&scope,resolution_endpoint(&r),0,now).unwrap().unwrap();
    tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(5),now).unwrap();
    let reply=peer_reply_fixture(2,false,0x55667788,1);let alive=node_control_fixture(&[1]);
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reply,&alive],now);tracker.on_packets(&owner,&scope,input,now).unwrap();
    for received in tracker.take_data(&owner,&scope).unwrap(){ports.on_data(&owner,&scope,&assignment,&received,now).unwrap();}
    let peer=tracker.take_peer_binding(&owner,&scope,&target).unwrap();
    let prepared=tracker.prepare_datagram(&owner,&scope,&lease,&peer,77,0,b"opaque-channel",timing(),now).unwrap().unwrap();
    let (header,body)=decode_path_header(prepared.packets_for(&owner,&scope).unwrap()[0].expose_to_transport()).unwrap();
    assert_eq!((header.packet_kind,header.fragment_part),(0,1));let route=super::super::native_routing::decode_routing_body(body).unwrap();
    assert_eq!((route.routing.source.port,route.routing.destination.network,route.routing.destination.node,route.routing.destination.port),(12000,42,7,98));
    assert_eq!((route.routing.destination.network_tag,route.routing.destination.node_tag),(1,5));assert_eq!(route.payload,b"opaque-channel");
    lease.release(&owner,&scope);assert!(prepared.packets_for(&owner,&scope).is_none());
    let replacement=ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).unwrap().unwrap();
    assert!(prepared.packets_for(&owner,&scope).is_none());assert!(tracker.prepare_datagram(&owner,&scope,&replacement,&peer,77,0,b"new",timing(),now).unwrap().is_some());
    assert_eq!(protocol.phase(),super::super::arc_session::ArcPhase::Initial);
}

#[test]
fn original_tls_header_measurements_feed_scoped_timing_and_not_node_or_sdk_grants(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);tracker.prepare(&owner,&scope,timing(),now).unwrap();
    let mut initial=control_reply(1,0,true,None,0x55667788,b"");initial[4..6].fill(0);
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&initial],now);tracker.on_packets(&owner,&scope,input,now).unwrap();
    let mut probe=wire(5000,2,1,&[]);probe[2..4].copy_from_slice(&1000u16.to_be_bytes());probe[4..6].copy_from_slice(&80u16.to_be_bytes());
    let at=now+Duration::from_millis(100);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&probe],at);tracker.on_packets(&owner,&scope,input,at).unwrap();
    assert_eq!(tracker.metrics_for(&owner,&scope).unwrap().echoed_delay_ms,Some(20));assert!(tracker.take_data(&owner,&scope).unwrap().is_empty());
    let send=now+Duration::from_millis(125);let measured=tracker.timing_for(&owner,&scope,send).unwrap().unwrap();
    assert_eq!((measured.word1,measured.word2,measured.byte6),(125,1025,0));
    let feedback=tracker.prepare_feedback(&owner,&scope,measured,send).unwrap().unwrap();let bytes=feedback.packets_for(&owner,&scope).unwrap()[0].expose_to_transport();
    assert_eq!(&bytes[2..6],&[0,125,4,1]);assert_eq!(bytes[7],0x13);
    assert!(tracker.timing_for(&control_owner(),&scope,now-Duration::from_secs(1)).unwrap().is_none());
    assert_eq!(protocol.phase(),super::super::arc_session::ArcPhase::Initial);
}

#[test]
fn peer_clock_reset_clears_old_router_halves_without_renewing_original_query_deadline(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let r=resolution_reference("arc@AccountEntry",98);let deadline=now+Duration::from_secs(4);
    tracker.prepare_resolve(&owner,&scope,resolution_endpoint(&r),timing(),deadline,now).unwrap();
    let half=wire(41,0,2,b"old-prefix");let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&half],now);
    tracker.on_packets(&owner,&scope,input,now).unwrap();assert!(tracker.receiver.has_partial());
    let mut reset=wire(42,2,1,&[]);reset[2..4].copy_from_slice(&20000u16.to_be_bytes());
    let at=now+Duration::from_millis(1);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&reset],at);
    tracker.on_packets(&owner,&scope,input,at).unwrap();assert!(!tracker.receiver.has_partial());assert!(scope.is_open());
    assert_eq!(tracker.next_deadline(),Some(deadline));assert_eq!(tracker.next_control_id,Some(3));
    let mut late=wire(42,0,3,b"late-tail");late[2..4].copy_from_slice(&20000u16.to_be_bytes());
    let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&late],at);tracker.on_packets(&owner,&scope,input,at).unwrap();
    assert!(tracker.take_data(&owner,&scope).unwrap().is_empty());assert_eq!(tracker.tick(&owner,&scope,deadline).err(),Some(RouterConnectError::Timeout));
}

#[test]
fn malformed_later_packet_retires_original_timing_observation_and_prepared_output(){
    use super::super::tls_engine::tests::{Certificates,endpoint_setup};
    let cert=Certificates::generate("DNS:fixture.invalid");
    let (_protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
    let mut tracker=tracker(&owner,&scope,12001,now);finish_fixture_connect(&mut tracker,&owner,&scope,&mut engine,&mut server,now);
    let measured=tracker.timing_for(&owner,&scope,now).unwrap().unwrap();let prepared=tracker.prepare_node_probe(&owner,&scope,measured,now).unwrap().unwrap();
    let probe=wire(41,2,1,&[]);let input=endpoint_peer_group(&owner,&scope,&mut engine,&mut server,&[&probe,&[0xff]],now);
    assert!(tracker.on_packets(&owner,&scope,input,now).is_err());assert!(tracker.metrics_for(&owner,&scope).is_none());
    assert!(prepared.packets_for(&owner,&scope).is_none());assert!(tracker.timing_for(&owner,&scope,now).unwrap().is_none());
}
