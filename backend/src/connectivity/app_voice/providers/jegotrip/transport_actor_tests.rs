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

#[test]
fn closed_actor_events_retain_only_original_negative_callback_reason_not_ready() {
    use super::super::{rpc_pending::OwnedRpcCallback, session::BackendFailure};
    let original = owner();
    for (callback, expected) in [(Callback::SdkLogin { success: false }, BackendFailure::AuthenticationRejected),
        (Callback::HostingAck { success: false }, BackendFailure::HostingRejected),
        (Callback::Failure(BackendFailure::Expired), BackendFailure::Expired)] {
        let scope = RpcConnectionScope::fresh();
        let event = RuntimeEvent { owner: original.clone(), scope: scope.clone(),
            events: vec![OfflineEvent::Callback(OwnedRpcCallback { owner: original.clone(), callback })] };
        scope.close(); assert!(event.events_for(&original, &scope).is_none());
        assert_eq!(event.terminal_failure(&original, &scope), Some(expected));
        assert_eq!(event.terminal_failure(&owner(), &scope), None);
        assert_eq!(event.terminal_failure(&original, &RpcConnectionScope::fresh()), None);
        assert_eq!(event.into_terminal_callback(&original, &scope).unwrap().callback, callback);
    }
}

#[test]
fn terminal_actor_extraction_never_reopens_positive_acks_channels_or_other_owners() {
    use super::super::rpc_pending::OwnedRpcCallback;
    let original = owner(); let scope = RpcConnectionScope::fresh(); scope.close();
    for callback in [Callback::SdkLogin { success: true }, Callback::HostingAck { success: true },
        Callback::Channels { calls: true, messages: true }] {
        let event = RuntimeEvent { owner: original.clone(), scope: scope.clone(),
            events: vec![OfflineEvent::Callback(OwnedRpcCallback { owner: original.clone(), callback })] };
        assert_eq!(event.terminal_failure(&original, &scope), None);
        assert!(event.into_terminal_callback(&original, &scope).is_none());
    }
    let event = RuntimeEvent { owner: original.clone(), scope: scope.clone(), events: vec![OfflineEvent::Callback(
        OwnedRpcCallback { owner: owner(), callback: Callback::SdkLogin { success: false } })] };
    assert_eq!(event.terminal_failure(&original, &scope), None); assert!(event.into_terminal_callback(&original, &scope).is_none());
}

#[test]
fn historical_submission_correlation_after_close_is_not_an_open_io_or_ready_receipt() {
    let original = owner(); let scope = RpcConnectionScope::fresh();
    let receipt = RuntimeReceipt { owner: original.clone(), scope: scope.clone(), call_id: Some(1) };
    assert!(receipt.matches(&original, &scope)); scope.close();
    assert!(!receipt.matches(&original, &scope)); assert!(receipt.submitted_on(&original, &scope));
    assert!(!receipt.submitted_on(&owner(), &scope)); assert!(!receipt.submitted_on(&original, &RpcConnectionScope::fresh()));
}

#[test]
fn cancelled_prepared_runtime_retires_original_scope_before_any_io_exists() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner();
    let prepared = prepare(original, PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap();
    let scope = prepared.connection_scope(); assert!(scope.is_open());
    drop(prepared); assert!(!scope.is_open());
}

