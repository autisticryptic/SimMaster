use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    batch_scheduler::BatchWords, controller::Callback,
    rpc_envelope::{encode_reply, encode_request, decode_reply, decode_request},
    rpc_incoming::ONLINE_MESSAGE_METHOD, rpc_stream::Writer,
    session::{HostingSession, STATUS_REGISTER}, tests::imported,
    transport_packets::{encode_call_packets, CallAssembler, CallKind},
};

fn owner() -> AttemptOwner {
    let mut s = HostingSession::new("line-a".into()); s.import(imported(), 100).unwrap(); s.begin_login(100).unwrap()
}
fn login_inputs() -> ArcLoginInputs<'static> {
    ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-selector",
        phone_login_token: b"fixture-pwd", application_key: b"fixture-key", domain_id: b"fixture-domain", application_id: b"fixture-app",
        device_info_cc: b"", client_properties: &[] }
}
fn feed(engine: &mut OfflineBackend, owner: &AttemptOwner, scope: &RpcConnectionScope, bytes: &[u8], kind: CallKind, now: u64)
    -> Result<Vec<OfflineEvent>, ProtocolError> {
    let mut events = vec![];
    for packet in encode_call_packets(kind, bytes, 8).unwrap() {
        events.extend(engine.on_plaintext_packet(owner, scope, packet.expose_to_transport(), now)?);
    }
    Ok(events)
}
fn setup(incoming: bool) -> (OfflineBackend, AttemptOwner, RpcConnectionScope) {
    let original = owner(); let mut engine = OfflineBackend::new(original.clone()); let scope = engine.connection_scope();
    let login = engine.prepare_login(login_inputs(), &[]).unwrap();
    let mut result = Writer::new(); result.boolean(true).unwrap(); result.bytes(b"fixture-user").unwrap(); result.bytes(b"fixture-auth").unwrap();
    result.properties(&[]).unwrap(); result.bytes(b"").unwrap(); let result = result.finish();
    let envelope = encode_reply(login.call_id, 0, result.expose_to_transport()).unwrap();
    let events = feed(&mut engine, &original, &scope, envelope.expose_to_transport(), CallKind::Reply, 0).unwrap();
    assert!(matches!(&events[0], OfflineEvent::Callback(callback) if callback.callback == (Callback::SdkLogin { success: true })));
    engine.configure(0, if incoming { Some(IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain",
        receiver_kind: b"fixture.registered.kind" }) } else { None }).unwrap_err();
    engine.configure(7, if incoming { Some(IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain",
        receiver_kind: b"fixture.registered.kind" }) } else { None }).unwrap();
    (engine, original, scope)
}
fn registration() -> GatewayRegistration<'static> {
    GatewayRegistration { authname_prefix: "fixture-prefix", channel_id: "fixture-channel", delete_int_code: "1",
        sip_number: "fixture-number", sip_password: "fixture-SipPwd" }
}
fn message_arguments(kind: &[u8], body: &[u8]) -> super::super::rpc_stream::RpcBytes {
    let mut w = Writer::versioned().unwrap(); w.bytes(kind).unwrap();
    w.properties(&[RpcProperty { key: b"Sip.Message.From", value: &[0xff, 0] },
        RpcProperty { key: b"Sip.Message.Time", value: b"fixture-opaque-time" }]).unwrap();
    w.bytes(body).unwrap(); w.finish()
}
fn batch_response(id: i32, words: BatchWords, requests: &[(i32, &[u8], &[u8])], results: &[(i32, &[u8])]) -> super::super::rpc_stream::RpcBytes {
    let mut w = Writer::new(); w.boolean(true).unwrap(); for field in words.fields() { w.u32(field).unwrap(); }
    w.u32(requests.len() as u32).unwrap();
    for &(sequence, method, args) in requests {
        w.i32(sequence).unwrap(); w.bytes(method).unwrap(); w.bytes(args).unwrap();
        w.properties(&[RpcProperty { key: b"id", value: b"#fixture-client" },
            RpcProperty { key: b"domain", value: b"fixture-domain" }, RpcProperty { key: b"__magic", value: &[0, 0xff] }]).unwrap();
    }
    w.u32(results.len() as u32).unwrap();
    for &(sequence, stream) in results { w.i32(sequence).unwrap(); w.u32(0).unwrap(); w.bytes(stream).unwrap(); }
    w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); let w = w.finish(); encode_reply(id, 0, w.expose_to_transport()).unwrap()
}
fn unframe(packets: &OwnedCallPackets) -> super::super::transport_packets::CallMessage {
    let mut assembler = CallAssembler::default(); let mut result = None;
    for packet in &packets.packets { result = assembler.on_packet(packet.expose_to_transport()).unwrap(); }
    result.unwrap()
}
fn peer() -> BatchWords { BatchWords { incarnation: 9, next_batch: 1, batches: 1, ack_incarnation: 7,
    ack_next_batch: 1, ack_batches: 1 } }

