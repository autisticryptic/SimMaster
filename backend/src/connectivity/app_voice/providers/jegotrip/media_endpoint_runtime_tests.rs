//! Actions-only SOURCE tests: actual isolated loopback TCP + native TLS13
//! PacketStream carriers. Not executed by the editing agent. Endpoint signaling
//! is not SDK CallChanged, UDP-carrier success or a whole audible pipeline.
use super::*;
use super::super::{
    arc_session::{ArcPhase, ArcRpcSession}, call_control::CallControlTicket,
    media_endpoint::{MediaEndpointProfile, MediaEndpointProfileInputs},
    media_test_support::{accepted_context, assert_endpoint_request, endpoint_result, profile, CONTENT},
    packet_stream::{encode_packet_stream_frame, PacketStreamDecoder},
    rpc_envelope::encode_reply, rpc_stream::{RpcProperty, Writer},
    runtime_connector::{ConnectError, ConnectFuture, ConnectedIo, ConnectionInput, TcpRuntimeConnector},
    runtime_provision::{AccountTrust, AccountTrustInputs}, tls_engine::tests::Certificates,
    tls_plaintext::PlaintextPacketDecoder,
    transport_packets::{decode_tls_carrier, encode_tls_carrier, CallMessage},
};
use std::{collections::VecDeque, io, sync::{Mutex as StdMutex, atomic::{AtomicBool, Ordering}}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream, UdpSocket}};
use tokio_rustls::rustls;

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(3), future).await.expect("bounded isolated fixture")
}
fn peer_task<F: Future<Output = ()> + Send + 'static>(future: F) -> JoinHandle<()> {
    tokio::spawn(async move { bounded(future).await })
}

struct Fixture {
    rpc: ArcRpcSession, owner: AttemptOwner, account: RpcConnectionScope,
    ticket: CallControlTicket, supplied: Arc<MediaEndpointProfile>,
    handle: MediaEndpointHandle, result: MediaEndpointReady,
    // Only reserves the ORIGINAL supplied port. No UDP read/write or success
    // assertion: a real bound port is still DATA, not a recovered UDP carrier.
    _original_bound_port: UdpSocket, port: u16,
}
async fn start(supplied: Arc<MediaEndpointProfile>) -> (Fixture, TcpListener) {
    let listener = bounded(TcpListener::bind(("127.0.0.1", 0))).await.unwrap();
    let endpoint = format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};", listener.local_addr().unwrap().port());
    let original_bound_port = bounded(UdpSocket::bind(("127.0.0.1", 0))).await.unwrap();
    let port = original_bound_port.local_addr().unwrap().port();
    let (rpc, owner, account, ticket, accepted) = accepted_context(endpoint.as_bytes(), CONTENT);
    let plan = MediaEndpointPlan::from_accept(&accepted, &ticket, RpcConnectionScope::fresh(), supplied.clone(), port).unwrap();
    let connector = Arc::new(TcpRuntimeConnector::new(Duration::from_secs(2)).unwrap());
    let (handle, result) = spawn(plan, connector);
    (Fixture { rpc, owner, account, ticket, supplied, handle, result, _original_bound_port: original_bound_port, port }, listener)
}
fn account_live(fixture: &Fixture) {
    assert!(fixture.account.is_open()); assert!(fixture.ticket.is_live());
    assert_eq!(fixture.rpc.phase(), ArcPhase::Authenticated);
    assert!(fixture.ticket.connection_scope().matches(&fixture.account));
    assert_eq!(fixture.ticket.owner(), &fixture.owner);
    assert!(!fixture.handle.scope().matches(&fixture.account));
}

