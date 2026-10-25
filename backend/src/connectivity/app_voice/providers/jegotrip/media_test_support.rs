//! Source fixtures only. Executed by the private Actions test inventory, never
//! against an account, device, SDK or external peer. Retain the returned RPC:
//! dropping it retires the original CallBook and its tickets.
use super::{
    arc_session::{ArcLoginInputs, ArcPhase, ArcReply, ArcRpcSession},
    call_binding::CallIncomingBinding,
    call_control::{CallControlTicket, OwnedCallOperationReply},
    rpc_calls::{CallOperation, INVITED_METHOD, NativeCallId},
    rpc_envelope::{decode_request, encode_reply, RpcRequest},
    rpc_pending::RpcConnectionScope,
    rpc_stream::{Reader, RpcBytes, RpcProperty, Writer},
    media_endpoint::{MediaEndpointProfile, MediaEndpointProfileInputs},
    runtime_provision::{AccountTrust, AccountTrustInputs},
    session::{AttemptOwner, HostingSession}, tests::imported,
};
use std::{sync::Arc, time::Duration};

pub(crate) const NATIVE_ID: u64 = 0x0123_4567_89ab_cdef;
pub(crate) const ENDPOINT: &[u8] = b"#fixture-endpoint:tcp -h fixture.invalid -p 443;";
pub(crate) const CONTENT: &[u8] = b"fixture-opaque-content\0\xff\x80";

pub(crate) fn invited_ticket(
    rpc: &mut ArcRpcSession, owner: &AttemptOwner, scope: &RpcConnectionScope, native_id: u64,
) -> CallControlTicket {
    let dispatcher = rpc.incoming_call_dispatcher(CallIncomingBinding {
        object_id: b"#fixture-call-client", domain: b"fixture-domain",
    }).unwrap();
    let mut arguments = Writer::versioned().unwrap();
    arguments.u64(native_id).unwrap();
    arguments.bytes(b"fixture-peer-not-a-media-authority").unwrap();
    arguments.properties(&[]).unwrap();
    let arguments = arguments.finish();
    let request = RpcRequest {
        call_id: 37, method: INVITED_METHOD.as_bytes(), arguments: arguments.expose_to_transport(),
        metadata: vec![RpcProperty { key: b"id", value: b"#fixture-call-client" },
            RpcProperty { key: b"domain", value: b"fixture-domain" }],
    };
    let notice = dispatcher.on_direct_request(owner, scope, &request).unwrap().unwrap();
    let ticket = rpc.register_call_notice(&notice).unwrap().unwrap();
    assert_eq!(ticket.native_id(), NativeCallId::from_bits(native_id));
    rpc.call_acknowledged(&ticket, true).unwrap();
    assert!(rpc.call_acknowledgement_ready(&ticket));
    ticket
}

