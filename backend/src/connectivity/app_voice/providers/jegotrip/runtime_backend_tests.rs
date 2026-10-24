//! Actions-only: real in-memory PacketStream/FF06/rustls Common peers. No DNS,
//! TCP, device, native SDK, provider registration or paid-service simulation.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    controller::HostingController,
    credentials::ImportedCredentials,
    packet_stream::{encode_packet_stream_frame, PacketStreamDecoder},
    rpc_envelope::{decode_request, encode_reply},
    rpc_stream::{Reader, Writer},
    runtime_connector::{ConnectError, ConnectFuture, ConnectedIo},
    session::{HostingSession, Phase},
    tls_engine::tests::Certificates,
    tls_plaintext::PlaintextPacketDecoder,
    transport_packets::{decode_tls_carrier, encode_call_packets, encode_tls_carrier, CallAssembler, CallKind, CallMessage, TLS_FRAGMENT_THRESHOLD},
};
use std::{collections::VecDeque, io, pin::Pin, sync::atomic::{AtomicBool, AtomicUsize, Ordering}, task::{Context, Poll}};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf}, sync::oneshot};
use tokio_rustls::rustls;

fn credentials(root: &str) -> ImportedCredentials {
    serde_json::from_value(serde_json::json!({
        "sdk": { "account_identity": "fixture-account", "phone_login_token": "fixture-original-pwd",
            "application_key": "fixture-original-appkey", "server_address": "#fixture-account:tcp -h fixture.invalid -p 443;",
            "selected_session_id": "fixture-original-selector", "device_id": "fixture-original-device",
            "root_der_base64": root, "tls_only": true, "expires_at_unix_seconds": 200 },
        "hosting": { "sip_number": "fixture-number", "sip_password": "fixture-SipPwd", "authname_prefix": "fixture-prefix",
            "channel_id": "fixture-channel", "delete_int_code": "1", "enrollment_confirmed": true }
    })).unwrap()
}
fn owner(root: &str) -> AttemptOwner {
    let mut session = HostingSession::new("fixture-backend-line".into());
    session.import(credentials(root), 100).unwrap(); session.begin_login(100).unwrap()
}
fn request(root: &str) -> SdkLoginRequest<'_> {
    SdkLoginRequest { account_identity: "fixture-account", phone_login_token: "fixture-original-pwd",
        application_key: "fixture-original-appkey", server_address: "#fixture-account:tcp -h fixture.invalid -p 443;",
        device_id: Some("fixture-original-device"), selected_session_id: Some("fixture-original-selector"), root_der_base64: root, tls_only: true }
}
fn gateway() -> GatewayRegistration<'static> {
    GatewayRegistration { authname_prefix: "fixture-prefix", channel_id: "fixture-channel", delete_int_code: "1",
        sip_number: "fixture-number", sip_password: "fixture-SipPwd" }
}
fn inputs<'a>(client: &'a [RpcProperty<'a>]) -> RuntimeProfileInputs<'a> {
    RuntimeProfileInputs { route: PacketRoute::DirectTlsCarrier, authority: "fixture.invalid", enable_sni: true,
        identity_type: "username", domain_name: "fixture.invalid", domain_id: "fixture-domain", application_id: "fixture-app",
        device_info_cc: "fixture-cc", client_properties: client, include_device_id: true,
        login_routing: &[], gateway_uri: "fixture-exact-gateway-uri",
        gateway_metadata: &[RpcProperty { key: b"id", value: b"#fixture-gateway" }, RpcProperty { key: b"domain", value: b"fixture-domain" }],
        poll_routing: &[], logout_routing: &[],
        incoming: Some(super::super::rpc_incoming::IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"fixture.kind" }),
        reference_decode_key: None, handshake_timeout: Duration::from_secs(3), partial_timeout: Duration::from_millis(300),
        io_timeout: Duration::from_secs(2), rpc_timeout: Duration::from_secs(2) }
}
fn profile() -> RuntimeProfile {
    RuntimeProfile::supplied(inputs(&[RpcProperty { key: b"ClientId", value: b"fixture-exact-client-id" },
        RpcProperty { key: b"__appKey", value: b"fixture-ignored-appkey" }])).unwrap()
}
struct TrackedIo { io: DuplexStream, dropped: Arc<AtomicBool>, block_writes: Arc<AtomicBool> }
impl Drop for TrackedIo { fn drop(&mut self) { self.dropped.store(true, Ordering::SeqCst); } }
impl AsyncRead for TrackedIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for TrackedIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if self.block_writes.load(Ordering::SeqCst) { return Poll::Pending; }
        Pin::new(&mut self.io).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> { Pin::new(&mut self.io).poll_flush(cx) }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> { Pin::new(&mut self.io).poll_shutdown(cx) }
}
struct DuplexConnector {
    io: Mutex<Option<TrackedIo>>, calls: AtomicUsize, captured: Mutex<Option<(AttemptOwner, RpcConnectionScope)>>,
    substitute_scope: bool,
}
impl RuntimeConnector for DuplexConnector {
    fn connect<'a>(&'a self, owner: &'a AttemptOwner, scope: &'a RpcConnectionScope, input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *lock(&self.captured) = Some((owner.clone(), scope.clone()));
            let io = lock(&self.io).take().ok_or(ConnectError::Io)?;
            if self.substitute_scope { ConnectedIo::original(owner, &RpcConnectionScope::fresh(), input, io) }
            else { ConnectedIo::original(owner, scope, input, io) }
        })
    }
}
struct Peer {
    io: DuplexStream, server: rustls::ServerConnection, network: PacketStreamDecoder,
    plaintext: PlaintextPacketDecoder, assembler: CallAssembler, received: VecDeque<CallMessage>,
}
impl Peer {
    async fn receive(&mut self) -> Option<CallMessage> {
        let mut bytes = [0; 8192];
        loop {
            if let Some(message) = self.received.pop_front() { return Some(message); }
            let count = self.io.read(&mut bytes).await.unwrap(); if count == 0 { return None; }
            for packet in self.network.on_bytes(&bytes[..count]).unwrap() {
                let raw = decode_tls_carrier(packet.expose_to_transport()).unwrap(); let mut cursor = io::Cursor::new(raw);
                while cursor.position() < raw.len() as u64 {
                    assert!(self.server.read_tls(&mut cursor).unwrap() > 0); self.server.process_new_packets().unwrap();
                }
                let mut output = vec![]; while self.server.wants_write() { self.server.write_tls(&mut output).unwrap(); }
                if !output.is_empty() { self.send_cipher(&output).await; }
                let mut plaintext = [0; 16_384];
                loop {
                    match std::io::Read::read(&mut self.server.reader(), &mut plaintext) {
                        Ok(0) => break,
                        Ok(count) => for packet in self.plaintext.on_bytes(&plaintext[..count]).unwrap() {
                            if let Some(message) = self.assembler.on_packet(packet.expose_to_transport()).unwrap() { self.received.push_back(message); }
                        },
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => panic!("fixture memory TLS reader {error:?}"),
                    }
                }
            }
        }
    }
    async fn send_cipher(&mut self, cipher: &[u8]) {
        for chunk in cipher.chunks(1024) {
            let frame = encode_packet_stream_frame(encode_tls_carrier(chunk).unwrap().expose_to_transport()).unwrap();
            self.io.write_all(frame.expose_to_transport()).await.unwrap();
        }
    }
    async fn send_call(&mut self, kind: CallKind, envelope: &[u8]) {
        for packet in encode_call_packets(kind, envelope, TLS_FRAGMENT_THRESHOLD).unwrap() {
            std::io::Write::write_all(&mut self.server.writer(), packet.expose_to_transport()).unwrap();
        }
        let mut output = Vec::new(); while self.server.wants_write() { self.server.write_tls(&mut output).unwrap(); }
        self.send_cipher(&output).await;
    }
    async fn reply(&mut self, id: i32, stream: &[u8]) {
        let envelope = encode_reply(id, 0, stream).unwrap(); self.send_call(CallKind::Reply, envelope.expose_to_transport()).await;
    }
    async fn authenticate(&mut self) {
        let message = self.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
        assert_eq!(request.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, b"fixture-original-pwd");
        assert!(!request.metadata.iter().any(|p| p.key == b"token"));
        let mut args = Reader::versioned(request.arguments).unwrap();
        assert_eq!(args.bytes().unwrap(), b"[username:fixture-account@fixture.invalid]");
        assert_eq!(args.bytes().unwrap(), b"fixture-original-selector");
        let props = args.properties().unwrap(); args.finish().unwrap();
        for (key, expected) in [(b"ClientId".as_slice(), b"fixture-exact-client-id".as_slice()),
            (b"DeviceId", b"fixture-original-device"), (b"__appKey", b"fixture-original-appkey"),
            (b"__domain", b"fixture-domain"), (b"__app", b"fixture-app"), (b"DeviceInfo.CC", b"fixture-cc")] {
            assert_eq!(props.iter().find(|p| p.key == key).unwrap().value, expected);
        }
        let mut w = Writer::new(); w.boolean(true).unwrap(); w.bytes(b"fixture-user").unwrap(); w.bytes(b"fixture-auth").unwrap();
        w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); self.reply(request.call_id, w.finish().expose_to_transport()).await;
    }
    async fn swap(&mut self, status: Option<i32>, nested: bool) {
        loop {
            let message = self.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
            let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
            let payload = super::super::rpc_batch::read_payload(&mut args).unwrap();
            if status.is_some() && payload.requests.is_empty() {
                self.reply(request.call_id, &swap_result(None, true, payload.fields)).await; continue;
            }
            let sequence = payload.requests.keys().next().copied();
            if let Some(status) = status {
                assert_eq!(payload.requests.len(), 1); let call = &payload.requests[&sequence.unwrap()];
                assert_eq!(call.metadata.iter().find(|p| p.key == b"id").unwrap().value, b"#fixture-gateway");
                let mut fields = Reader::versioned(call.arguments).unwrap(); assert_eq!(fields.bytes().unwrap(), b"fixture-exact-gateway-uri");
                let properties = fields.properties().unwrap(); fields.finish().unwrap();
                assert_eq!(properties.iter().find(|p| p.key == b"SipFlag").unwrap().value, status.to_string().as_bytes());
                assert_eq!(properties.iter().find(|p| p.key == b"SipPwd").unwrap().value, b"fixture-SipPwd");
            }
            let result = swap_result(if nested { sequence } else { None }, true, payload.fields);
            self.reply(request.call_id, &result).await; return;
        }
    }
    async fn incoming(&mut self, id: i32) {
        let mut args = Writer::versioned().unwrap(); args.bytes(b"fixture.kind").unwrap(); args.properties(&[]).unwrap(); args.bytes(&[0xff, 0, 3]).unwrap();
        let request = super::super::rpc_envelope::encode_request(id, &[
            RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
            RpcProperty { key: b"__magic", value: &[0, 0xff] }], super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),
            args.finish().expose_to_transport()).unwrap();
        self.send_call(CallKind::Request, request.expose_to_transport()).await;
    }
}
fn swap_result(sequence: Option<i32>, accepted: bool, original_words: [u32; 6]) -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(true).unwrap();
    for word in [9, 1, 1, original_words[0], original_words[1], original_words[2]] { w.u32(word).unwrap(); }
    w.u32(0).unwrap(); w.u32(u32::from(sequence.is_some())).unwrap();
    if let Some(sequence) = sequence { w.i32(sequence).unwrap(); w.u32(0).unwrap(); w.bytes(&[0, u8::from(accepted), 0, 0, 0, 0]).unwrap(); }
    w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); w.finish().expose_to_transport().to_vec()
}
fn fixture(cert: &Certificates, substitute_scope: bool) -> (Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>, Arc<DuplexConnector>, Peer, Arc<AtomicBool>, Arc<AtomicBool>) {
    let (client, server) = tokio::io::duplex(65_536); let dropped = Arc::new(AtomicBool::new(false)); let blocked = Arc::new(AtomicBool::new(false));
    let connector = Arc::new(DuplexConnector { io: Mutex::new(Some(TrackedIo { io: client, dropped: dropped.clone(), block_writes: blocked.clone() })),
        calls: AtomicUsize::new(0), captured: Mutex::new(None), substitute_scope });
    let (backend, events) = NativeHostingBackend::new(profile(), connector.clone());
    let peer = Peer { io: server, server: cert.server(&rustls::version::TLS13), network: PacketStreamDecoder::default(),
        plaintext: PlaintextPacketDecoder::default(), assembler: CallAssembler::default(), received: VecDeque::new() };
    (backend, events, connector, peer, dropped, blocked)
}
async fn callback(events: &mut mpsc::Receiver<BackendEvent>) -> BackendEvent {
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert!(matches!(event.payload, BackendEventPayload::Callback(_))); event
}
async fn logged_in(backend: &NativeHostingBackend, events: &mut mpsc::Receiver<BackendEvent>, original: &AttemptOwner, root: &str) -> RpcConnectionScope {
    backend.login(original.clone(), request(root)).await.unwrap(); let event = callback(events).await;
    assert_eq!(event.owner(), original); let scope = event.connection_scope();
    assert!(event.matches(original, &scope));
    assert!(matches!(event.payload, BackendEventPayload::Callback(OwnedRpcCallback { callback: Callback::SdkLogin { success: true }, .. })));
    scope
}