/// Native carrier/packet parser modelled on transport_actor_tests::Peer, with
/// actual TCP IO. TLS errors are returned so mismatch peers can assert no RPC.
struct Peer {
    io: TcpStream, tls: rustls::ServerConnection, network: PacketStreamDecoder,
    plaintext: PlaintextPacketDecoder, assembler: CallAssembler, received: VecDeque<CallMessage>,
}
impl Peer {
    async fn accept(listener: TcpListener, tls: rustls::ServerConnection) -> Self {
        let (io, address) = listener.accept().await.unwrap(); assert!(address.ip().is_loopback());
        Self { io, tls, network: PacketStreamDecoder::default(), plaintext: PlaintextPacketDecoder::default(),
            assembler: CallAssembler::default(), received: VecDeque::new() }
    }
    async fn receive(&mut self) -> io::Result<Option<CallMessage>> {
        let mut bytes = [0; 8192];
        loop {
            if let Some(message) = self.received.pop_front() { return Ok(Some(message)); }
            let count = self.io.read(&mut bytes).await?; if count == 0 { return Ok(None); }
            for carrier in self.network.on_bytes(&bytes[..count]).unwrap() {
                let raw = decode_tls_carrier(carrier.expose_to_transport()).unwrap();
                let mut cursor = io::Cursor::new(raw);
                while cursor.position() < raw.len() as u64 {
                    assert!(self.tls.read_tls(&mut cursor)? > 0);
                    self.tls.process_new_packets().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                }
                let cipher = self.take_cipher(); if !cipher.is_empty() { self.send_cipher(&cipher).await?; }
                let mut plaintext = [0; 16_384];
                loop {
                    match std::io::Read::read(&mut self.tls.reader(), &mut plaintext) {
                        Ok(0) => break,
                        Ok(count) => for packet in self.plaintext.on_bytes(&plaintext[..count]).unwrap() {
                            if let Some(message) = self.assembler.on_packet(packet.expose_to_transport()).unwrap() {
                                self.received.push_back(message);
                            }
                        },
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error),
                    }
                }
            }
        }
    }
    fn take_cipher(&mut self) -> Vec<u8> {
        let mut cipher = Vec::new();
        while self.tls.wants_write() { assert!(self.tls.write_tls(&mut cipher).unwrap() > 0); }
        cipher
    }
    async fn send_cipher(&mut self, cipher: &[u8]) -> io::Result<()> {
        for chunk in cipher.chunks(1024) {
            let carrier = encode_tls_carrier(chunk).unwrap();
            let frame = encode_packet_stream_frame(carrier.expose_to_transport()).unwrap();
            self.io.write_all(frame.expose_to_transport()).await?;
        }
        self.io.flush().await
    }
    async fn send_call(&mut self, kind: CallKind, envelope: &[u8]) -> io::Result<()> {
        for packet in encode_call_packets(kind, envelope, TLS_FRAGMENT_THRESHOLD).unwrap() {
            std::io::Write::write_all(&mut self.tls.writer(), packet.expose_to_transport())?;
        }
        let cipher = self.take_cipher(); self.send_cipher(&cipher).await
    }
    async fn send_reply(&mut self, id: i32, code: u32, stream: &[u8]) -> io::Result<()> {
        let reply = encode_reply(id, code, stream).unwrap();
        self.send_call(CallKind::Reply, reply.expose_to_transport()).await
    }
    async fn eof(&mut self) {
        assert!(self.receive().await.unwrap().is_none(), "no replay, retry or extra RPC on retirement");
        assert!(self.received.is_empty());
    }
}