#[tokio::test]
async fn attaching_io_preserves_prepared_original_scope_and_does_not_close_it_on_seed_drop() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner();
    let prepared = prepare(original, PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap();
    let scope = prepared.connection_scope(); let (io, bytes, _, _, dropped) = poll_io(true);
    let (mut runtime, _events) = prepared.attach(io).unwrap();
    assert!(runtime.connection_scope().matches(&scope)); assert!(scope.is_open());
    let receipt = runtime.close().await; assert!(receipt.io_released()); assert!(!scope.is_open());
    assert!(dropped.load(Ordering::SeqCst)); assert!(bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn connecting_time_is_not_removed_from_prepared_tls_deadline_at_attach() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let mut tls = parameters(&cert.root);
    tls.handshake_timeout = Duration::from_millis(20);
    let prepared = prepare(original, PacketRoute::DirectTlsCarrier, tls, policy(), None).unwrap();
    let scope = prepared.connection_scope(); let (io, bytes, _, _, dropped) = poll_io(false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(prepared.attach(io).err(), Some(RuntimeError::Tls(TlsError::Timeout)));
    assert!(!scope.is_open()); assert!(dropped.load(Ordering::SeqCst)); assert!(bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retired_prepared_scope_refuses_attach_and_drops_supplied_io_without_writes() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner();
    let prepared = prepare(original, PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap();
    let scope = prepared.connection_scope(); scope.close(); let (io, bytes, _, _, dropped) = poll_io(false);
    assert_eq!(prepared.attach(io).err(), Some(RuntimeError::Scope));
    assert!(dropped.load(Ordering::SeqCst)); assert!(bytes.lock().unwrap().is_empty());
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
        policy: policy(), timing: None, start: now, pending: std::collections::BTreeMap::new(), logout_pending: None, unhosting_pending: None };
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

async fn commands_logged_in(commands:&RuntimeCommands,original:&AttemptOwner,scope:&RpcConnectionScope,events:&mut mpsc::Receiver<RuntimeEvent>) {
    tokio::time::timeout(Duration::from_secs(2),async {loop {match commands.login(original,scope,login(),&[]).await {
        Ok(_)=>break,Err(RuntimeError::Tls(TlsError::NotReady))=>tokio::time::sleep(Duration::from_millis(5)).await,Err(e)=>panic!("login {e:?}"),
    }}}).await.unwrap();let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();
    assert!(matches!(&event.events_for(original,scope).unwrap()[0],OfflineEvent::Callback(c) if c.callback==(Callback::SdkLogin{success:true})));
    commands.configure(original,scope,7,None).await.unwrap();
}
fn unhosting_swap_result(sequence:Option<i32>,accepted:bool)->super::super::rpc_stream::RpcBytes {
    let mut writer=Writer::new();writer.boolean(true).unwrap();for word in [9,1,1,7,1,1]{writer.u32(word).unwrap();}
    writer.u32(0).unwrap();writer.u32(u32::from(sequence.is_some())).unwrap();
    if let Some(sequence)=sequence {writer.i32(sequence).unwrap();writer.u32(0).unwrap();writer.bytes(&[0,u8::from(accepted),0,0,0,0]).unwrap();}
    writer.properties(&[]).unwrap();writer.bytes(b"").unwrap();writer.finish()
}

#[tokio::test]
async fn native_unregistration_waits_for_original_nested_bool_then_keeps_io_for_sdk_logout() {
    for accepted in [true,false] {
        let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (client,server)=tokio::io::duplex(65_536);
        let (mut runtime,mut events)=start(client,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),policy(),None).unwrap();let scope=runtime.connection_scope();
        let mut peer=Peer{io:server,server:cert.server(&rustls::version::TLS13),network:PacketStreamDecoder::default(),plaintext:super::super::tls_plaintext::PlaintextPacketDecoder::default(),assembler:CallAssembler::default(),received:std::collections::VecDeque::new()};
        let server=tokio::spawn(async move {
            let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.send_reply(request.call_id,&login_result()).await;
            let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();assert_eq!(request.method,super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
            let mut arguments=super::super::rpc_stream::Reader::versioned(request.arguments).unwrap();arguments.bytes().unwrap();arguments.bytes().unwrap();let payload=super::super::rpc_batch::read_payload(&mut arguments).unwrap();
            assert_eq!(payload.requests.len(),1);let sequence=*payload.requests.keys().next().unwrap();let mut method=super::super::rpc_stream::Reader::versioned(payload.requests[&sequence].arguments).unwrap();
            assert_eq!(method.bytes().unwrap(),b"fixture-uri");assert_eq!(method.properties().unwrap().iter().find(|p|p.key==b"SipFlag").unwrap().value,b"3");method.finish().unwrap();
            let result=unhosting_swap_result(Some(sequence),accepted);peer.send_reply(request.call_id,result.expose_to_transport()).await;
            if accepted {let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();assert_eq!(request.method,super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());peer.send_reply(request.call_id,b"\0\x01\0\0\0\0").await;}
            while peer.io.read(&mut[0;1024]).await.unwrap()!=0 {}
        });
        let commands=runtime.command_handle();commands_logged_in(&commands,&original,&scope,&mut events).await;
        let result=tokio::time::timeout(Duration::from_secs(2),runtime.hosting(&original,&scope,"fixture-uri",super::super::session::STATUS_UNREGISTER,gateway(),&[],&[])).await.unwrap();
        if accepted {assert!(result.unwrap().matches(&original,&scope));assert!(scope.is_open());assert!(runtime.logout(&original,&scope,&[]).await.unwrap().io_released());}
        else {assert!(result.is_err());assert!(!scope.is_open());assert!(runtime.close().await.io_released());}
        assert!(!scope.is_open());server.await.unwrap();
    }
}

#[tokio::test]
async fn outer_swap_ack_without_unregistration_result_is_not_native_success_and_cancel_retires_io() {
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (client,server)=tokio::io::duplex(65_536);
    let (mut runtime,mut events)=start(client,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),policy(),None).unwrap();let scope=runtime.connection_scope();
    let mut peer=Peer{io:server,server:cert.server(&rustls::version::TLS13),network:PacketStreamDecoder::default(),plaintext:super::super::tls_plaintext::PlaintextPacketDecoder::default(),assembler:CallAssembler::default(),received:std::collections::VecDeque::new()};
    let (acked,ack_seen)=oneshot::channel();let server=tokio::spawn(async move {
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.send_reply(request.call_id,&login_result()).await;
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();let result=unhosting_swap_result(None,true);peer.send_reply(request.call_id,result.expose_to_transport()).await;acked.send(()).unwrap();
        while peer.io.read(&mut[0;1024]).await.unwrap()!=0 {}
    });
    let commands=runtime.command_handle();commands_logged_in(&commands,&original,&scope,&mut events).await;
    {
        let unhosting=commands.hosting(&original,&scope,"fixture-uri",super::super::session::STATUS_UNREGISTER,gateway(),&[],&[]);tokio::pin!(unhosting);
        tokio::select!{result=unhosting.as_mut()=>panic!("outer ACK isn't gateway result {result:?}"),_ = ack_seen=>{}}
        assert!(tokio::time::timeout(Duration::from_millis(10),unhosting.as_mut()).await.is_err());assert!(scope.is_open());
    }
    assert!(!scope.is_open());assert!(runtime.close().await.io_released());server.await.unwrap();
}

#[tokio::test]
async fn unregistration_inner_deadline_survives_retirement_of_acknowledged_outer_rpc() {
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (client,server)=tokio::io::duplex(65_536);let mut p=policy();p.rpc_timeout=Duration::from_millis(80);
    let (mut runtime,mut events)=start(client,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),p,None).unwrap();let scope=runtime.connection_scope();
    let mut peer=Peer{io:server,server:cert.server(&rustls::version::TLS13),network:PacketStreamDecoder::default(),plaintext:super::super::tls_plaintext::PlaintextPacketDecoder::default(),assembler:CallAssembler::default(),received:std::collections::VecDeque::new()};
    let server=tokio::spawn(async move {
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.send_reply(request.call_id,&login_result()).await;
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();let result=unhosting_swap_result(None,true);peer.send_reply(request.call_id,result.expose_to_transport()).await;
        while peer.io.read(&mut[0;1024]).await.unwrap()!=0 {}
    });
    let commands=runtime.command_handle();commands_logged_in(&commands,&original,&scope,&mut events).await;
    assert!(tokio::time::timeout(Duration::from_secs(1),commands.hosting(&original,&scope,"fixture-uri",super::super::session::STATUS_UNREGISTER,gateway(),&[],&[])).await.unwrap().is_err());
    let close=runtime.close().await;assert!(close.io_released());assert_eq!(close.reason,RuntimeError::Timeout);server.await.unwrap();
}

#[tokio::test]
async fn invalid_unregistration_aggregate_has_no_queue_or_scope_side_effects() {
    let original = owner(); let scope = RpcConnectionScope::fresh();
    let (commands, mut receiver) = mpsc::channel(MAX_COMMANDS);
    let runtime = RuntimeCommands { owner: original.clone(), scope: scope.clone(), commands };
    let field = "x".repeat(16_384);
    let registration = || GatewayRegistration { authname_prefix: &field, channel_id: &field,
        delete_int_code: &field, sip_number: &field, sip_password: &field };
    let routing_value = vec![b'y'; 60 * 1024];
    let routing = [RpcProperty { key: b"fixture-route", value: &routing_value }];
    // Each capture separately fits; only the combined operation is oversized.
    assert!(Gateway::capture(&field, super::super::session::STATUS_UNREGISTER, registration(), &[]).is_ok());
    assert!(Property::capture(&routing).is_ok());
    assert_eq!(runtime.hosting(&original, &scope, &field, super::super::session::STATUS_UNREGISTER,
        registration(), &[], &routing).await.err(), Some(RuntimeError::InvalidConfiguration));
    assert!(scope.is_open()); assert_eq!(runtime.commands.capacity(), MAX_COMMANDS);
    assert!(matches!(receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn unregistration_and_logout_queue_rejection_do_not_arm_cancellation_guards() {
    let original = owner(); let scope = RpcConnectionScope::fresh();
    let (commands, _receiver) = mpsc::channel(MAX_COMMANDS);
    let runtime = RuntimeCommands { owner: original.clone(), scope: scope.clone(), commands };
    let mut waiting = Vec::new();
    for _ in 0..MAX_COMMANDS {
        let (completion, result) = oneshot::channel(); waiting.push(result);
        runtime.commands.try_send(Command { operation: Operation::Configure(1, None), completion })
            .unwrap_or_else(|_| panic!("fixture queue capacity"));
    }
    assert_eq!(runtime.hosting(&original, &scope, "fixture-uri", super::super::session::STATUS_UNREGISTER,
        gateway(), &[], &[]).await.err(), Some(RuntimeError::Backpressure));
    assert!(scope.is_open());
    assert_eq!(runtime.logout(&original, &scope, &[]).await.err(), Some(RuntimeError::Backpressure));
    assert!(scope.is_open()); assert_eq!(runtime.commands.capacity(), 0);
    drop(waiting);
}

fn unhosting_actor_fixture(root: &str, original: AttemptOwner, blocked: bool)
    -> (Actor<PollIo>, Vec<OwnedWireFrames>, Arc<Mutex<Vec<u8>>>, mpsc::Receiver<RuntimeEvent>) {
    let mut backend = OfflineBackend::new(original.clone()); let scope = backend.connection_scope(); let now = Instant::now();
    let mut tls = backend.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters(root), now).unwrap();
    let wire = tls.take_wire_output(&original, &scope, None, now).unwrap();
    let (io, bytes, _, _, _) = poll_io(blocked);
    let (_commands, commands) = mpsc::channel(MAX_COMMANDS); let (events, receive_events) = mpsc::channel(MAX_EVENTS);
    let actor = Actor { io: Some(io), owner: original, scope, backend, tls, commands, events,
        policy: policy(), timing: None, start: now, pending: std::collections::BTreeMap::new(),
        logout_pending: None, unhosting_pending: None };
    (actor, wire, bytes, receive_events)
}

fn original_unhosting_completion(actor: &Actor<PollIo>) -> (i32, super::super::rpc_pending::OwnedUnhostingReply) {
    let mut tracker = super::super::rpc_pending::HostingRpcTracker::for_arc_session(actor.owner.clone(), actor.scope.clone());
    let (sequence, _) = tracker.prepare_update_status(actor.owner.clone(), "fixture-uri", super::super::session::STATUS_UNREGISTER,
        &gateway(), &[]).unwrap();
    let envelope = encode_reply(sequence, 0, b"\0\x01\0\0\0\0").unwrap();
    assert!(tracker.on_reply(&actor.scope, envelope.expose_to_transport()).unwrap().is_none());
    (sequence, tracker.take_unhosting_replies().pop().unwrap())
}

#[tokio::test]
async fn original_unregistration_deadline_bounds_all_writes_after_outer_rpc_ack() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for blocked in [true, false] {
        let (mut actor, wire, bytes, _events) = unhosting_actor_fixture(&cert.root, owner(), blocked);
        let (completion, _result) = oneshot::channel(); let scope = actor.scope.clone();
        let deadline = if blocked { Instant::now() + Duration::from_millis(40) }
            else { Instant::now() - Duration::from_secs(1) };
        // An outer ACK already retired all ARC ids; its inner result is still pending.
        assert!(actor.pending.is_empty());
        actor.unhosting_pending = Some(UnhostingPending { sequence: 1, outer_id: 2, deadline, completion });
        assert_eq!(tokio::time::timeout(Duration::from_millis(500), actor.send_wire(&wire, None)).await.unwrap(),
            Err(RuntimeError::Timeout));
        assert!(bytes.lock().unwrap().is_empty()); assert!(!scope.is_open());
        drop(actor);
    }
}

#[tokio::test]
async fn matched_but_expired_unregistration_result_cannot_win_between_timer_ticks() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (mut actor, _wire, bytes, mut events) = unhosting_actor_fixture(&cert.root, owner(), false);
    let scope = actor.scope.clone(); let (sequence, reply) = original_unhosting_completion(&actor);
    let (completion, result) = oneshot::channel();
    actor.unhosting_pending = Some(UnhostingPending { sequence, outer_id: 2,
        deadline: Instant::now() - Duration::from_secs(1), completion });
    assert_eq!(actor.deliver(vec![OfflineEvent::Unhosting(reply)]).await, Err(RuntimeError::Timeout));
    assert_eq!(result.await.unwrap().err(), Some(RuntimeError::Timeout));
    assert!(bytes.lock().unwrap().is_empty());
    assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    drop(actor); assert!(!scope.is_open());
}

#[tokio::test]
async fn accepted_unregistration_with_same_delivery_event_backpressure_never_reports_success() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (mut actor, _wire, bytes, mut events) = unhosting_actor_fixture(&cert.root, owner(), false);
    let scope = actor.scope.clone(); let (sequence, reply) = original_unhosting_completion(&actor);
    let (completion, result) = oneshot::channel();
    actor.unhosting_pending = Some(UnhostingPending { sequence, outer_id: 2,
        deadline: Instant::now() + Duration::from_secs(1), completion });
    for _ in 0..MAX_EVENTS {
        actor.events.try_send(RuntimeEvent { owner: actor.owner.clone(), scope: scope.clone(), events: Vec::new() }).unwrap();
    }
    assert_eq!(actor.deliver(vec![OfflineEvent::Unhosting(reply), OfflineEvent::Control(CallKind::Ping)]).await,
        Err(RuntimeError::Backpressure));
    assert_eq!(result.await.unwrap().err(), Some(RuntimeError::Backpressure));
    assert!(bytes.lock().unwrap().is_empty());
    for _ in 0..MAX_EVENTS { assert!(events.try_recv().unwrap().events.is_empty()); }
    assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
    drop(actor); assert!(!scope.is_open());
}

#[tokio::test]
async fn accepted_unregistration_followed_by_same_delivery_io_failure_never_reports_success() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let now = Instant::now();
    let mut backend = OfflineBackend::new(original.clone()); let scope = backend.connection_scope();
    let mut tls = backend.take_tls_engine(PacketRoute::DirectTlsCarrier, parameters(&cert.root), now).unwrap();
    let mut peer = cert.server(&rustls::version::TLS13);
    super::super::tls_engine::tests::handshake(&mut tls, &original, &scope, &mut peer, now).unwrap();
    assert_eq!(tls.phase(), TlsPhase::Verified);
    let (mut io, bytes, _, _, _) = poll_io(false); io.limit = 0;
    let (_commands, commands) = mpsc::channel(MAX_COMMANDS); let (events, _events) = mpsc::channel(MAX_EVENTS);
    let mut actor = Actor { io: Some(io), owner: original, scope: scope.clone(), backend, tls, commands, events,
        policy: policy(), timing: None, start: now, pending: std::collections::BTreeMap::new(),
        logout_pending: None, unhosting_pending: None };
    let (sequence, reply) = original_unhosting_completion(&actor); let (completion, result) = oneshot::channel();
    actor.unhosting_pending = Some(UnhostingPending { sequence, outer_id: 2,
        deadline: Instant::now() + Duration::from_secs(1), completion });
    let outgoing = actor.backend.prepare_bootstrap(b"fixture-bootstrap-key", &[]).unwrap();
    assert_eq!(actor.deliver(vec![OfflineEvent::Unhosting(reply), OfflineEvent::Outgoing(outgoing)]).await, Err(RuntimeError::Io));
    assert_eq!(result.await.unwrap().err(), Some(RuntimeError::Io));
    assert!(!scope.is_open()); assert!(bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn command_only_clones_preserve_original_owner_and_do_not_own_release_receipts() {
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (io,bytes,_,_,dropped)=poll_io(true);
    let (mut runtime,_events)=start(io,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),policy(),None).unwrap();
    let scope=runtime.connection_scope();let commands=runtime.command_handle();let another=commands.clone();drop(another);assert!(scope.is_open());
    assert_eq!(commands.login(&owner(),&scope,login(),&[]).await.err(),Some(RuntimeError::Scope));
    assert_eq!(commands.login(&original,&RpcConnectionScope::fresh(),login(),&[]).await.err(),Some(RuntimeError::Scope));
    assert_eq!(runtime.commands.capacity(),MAX_COMMANDS);assert!(bytes.lock().unwrap().is_empty());
    commands.request_close();assert!(!scope.is_open());assert!(runtime.close().await.io_released());assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn owning_actor_drop_invalidates_surviving_command_clones_without_io_rebinding() {
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (io,_,_,_,dropped)=poll_io(true);
    let (runtime,_events)=start(io,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),policy(),None).unwrap();
    let scope=runtime.connection_scope();let commands=runtime.command_handle();drop(runtime);
    assert!(!scope.is_open());assert_eq!(commands.poll(&original,&scope,&[]).await.err(),Some(RuntimeError::Scope));
    tokio::time::timeout(Duration::from_secs(1),async{while !dropped.load(Ordering::SeqCst){tokio::task::yield_now().await;}}).await.unwrap();
}

#[tokio::test]
async fn real_tls_logout_has_original_selector_semantic_result_and_join_confirmed_release() {
    for accepted in [true,false] {
        let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (client,server)=tokio::io::duplex(65_536);
        let (mut runtime,mut events)=start(client,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),policy(),None).unwrap();let scope=runtime.connection_scope();
        let mut peer=Peer{io:server,server:cert.server(&rustls::version::TLS13),network:PacketStreamDecoder::default(),plaintext:super::super::tls_plaintext::PlaintextPacketDecoder::default(),assembler:CallAssembler::default(),received:std::collections::VecDeque::new()};
        let server=tokio::spawn(async move {
            let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.send_reply(request.call_id,&login_result()).await;
            let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method,super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
            assert!(request.metadata.iter().any(|p|p.key==b"token"));assert!(!request.metadata.iter().any(|p|p.key==b"pwd"));
            let mut arguments=super::super::rpc_stream::Reader::versioned(request.arguments).unwrap();assert_eq!(arguments.bytes().unwrap(),b"fixture-user");
            assert_eq!(arguments.bytes().unwrap(),b"fixture-selector");arguments.finish().unwrap();
            peer.send_reply(request.call_id,&[0,u8::from(accepted),0,0,0,0]).await;
            while peer.io.read(&mut[0;1024]).await.unwrap()!=0 {}
        });
        let commands=runtime.command_handle();
        tokio::time::timeout(Duration::from_secs(2),async{loop{match commands.login(&original,&scope,login(),&[]).await {
            Ok(_)=>break,Err(RuntimeError::Tls(TlsError::NotReady))=>tokio::time::sleep(Duration::from_millis(5)).await,Err(e)=>panic!("login {e:?}"),
        }}}).await.unwrap();
        let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();assert!(matches!(&event.events_for(&original,&scope).unwrap()[0],OfflineEvent::Callback(_)));
        assert_eq!(commands.logout(&owner(),&scope,&[]).await.err(),Some(RuntimeError::Scope));assert!(scope.is_open());
        assert_eq!(commands.logout(&original,&scope,&[RpcProperty{key:b"token",value:b"foreign"}]).await.err(),Some(RuntimeError::InvalidConfiguration));assert!(scope.is_open());
        let result=tokio::time::timeout(Duration::from_secs(2),runtime.logout(&original,&scope,&[])).await.unwrap();
        if accepted {let receipt=result.unwrap();assert!(receipt.io_released());assert_eq!(receipt.reason,RuntimeError::LoggedOut);} else {assert_eq!(result.err(),Some(RuntimeError::Protocol));}
        assert!(!scope.is_open());server.await.unwrap();
    }
}

