//! Actions-only runtime/IO lifecycle tests, pure duplex peers and rustls.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    controller::Callback, packet_stream::{encode_packet_stream_frame, PacketStreamDecoder},
    rpc_envelope::{decode_request, encode_reply}, rpc_stream::Writer,
    session::{HostingSession, STATUS_REGISTER}, tests::imported,
    tls_engine::tests::Certificates,
    transport_packets::{decode_tls_carrier, encode_call_packets, encode_tls_carrier, CallAssembler, CallKind},
};
use std::{sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, task::{Context, Waker}};
use tokio::{io::{AsyncWriteExt, ReadBuf}};
use tokio_rustls::rustls;

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("fixture-runtime-line".into()); session.import(imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn parameters<'a>(root: &'a str) -> TlsParameters<'a> {
    TlsParameters { authority: "fixture.invalid", root_der_base64: root, tls_only: true, enable_sni: true,
        handshake_timeout: Duration::from_secs(3), partial_timeout: Duration::from_millis(300) }
}
fn policy() -> RuntimePolicy { RuntimePolicy { io_timeout: Duration::from_secs(2), rpc_timeout: Duration::from_secs(2) } }
fn login() -> ArcLoginInputs<'static> {
    ArcLoginInputs { typed_identity: b"username:fixture", domain_name: b"fixture.invalid", selected_session_id: b"fixture-selector",
        phone_login_token: b"fixture-pwd", application_key: b"fixture-appkey", domain_id: b"fixture-domain", application_id: b"fixture-app",
        device_info_cc: b"", client_properties: &[] }
}
fn gateway() -> GatewayRegistration<'static> {
    GatewayRegistration { authname_prefix: "fixture-prefix", channel_id: "fixture-channel", delete_int_code: "1", sip_number: "fixture-number", sip_password: "fixture-SipPwd" }
}