#[tokio::test]
async fn actual_tcp_native_request_pending_wrong_sequence_ping_only_pong_then_original_true_and_join_eof() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = start(profile(&cert.root, "127.0.0.1")).await;
    let port = fixture.port; let server = cert.server(&rustls::version::TLS13);
    let (pending, pending_rx) = oneshot::channel(); let (allow_true, allow_true_rx) = oneshot::channel();
    let peer = peer_task(async move {
        let mut peer = Peer::accept(listener, server).await;
        let message = peer.receive().await.unwrap().unwrap(); assert_eq!(message.kind, CallKind::Request);
        assert_endpoint_request(message.payload.expose_to_transport(), CONTENT, port);
        let result = endpoint_result(true, b"");
        peer.send_reply(2, 0, result.expose_to_transport()).await.unwrap();
        peer.send_call(CallKind::Ping, &[]).await.unwrap();
        let pong = peer.receive().await.unwrap().unwrap(); assert_eq!(pong.kind, CallKind::Pong);
        assert!(pong.payload.expose_to_transport().is_empty());
        // Pong input must not manufacture another Pong, request or completion.
        peer.send_call(CallKind::Pong, &[]).await.unwrap(); pending.send(()).unwrap();
        allow_true_rx.await.unwrap();
        peer.send_reply(1, 0, result.expose_to_transport()).await.unwrap(); peer.eof().await;
    });
    bounded(pending_rx).await.unwrap();
    assert!(matches!(fixture.result.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    assert!(fixture.handle.scope().is_open()); account_live(&fixture);
    allow_true.send(()).unwrap();
    let proof = bounded(&mut fixture.result).await.unwrap().unwrap();
    let media_scope = fixture.handle.scope();
    assert!(proof.matches(&fixture.ticket, &media_scope, &fixture.supplied));
    assert_eq!(proof.features(&fixture.ticket, &media_scope, &fixture.supplied).unwrap(), vec![b"tcp".as_slice(), b"udp"]);
    assert!(proof.properties(&fixture.ticket, &fixture.account, &fixture.supplied).is_none());
    let closed = bounded(fixture.handle.close()).await; assert!(closed.io_released());
    assert!(!media_scope.is_open()); assert!(proof.properties(&fixture.ticket, &media_scope, &fixture.supplied).is_none());
    account_live(&fixture); bounded(peer).await.unwrap();
    assert_eq!(format!("{:?}", fixture.handle), "MediaEndpointHandle(<redacted>)");
}

#[tokio::test]
async fn remote_original_ticket_revoke_between_pending_and_true_ends_actor_without_accept_data() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = start(profile(&cert.root, "127.0.0.1")).await;
    let port = fixture.port; let server = cert.server(&rustls::version::TLS13);
    let (pending, pending_rx) = oneshot::channel(); let (late_true, late_true_rx) = oneshot::channel();
    let peer = peer_task(async move {
        let mut peer = Peer::accept(listener, server).await;
        let request = peer.receive().await.unwrap().unwrap(); assert_eq!(request.kind, CallKind::Request);
        assert_endpoint_request(request.payload.expose_to_transport(), CONTENT, port);
        let result = endpoint_result(true, b""); pending.send(()).unwrap();
        late_true_rx.await.unwrap();
        // The actor has already joined; even a prepared native positive result
        // cannot revive its original capability. TCP may reject this late write.
        let _ = peer.send_reply(1, 0, result.expose_to_transport()).await;
        let mut bytes = [0; 8192];
        loop { match peer.io.read(&mut bytes).await {
            Ok(0) => break, Ok(_) => panic!("retired actor wrote extra data"),
            Err(e) if matches!(e.kind(), io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe) => break,
            Err(e) => panic!("unexpected late TCP result {e:?}"),
        }}
    });
    bounded(pending_rx).await.unwrap(); assert!(fixture.handle.scope().is_open());
    let original_remote_ticket = fixture.ticket.captured_copy(); original_remote_ticket.revoke();
    assert!(bounded(&mut fixture.result).await.unwrap().is_err());
    let closed = bounded(fixture.handle.close()).await; assert!(closed.io_released());
    assert!(!fixture.handle.scope().is_open()); assert!(!fixture.ticket.is_live());
    assert!(fixture.account.is_open()); assert_eq!(fixture.rpc.phase(), ArcPhase::Authenticated);
    late_true.send(()).unwrap(); bounded(peer).await.unwrap();
}

