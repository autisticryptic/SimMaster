//! GitHub Actions source-only fixtures: no registration, target execution,
//! protocol simulation, command submission or call/media readiness grant.
use super::*;
use super::super::{
    arc_session::{ArcLoginInputs, ArcReply, ArcRpcSession},
    batch_scheduler::BatchScheduler,
    rpc_envelope::encode_reply,
    rpc_stream::{RpcBytes, Writer},
    session::HostingSession,
    tests::imported,
};

const ID: u64 = 0xfedc_ba98_7654_3210;
const OBJECT: &[u8] = b"#fixture-call-client";
const DOMAIN: &[u8] = b"fixture-domain";

fn owner() -> (HostingSession, AttemptOwner) {
    let mut hosting = HostingSession::new("line-a".into());
    hosting.import(imported(), 100).unwrap();
    let owner = hosting.begin_login(100).unwrap();
    (hosting, owner)
}

fn dispatcher(owner: AttemptOwner, scope: RpcConnectionScope) -> CallDispatcher {
    CallDispatcher::for_arc_session(owner, scope, CallIncomingBinding { object_id: OBJECT, domain: DOMAIN }).unwrap()
}

fn context() -> (HostingSession, ArcRpcSession, AttemptOwner, RpcConnectionScope, CallDispatcher) {
    let (hosting, owner) = owner();
    let rpc = ArcRpcSession::new(owner.clone());
    let scope = rpc.connection_scope();
    let dispatcher = dispatcher(owner.clone(), scope.clone());
    (hosting, rpc, owner, scope, dispatcher)
}