#[test]
fn offline_owned_login_scheduler_nested_hosting_and_message_ticket_are_separate_from_channels() {
    let (mut engine, original, scope) = setup(true);
    let sequence = engine.queue_hosting(original.clone(), "fixture-uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    let swap = engine.prepare_swap(1, &[]).unwrap(); let framed = unframe(&swap); let request = decode_request(framed.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
    assert_eq!(request.metadata.iter().find(|p| p.key == b"token").unwrap().value, b"fixture-auth");
    let args = message_arguments(b"fixture.registered.kind", &[0xff, 0, 1]);
    let reply = batch_response(swap.call_id, peer(), &[(1, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport())],
        &[(sequence, &[0, 1, 0, 0, 0, 0])]);
    let events = feed(&mut engine, &original, &scope, reply.expose_to_transport(), CallKind::Reply, 2).unwrap();
    assert_eq!(events.len(), 2); assert!(matches!(&events[0], OfflineEvent::Callback(c) if c.owner == original && c.callback == (Callback::HostingAck { success: true })));
    let incoming = match events.into_iter().nth(1).unwrap() { OfflineEvent::Message(message) => message, _ => panic!("message expected") };
    assert_eq!(incoming.owner(), &original); assert_eq!(incoming.message().body, &[0xff, 0, 1]);
    assert_eq!(incoming.message().from, Some(vec![0xff, 0])); assert_eq!(incoming.message().time, Some(b"fixture-opaque-time".to_vec()));
    assert!(engine.complete_message(incoming.ticket(), true).unwrap()); assert!(!engine.complete_message(incoming.ticket(), true).unwrap());
    let next = engine.prepare_swap(3, &[]).unwrap(); let framed = unframe(&next); let request = decode_request(framed.payload.expose_to_transport()).unwrap();
    let mut reader = super::super::rpc_stream::Reader::versioned(request.arguments).unwrap(); reader.bytes().unwrap(); reader.bytes().unwrap();
    let payload = super::super::rpc_batch::read_payload(&mut reader).unwrap();
    assert_eq!(payload.fields, [7, 2, 1, 9, 1, 1]); assert_eq!(payload.results[&1].result_stream, &[0, 1, 0, 0, 0, 0]);
    assert_eq!(engine.phase(), ArcPhase::Authenticated); assert!(!format!("{engine:?}").contains("fixture"));
}

#[test]
fn malformed_ack_cannot_deliver_otherwise_successful_nested_hosting_result() {
    let (mut engine, original, scope) = setup(false);
    let sequence = engine.queue_hosting(original.clone(), "uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    let swap = engine.prepare_swap(1, &[]).unwrap(); let words = BatchWords { ack_incarnation: 8, ..peer() };
    let response = batch_response(swap.call_id, words, &[], &[(sequence, &[0, 1, 0, 0, 0, 0])]);
    assert!(feed(&mut engine, &original, &scope, response.expose_to_transport(), CallKind::Reply, 2).is_err());
    assert_eq!(engine.phase(), ArcPhase::Closed); assert!(engine.prepare_swap(3, &[]).is_err());
}

#[test]
fn duplicate_nested_request_is_not_redelivered_but_existing_ticket_remains_completable() {
    let (mut engine, original, scope) = setup(true); let args = message_arguments(b"fixture.registered.kind", b"body"); let mut ticket = None;
    for now in 0..2 {
        let swap = engine.prepare_swap(now, &[]).unwrap();
        let words = BatchWords { ack_next_batch: 0, ack_batches: 0, ..peer() };
        let response = batch_response(swap.call_id, words, &[(1, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport())], &[]);
        let events = feed(&mut engine, &original, &scope, response.expose_to_transport(), CallKind::Reply, now).unwrap();
        if now == 0 { ticket = match events.into_iter().next().unwrap() { OfflineEvent::Message(msg) => Some(msg), _ => panic!("message") }; }
        else { assert!(events.is_empty()); }
    }
    assert!(engine.complete_message(ticket.unwrap().ticket(), true).unwrap());
}

#[test]
fn direct_server_message_requires_explicit_binding_and_consumer_completion_then_deduplicates() {
    let (mut engine, original, scope) = setup(true); let args = message_arguments(b"fixture.registered.kind", b"body");
    let routing = [RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
        RpcProperty { key: b"__magic", value: &[0, 0xff] }];
    let request = encode_request(1, &routing, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
    let events = feed(&mut engine, &original, &scope, request.expose_to_transport(), CallKind::Request, 0).unwrap();
    assert_eq!(events.len(), 1);
    let message = match events.into_iter().next().unwrap() { OfflineEvent::DirectMessage(msg) => msg, _ => panic!("direct message") };
    assert_eq!(message.owner(), &original); assert_eq!(message.message().body, b"body");
    assert!(feed(&mut engine, &original, &scope, request.expose_to_transport(), CallKind::Request, 1).unwrap().is_empty());
    let reply = engine.complete_direct_message(message.reply_ticket(), true).unwrap().unwrap(); let framed = unframe(&reply);
    assert_eq!(framed.kind, CallKind::Reply); let reply = decode_reply(framed.payload.expose_to_transport()).unwrap();
    assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]);
    assert!(engine.complete_direct_message(message.reply_ticket(), true).unwrap().is_none());
    let duplicate_magic = encode_request(2, &routing, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
    assert!(feed(&mut engine, &original, &scope, duplicate_magic.expose_to_transport(), CallKind::Request, 2).unwrap().is_empty());
}

#[test]
fn old_owner_bad_object_unsupported_method_and_no_receiver_never_receive_or_grant_ready() {
    let (mut engine, original, scope) = setup(true);
    assert!(engine.on_plaintext_packet(&owner(), &scope, &[0xff], 0).unwrap().is_empty());
    assert!(engine.on_plaintext_packet(&original, &RpcConnectionScope::fresh(), &[0xff], 0).unwrap().is_empty());
    let args = message_arguments(b"fixture.registered.kind", b"body");
    let routing = [RpcProperty { key: b"id", value: b"#other" }, RpcProperty { key: b"domain", value: b"fixture-domain" }, RpcProperty { key: b"__magic", value: &[0, 1] }];
    let request = encode_request(1, &routing, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
    assert!(feed(&mut engine, &original, &scope, request.expose_to_transport(), CallKind::Request, 0).is_err());
    let (mut engine, original, scope) = setup(false);
    let request = encode_request(1, &routing, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
    assert!(feed(&mut engine, &original, &scope, request.expose_to_transport(), CallKind::Request, 0).is_err());
}

#[test]
fn accepted_server_ticket_becomes_unusable_after_original_arc_closes_or_another_backend_receives_it() {
    let (mut engine, original, scope) = setup(true); let args = message_arguments(b"fixture.registered.kind", b"body");
    let swap = engine.prepare_swap(0, &[]).unwrap();
    let response = batch_response(swap.call_id, BatchWords { ack_next_batch: 0, ack_batches: 0, ..peer() },
        &[(1, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport())], &[]);
    let events = feed(&mut engine, &original, &scope, response.expose_to_transport(), CallKind::Reply, 0).unwrap();
    let message = match events.into_iter().next().unwrap() { OfflineEvent::Message(msg) => msg, _ => panic!("message") };
    let (mut other, _, _) = setup(true);
    assert!(other.complete_message(message.ticket(), true).is_err());
    assert_eq!(other.phase(), ArcPhase::Authenticated);
    engine.close(); assert!(!message.ticket().is_open()); assert!(engine.complete_message(message.ticket(), true).is_err());
}

#[test]
fn unsupported_nested_methods_or_unconfigured_receiver_close_without_boolean_success() {
    let args = message_arguments(b"fixture.registered.kind", b"body");
    for incoming in [true, false] {
        let (mut engine, original, scope) = setup(incoming); let swap = engine.prepare_swap(0, &[]).unwrap();
        let method = if incoming { b"receiveSipMESSAGE.SipGatewayPub.SipGateway".as_slice() } else { ONLINE_MESSAGE_METHOD.as_bytes() };
        let response = batch_response(swap.call_id, BatchWords { ack_next_batch: 0, ack_batches: 0, ..peer() },
            &[(1, method, args.expose_to_transport())], &[]);
        assert!(feed(&mut engine, &original, &scope, response.expose_to_transport(), CallKind::Reply, 0).is_err());
        assert_eq!(engine.phase(), ArcPhase::Closed);
    }
}

#[test]
fn lower_stream_transport_is_owned_but_cannot_turn_carrier_bytes_into_hosting_or_inbox_events() {
    let (mut engine, original, scope) = setup(true);
    let mut transport = engine.take_packet_transport(super::super::packet_transport::PacketRoute::DirectTlsCarrier).unwrap();
    assert!(engine.take_packet_transport(super::super::packet_transport::PacketRoute::DirectTlsCarrier).is_err());
    let sequence = engine.queue_hosting(original.clone(), "fixture-uri", STATUS_REGISTER, &registration(), &[]).unwrap();
    let swap = engine.prepare_swap(1, &[]).unwrap();
    let response = batch_response(swap.call_id, peer(), &[], &[(sequence, &[0, 1, 0, 0, 0, 0])]);
    let wire = transport.encode_tls_bytes(&original, &scope, response.expose_to_transport(), None).unwrap().unwrap();
    let tls = transport.on_bytes(&original, &scope, wire.frames_for(&original, &scope).unwrap()[0].expose_to_transport()).unwrap();
    assert_eq!(tls[0].bytes_for_tls(&original, &scope), Some(response.expose_to_transport()));
    assert_eq!(engine.phase(), ArcPhase::Authenticated);
    // Only the separate explicitly trusted plaintext packet path can dispatch
    // the original nested result; parsing an opaque carrier never does that.
    let callbacks = feed(&mut engine, &original, &scope, response.expose_to_transport(), CallKind::Reply, 2).unwrap();
    assert_eq!(callbacks.len(), 1);
    assert!(matches!(&callbacks[0], OfflineEvent::Callback(c) if c.owner == original && c.callback == (Callback::HostingAck { success: true })));
    assert!(transport.on_bytes(&original, &scope, &[0, 0, 0, 0]).is_err());
    assert_eq!(engine.phase(), ArcPhase::Closed);
    assert!(tls[0].bytes_for_tls(&original, &scope).is_none());
    assert!(engine.prepare_swap(3, &[]).is_err());
    assert!(feed(&mut engine, &original, &scope, response.expose_to_transport(), CallKind::Reply, 3).unwrap().is_empty());
}

#[test]
fn incoming_unknown_kind_returns_false_not_success_and_closed_ticket_cannot_complete() {
    let (mut engine, original, scope) = setup(true); let args = message_arguments(b"other-kind", b"body");
    let routing = [RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" }, RpcProperty { key: b"__magic", value: &[0, 2] }];
    let request = encode_request(1, &routing, ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
    let events = feed(&mut engine, &original, &scope, request.expose_to_transport(), CallKind::Request, 0).unwrap();
    let reply = match events.into_iter().next().unwrap() { OfflineEvent::Outgoing(value) => value, _ => panic!("false reply") };
    let frame = unframe(&reply); assert_eq!(decode_reply(frame.payload.expose_to_transport()).unwrap().result_stream, &[0, 0, 0, 0, 0, 0]);
    let args = message_arguments(b"fixture.registered.kind", b"body");
    let request = encode_request(2, &[RpcProperty { key: b"__magic", value: &[0, 3] }, routing[0], routing[1]], ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
    let events = feed(&mut engine, &original, &scope, request.expose_to_transport(), CallKind::Request, 1).unwrap();
    let message = match events.into_iter().next().unwrap() { OfflineEvent::DirectMessage(value) => value, _ => panic!("message") };
    engine.close(); assert!(engine.complete_direct_message(message.reply_ticket(), true).unwrap().is_none());
}