#[tokio::test]
async fn cancelling_native_logout_reply_wait_retires_original_poll_gate_without_replaying_rpc() {
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let (client,server)=tokio::io::duplex(65_536);
    let (mut runtime,mut events)=start(client,original.clone(),PacketRoute::DirectTlsCarrier,parameters(&cert.root),policy(),None).unwrap();let scope=runtime.connection_scope();
    let mut peer=Peer{io:server,server:cert.server(&rustls::version::TLS13),network:PacketStreamDecoder::default(),plaintext:super::super::tls_plaintext::PlaintextPacketDecoder::default(),assembler:CallAssembler::default(),received:std::collections::VecDeque::new()};
    let (submitted,received)=oneshot::channel();let server=tokio::spawn(async move {
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();peer.send_reply(request.call_id,&login_result()).await;
        let message=peer.receive().await.unwrap();let request=decode_request(message.payload.expose_to_transport()).unwrap();assert_eq!(request.method,super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());submitted.send(()).unwrap();
        while peer.io.read(&mut[0;1024]).await.unwrap()!=0 {}
    });
    let commands=runtime.command_handle();tokio::time::timeout(Duration::from_secs(2),async{loop{match commands.login(&original,&scope,login(),&[]).await {
        Ok(_)=>break,Err(RuntimeError::Tls(TlsError::NotReady))=>tokio::time::sleep(Duration::from_millis(5)).await,Err(e)=>panic!("login {e:?}"),
    }}}).await.unwrap();let _event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();
    {
        let logging_out=commands.logout(&original,&scope,&[]);tokio::pin!(logging_out);
        tokio::select! { result=logging_out.as_mut()=>panic!("no logout reply: {result:?}"),_ = received=>{} }
        assert!(scope.is_open());
    }
    assert!(!scope.is_open());assert!(runtime.close().await.io_released());server.await.unwrap();
}