fn arguments(kind: CallNoticeKind, id: u64, peer: &[u8], properties: &[RpcProperty<'_>]) -> RpcBytes {
    let mut w = Writer::versioned().unwrap();
    w.u64(id).unwrap();
    if kind == CallNoticeKind::Invited { w.bytes(peer).unwrap(); }
    w.properties(properties).unwrap();
    w.finish()
}

fn request<'a>(method: &'a [u8], arguments: &'a [u8]) -> RpcRequest<'a> {
    RpcRequest { call_id: 37, method, arguments, metadata: vec![
        RpcProperty { key: b"id", value: OBJECT }, RpcProperty { key: b"domain", value: DOMAIN },
    ] }
}

fn authenticate(rpc: &mut ArcRpcSession, original: &AttemptOwner, scope: &RpcConnectionScope) {
    let (id, _) = rpc.prepare_login(ArcLoginInputs {
        typed_identity: b"username:fixture", domain_name: b"fixture.invalid",
        selected_session_id: b"fixture-selected-session", phone_login_token: b"fixture-phone-token",
        application_key: b"fixture-app-key", domain_id: b"fixture-domain-id",
        application_id: b"fixture-app-id", device_info_cc: b"", client_properties: &[],
    }, &[]).unwrap();
    let mut w = Writer::new();
    w.boolean(true).unwrap(); w.bytes(b"fixture-user").unwrap(); w.bytes(b"fixture-auth-token").unwrap();
    w.properties(&[]).unwrap(); w.bytes(b"").unwrap();
    let result = w.finish();
    let envelope = encode_reply(id, 0, result.expose_to_transport()).unwrap();
    assert!(matches!(rpc.on_reply(original, scope, envelope.expose_to_transport()).unwrap(), Some(ArcReply::Login(_))));
}

// Build real scheduler-owned incoming data using the existing ARC/Writer test
// convention, never by minting ServerRequestTicket or DirectReplyTicket fields.
fn incoming(rpc: &mut ArcRpcSession, scheduler: &mut BatchScheduler, original: &AttemptOwner,
    request: &RpcRequest<'_>, sequence: i32, now: u64) -> IncomingBatchRequest {
    let batch = scheduler.prepare(now).unwrap();
    let (id, _) = rpc.prepare_scheduled_swap(&batch, &[]).unwrap();
    scheduler.commit_submission(batch, now).unwrap();
    let mut w = Writer::new();
    w.boolean(true).unwrap();
    for field in [9, 1, 1, 0, 0, 0] { w.u32(field).unwrap(); }
    w.u32(1).unwrap(); w.i32(sequence).unwrap();
    w.bytes(request.method).unwrap(); w.bytes(request.arguments).unwrap(); w.properties(&request.metadata).unwrap();
    w.u32(0).unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap();
    let result = w.finish();
    let envelope = encode_reply(id, 0, result.expose_to_transport()).unwrap();
    let scope = rpc.connection_scope();
    let reply = match rpc.on_reply(original, &scope, envelope.expose_to_transport()).unwrap().unwrap() {
        ArcReply::Swap(reply) => reply, _ => panic!("original scheduled batch reply expected"),
    };
    let mut delivery = scheduler.on_reply(&reply, now).unwrap().unwrap();
    assert_eq!(delivery.requests.len(), 1);
    delivery.requests.remove(0)
}

#[test]
fn binding_rejects_empty_wildcard_enc_control_oversize_closed_and_captures_original_once() {
    let (_hosting, rpc, original, scope, _d) = context();
    for invalid in [b"".as_slice(), b"*", b"fixture.*", b"ENC(unresolved)", b"prefixENC(unresolved)", b"bad\0", b"bad\n", b"bad\x7f"] {
        for object_field in [true, false] {
            let binding = if object_field { CallIncomingBinding { object_id: invalid, domain: DOMAIN } }
                else { CallIncomingBinding { object_id: OBJECT, domain: invalid } };
            assert_eq!(CallDispatcher::for_arc_session(original.clone(), scope.clone(), binding).err(), Some(CodecError::InvalidField));
        }
    }
    let oversized = vec![b'x'; MAX_FIELD_BYTES + 1];
    for binding in [CallIncomingBinding { object_id: &oversized, domain: DOMAIN },
        CallIncomingBinding { object_id: OBJECT, domain: &oversized }] {
        assert_eq!(CallDispatcher::for_arc_session(original.clone(), scope.clone(), binding).err(), Some(CodecError::TooLarge));
    }
    let maximum = vec![b'x'; MAX_FIELD_BYTES];
    assert!(CallDispatcher::for_arc_session(original.clone(), scope.clone(),
        CallIncomingBinding { object_id: &maximum, domain: &maximum }).is_ok());
    let mut object = OBJECT.to_vec(); let mut domain = DOMAIN.to_vec();
    let d = CallDispatcher::for_arc_session(original.clone(), scope.clone(),
        CallIncomingBinding { object_id: &object, domain: &domain }).unwrap();
    object.fill(b'x'); domain.fill(b'y'); drop(object); drop(domain);
    let bytes = arguments(CallNoticeKind::Released, ID, b"", &[]);
    assert!(d.on_direct_request(&original, &scope, &request(RELEASED_METHOD.as_bytes(), bytes.expose_to_transport())).unwrap().is_some());
    drop(rpc);
    assert_eq!(CallDispatcher::for_arc_session(original, scope, CallIncomingBinding { object_id: OBJECT, domain: DOMAIN }).err(), Some(CodecError::InvalidField));
}

#[test]
fn exact_three_methods_preserve_binary_peer_all_properties_and_full_be64_without_readiness() {
    let (hosting, _rpc, original, scope, d) = context();
    let properties = [RpcProperty { key: &[0xff, 0], value: &[0, 0xfe, 3] },
        RpcProperty { key: b"RseEx.SDP", value: b"fixture-opaque-sdp" },
        RpcProperty { key: b"endpoint", value: b"fixture-opaque-endpoint" },
        RpcProperty { key: b"crypto", value: b"ENC(opaque-property-not-interpreted)" },
        RpcProperty { key: b"time", value: b"not-a-time" },
        RpcProperty { key: b"", value: b"" }];
    for (kind, method) in [(CallNoticeKind::Invited, INVITED_METHOD), (CallNoticeKind::Released, RELEASED_METHOD),
        (CallNoticeKind::Accepted, ACCEPTED_METHOD)] {
        for id in [0, ID, u32::MAX as u64 + 1, u64::MAX] {
            let bytes = arguments(kind, id, &[0xff, 0, 7], &properties);
            let owned = d.on_direct_request(&original, &scope, &request(method.as_bytes(), bytes.expose_to_transport())).unwrap().unwrap();
            assert_eq!(owned.owner(), &original); assert!(owned.connection_scope().matches(&scope));
            assert_eq!(owned.kind(), kind); assert_eq!(owned.native_id().expose_to_backend(), id);
            let data = owned.data_for(&original, &scope).unwrap();
            assert_eq!(data.kind, kind); assert_eq!(data.call_id.expose_to_backend(), id);
            assert_eq!(data.peer, if kind == CallNoticeKind::Invited { Some([0xff, 0, 7].as_slice()) } else { None });
            assert_eq!(data.properties.len(), properties.len());
            for p in properties { assert!(data.properties.contains(&p)); }
        }
    }
    let empty_peer = arguments(CallNoticeKind::Invited, ID, b"", &[]);
    let owned = d.on_direct_request(&original, &scope, &request(INVITED_METHOD.as_bytes(), empty_peer.expose_to_transport())).unwrap().unwrap();
    assert_eq!(owned.data_for(&original, &scope).unwrap().peer, Some(b"".as_slice()));
    assert!(!hosting.incoming_call_ready()); assert!(!hosting.incoming_message_ready()); assert!(!hosting.outbound_authorized());
}

#[test]
fn wrong_owner_incarnation_generation_scope_and_closed_scope_are_checked_before_garbage() {
    let (mut hosting, _rpc, original, scope, d) = context();
    let (_recreated_hosting, recreated_owner) = owner();
    assert_ne!(original, recreated_owner); // same line/generation, different incarnation
    let newer_owner = hosting.begin_login(100).unwrap();
    let garbage = RpcRequest { call_id: 37, method: INVITED_METHOD.as_bytes(), arguments: &[0xff], metadata: vec![] };
    for wrong_owner in [&recreated_owner, &newer_owner] {
        assert!(!d.matches(wrong_owner, &scope));
        assert!(d.on_direct_request(wrong_owner, &scope, &garbage).unwrap().is_none());
    }
    let foreign = RpcConnectionScope::fresh();
    assert!(!d.matches(&original, &foreign));
    assert!(d.on_direct_request(&original, &foreign, &garbage).unwrap().is_none());
    assert!(d.matches(&original.clone(), &scope.clone()));
    scope.close();
    assert!(!d.matches(&original, &scope));
    assert!(d.on_direct_request(&original, &scope, &garbage).unwrap().is_none());
}

#[test]
fn unrelated_methods_return_none_before_garbage_duplicate_or_oversized_metadata_parse() {
    let (_hosting, _rpc, original, scope, d) = context();
    let oversized = vec![0; MAX_FIELD_BYTES + 1];
    for method in [b"onInvited".as_slice(), b"onInvited.CallClientEx.CallEx.extra", b"oninvited.CallClientEx.CallEx",
        b"onReplaced.CallClientEx.CallEx", b"accept.CallServerEx.CallEx", b"recvOnlineMessage.MessageReceiver.Client",
        b"SDK.notification", b"", b"onAccepted.CallClientEx.CallEx\0"] {
        let garbage = RpcRequest { call_id: 37, method, arguments: &[0xff], metadata: vec![
            RpcProperty { key: b"id", value: &oversized }; MAX_MAP_ENTRIES + 1
        ] };
        assert!(d.on_direct_request(&original, &scope, &garbage).unwrap().is_none());
    }
}

#[test]
fn routing_requires_exact_original_id_domain_and_never_infers_call_server_object() {
    let (_hosting, _rpc, original, scope, d) = context();
    let bytes = arguments(CallNoticeKind::Invited, ID, b"fixture-peer", &[
        RpcProperty { key: b"callServerEx.oid", value: OBJECT },
    ]);
    for (index, values) in [(0, vec![b"".as_slice(), b"*", b"#different", b"ENC(unresolved)", b"#fixture-call-client\0"]),
        (1, vec![b"".as_slice(), b"*", b"different-domain", b"ENC(unresolved)", b"fixture-domain\0"])] {
        for value in values {
            let mut r = request(INVITED_METHOD.as_bytes(), bytes.expose_to_transport()); r.metadata[index].value = value;
            assert_eq!(d.on_direct_request(&original, &scope, &r).err(), Some(CodecError::InvalidField));
        }
        let mut missing = request(INVITED_METHOD.as_bytes(), bytes.expose_to_transport()); missing.metadata.remove(index);
        missing.metadata.push(RpcProperty { key: b"callServerEx.oid", value: OBJECT });
        assert_eq!(d.on_direct_request(&original, &scope, &missing).err(), Some(CodecError::InvalidField));
    }
    let mut valid = request(INVITED_METHOD.as_bytes(), bytes.expose_to_transport());
    valid.metadata.push(RpcProperty { key: b"__magic", value: &[0xff, 0] });
    valid.metadata.push(RpcProperty { key: b"callServerEx.oid", value: b"#not-the-receiver" });
    assert!(d.on_direct_request(&original, &scope, &valid).unwrap().is_some());
    valid.arguments = &[0xff]; valid.metadata[0].value = b"#wrong";
    assert_eq!(d.on_direct_request(&original, &scope, &valid).err(), Some(CodecError::InvalidField));
}

#[test]
fn routing_duplicate_count_field_and_aggregate_bounds_are_strict_before_arguments() {
    let (_hosting, _rpc, original, scope, d) = context();
    let bytes = arguments(CallNoticeKind::Released, ID, b"", &[]);
    for duplicate_key in [b"id".as_slice(), b"domain", b"opaque"] {
        let mut r = request(RELEASED_METHOD.as_bytes(), &[0xff]);
        r.metadata.push(RpcProperty { key: b"opaque", value: b"" });
        r.metadata.push(RpcProperty { key: duplicate_key, value: b"conflict" });
        assert_eq!(d.on_direct_request(&original, &scope, &r).err(), Some(CodecError::DuplicateKey));
    }
    let large = vec![0; MAX_FIELD_BYTES + 1];
    for field in [RpcProperty { key: &large, value: b"" }, RpcProperty { key: b"opaque", value: &large }] {
        let mut r = request(RELEASED_METHOD.as_bytes(), &[0xff]); r.metadata.push(field);
        assert_eq!(d.on_direct_request(&original, &scope, &r).err(), Some(CodecError::TooLarge));
    }
    let keys: Vec<_> = (0..MAX_MAP_ENTRIES - 2).map(|n| format!("k{n}").into_bytes()).collect();
    let mut r = request(RELEASED_METHOD.as_bytes(), bytes.expose_to_transport());
    r.metadata.extend(keys.iter().map(|key| RpcProperty { key, value: b"" }));
    assert_eq!(r.metadata.len(), MAX_MAP_ENTRIES);
    assert!(d.on_direct_request(&original, &scope, &r).unwrap().is_some());
    r.metadata.push(RpcProperty { key: b"one-too-many", value: b"" }); r.arguments = &[0xff];
    assert_eq!(d.on_direct_request(&original, &scope, &r).err(), Some(CodecError::TooLarge));
    let mut r = request(RELEASED_METHOD.as_bytes(), bytes.expose_to_transport());
    let routing_bytes: usize = r.metadata.iter().map(|p| p.key.len() + p.value.len()).sum();
    let first = vec![0; MAX_FIELD_BYTES];
    let second = vec![0; MAX_STREAM_BYTES - routing_bytes - b"opaque".len() - b"more".len() - first.len()];
    r.metadata.push(RpcProperty { key: b"opaque", value: &first });
    r.metadata.push(RpcProperty { key: b"more", value: &second });
    assert!(d.on_direct_request(&original, &scope, &r).unwrap().is_some());
    let beyond = vec![0; second.len() + 1]; r.metadata[3].value = &beyond; r.arguments = &[0xff];
    assert_eq!(d.on_direct_request(&original, &scope, &r).err(), Some(CodecError::TooLarge));
}

#[test]
fn notice_truncation_stream_version_and_trailing_data_never_publish_partial_owned_data() {
    let (_hosting, _rpc, original, scope, d) = context();
    for (kind, method) in [(CallNoticeKind::Invited, INVITED_METHOD), (CallNoticeKind::Released, RELEASED_METHOD),
        (CallNoticeKind::Accepted, ACCEPTED_METHOD)] {
        let valid = arguments(kind, ID, &[0xff, 0], &[RpcProperty { key: b"p", value: b"v" }]);
        let valid = valid.expose_to_transport();
        for end in 0..valid.len() {
            assert!(d.on_direct_request(&original, &scope, &request(method.as_bytes(), &valid[..end])).is_err(), "prefix {end}");
        }
        for (offset, value, error) in [(0, 1, CodecError::UnsupportedStream), (2, 2, CodecError::UnsupportedVersion),
            (4, 1, CodecError::UnsupportedVersion)] {
            let mut malformed = valid.to_vec(); malformed[offset] = value;
            assert_eq!(d.on_direct_request(&original, &scope, &request(method.as_bytes(), &malformed)).err(), Some(error));
        }
        let mut trailing = valid.to_vec(); trailing.push(0);
        assert_eq!(d.on_direct_request(&original, &scope, &request(method.as_bytes(), &trailing)).err(), Some(CodecError::TrailingData));
    }
}

#[test]
fn property_duplicates_map_peer_field_and_stream_limits_are_rejected_not_truncated() {
    let (_hosting, _rpc, original, scope, d) = context();
    let mut w = Writer::versioned().unwrap(); w.u64(ID).unwrap(); w.u32(2).unwrap();
    for _ in 0..2 { w.bytes(b"p").unwrap(); w.bytes(b"v").unwrap(); }
    let duplicate = w.finish();
    assert_eq!(d.on_direct_request(&original, &scope, &request(RELEASED_METHOD.as_bytes(), duplicate.expose_to_transport())).err(), Some(CodecError::DuplicateKey));
    let mut w = Writer::versioned().unwrap(); w.u64(ID).unwrap(); w.u32((MAX_MAP_ENTRIES + 1) as u32).unwrap();
    let map = w.finish();
    assert_eq!(d.on_direct_request(&original, &scope, &request(RELEASED_METHOD.as_bytes(), map.expose_to_transport())).err(), Some(CodecError::TooLarge));
    let mut w = Writer::versioned().unwrap(); w.u64(ID).unwrap(); w.u32((MAX_FIELD_BYTES + 1) as u32).unwrap();
    let peer = w.finish();
    assert_eq!(d.on_direct_request(&original, &scope, &request(INVITED_METHOD.as_bytes(), peer.expose_to_transport())).err(), Some(CodecError::TooLarge));
    for oversized_key in [true, false] {
        let mut w = Writer::versioned().unwrap(); w.u64(ID).unwrap(); w.u32(1).unwrap();
        if !oversized_key { w.bytes(b"p").unwrap(); }
        w.u32((MAX_FIELD_BYTES + 1) as u32).unwrap(); w.u32(0).unwrap();
        let field = w.finish();
        assert_eq!(d.on_direct_request(&original, &scope, &request(RELEASED_METHOD.as_bytes(), field.expose_to_transport())).err(), Some(CodecError::TooLarge));
    }
    let oversized_stream = vec![0; MAX_STREAM_BYTES + 1];
    assert_eq!(d.on_direct_request(&original, &scope, &request(RELEASED_METHOD.as_bytes(), &oversized_stream)).err(), Some(CodecError::TooLarge));
    let maximum = vec![0xff; MAX_FIELD_BYTES];
    for (kind, method, properties) in [(CallNoticeKind::Invited, INVITED_METHOD, vec![]),
        (CallNoticeKind::Released, RELEASED_METHOD, vec![RpcProperty { key: &maximum, value: b"" }]),
        (CallNoticeKind::Accepted, ACCEPTED_METHOD, vec![RpcProperty { key: b"p", value: &maximum }])] {
        let valid = arguments(kind, ID, &maximum, &properties);
        let owned = d.on_direct_request(&original, &scope, &request(method.as_bytes(), valid.expose_to_transport())).unwrap().unwrap();
        let data = owned.data_for(&original, &scope).unwrap();
        if kind == CallNoticeKind::Invited { assert_eq!(data.peer, Some(maximum.as_slice())); }
        else { assert_eq!(data.properties, properties); }
    }
    let keys: Vec<_> = (0..MAX_MAP_ENTRIES).map(|n| format!("p{n}").into_bytes()).collect();
    let properties: Vec<_> = keys.iter().map(|key| RpcProperty { key, value: b"" }).collect();
    let bytes = arguments(CallNoticeKind::Released, ID, b"", &properties);
    let owned = d.on_direct_request(&original, &scope, &request(RELEASED_METHOD.as_bytes(), bytes.expose_to_transport())).unwrap().unwrap();
    assert_eq!(owned.data_for(&original, &scope).unwrap().properties.len(), MAX_MAP_ENTRIES);
}

#[test]
fn owned_payload_survives_input_dispatcher_drop_redacts_debug_and_denies_new_access_after_arc_drop() {
    let (hosting, rpc, original, scope, d) = context();
    let (_other_hosting, other_owner) = owner();
    let owned = {
        let peer = b"fixture-private-peer".to_vec();
        let key = b"fixture-private-key".to_vec(); let value = vec![0xff, 0, 7];
        let bytes = arguments(CallNoticeKind::Invited, ID, &peer, &[RpcProperty { key: &key, value: &value }]);
        let r = request(INVITED_METHOD.as_bytes(), bytes.expose_to_transport());
        d.on_direct_request(&original, &scope, &r).unwrap().unwrap()
    };
    assert_eq!(format!("{:?}", CallIncomingBinding { object_id: OBJECT, domain: DOMAIN }), "CallIncomingBinding(<redacted>)");
    assert_eq!(format!("{d:?}"), "CallDispatcher(<redacted>)");
    assert_eq!(format!("{owned:?}"), "OwnedCallNotice(<redacted>)");
    drop(d); drop(hosting);
    assert!(!owned.matches(&other_owner, &scope)); assert!(owned.data_for(&other_owner, &scope).is_none());
    let foreign = RpcConnectionScope::fresh();
    assert!(!owned.matches(&original, &foreign)); assert!(owned.data_for(&original, &foreign).is_none());
    let copied = {
        let data = owned.data_for(&original, &scope.clone()).unwrap();
        assert_eq!(data.properties[0].key, b"fixture-private-key"); assert_eq!(data.properties[0].value, &[0xff, 0, 7]);
        data.peer.unwrap().to_vec()
    };
    drop(rpc);
    assert!(!owned.matches(&original, &scope)); assert!(!owned.connection_scope().is_open());
    assert!(owned.data_for(&original, &scope).is_none());
    assert_eq!(copied, b"fixture-private-peer"); // previously copied data is not physically revocable
    drop(owned); // Zeroizing fields own their drop cleanup; no freed-memory inspection
}

#[test]
fn batch_original_ticket_provenance_is_checked_before_parse_and_kept_separate_from_notice() {
    let (_hosting, mut rpc, original, scope, d) = context();
    authenticate(&mut rpc, &original, &scope);
    let mut scheduler = rpc.take_batch_scheduler(7).unwrap();
    let (_foreign_hosting, foreign_owner) = owner();
    let foreign_owner_dispatcher = dispatcher(foreign_owner, scope.clone());
    let foreign_scope_dispatcher = dispatcher(original.clone(), RpcConnectionScope::fresh());
    let garbage = RpcRequest { call_id: 37, method: INVITED_METHOD.as_bytes(), arguments: &[0xff], metadata: vec![] };
    let bad = incoming(&mut rpc, &mut scheduler, &original, &garbage, 5, 0);
    assert!(bad.ticket().matches(&original, &scope)); assert_eq!(bad.ticket().sequence(), 5);
    assert!(foreign_owner_dispatcher.on_batch_request(&bad).unwrap().is_none());
    assert!(foreign_scope_dispatcher.on_batch_request(&bad).unwrap().is_none());
    assert_eq!(d.on_batch_request(&bad).err(), Some(CodecError::InvalidField));
    let bytes = arguments(CallNoticeKind::Invited, ID, &[0xff, 0, 7], &[RpcProperty { key: b"p", value: &[0, 0xfe] }]);
    let good = {
        let r = request(INVITED_METHOD.as_bytes(), bytes.expose_to_transport());
        incoming(&mut rpc, &mut scheduler, &original, &r, 6, 1)
    };
    let notice = d.on_batch_request(&good).unwrap().unwrap();
    drop(bytes);
    assert!(notice.matches(&original, &scope)); assert_eq!(notice.native_id().expose_to_backend(), ID);
    assert_eq!(notice.data_for(&original, &scope).unwrap().peer, Some([0xff, 0, 7].as_slice()));
    let ticket = good.into_ticket(); // original sequence/capability stays outside OwnedCallNotice
    assert_eq!(ticket.sequence(), 6); assert!(ticket.matches(&original, &scope));
    assert!(scheduler.complete_server_request(&ticket, 0, b"").unwrap());
    assert!(!scheduler.complete_server_request(&ticket, 0, b"").unwrap());
    scope.close();
    assert!(d.on_batch_request(&bad).unwrap().is_none()); assert!(notice.data_for(&original, &scope).is_none());
}