#[tokio::test]
async fn dropping_original_result_receiver_cancels_pending_actor_and_join_does_not_widen_account() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (fixture, listener) = start(profile(&cert.root, "127.0.0.1")).await;
    let port = fixture.port; let server = cert.server(&rustls::version::TLS13);
    let (pending, pending_rx) = oneshot::channel();
    let peer = peer_task(async move {
        let mut peer = Peer::accept(listener, server).await;
        let request = peer.receive().await.unwrap().unwrap(); assert_eq!(request.kind, CallKind::Request);
        assert_endpoint_request(request.payload.expose_to_transport(), CONTENT, port);
        pending.send(()).unwrap(); peer.eof().await;
    });
    bounded(pending_rx).await.unwrap(); account_live(&fixture);
    let Fixture { rpc, owner, account, ticket, supplied: _, handle, result, _original_bound_port, port: _ } = fixture;
    let media_scope = handle.scope(); drop(result);
    bounded(media_scope.closed()).await;
    let closed = bounded(handle.close()).await; assert!(closed.io_released());
    assert!(!handle.scope().is_open()); assert!(account.is_open()); assert!(ticket.is_live());
    assert_eq!(rpc.phase(), ArcPhase::Authenticated); assert_eq!(ticket.owner(), &owner);
    bounded(peer).await.unwrap(); drop(_original_bound_port);
}

#[tokio::test]
async fn native_false_nonzero_code_nonempty_footer_and_malformed_reply_are_never_positive() {
    let cert = Certificates::generate("IP:127.0.0.1");
    let positive = endpoint_result(true, b""); let negative = endpoint_result(false, b"");
    let footer = endpoint_result(true, b"fixture-rejection");
    let mut missing_footer = Writer::new(); missing_footer.boolean(true).unwrap();
    missing_footer.u32(0).unwrap(); missing_footer.properties(&[]).unwrap(); let missing_footer = missing_footer.finish();
    for (code, stream, expected) in [(0, negative.expose_to_transport(), MediaError::Rejected),
        (9, positive.expose_to_transport(), MediaError::Rejected), (0, footer.expose_to_transport(), MediaError::Rejected),
        (0, missing_footer.expose_to_transport(), MediaError::Protocol), (0, b"\0\x01".as_slice(), MediaError::Protocol)] {
        let (mut fixture, listener) = start(profile(&cert.root, "127.0.0.1")).await;
        let port = fixture.port; let server = cert.server(&rustls::version::TLS13); let stream = stream.to_vec();
        let peer = peer_task(async move {
            let mut peer = Peer::accept(listener, server).await;
            let request = peer.receive().await.unwrap().unwrap(); assert_eq!(request.kind, CallKind::Request);
            assert_endpoint_request(request.payload.expose_to_transport(), CONTENT, port);
            peer.send_reply(1, code, &stream).await.unwrap(); peer.eof().await;
        });
        assert_eq!(bounded(&mut fixture.result).await.unwrap().err(), Some(expected));
        let closed = bounded(fixture.handle.close()).await; assert!(closed.io_released());
        assert_eq!(closed.reason, Some(expected)); assert!(!fixture.handle.scope().is_open());
        account_live(&fixture); bounded(peer).await.unwrap();
    }
}

#[tokio::test]
async fn exact_ip_san_and_independent_root_mismatch_fail_tls_before_any_native_rpc() {
    let wrong_san = Certificates::generate("DNS:fixture.invalid");
    let correct_san = Certificates::generate("IP:127.0.0.1");
    let wrong_root = Certificates::generate("IP:127.0.0.1");
    for (server_cert, root) in [(&wrong_san, &wrong_san.root), (&correct_san, &wrong_root.root)] {
        let (mut fixture, listener) = start(profile(root, "127.0.0.1")).await;
        let server = server_cert.server(&rustls::version::TLS13);
        let peer = peer_task(async move {
            let mut peer = Peer::accept(listener, server).await;
            match peer.receive().await {
                Ok(None) | Err(_) => {}, Ok(Some(_)) => panic!("TLS mismatch delivered native RPC"),
            }
            assert!(peer.received.is_empty());
        });
        assert_eq!(bounded(&mut fixture.result).await.unwrap().err(), Some(MediaError::Protocol));
        let closed = bounded(fixture.handle.close()).await; assert!(closed.io_released());
        assert!(!fixture.handle.scope().is_open()); account_live(&fixture); bounded(peer).await.unwrap();
    }
}