async fn next_non_poll(peer: &mut Peer) -> CallMessage {
    loop {
        let message = peer.receive().await.unwrap();
        if message.kind == CallKind::Request {
            let request = decode_request(message.payload.expose_to_transport()).unwrap();
            if request.method == super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes() {
                let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
                let payload = super::super::rpc_batch::read_payload(&mut args).unwrap();
                if payload.requests.is_empty() && payload.results.is_empty() {
                    peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await; continue;
                }
            }
        }
        return message;
    }
}

#[test]
fn configured_device_identity_is_not_dropped_and_common_reference_enc_is_only_data() {
    let client = [RpcProperty { key: b"ClientId", value: b"fixture-exact-client-id" }];
    let mut supplied = inputs(&client); supplied.include_device_id = false;
    let unsupported = RuntimeProfile::supplied(supplied).unwrap();
    assert_eq!(LoginCapture::capture(request("fixture-public-root"), &unsupported).err(), Some(BackendFailure::Unavailable));
    let accepted = profile(); let mut imported = request("fixture-public-root"); imported.server_address = "ENC(fixture-original-reference)";
    let captured = LoginCapture::capture(imported, &accepted).unwrap();
    assert_eq!(captured.reference.as_str(), "ENC(fixture-original-reference)");
    assert_eq!(captured.password.as_str(), "fixture-original-pwd");
    assert_eq!(captured.selector.as_str(), "fixture-original-selector");
    assert!(!format!("{captured:?}").contains("fixture"));
}

