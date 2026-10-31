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
    server: &mut tokio_rustls::rustls::ServerConnection, now: Instant) {
    tracker.prepare(owner, scope, timing(), now).unwrap().unwrap();
    let reply = control_reply(1, 0, true, None, 0x55667788, b"");
    let input = endpoint_peer_group(owner, scope, engine, server, &[&reply], now);
    assert!(tracker.on_packets(owner, scope, input, now).unwrap().is_some());
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