fn framed_cipher(cipher: &[u8]) -> Vec<u8> {
    cipher.chunks(1024).flat_map(|chunk| {
        let carrier = encode_tls_carrier(chunk).unwrap();
        encode_packet_stream_frame(carrier.expose_to_transport()).unwrap().expose_to_transport().to_vec()
    }).collect()
}
async fn drip_until_eof(io: &mut TcpStream, pieces: Vec<Vec<u8>>) {
    let (mut read, mut write) = io.split();
    let eof = async {
        let mut bytes = [0; 8192];
        assert_eq!(read.read(&mut bytes).await.unwrap(), 0, "no retry or extra RPC during timeout");
    };
    let drip = async {
        for piece in pieces {
            write.write_all(&piece).await.unwrap();
            tokio::time::sleep(Duration::from_millis(55)).await;
        }
        std::future::pending::<()>().await;
    };
    // Stop the dripper as soon as the original actor releases IO; never write
    // after observed EOF merely to manufacture a reset/error fixture.
    tokio::select! { biased; _ = eof => {}, _ = drip => unreachable!() }
}

fn deadline_profile(root: &str) -> Arc<MediaEndpointProfile> {
    let trust = Arc::new(AccountTrust::supplied(AccountTrustInputs {
        authority: "127.0.0.1", root_der_base64: root, enable_sni: true,
    }).unwrap());
    Arc::new(MediaEndpointProfile::supplied(MediaEndpointProfileInputs {
        trust, routing: &[RpcProperty { key: b"id", value: b"#fixture-endpoint" }, RpcProperty { key: b"domain", value: b"fixture-domain" }],
        features: &[b"tcp", b"udp"], properties: &[], reference_decode_key: None,
        handshake_timeout: Duration::from_secs(2), partial_timeout: Duration::from_millis(120),
        io_timeout: Duration::from_secs(2), rpc_timeout: Duration::from_millis(240),
    }).unwrap())
}