#[tokio::test]
async fn unregister_outer_ack_wait_keeps_original_poll_live_until_later_nested_result() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move {
        peer.authenticate().await;
        let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
        let payload = super::super::rpc_batch::read_payload(&mut args).unwrap(); let sequence = *payload.requests.keys().next().unwrap();
        peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await;
        // Inner result is intentionally absent from the outer ACK. A later
        // ORIGINAL poll is required; serializing polls with status work would
        // deadlock this real native scheduler exchange until timeout.
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
        let payload = super::super::rpc_batch::read_payload(&mut args).unwrap(); assert!(payload.requests.is_empty());
        peer.reply(request.call_id, &swap_result(Some(sequence), true, payload.fields)).await;
        let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await;
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    tokio::time::timeout(Duration::from_secs(2), backend.update_status(original.clone(), STATUS_UNREGISTER, gateway())).await.unwrap().unwrap();
    assert!(scope.is_open());
    assert_eq!(backend.update_status(original.clone(), STATUS_REGISTER, gateway()).await, Err(BackendFailure::Disconnected));
    // Re-observation is the remembered result, not another unregister packet.
    backend.update_status(original.clone(), STATUS_UNREGISTER, gateway()).await.unwrap();
    backend.logout(original).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[test]
fn explicit_profile_rejects_missing_client_type_native_mode_and_auth_substitutes() {
    assert!(RuntimeProfile::supplied(inputs(&[])).is_err());
    assert!(RuntimeProfile::supplied(inputs(&[RpcProperty { key: b"ClientId", value: b"fixture" }, RpcProperty { key: b"DeviceId", value: b"substitute" }])).is_err());
    for key in [b"pwd".as_slice(), b"token", b"ForceFlag"] {
        assert!(RuntimeProfile::supplied(inputs(&[RpcProperty { key: b"ClientId", value: b"fixture" }, RpcProperty { key, value: b"1" }])).is_err());
    }
    let client = [RpcProperty { key: b"ClientId", value: b"fixture" }]; let mut input = inputs(&client); input.identity_type = "";
    assert!(RuntimeProfile::supplied(input).is_err());
    let mut input = inputs(&client); input.gateway_uri = ""; assert!(RuntimeProfile::supplied(input).is_err());
    let router_binding = || super::super::packet_transport::RouterBinding { packet_kind: 0,
        routing: super::super::router_packets::RouterRouting { tag: 0, flags: 0, path_words: &[],
            address1: super::super::router_packets::RouterAddress { mask: 0, label: 0, bytes: &[] },
            address2: super::super::router_packets::RouterAddress { mask: 0, label: 0, bytes: &[] } } };
    let mut input = inputs(&client); input.route = PacketRoute::Router { transmit: router_binding(), receive: router_binding(), mtu: 128, first_sequence: 1 };
    assert_eq!(RuntimeProfile::supplied(input).err(), Some(ProfileError::UnsupportedTransport));
    let mut input = inputs(&client); input.poll_routing = &[RpcProperty { key: b"token", value: b"substitute" }];
    assert!(RuntimeProfile::supplied(input).is_err());
    let profile = profile(); assert!(!format!("{profile:?}").contains("fixture"));
    let mut missing = request("fixture-public-root"); missing.selected_session_id = None;
    assert_eq!(LoginCapture::capture(missing, &profile).err(), Some(BackendFailure::Unavailable));
    let mut plain = request("fixture-public-root"); plain.tls_only = false;
    assert_eq!(LoginCapture::capture(plain, &profile).err(), Some(BackendFailure::Unavailable));
}

#[tokio::test]
async fn original_controller_login_nested_hosted_ack_unregistration_sdk_logout_join_no_channels() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move {
        peer.authenticate().await;
        peer.swap(Some(STATUS_REGISTER), true).await;
        peer.swap(Some(STATUS_UNREGISTER), true).await;
        let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        let mut arguments = Reader::versioned(request.arguments).unwrap(); assert_eq!(arguments.bytes().unwrap(), b"fixture-user");
        assert_eq!(arguments.bytes().unwrap(), b"fixture-original-selector"); arguments.finish().unwrap();
        assert!(request.metadata.iter().any(|p| p.key == b"token" && p.value == b"fixture-auth"));
        assert!(!request.metadata.iter().any(|p| p.key == b"pwd"));
        peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await;
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    let mut controller = HostingController::new("fixture-controller-line".into(), Some(backend.clone()));
    controller.import(credentials(&cert.root), 100).unwrap();
    assert!(!controller.incoming_call_ready()); assert!(!controller.incoming_message_ready()); assert!(!controller.outbound_authorized());
    let original = controller.start(100).await.unwrap(); assert_eq!(controller.phase(), Phase::SdkLoginPending);
    let event = callback(&mut events).await; assert_eq!(event.owner(), &original);
    let scope = event.connection_scope(); let captured = lock(&connector.captured); assert!(captured.as_ref().unwrap().1.matches(&scope)); drop(captured);
    let BackendEventPayload::Callback(callback) = event.payload else { unreachable!() };
    assert!(controller.on_callback(&callback.owner, callback.callback, 100).await.unwrap());
    let event = callback(&mut events).await;
    assert!(event.matches(&original, &scope));
    let BackendEventPayload::Callback(callback) = event.payload else { unreachable!() };
    assert_eq!(callback.callback, Callback::HostingAck { success: true });
    controller.on_callback(&callback.owner, callback.callback, 100).await.unwrap(); assert_eq!(controller.phase(), Phase::Hosted);
    assert!(!controller.incoming_call_ready()); assert!(!controller.incoming_message_ready()); assert!(!controller.outbound_authorized());
    controller.stop().await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert!(!scope.is_open());
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1); serving.await.unwrap();
}