struct PollIo { data: Arc<Mutex<Vec<u8>>>, limit: usize, pending: Arc<AtomicBool>, wake: Arc<Mutex<Option<Waker>>>, dropped: Arc<AtomicBool> }
impl Drop for PollIo { fn drop(&mut self) { self.dropped.store(true, Ordering::SeqCst); } }
impl AsyncWrite for PollIo {
    fn poll_write(self: Pin<&mut Self>, context: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if self.pending.load(Ordering::SeqCst) {
            *self.wake.lock().unwrap() = Some(context.waker().clone()); return Poll::Pending;
        }
        let count = self.limit.min(bytes.len()); self.data.lock().unwrap().extend_from_slice(&bytes[..count]);
        Poll::Ready(Ok(count))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { Poll::Ready(Ok(())) }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { Poll::Ready(Ok(())) }
}
impl AsyncRead for PollIo {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> { Poll::Pending }
}
fn poll_io(pending: bool) -> (PollIo, Arc<Mutex<Vec<u8>>>, Arc<AtomicBool>, Arc<Mutex<Option<Waker>>>, Arc<AtomicBool>) {
    let data = Arc::new(Mutex::new(Vec::new())); let pending = Arc::new(AtomicBool::new(pending)); let wake = Arc::new(Mutex::new(None)); let dropped = Arc::new(AtomicBool::new(false));
    (PollIo { data: data.clone(), limit: 3, pending: pending.clone(), wake: wake.clone(), dropped: dropped.clone() }, data, pending, wake, dropped)
}

#[tokio::test]
async fn write_poll_gate_preserves_offsets_and_checks_original_scope_at_every_submission() {
    let (mut io, bytes, _, _, _) = poll_io(false); let scope = RpcConnectionScope::fresh();
    write_frames(&mut io, &scope, &[b"abcdefg", b"hij"]).await.unwrap(); assert_eq!(&*bytes.lock().unwrap(), b"abcdefghij");
    scope.close(); assert!(write_frames(&mut io, &scope, &[b"late"]).await.is_err());
    assert_eq!(&*bytes.lock().unwrap(), b"abcdefghij");
}

#[tokio::test]
async fn close_between_pending_write_polls_blocks_future_write_even_with_prior_borrowed_bytes() {
    let (mut io, bytes, pending, wake, _) = poll_io(true); let scope = RpcConnectionScope::fresh();
    let input = b"fixture-private-wire"; let frames = [input.as_slice()];
    let writing = write_frames(&mut io, &scope, &frames); tokio::pin!(writing);
    assert!(tokio::time::timeout(Duration::from_millis(10), writing.as_mut()).await.is_err());
    scope.close(); pending.store(false, Ordering::SeqCst); if let Some(waker) = wake.lock().unwrap().take() { waker.wake(); }
    assert!(writing.await.is_err()); assert!(bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn close_gate_serializes_with_in_progress_nonblocking_poll_not_just_atomic_snapshot() {
    let scope = RpcConnectionScope::fresh(); let entered = Arc::new(std::sync::Barrier::new(2)); let release = Arc::new(std::sync::Barrier::new(2));
    let copied_scope = scope.clone(); let entered2 = entered.clone(); let release2 = release.clone();
    let polling = std::thread::spawn(move || copied_scope.with_io_gate(|| { entered2.wait(); release2.wait(); 7 }));
    entered.wait(); let closed = Arc::new(AtomicBool::new(false)); let closed2 = closed.clone(); let close_scope = scope.clone();
    let closing = std::thread::spawn(move || { close_scope.close(); closed2.store(true, Ordering::SeqCst); });
    std::thread::sleep(Duration::from_millis(5)); assert!(!closed.load(Ordering::SeqCst)); release.wait();
    assert_eq!(polling.join().unwrap(), Some(7)); closing.join().unwrap(); assert!(closed.load(Ordering::SeqCst));
    assert_eq!(scope.with_io_gate(|| 9), None);
}

#[tokio::test]
async fn closed_scope_notification_has_no_check_then_wait_lost_wakeup() {
    let scope = RpcConnectionScope::fresh(); let waiting = scope.clone();
    let task = tokio::spawn(async move { waiting.closed().await; });
    scope.close(); tokio::time::timeout(Duration::from_secs(1), task).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(1), scope.closed()).await.unwrap();
}

#[tokio::test]
async fn poison_of_nonblocking_poll_gate_is_terminal_not_a_recoverable_permission() {
    let scope = RpcConnectionScope::fresh(); let worker = scope.clone();
    assert!(std::thread::spawn(move || worker.with_io_gate(|| panic!("synthetic poll panic"))).join().is_err());
    assert_eq!(scope.with_io_gate(|| 1), None); assert!(!scope.is_open()); scope.close();
}

#[tokio::test]
async fn cancelled_initial_write_close_confirmed_after_actor_drops_original_io_and_tls() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (io, bytes, _, _, dropped) = poll_io(true);
    let (mut runtime, _events) = start(io, original, PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let result = tokio::time::timeout(Duration::from_secs(1), runtime.close()).await.unwrap();
    assert_eq!(result.reason, RuntimeError::Closed); assert!(result.io_released()); assert!(dropped.load(Ordering::SeqCst)); assert!(bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancelling_original_command_during_send_terminates_and_releases_actor_io() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let mut backend = OfflineBackend::new(original.clone());
    let scope = backend.connection_scope(); let now = Instant::now();
    let mut tls = backend.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters(&cert.root), now).unwrap();
    let wire = tls.take_wire_output(&original, &scope, None, now).unwrap();
    let (io, bytes, _, _, dropped) = poll_io(true);
    let (_commands, commands) = mpsc::channel(MAX_COMMANDS); let (events, _events) = mpsc::channel(MAX_EVENTS);
    let mut actor = Actor { io: Some(io), owner: original, scope: scope.clone(), backend, tls, commands, events,
        policy: policy(), timing: None, start: now, pending: std::collections::BTreeMap::new() };
    let (mut completion, receiver) = oneshot::channel();
    {
        let sending = actor.send_wire(&wire, Some(&mut completion)); tokio::pin!(sending);
        assert!(tokio::time::timeout(Duration::from_millis(10), sending.as_mut()).await.is_err());
        drop(receiver); assert_eq!(sending.await, Err(RuntimeError::Cancelled));
    }
    drop(actor); assert!(!scope.is_open()); assert!(dropped.load(Ordering::SeqCst)); assert!(bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn actor_drop_requests_close_and_abort_guard_never_leaves_original_scope_open() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (io, _, _, _, dropped) = poll_io(true);
    let (runtime, _events) = start(io, original, PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap();
    let scope = runtime.connection_scope(); drop(runtime);
    tokio::time::timeout(Duration::from_secs(1), async { while !dropped.load(Ordering::SeqCst) { tokio::task::yield_now().await; } }).await.unwrap();
    assert!(!scope.is_open());
}

#[tokio::test]
async fn timeout_during_physical_partial_write_poison_scope_and_preserve_already_sent_prefix_only() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (client, mut peer) = tokio::io::duplex(8);
    let mut p = policy(); p.io_timeout = Duration::from_millis(30);
    let (mut runtime, _events) = start(client, original, PacketRoute::DirectTlsCarrier, parameters(&cert.root), p, None).unwrap();
    tokio::time::sleep(Duration::from_millis(70)).await;
    let closed = runtime.close().await; assert_eq!(closed.reason, RuntimeError::Timeout); assert!(closed.io_released());
    let mut prefix = vec![]; peer.read_to_end(&mut prefix).await.unwrap(); assert_eq!(prefix.len(), 8);
}

#[tokio::test]
async fn wrong_owner_or_connection_has_zero_command_and_io_side_effects() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (io, _, _, _, _) = poll_io(true);
    let (mut runtime, _events) = start(io, original.clone(), PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap();
    let scope = runtime.connection_scope();
    assert_eq!(runtime.login(&owner(), &scope, login(), &[]).await.err(), Some(RuntimeError::Scope));
    assert_eq!(runtime.login(&original, &RpcConnectionScope::fresh(), login(), &[]).await.err(), Some(RuntimeError::Scope));
    assert_eq!(runtime.commands.capacity(), MAX_COMMANDS); runtime.close().await;
}

#[test]
fn runtime_import_bounds_and_declared_absent_transport_policy_fail_closed() {
    assert!(Login::capture(ArcLoginInputs { phone_login_token: &vec![b'x'; MAX_PARAMETER_BYTES + 1], ..login() }).is_err());
    assert!(Binding::capture(IncomingBinding { object_id: b"x", domain: b"d", receiver_kind: b"wild.*" }).is_err());
    assert!(Gateway::capture("uri", 99, gateway(), &[]).is_err());
    assert!(Property::capture(&[RpcProperty { key: b"a", value: b"1" }, RpcProperty { key: b"a", value: b"2" }]).is_err());
    assert!(!RuntimePolicy { io_timeout: Duration::ZERO, rpc_timeout: Duration::from_secs(1) }.valid());
}

#[tokio::test]
async fn bounded_command_queue_returns_backpressure_without_preparing_or_reencrypting_requests() {
    let (commands, _receiver) = mpsc::channel(MAX_COMMANDS); let original = owner(); let scope = RpcConnectionScope::fresh();
    let runtime = RuntimeHandle { owner: original, scope, commands, task: None };
    let mut receivers = vec![];
    for _ in 0..MAX_COMMANDS { let (completion, receiver) = oneshot::channel(); receivers.push(receiver);
        runtime.commands.try_send(Command { operation: Operation::Configure(1, None), completion }).unwrap_or_else(|_| panic!("queue capacity")); }
    let (completion, _) = oneshot::channel();
    assert!(matches!(runtime.commands.try_send(Command { operation: Operation::Configure(1, None), completion }), Err(mpsc::error::TrySendError::Full(_))));
    drop(receivers); drop(runtime);
}

struct Peer {
    io: tokio::io::DuplexStream, server: rustls::ServerConnection, network: PacketStreamDecoder,
    plaintext: super::super::tls_plaintext::PlaintextPacketDecoder, assembler: CallAssembler,
    received: std::collections::VecDeque<super::super::transport_packets::CallMessage>,
}
impl Peer {
    async fn receive(&mut self) -> Option<super::super::transport_packets::CallMessage> {
        let mut bytes = [0; 8192];
        loop {
            if let Some(message) = self.received.pop_front() { return Some(message); }
            let count = self.io.read(&mut bytes).await.unwrap(); if count == 0 { return None; }
            for packet in self.network.on_bytes(&bytes[..count]).unwrap() {
                let raw = decode_tls_carrier(packet.expose_to_transport()).unwrap(); let mut cursor = std::io::Cursor::new(raw);
                while cursor.position() < raw.len() as u64 { assert!(self.server.read_tls(&mut cursor).unwrap() > 0); self.server.process_new_packets().unwrap(); }
                let mut output = vec![]; while self.server.wants_write() { self.server.write_tls(&mut output).unwrap(); }
                if !output.is_empty() { self.send_cipher(&output).await; }
                let mut plaintext = [0; 16_384];
                loop {
                    match std::io::Read::read(&mut self.server.reader(), &mut plaintext) {
                        Ok(0) => break,
                        Ok(count) => {
                            for packet in self.plaintext.on_bytes(&plaintext[..count]).unwrap() {
                                if let Some(message) = self.assembler.on_packet(packet.expose_to_transport()).unwrap() { self.received.push_back(message); }
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => panic!("memory TLS reader {error:?}"),
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
        for packet in encode_call_packets(kind, envelope, super::super::transport_packets::TLS_FRAGMENT_THRESHOLD).unwrap() {
            std::io::Write::write_all(&mut self.server.writer(), packet.expose_to_transport()).unwrap();
        }
        let mut output = Vec::new(); while self.server.wants_write() { self.server.write_tls(&mut output).unwrap(); }
        self.send_cipher(&output).await;
    }
    async fn send_reply(&mut self, id: i32, stream: &[u8]) {
        let reply = encode_reply(id, 0, stream).unwrap();
        self.send_call(CallKind::Reply, reply.expose_to_transport()).await;
    }
}
fn login_result() -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(true).unwrap(); w.bytes(b"fixture-user").unwrap(); w.bytes(b"fixture-auth").unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); w.finish().expose_to_transport().to_vec()
}

#[tokio::test]
async fn real_duplex_batch_ack_is_not_hosting_success_and_original_message_completion_uses_original_ticket() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (client, server) = tokio::io::duplex(65_536);
    let (mut runtime, mut events) = start(client, original.clone(), PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap(); let scope = runtime.connection_scope();
    let mut peer = Peer { io: server, server: cert.server(&rustls::version::TLS13), network: PacketStreamDecoder::default(),
        plaintext: super::super::tls_plaintext::PlaintextPacketDecoder::default(), assembler: CallAssembler::default(), received: std::collections::VecDeque::new() };
    let received = tokio::spawn(async move {
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        peer.send_reply(request.call_id, &login_result()).await;
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut r = super::super::rpc_stream::Reader::versioned(request.arguments).unwrap(); r.bytes().unwrap(); r.bytes().unwrap();
        let payload = super::super::rpc_batch::read_payload(&mut r).unwrap(); assert_eq!(payload.requests.len(), 1); let sequence = *payload.requests.keys().next().unwrap();
        assert_eq!(payload.requests[&sequence].metadata.iter().find(|p| p.key == b"id").unwrap().value, b"#fixture-gateway");
        assert_eq!(payload.requests[&sequence].metadata.iter().find(|p| p.key == b"domain").unwrap().value, b"fixture-domain");
        let mut result = Writer::new(); result.boolean(true).unwrap(); for word in [9, 1, 1, 7, 1, 1] { result.u32(word).unwrap(); }
        result.u32(0).unwrap(); result.u32(1).unwrap(); result.i32(sequence).unwrap(); result.u32(0).unwrap(); result.bytes(&[0, 1, 0, 0, 0, 0]).unwrap();
        result.properties(&[]).unwrap(); result.bytes(b"").unwrap(); let result = result.finish(); peer.send_reply(request.call_id, result.expose_to_transport()).await;
        let mut args = Writer::versioned().unwrap(); args.bytes(b"fixture.kind").unwrap(); args.properties(&[]).unwrap(); args.bytes(&[0xff, 0, 3]).unwrap(); let args = args.finish();
        let request = super::super::rpc_envelope::encode_request(1, &[
            RpcProperty { key: b"id", value: b"#fixture-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" },
            RpcProperty { key: b"__magic", value: &[0, 0xff] }], super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(), args.expose_to_transport()).unwrap();
        peer.send_call(CallKind::Request, request.expose_to_transport()).await;
        let response = peer.receive().await.unwrap(); assert_eq!(response.kind, CallKind::Reply);
        let result = super::super::rpc_envelope::decode_reply(response.payload.expose_to_transport()).unwrap();
        assert_eq!(result.call_id, 1); assert_eq!(result.result_stream, &[0, 1, 0, 0, 0, 0]);
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    tokio::time::timeout(Duration::from_secs(2), async { loop {
        match runtime.login(&original, &scope, login(), &[]).await {
            Ok(_) => break,
            Err(RuntimeError::Tls(TlsError::NotReady)) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(error) => panic!("runtime login {error:?}"),
        }
    }}).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert!(matches!(&event.events_for(&original, &scope).unwrap()[0], OfflineEvent::Callback(c) if c.callback == (Callback::SdkLogin { success: true })));
    runtime.configure(&original, &scope, 7, Some(IncomingBinding { object_id: b"#fixture-client", domain: b"fixture-domain", receiver_kind: b"fixture.kind" })).await.unwrap();
    let receipt = runtime.hosting(&original, &scope, "fixture-uri", STATUS_REGISTER, gateway(), &[
        RpcProperty { key: b"id", value: b"#fixture-gateway" }, RpcProperty { key: b"domain", value: b"fixture-domain" }], &[]).await.unwrap(); assert!(receipt.matches(&original, &scope));
    let mut hosting = false; let mut ticket = None;
    while !hosting || ticket.is_none() {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
        for event in event.into_events(&original, &scope).unwrap() {
            match event {
                OfflineEvent::Callback(callback) => { assert_eq!(callback.callback, Callback::HostingAck { success: true }); hosting = true; }
                OfflineEvent::DirectMessage(message) => { assert_eq!(message.message().body, &[0xff, 0, 3]); let (_, _, reply) = message.into_parts(); ticket = Some(reply); }
                _ => panic!("unexpected runtime event"),
            }
        }
    }
    assert!(hosting); let ticket = ticket.unwrap(); runtime.complete_direct(&original, &scope, &ticket, true).await.unwrap();
    assert!(runtime.complete_direct(&original, &scope, &ticket, true).await.is_err());
    assert!(runtime.close().await.io_released()); received.await.unwrap();
}

#[tokio::test]
async fn real_duplex_tls_actor_login_keeps_transport_receipt_and_owned_sdk_callback_distinct() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (client, server) = tokio::io::duplex(65_536);
    let (mut runtime, mut events) = start(client, original.clone(), PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap(); let scope = runtime.connection_scope();
    let mut peer = Peer { io: server, server: cert.server(&rustls::version::TLS13), network: PacketStreamDecoder::default(),
        plaintext: super::super::tls_plaintext::PlaintextPacketDecoder::default(), assembler: CallAssembler::default(), received: std::collections::VecDeque::new() };
    let received = tokio::spawn(async move {
        let request = peer.receive().await.unwrap(); let request = decode_request(request.payload.expose_to_transport()).unwrap();
        assert_eq!(request.metadata.iter().find(|p| p.key == b"pwd").unwrap().value, b"fixture-pwd");
        peer.send_reply(request.call_id, &login_result()).await;
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    // Early command is rejected (not silently queued before trust). Retry is a
    // caller-controlled new command before any ARC login was submitted.
    let receipt = tokio::time::timeout(Duration::from_secs(2), async {
        loop { match runtime.login(&original, &scope, login(), &[]).await {
            Ok(receipt) => break receipt,
            Err(RuntimeError::Tls(TlsError::NotReady)) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(error) => panic!("login submission {error:?}"),
        } }
    }).await.unwrap();
    assert!(receipt.matches(&original, &scope));
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    let data = event.events_for(&original, &scope).unwrap(); assert!(matches!(&data[0], OfflineEvent::Callback(c) if c.owner == original && c.callback == (Callback::SdkLogin { success: true })));
    assert!(event.events_for(&owner(), &scope).is_none());
    runtime.close().await; received.await.unwrap(); assert!(!scope.is_open()); assert!(!receipt.matches(&original, &scope));
}