#[tokio::test]
async fn partial_response_and_wrong_sequence_drips_do_not_extend_fixed_deadline_and_cleanup() {
    let cert = Certificates::generate("IP:127.0.0.1");
    for partial in [true, false] {
        let (mut fixture, listener) = start(deadline_profile(&cert.root)).await;
        let port = fixture.port; let server = cert.server(&rustls::version::TLS13);
        let (started, started_rx) = oneshot::channel();
        let peer = peer_task(async move {
            let mut peer = Peer::accept(listener, server).await;
            let request = peer.receive().await.unwrap().unwrap(); assert_eq!(request.kind, CallKind::Request);
            assert_endpoint_request(request.payload.expose_to_transport(), CONTENT, port); started.send(()).unwrap();
            if partial {
                let result = endpoint_result(true, b""); let envelope = encode_reply(1, 0, result.expose_to_transport()).unwrap();
                let packets = encode_call_packets(CallKind::Reply, envelope.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
                std::io::Write::write_all(&mut peer.tls.writer(), packets[0].expose_to_transport()).unwrap();
                let cipher = peer.take_cipher(); let frame = framed_cipher(&cipher);
                let pieces = frame[..12].iter().map(|byte| vec![*byte]).collect();
                drip_until_eof(&mut peer.io, pieces).await;
            } else {
                let result = endpoint_result(true, b"");
                let envelope = encode_reply(2, 0, result.expose_to_transport()).unwrap();
                let packets = encode_call_packets(CallKind::Reply, envelope.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
                let mut pieces = Vec::new();
                for _ in 0..12 {
                    for packet in &packets { std::io::Write::write_all(&mut peer.tls.writer(), packet.expose_to_transport()).unwrap(); }
                    pieces.push(framed_cipher(&peer.take_cipher()));
                }
                drip_until_eof(&mut peer.io, pieces).await;
            }
        });
        bounded(started_rx).await.unwrap(); let start = Instant::now();
        assert_eq!(bounded(&mut fixture.result).await.unwrap().err(), Some(MediaError::Timeout));
        assert!(start.elapsed() < Duration::from_millis(500), "drips continuing past 600ms cannot renew the original deadline");
        let closed = bounded(fixture.handle.close()).await; assert!(closed.io_released());
        assert_eq!(closed.reason, Some(MediaError::Timeout)); assert!(!fixture.handle.scope().is_open());
        account_live(&fixture); bounded(peer).await.unwrap();
    }
}

struct ConnectRetirement(Arc<AtomicBool>);
impl Drop for ConnectRetirement { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
struct PendingConnector {
    entered: StdMutex<Option<oneshot::Sender<()>>>, dropped: Arc<AtomicBool>,
    owner: AttemptOwner, scope: RpcConnectionScope, root: String,
}
impl RuntimeConnector for PendingConnector {
    fn connect<'a>(&'a self, owner: &'a AttemptOwner, scope: &'a RpcConnectionScope, input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move {
            let _retirement = ConnectRetirement(self.dropped.clone());
            assert_eq!(owner, &self.owner); assert!(scope.matches(&self.scope));
            assert_eq!(input.authority, "fixture.invalid"); assert_eq!(input.root_der_base64, self.root);
            assert_eq!(input.server_reference.as_bytes(), super::super::media_test_support::ENDPOINT);
            let (io, _peer) = tokio::io::duplex(32);
            // Trusted test adapter captures EXACT originals, not an invented
            // owner/scope/current callback. No IO is attached until connect ends.
            let bound = ConnectedIo::original(owner, scope, input, io)?;
            self.entered.lock().unwrap().take().unwrap().send(()).unwrap();
            std::future::pending::<Result<ConnectedIo, ConnectError>>().await?;
            Ok(bound)
        })
    }
}

#[tokio::test]
async fn cancelled_close_future_retains_original_join_during_pending_bound_connector_cleanup() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (rpc, owner, account, ticket, accepted) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
    let scope = RpcConnectionScope::fresh();
    let plan = MediaEndpointPlan::from_accept(&accepted, &ticket, scope.clone(), profile(&cert.root, "fixture.invalid"), 31009).unwrap();
    {
        let original = plan.connection().unwrap();
        for substituted in [ConnectionInput { server_reference: "#other:tcp -h fixture.invalid -p 443;", ..original },
            ConnectionInput { authority: "other.invalid", ..original },
            ConnectionInput { root_der_base64: "AA==", ..original },
            ConnectionInput { enable_sni: false, ..original }] {
            let (io, _peer) = tokio::io::duplex(32);
            let bound = ConnectedIo::original(&owner, &scope, original, io).unwrap();
            assert_eq!(bound.into_original_io(&owner, &scope, substituted).err(), Some(ConnectError::Scope));
        }
        let (io, _peer) = tokio::io::duplex(32);
        let bound = ConnectedIo::original(&owner, &scope, original, io).unwrap();
        assert_eq!(bound.into_original_io(&owner, &RpcConnectionScope::fresh(), original).err(), Some(ConnectError::Scope));
    }
    let dropped = Arc::new(AtomicBool::new(false)); let (entered, entered_rx) = oneshot::channel();
    let connector = Arc::new(PendingConnector { entered: StdMutex::new(Some(entered)), dropped: dropped.clone(),
        owner: owner.clone(), scope: scope.clone(), root: cert.root.clone() });
    let (handle, mut result) = spawn(plan, connector); bounded(entered_rx).await.unwrap();
    assert!(matches!(result.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    assert!(!dropped.load(Ordering::SeqCst)); assert!(account.is_open()); assert!(ticket.is_live());
    // Poll once, then cancel without awaiting the actor: close owns its Join in
    // the mutex, so dropping this future cannot take/detach that JoinHandle.
    let observed = {
        let closing = handle.close(); tokio::pin!(closing);
        poll_fn(|cx| Poll::Ready(closing.as_mut().poll(cx).is_pending())).await
    };
    assert!(observed, "pending original owner cannot already be a release proof");
    assert!(handle.join.lock().await.is_some());
    let closed = bounded(handle.close()).await; assert!(closed.io_released());
    assert!(dropped.load(Ordering::SeqCst)); assert!(!scope.is_open());
    assert!(handle.join.lock().await.is_none()); assert!(bounded(result).await.unwrap().is_err());
    assert!(account.is_open()); assert!(ticket.is_live()); assert_eq!(rpc.phase(), ArcPhase::Authenticated);
}

#[tokio::test]
async fn dropping_semantic_waiter_during_pending_connect_closes_destination_before_io_timeout() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_rpc, owner, account, ticket, accepted) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
    let scope = RpcConnectionScope::fresh();
    let plan = MediaEndpointPlan::from_accept(&accepted, &ticket, scope.clone(), profile(&cert.root, "fixture.invalid"), 31014).unwrap();
    let dropped = Arc::new(AtomicBool::new(false)); let (entered, entered_rx) = oneshot::channel();
    let connector = Arc::new(PendingConnector { entered: StdMutex::new(Some(entered)), dropped: dropped.clone(),
        owner, scope: scope.clone(), root: cert.root.clone() });
    let (handle, result) = spawn(plan, connector); bounded(entered_rx).await.unwrap();
    let start = Instant::now(); drop(result); assert!(!scope.is_open());
    let closed = bounded(handle.close()).await; assert!(closed.io_released());
    assert!(start.elapsed() < Duration::from_millis(500)); assert!(dropped.load(Ordering::SeqCst));
    assert!(account.is_open()); assert!(ticket.is_live());
}