#[tokio::test]
async fn wrong_owner_scope_and_stopped_slot_do_not_reimport_or_restart_before_actual_release() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move { peer.authenticate().await; while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {} });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    let wrong = owner(&cert.root);
    assert_eq!(backend.update_status(wrong.clone(), STATUS_REGISTER, gateway()).await, Err(BackendFailure::Disconnected));
    assert_eq!(backend.logout(wrong.clone()).await, Err(BackendFailure::Disconnected)); backend.retire(&wrong); assert!(scope.is_open());
    let attempt = backend.original(&original).unwrap();
    assert_eq!(attempt.commands(&original, &RpcConnectionScope::fresh()).err(), Some(RuntimeError::Scope)); assert!(scope.is_open());
    backend.retire(&original); assert!(!scope.is_open());
    assert_eq!(backend.login(wrong, request(&cert.root)).await, Err(BackendFailure::Unavailable));
    assert!(!lock(&attempt.state).released); backend.logout(original.clone()).await.unwrap();
    assert!(lock(&attempt.state).released); assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(backend.login(original, request(&cert.root)).await, Err(BackendFailure::Unavailable));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1); serving.await.unwrap();
}

#[tokio::test]
async fn connector_original_scope_mismatch_consumes_no_login_and_requires_cleanup_barrier() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, _events, connector, _peer, dropped, _) = fixture(&cert, true);
    let original = owner(&cert.root);
    assert_eq!(backend.login(original.clone(), request(&cert.root)).await, Err(BackendFailure::Disconnected));
    assert!(dropped.load(Ordering::SeqCst)); let attempt = backend.original(&original).unwrap(); assert!(!attempt.scope.is_open());
    assert_eq!(backend.login(owner(&cert.root), request(&cert.root)).await, Err(BackendFailure::Unavailable));
    backend.logout(original).await.unwrap(); assert!(lock(&attempt.state).released);
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
}