#[tokio::test]
async fn actor_bootstrap_result_is_original_owned_configuration_not_login_or_transport_authorization() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let (client, server) = tokio::io::duplex(65_536);
    let (mut runtime, mut events) = start(client, original.clone(), PacketRoute::DirectTlsCarrier, parameters(&cert.root), policy(), None).unwrap(); let scope = runtime.connection_scope();
    let mut peer = Peer { io: server, server: cert.server(&rustls::version::TLS13), network: PacketStreamDecoder::default(),
        plaintext: super::super::tls_plaintext::PlaintextPacketDecoder::default(), assembler: CallAssembler::default(), received: std::collections::VecDeque::new() };
    let password = b"fixture-reference-context";
    let entry = super::super::legacy_config::tests::fixture_envelope(b"#fixture-account:tcp -h fixture.invalid -p 443;", password, [3; 8]);
    let received = tokio::spawn(async move {
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_bootstrap::GET_APP_CONFIGS_METHOD.as_bytes());
        assert!(!request.metadata.iter().any(|p| p.key == b"pwd" || p.key == b"token"));
        let mut r = super::super::rpc_stream::Reader::versioned(request.arguments).unwrap(); assert_eq!(r.bytes().unwrap(), b"fixture-bootstrap-key"); r.finish().unwrap();
        let mut result = Writer::new(); result.boolean(true).unwrap(); result.properties(&[RpcProperty { key: b"DomainName", value: b"fixture.invalid" }]).unwrap();
        for value in [entry.as_slice(), b"", b"", b""] { result.bytes(value).unwrap(); } let result = result.finish();
        peer.send_reply(request.call_id, result.expose_to_transport()).await;
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_login::LOGIN_SESSION_METHOD.as_bytes());
        peer.send_reply(request.call_id, &login_result()).await;
        while peer.io.read(&mut [0; 1024]).await.unwrap() != 0 {}
    });
    let receipt = tokio::time::timeout(Duration::from_secs(2), async { loop {
        match runtime.bootstrap(&original, &scope, b"fixture-bootstrap-key", &[]).await {
            Ok(receipt) => break receipt,
            Err(RuntimeError::Tls(TlsError::NotReady)) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(error) => panic!("bootstrap submission {error:?}"),
        }
    }}).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    let result = match event.into_events(&original, &scope).unwrap().into_iter().next().unwrap() {
        OfflineEvent::Bootstrap(result) => result, _ => panic!("configuration, not callback, required"),
    };
    assert_eq!(result.call_id(), receipt.call_id.unwrap());
    let key = super::super::legacy_config::ConfigDecodeKey::supplied(password).unwrap();
    let deployment = super::super::deployment::BootstrapDeployment::from_reply(&result, &original, &scope, Some(&key)).unwrap();
    assert_eq!(deployment.raw_tcp_candidates_for(&original, &scope).unwrap()[0].host(), b"fixture.invalid");
    assert!(deployment.property_for(&original, &scope, b"missing", None).unwrap().is_none());
    assert!(super::super::deployment::BootstrapDeployment::from_reply(&result, &original, &RpcConnectionScope::fresh(), Some(&key)).is_err());
    runtime.login(&original, &scope, login(), &[]).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert!(matches!(&event.events_for(&original, &scope).unwrap()[0], OfflineEvent::Callback(c) if c.callback == (Callback::SdkLogin { success: true })));
    assert!(runtime.close().await.io_released()); received.await.unwrap(); assert!(deployment.account_reference_for(&original, &scope).is_none());
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