pub(crate) fn finish_operation(
    rpc: &mut ArcRpcSession, owner: &AttemptOwner, scope: &RpcConnectionScope,
    sequence: i32, operation: CallOperation, properties: &[RpcProperty<'_>],
) -> OwnedCallOperationReply {
    let mut result = Writer::new();
    result.boolean(true).unwrap();
    if operation == CallOperation::Accept { result.properties(properties).unwrap(); }
    result.bytes(b"").unwrap();
    let result = result.finish();
    let envelope = encode_reply(sequence, 0, result.expose_to_transport()).unwrap();
    let Some(ArcReply::Call(reply)) = rpc.on_reply(owner, scope, envelope.expose_to_transport()).unwrap()
        else { panic!("original pending CallEx reply required") };
    assert_eq!(reply.request_id(), sequence);
    assert_eq!(reply.operation(), operation);
    assert!(reply.accepted());
    assert!(!rpc.is_pending(sequence));
    reply
}

pub(crate) fn accepted_context(endpoint: &[u8], content: &[u8])
    -> (ArcRpcSession, AttemptOwner, RpcConnectionScope, CallControlTicket, OwnedCallOperationReply)
{
    accepted_with_properties(&[RpcProperty { key: b"endpoint", value: endpoint },
        RpcProperty { key: b"content", value: content }])
}

pub(crate) fn accepted_with_properties(properties: &[RpcProperty<'_>])
    -> (ArcRpcSession, AttemptOwner, RpcConnectionScope, CallControlTicket, OwnedCallOperationReply)
{
    let mut hosting = HostingSession::new("fixture-media-original-line".into());
    hosting.import(imported(), 100).unwrap();
    let owner = hosting.begin_login(100).unwrap();
    let mut rpc = ArcRpcSession::new(owner.clone());
    let scope = rpc.connection_scope();
    let (sequence, _) = rpc.prepare_login(ArcLoginInputs {
        typed_identity: b"username:fixture", domain_name: b"fixture.invalid",
        selected_session_id: b"fixture-selector", phone_login_token: b"fixture-pwd",
        application_key: b"fixture-appkey", domain_id: b"fixture-domain",
        application_id: b"fixture-app", device_info_cc: b"", client_properties: &[],
    }, &[]).unwrap();
    assert_eq!(sequence, 1);
    let mut login = Writer::new();
    login.boolean(true).unwrap(); login.bytes(b"fixture-user").unwrap();
    login.bytes(b"fixture-original-account-token").unwrap();
    login.properties(&[]).unwrap(); login.bytes(b"").unwrap();
    let login = login.finish();
    let envelope = encode_reply(sequence, 0, login.expose_to_transport()).unwrap();
    assert!(matches!(rpc.on_reply(&owner, &scope, envelope.expose_to_transport()).unwrap(),
        Some(ArcReply::Login(_))));
    assert_eq!(rpc.phase(), ArcPhase::Authenticated);
    rpc.configure_call_control(&[
        RpcProperty { key: b"id", value: b"#fixture-call-server" },
        RpcProperty { key: b"domain", value: b"fixture-domain" },
    ]).unwrap();
    let ticket = invited_ticket(&mut rpc, &owner, &scope, NATIVE_ID);
    let (inform, request) = rpc.prepare_call(&ticket, CallOperation::Inform, &[]).unwrap();
    assert_eq!(inform, 2);
    assert_eq!(decode_request(request.expose_to_transport()).unwrap().method,
        CallOperation::Inform.method().as_bytes());
    finish_operation(&mut rpc, &owner, &scope, inform, CallOperation::Inform, &[]);
    let (accept, _) = rpc.prepare_call(&ticket, CallOperation::Accept, &[]).unwrap();
    assert_eq!(accept, 3);
    let reply = finish_operation(&mut rpc, &owner, &scope, accept, CallOperation::Accept, properties);
    assert!(ticket.is_live());
    (rpc, owner, scope, ticket, reply)
}

pub(crate) fn profile(root: &str, authority: &str) -> Arc<MediaEndpointProfile> {
    let trust = Arc::new(AccountTrust::supplied(AccountTrustInputs {
        authority, root_der_base64: root, enable_sni: true,
    }).unwrap());
    Arc::new(MediaEndpointProfile::supplied(MediaEndpointProfileInputs {
        trust, routing: &[
            RpcProperty { key: b"id", value: b"#fixture-endpoint" },
            RpcProperty { key: b"domain", value: b"fixture-domain" },
        ], features: &[b"tcp", b"udp"], properties: &[], reference_decode_key: None,
        handshake_timeout: Duration::from_secs(2), partial_timeout: Duration::from_secs(2),
        io_timeout: Duration::from_secs(2), rpc_timeout: Duration::from_secs(2),
    }).unwrap())
}

/// Result encoder, not a request-codec equality oracle. Footer and binary
/// property values are preserved as native result DATA, not UDP authorization.
pub(crate) fn endpoint_result(accepted: bool, reason: &[u8]) -> RpcBytes {
    let mut result = Writer::new();
    result.boolean(accepted).unwrap(); result.u32(2).unwrap();
    result.bytes(b"tcp").unwrap(); result.bytes(b"udp").unwrap();
    result.properties(&[
        RpcProperty { key: b"Client.Host", value: b"opaque-not-an-authority" },
        RpcProperty { key: b"Client.ArcPort", value: b"65000" },
        RpcProperty { key: b"content", value: b"result\0\xff" },
    ]).unwrap();
    result.bytes(reason).unwrap();
    result.properties(&[RpcProperty { key: b"fixture-footer", value: b"private\0\xff" }]).unwrap();
    result.finish()
}

/// Independent field-by-field request inspection: BE16 version list, bytes,
/// BE32 set count/strings, BE32 map. No reuse of encode_endpoint_connect.
pub(crate) fn assert_endpoint_request(wire: &[u8], content: &[u8], original_port: u16) {
    let request = decode_request(wire).unwrap();
    assert_eq!(request.call_id, 1);
    assert_eq!(request.method, b"connect.Endpoint.Endpoint");
    assert_eq!(request.metadata.len(), 2);
    assert_eq!(request.metadata[0], RpcProperty { key: b"id", value: b"#fixture-endpoint" });
    assert_eq!(request.metadata[1], RpcProperty { key: b"domain", value: b"fixture-domain" });
    assert!(!request.metadata.iter().any(|p| matches!(p.key, b"pwd" | b"token")));
    assert_eq!(&request.arguments[..5], &[0, 0, 1, 0, 0]);
    let mut body = Reader::versioned(request.arguments).unwrap();
    assert_eq!(body.bytes().unwrap(), content);
    assert_eq!(body.u32().unwrap(), 2);
    assert_eq!(body.bytes().unwrap(), b"tcp"); assert_eq!(body.bytes().unwrap(), b"udp");
    assert_eq!(body.u32().unwrap(), 1);
    assert_eq!(body.bytes().unwrap(), b"Client.ArcPort");
    let port = original_port.to_string();
    assert_eq!(body.bytes().unwrap(), port.as_bytes());
    body.finish().unwrap();
}