#[tokio::test]
async fn original_handshake_deadline_includes_pending_connector_and_preserves_timeout_kind() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let (_rpc, owner, account, ticket, accepted) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
    let trust = Arc::new(AccountTrust::supplied(AccountTrustInputs {
        authority: "fixture.invalid", root_der_base64: &cert.root, enable_sni: true,
    }).unwrap());
    let supplied = Arc::new(MediaEndpointProfile::supplied(MediaEndpointProfileInputs {
        trust, routing: &[RpcProperty { key: b"id", value: b"#fixture-endpoint" }, RpcProperty { key: b"domain", value: b"fixture-domain" }],
        features: &[b"tcp"], properties: &[], reference_decode_key: None,
        handshake_timeout: Duration::from_millis(100), partial_timeout: Duration::from_secs(2),
        io_timeout: Duration::from_secs(2), rpc_timeout: Duration::from_secs(2),
    }).unwrap());
    let scope = RpcConnectionScope::fresh();
    let plan = MediaEndpointPlan::from_accept(&accepted, &ticket, scope.clone(), supplied, 31015).unwrap();
    let dropped = Arc::new(AtomicBool::new(false)); let (entered, entered_rx) = oneshot::channel();
    let connector = Arc::new(PendingConnector { entered: StdMutex::new(Some(entered)), dropped: dropped.clone(),
        owner, scope: scope.clone(), root: cert.root.clone() });
    let start = Instant::now(); let (handle, result) = spawn(plan, connector);
    bounded(entered_rx).await.unwrap();
    assert_eq!(bounded(result).await.unwrap().err(), Some(MediaError::Timeout));
    let closed = bounded(handle.close()).await;
    assert!(closed.io_released()); assert_eq!(closed.reason, Some(MediaError::Timeout));
    assert!(start.elapsed() < Duration::from_millis(500)); assert!(dropped.load(Ordering::SeqCst));
    assert!(!scope.is_open()); assert!(account.is_open()); assert!(ticket.is_live());
    assert_eq!(connect_error(ConnectError::Timeout), MediaError::Timeout);
    assert_eq!(tls_error(TlsError::Timeout), MediaError::Timeout);
}