struct PendingConnector { scope: Mutex<Option<RpcConnectionScope>>, calls: AtomicUsize }
impl RuntimeConnector for PendingConnector {
    fn connect<'a>(&'a self, _owner: &'a AttemptOwner, scope: &'a RpcConnectionScope, _input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst); *lock(&self.scope) = Some(scope.clone());
            std::future::pending::<Result<ConnectedIo, ConnectError>>().await
        })
    }
}
#[tokio::test]
async fn cancelled_connect_retires_prepared_original_scope_no_reattempt_until_cleanup() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let connector = Arc::new(PendingConnector { scope: Mutex::new(None), calls: AtomicUsize::new(0) });
    let (backend, _events) = NativeHostingBackend::new(profile(), connector.clone()); let original = owner(&cert.root);
    assert!(tokio::time::timeout(Duration::from_millis(20), backend.login(original.clone(), request(&cert.root))).await.is_err());
    let scope = lock(&connector.scope).clone().unwrap(); assert!(!scope.is_open());
    assert_eq!(backend.login(owner(&cert.root), request(&cert.root)).await, Err(BackendFailure::Unavailable));
    backend.logout(original.clone()).await.unwrap(); backend.logout(original).await.unwrap();
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_nested_unregister_wait_closes_original_gate_outer_ack_is_not_success() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let (acked, ack_seen) = oneshot::channel();
    let serving = tokio::spawn(async move { peer.authenticate().await; peer.swap(Some(STATUS_UNREGISTER), false).await;
        acked.send(()).unwrap(); while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {} });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    {
        let wait = backend.update_status(original.clone(), STATUS_UNREGISTER, gateway()); tokio::pin!(wait);
        tokio::select! { result = wait.as_mut() => panic!("outer ACK is not native unregister: {result:?}"), _ = ack_seen => {} }
        assert!(tokio::time::timeout(Duration::from_millis(10), wait.as_mut()).await.is_err()); assert!(scope.is_open());
    }
    assert!(!scope.is_open()); backend.logout(original).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[tokio::test]