struct StalledWrite { written: bool, polls: Arc<std::sync::atomic::AtomicUsize> }
impl AsyncRead for StalledWrite {
    fn poll_read(self: Pin<&mut Self>, _: &mut std::task::Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> { Poll::Pending }
}
impl AsyncWrite for StalledWrite {
    fn poll_write(mut self: Pin<&mut Self>, _: &mut std::task::Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if self.written { Poll::Pending } else { self.written = true; Poll::Ready(Ok(bytes.len().min(1))) }
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut std::task::Context<'_>) -> Poll<io::Result<()>> { Poll::Pending }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut std::task::Context<'_>) -> Poll<io::Result<()>> { Poll::Pending }
}
#[tokio::test]
async fn partial_initial_request_and_pong_writes_cannot_outlive_original_absolute_deadline() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for kind in [None, Some(CallKind::Request), Some(CallKind::Pong)] {
        let (_rpc, owner, account, ticket, accepted) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
        let plan = MediaEndpointPlan::prepare(&accepted, &ticket, profile(&cert.root, "fixture.invalid"), 31016).unwrap();
        let scope = plan.scope(); let input = plan.connection().unwrap();
        let mut tls = TlsEngine::for_arc_session(owner.clone(), scope.clone(), PacketRoute::DirectTlsCarrier, TlsParameters {
            authority: input.authority, root_der_base64: input.root_der_base64, enable_sni: input.enable_sni, tls_only: true,
            handshake_timeout: Duration::from_secs(2), partial_timeout: Duration::from_secs(2),
        }, Instant::now()).unwrap();
        // Directly exercise the production write helper with bound frames. The
        // payload distinction cannot create a new IO/RPC deadline or scope.
        let groups = if let Some(kind) = kind {
            let payload = if kind == CallKind::Request { plan.request().unwrap() } else { b"" };
            let packets = encode_call_packets(kind, payload, TLS_FRAGMENT_THRESHOLD).unwrap();
            let mut transport = super::super::packet_transport::OwnedPacketTransport::for_arc_session(owner.clone(), scope.clone(), PacketRoute::DirectTlsCarrier).unwrap();
            vec![transport.encode_tls_bytes(&owner, &scope, packets[0].expose_to_transport(), None).unwrap().unwrap()]
        } else { tls.take_wire_output(&owner, &scope, None, Instant::now()).unwrap() };
        let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut io: BoxRuntimeIo = Box::new(StalledWrite { written: false, polls: polls.clone() });
        let start = Instant::now(); let absolute = Some(start + Duration::from_millis(80));
        assert_eq!(bounded(write_frames(&plan, &mut io, groups, absolute)).await, Err(MediaError::Timeout));
        assert!(start.elapsed() < Duration::from_millis(500)); let before = polls.load(Ordering::SeqCst);
        assert!(before >= 2); tokio::task::yield_now().await; assert_eq!(polls.load(Ordering::SeqCst), before);
        assert!(account.is_open()); assert!(ticket.is_live());
    }
}

#[tokio::test]
async fn original_positive_reply_is_not_published_after_expiry_or_post_parse_revocation() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for expired in [true, false] {
        let (_rpc, owner, account, ticket, accepted) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
        let mut plan = MediaEndpointPlan::prepare(&accepted, &ticket, profile(&cert.root, "fixture.invalid"), 31017).unwrap();
        let stream = endpoint_result(true, b""); let wire = encode_reply(1, 0, stream.expose_to_transport()).unwrap();
        let reply = plan.on_reply(&owner, &plan.scope(), wire.expose_to_transport()).unwrap().unwrap();
        let (sender, mut result) = oneshot::channel(); let mut completion = Some(sender);
        let absolute = if expired { Some(Instant::now() - Duration::from_millis(1)) } else { ticket.revoke(); None };
        assert_eq!(publish_original(&plan, &mut completion, reply, absolute), Err(if expired { MediaError::Timeout } else { MediaError::Scope }));
        assert!(!matches!(result.try_recv(), Ok(Ok(_)))); assert!(account.is_open());
    }
}