async fn semantic_unregister_rejection_releases_without_replaying_unregister_or_logout() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move {
        peer.authenticate().await; let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
        let payload = super::super::rpc_batch::read_payload(&mut args).unwrap(); let sequence = *payload.requests.keys().next().unwrap();
        peer.reply(request.call_id, &swap_result(Some(sequence), false, payload.fields)).await;
        // Failed unregister retires IO, not an invented successful SDK logout.
        assert!(peer.receive().await.is_none());
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    assert_eq!(backend.update_status(original.clone(), STATUS_UNREGISTER, gateway()).await, Err(BackendFailure::HostingRejected));
    assert!(!scope.is_open()); backend.logout(original.clone()).await.unwrap(); assert!(dropped.load(Ordering::SeqCst));
    backend.logout(original).await.unwrap(); serving.await.unwrap();
}

#[tokio::test]
async fn sdk_logout_false_is_semantic_failure_but_retry_confirms_remembered_join_release() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move { peer.authenticate().await; let message = next_non_poll(&mut peer).await;
        let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes()); peer.reply(request.call_id, &[0, 0, 0, 0, 0, 0]).await;
        assert!(peer.receive().await.is_none()); });
    let original = owner(&cert.root); logged_in(&backend, &mut events, &original, &cert.root).await;
    assert_eq!(backend.logout(original.clone()).await, Err(BackendFailure::Disconnected)); assert!(dropped.load(Ordering::SeqCst));
    assert!(lock(&backend.original(&original).unwrap().state).released); backend.logout(original).await.unwrap(); serving.await.unwrap();
}

#[tokio::test]
async fn direct_durable_consumer_ticket_has_no_premature_ack_and_retains_original_scope() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let (no_ack, seen_no_ack) = oneshot::channel(); let (begin, begun) = oneshot::channel();
    let serving = tokio::spawn(async move { peer.authenticate().await; begun.await.unwrap(); peer.incoming(1).await;
        assert!(tokio::time::timeout(Duration::from_millis(40), peer.receive()).await.is_err()); no_ack.send(()).unwrap();
        loop {
            let message = peer.receive().await.unwrap();
            if message.kind == CallKind::Reply {
                let reply = super::super::rpc_envelope::decode_reply(message.payload.expose_to_transport()).unwrap();
                assert_eq!(reply.call_id, 1); assert_eq!(reply.result_stream, &[0, 1, 0, 0, 0, 0]); break;
            }
            let request = decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
            let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
            let payload = super::super::rpc_batch::read_payload(&mut args).unwrap();
            peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await;
        }
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    begin.send(()).unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap(); assert!(event.matches(&original, &scope));
    let BackendEventPayload::DirectMessage(message) = event.payload else { panic!("expected original direct delivery") };
    seen_no_ack.await.unwrap();
    assert_eq!(backend.complete_direct(&owner(&cert.root), &scope, message.reply_ticket(), true).await.err(), Some(RuntimeError::Scope));
    assert_eq!(backend.complete_direct(&original, &RpcConnectionScope::fresh(), message.reply_ticket(), true).await.err(), Some(RuntimeError::Scope));
    assert!(message.reply_ticket().is_open());
    assert!(backend.complete_direct(&original, &scope, message.reply_ticket(), true).await.unwrap().matches(&original, &scope));
    backend.retire(&original); backend.logout(original).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[tokio::test]
async fn original_batch_ticket_defers_nested_result_until_durable_consumer_completion() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let (no_result, checked) = oneshot::channel(); let (completed, completion_seen) = oneshot::channel();
    let serving = tokio::spawn(async move {
        peer.authenticate().await;
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
        let payload = super::super::rpc_batch::read_payload(&mut args).unwrap(); assert!(payload.requests.is_empty());
        let mut incoming = Writer::versioned().unwrap(); incoming.bytes(b"fixture.kind").unwrap(); incoming.properties(&[]).unwrap(); incoming.bytes(&[0xff, 0, 1]).unwrap();
        let mut response = Writer::new(); response.boolean(true).unwrap();
        for word in [9, 1, 1, payload.fields[0], payload.fields[1], payload.fields[2]] { response.u32(word).unwrap(); }
        response.u32(1).unwrap(); response.i32(1).unwrap(); response.bytes(super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes()).unwrap();
        response.bytes(incoming.finish().expose_to_transport()).unwrap();
        response.properties(&[RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
            RpcProperty { key: b"__magic", value: &[0, 0xff] }]).unwrap();
        response.u32(0).unwrap(); response.properties(&[]).unwrap(); response.bytes(b"").unwrap();
        peer.reply(request.call_id, response.finish().expose_to_transport()).await;
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
        let payload = super::super::rpc_batch::read_payload(&mut args).unwrap(); assert!(!payload.results.contains_key(&1));
        peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await; no_result.send(()).unwrap();
        loop {
            let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
            let mut args = Reader::versioned(request.arguments).unwrap(); args.bytes().unwrap(); args.bytes().unwrap();
            let payload = super::super::rpc_batch::read_payload(&mut args).unwrap();
            let completed = payload.results.get(&1);
            if let Some(result) = completed { assert_eq!(result.result_stream, &[0, 1, 0, 0, 0, 0]); }
            peer.reply(request.call_id, &swap_result(None, true, payload.fields)).await;
            if completed.is_some() { break; }
        }
        completed.send(()).unwrap();
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap(); assert!(event.matches(&original, &scope));
    let BackendEventPayload::BatchMessage(message) = event.payload else { panic!("original scheduled consumer ticket") };
    assert_eq!(message.message().body, &[0xff, 0, 1]); checked.await.unwrap(); assert!(message.ticket().is_open());
    assert_eq!(backend.complete_batch(&owner(&cert.root), &scope, message.ticket(), true).await.err(), Some(RuntimeError::Scope));
    assert!(backend.complete_batch(&original, &scope, message.ticket(), true).await.unwrap().matches(&original, &scope));
    // Wait until the ORIGINAL scheduler transmits the completion, not an ACK
    // invented by the event pump. Server will close only after observing it.
    tokio::time::timeout(Duration::from_secs(2), completion_seen).await.unwrap().unwrap();
    backend.retire(&original); backend.logout(original).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[tokio::test]
async fn periodic_poll_outer_ack_never_extends_original_pending_deadline_or_grants_hosting() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move { peer.authenticate().await;
        // Periodic polling is real transport, but unanswered RPC remains bounded.
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    let event = tokio::time::timeout(Duration::from_secs(4), events.recv()).await.unwrap().unwrap();
    assert!(event.matches(&original, &scope)); assert!(matches!(event.payload, BackendEventPayload::Terminal(BackendFailure::Disconnected)));
    assert!(!scope.is_open()); backend.logout(original).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[tokio::test]
async fn cancelled_close_wait_retains_same_owning_join_and_blocks_restart_until_release() {
    use std::future::Future;
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move { peer.authenticate().await; while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {} });
    let original = owner(&cert.root); logged_in(&backend, &mut events, &original, &cert.root).await;
    let attempt = backend.original(&original).unwrap(); backend.retire(&original);
    {
        let cleanup = backend.logout(original.clone()); tokio::pin!(cleanup);
        // Current-thread Actions executor: close requests retirement and polls
        // original Join once, without running that actor to completion yet.
        std::future::poll_fn(|cx| { assert!(cleanup.as_mut().poll(cx).is_pending()); Poll::Ready(()) }).await;
    }
    assert!(attempt.handle.try_lock().unwrap().is_some()); assert!(!lock(&attempt.state).released);
    assert_eq!(backend.login(owner(&cert.root), request(&cert.root)).await, Err(BackendFailure::Unavailable));
    backend.logout(original.clone()).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); assert!(lock(&attempt.state).released);
    backend.logout(original).await.unwrap(); assert_eq!(connector.calls.load(Ordering::SeqCst), 1); serving.await.unwrap();
}

#[tokio::test]
async fn original_direct_ticket_survives_command_queue_pressure_without_automatic_ack() {
    use futures_util::{stream::FuturesUnordered, StreamExt};
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, blocked) = fixture(&cert, false);
    let (begin, begun) = oneshot::channel();
    let serving = tokio::spawn(async move { peer.authenticate().await; begun.await.unwrap(); peer.incoming(1).await;
        assert!(peer.receive().await.is_none()); });
    let original = owner(&cert.root); let scope = logged_in(&backend, &mut events, &original, &cert.root).await;
    begin.send(()).unwrap(); let event = events.recv().await.unwrap();
    let BackendEventPayload::DirectMessage(message) = event.payload else { panic!("original direct ticket") };
    blocked.store(true, Ordering::SeqCst);
    let mut pending = FuturesUnordered::new();
    for _ in 0..64 { pending.push(backend.complete_direct(&original, &scope, message.reply_ticket(), true)); }
    let result = tokio::time::timeout(Duration::from_millis(100), pending.next()).await.unwrap().unwrap();
    assert_eq!(result.err(), Some(RuntimeError::Backpressure)); assert!(message.reply_ticket().is_open());
    assert!(scope.is_open()); // Full admission neither consumes caller ticket nor closes the gate.
    backend.retire(&original); drop(pending); backend.logout(original).await.unwrap();
    assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[tokio::test]
async fn actual_negative_sdk_reply_never_configures_scheduler_or_synthesizes_login_success() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, mut events, _connector, mut peer, dropped, _) = fixture(&cert, false);
    let serving = tokio::spawn(async move {
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut w = Writer::new(); w.boolean(false).unwrap(); w.bytes(b"").unwrap(); w.bytes(b"").unwrap();
        w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); peer.reply(request.call_id, w.finish().expose_to_transport()).await;
        assert!(peer.receive().await.is_none());
    });
    let original = owner(&cert.root); let result = backend.login(original.clone(), request(&cert.root)).await;
    assert!(result.is_ok() || result == Err(BackendFailure::Disconnected));
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert_eq!(event.owner(), &original);
    // Original actor-produced rejection survives ARC scope close; it is not
    // reinterpreted as a transport failure or a successful login callback.
    assert!(matches!(event.payload, BackendEventPayload::Terminal(BackendFailure::AuthenticationRejected)));
    assert!(!lock(&backend.original(&original).unwrap().state).configured);
    backend.logout(original).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[test]
fn bounded_backend_queue_reserves_terminal_slot_and_pressure_retires_original_attempt() {
    let original = owner("fixture-public-root"); let scope = RpcConnectionScope::fresh();
    let attempt = Attempt { owner: original.clone(), scope: scope.clone(), incarnation: 1,
        state: Mutex::new(AttemptState::default()), commands: Mutex::new(None), handle: AsyncMutex::new(None),
        work: AsyncMutex::new(()), poll_work: AsyncMutex::new(()) };
    let (sender, mut receiver) = mpsc::channel(EVENT_CAPACITY);
    for _ in 0..EVENT_CAPACITY - 1 {
        assert!(emit(&sender, &attempt, BackendEventPayload::Callback(OwnedRpcCallback { owner: original.clone(), callback: Callback::Failure(BackendFailure::Disconnected) })));
    }
    assert!(!emit(&sender, &attempt, BackendEventPayload::Callback(OwnedRpcCallback { owner: original.clone(), callback: Callback::Failure(BackendFailure::Disconnected) })));
    terminal(&sender, &attempt, BackendFailure::Disconnected); assert!(!scope.is_open());
    for _ in 0..EVENT_CAPACITY - 1 { assert!(matches!(receiver.try_recv().unwrap().payload, BackendEventPayload::Callback(_))); }
    let terminal = receiver.try_recv().unwrap(); assert!(terminal.matches(&original, &scope));
    assert!(matches!(terminal.payload, BackendEventPayload::Terminal(BackendFailure::Disconnected)));
}

#[tokio::test]
async fn drop_backend_retires_original_attempt_without_granting_channel_or_releasing_join_receipt() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let (backend, _events, connector, _peer, dropped, blocked) = fixture(&cert, false);
    blocked.store(true, Ordering::SeqCst); let original = owner(&cert.root);
    assert!(tokio::time::timeout(Duration::from_millis(20), backend.login(original.clone(), request(&cert.root))).await.is_err());
    let attempt = backend.original(&original).unwrap(); drop(backend); assert!(!attempt.scope.is_open());
    assert!(!lock(&attempt.state).released); let scope = lock(&connector.captured).as_ref().unwrap().1.clone(); assert!(!scope.is_open());
    tokio::time::timeout(Duration::from_secs(1), async { while !dropped.load(Ordering::SeqCst) { tokio::task::yield_now().await; } }).await.unwrap();
}
