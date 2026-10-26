//! Actions-only actual isolated Endpoint TCP/TLS + native Mpath + UDP RTP
//! bidirectional relay tests. No SDK/device/service or local compiled execution.
use super::*;
use super::super::{
    arc_session::ArcRpcSession,
    media_test_support::{accepted_context, endpoint_result, profile, CONTENT},
    rpc_envelope::{decode_request, encode_reply}, rpc_stream::Reader,
    tls_engine::tests::Certificates, tls_plaintext::PlaintextPacketDecoder,
    transport_packets::{decode_tls_carrier, encode_tls_carrier, CallMessage},
};
use std::{collections::VecDeque, io};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}};
use tokio_rustls::rustls;
async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future).await.expect("bounded original audio fixture")
}
pub(crate) struct Peer { io: TcpStream, tls: rustls::ServerConnection, network: PacketStreamDecoder,
    plaintext: PlaintextPacketDecoder, calls: CallAssembler, rpc: VecDeque<CallMessage>, raw: VecDeque<Vec<u8>> }
impl Peer {
    pub(crate) async fn accept(listener: TcpListener, tls: rustls::ServerConnection) -> Self {
        let (io, peer) = listener.accept().await.unwrap(); assert!(peer.ip().is_loopback());
        Self { io, tls, network: PacketStreamDecoder::default(), plaintext: PlaintextPacketDecoder::default(),
            calls: CallAssembler::default(), rpc: VecDeque::new(), raw: VecDeque::new() }
    }
    async fn pump(&mut self) -> io::Result<bool> {
        let mut bytes = [0; 16_384]; let count = self.io.read(&mut bytes).await?; if count == 0 { return Ok(false); }
        for packet in self.network.on_bytes(&bytes[..count]).unwrap() {
            let bytes = packet.expose_to_transport();
            if !bytes.starts_with(&[0xff, 0xff, 6]) { self.raw.push_back(bytes.to_vec()); continue; }
            let raw = decode_tls_carrier(bytes).unwrap(); let mut cursor = io::Cursor::new(raw);
            while cursor.position() < raw.len() as u64 {
                assert!(self.tls.read_tls(&mut cursor)? > 0); self.tls.process_new_packets().map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            }
            let cipher = self.cipher(); if !cipher.is_empty() { self.send_cipher(&cipher).await?; }
            let mut plaintext = [0; 16_384];
            loop {
                match std::io::Read::read(&mut self.tls.reader(), &mut plaintext) {
                    Ok(0) => break,
                    Ok(count) => for packet in self.plaintext.on_bytes(&plaintext[..count]).unwrap() {
                        if let Some(call) = self.calls.on_packet(packet.expose_to_transport()).unwrap() { self.rpc.push_back(call); }
                    },
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break, Err(error) => return Err(error),
                }
            }
        }
        Ok(true)
    }
    fn cipher(&mut self) -> Vec<u8> { let mut out = Vec::new(); while self.tls.wants_write() { assert!(self.tls.write_tls(&mut out).unwrap() > 0); } out }
    async fn send_cipher(&mut self, bytes: &[u8]) -> io::Result<()> {
        for bytes in bytes.chunks(1024) {
            let packet = encode_tls_carrier(bytes).unwrap(); let frame = encode_packet_stream_frame(packet.expose_to_transport()).unwrap();
            self.io.write_all(frame.expose_to_transport()).await?;
        }
        self.io.flush().await
    }
    pub(crate) async fn next_rpc(&mut self) -> io::Result<Option<CallMessage>> {
        loop { if let Some(call) = self.rpc.pop_front() { return Ok(Some(call)); } if !self.pump().await? { return Ok(None); } }
    }
    pub(crate) async fn next_raw(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop { if let Some(raw) = self.raw.pop_front() { return Ok(Some(raw)); } if !self.pump().await? { return Ok(None); } }
    }
    pub(crate) async fn reply(&mut self, accepted: bool) {
        let result = endpoint_result(accepted, b""); let envelope = encode_reply(1, 0, result.expose_to_transport()).unwrap();
        for packet in encode_call_packets(CallKind::Reply, envelope.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap() {
            std::io::Write::write_all(&mut self.tls.writer(), packet.expose_to_transport()).unwrap();
        }
        let cipher = self.cipher(); self.send_cipher(&cipher).await.unwrap();
    }
    pub(crate) async fn send_raw(&mut self, packet: &[u8]) { let frame = encode_packet_stream_frame(packet).unwrap(); self.io.write_all(frame.expose_to_transport()).await.unwrap(); self.io.flush().await.unwrap(); }
    pub(crate) async fn eof(&mut self) {
        loop { match self.pump().await {
            Ok(false) => break, Ok(true) => { assert!(self.rpc.is_empty()); assert!(self.raw.is_empty()); },
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => break, Err(error) => panic!("unexpected retirement {error:?}"),
        }}
        assert!(self.rpc.is_empty()); assert!(self.raw.is_empty());
    }
}
fn sdp(endpoint: SocketAddr) -> Vec<u8> {
    format!("v=0\r\no=fixture 10 11 IN IP4 {}\r\ns=fixture\r\nc=IN IP4 {}\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\na=rtcp-mux\r\n",
        endpoint.ip(), endpoint.ip(), endpoint.port()).into_bytes()
}
fn rtp(sequence: u16, fill: u8, length: usize) -> Vec<u8> {
    let mut bytes = vec![0x80, 0x80, 0, 0, 0, 0, 0, 160, 0x12, 0x34, 0x56, 0x78];
    bytes[2..4].copy_from_slice(&sequence.to_be_bytes()); bytes.resize(length.max(13), fill); bytes
}
struct Fixture { _rpc: ArcRpcSession, account: RpcConnectionScope, ticket: CallControlTicket,
    handle: AudioHandle, ready: AudioWaiter, asterisk: UdpSocket, local: SocketAddr, metrics: crate::services::trunk::operator::OperatorLink }
async fn fixture(cert: &Certificates) -> (Fixture, TcpListener) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let endpoint = format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};", listener.local_addr().unwrap().port());
    let (rpc, _, account, ticket, accepted) = accepted_context(endpoint.as_bytes(), CONTENT);
    let plan = MediaEndpointPlan::prepare_mpath(&accepted, &ticket, profile(&cert.root, "127.0.0.1")).unwrap();
    let asterisk = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap(); let peer = asterisk.local_addr().unwrap();
    let native = PlainAudioOffer::trunk(&sdp(peer), true).unwrap(); let answer = PlainAudioOffer::trunk(&sdp(peer), true).unwrap();
    let bound = BoundAudioSocket::bind(&ticket, "127.0.0.1".parse().unwrap()).await.unwrap(); let local = bound.address(&ticket).unwrap();
    let metrics = crate::services::trunk::operator::OperatorLink::default();
    let relay = bound.for_peer(&ticket, &native, &answer, Some(metrics.media_metrics())).unwrap();
    let connector = Arc::new(super::super::runtime_connector::TcpRuntimeConnector::new(Duration::from_secs(2)).unwrap());
    let (handle, ready) = spawn(plan, connector, relay).unwrap();
    (Fixture { _rpc: rpc, account, ticket, handle, ready, asterisk, local, metrics }, listener)
}
fn request_is_original_mpath(wire: &[u8]) {
    let request = decode_request(wire).unwrap(); assert_eq!(request.call_id, 1); assert_eq!(request.method, b"connect.Endpoint.Endpoint");
    let mut reader = Reader::versioned(request.arguments).unwrap(); assert_eq!(reader.bytes().unwrap(), CONTENT);
    let count = reader.u32().unwrap(); for _ in 0..count { reader.bytes().unwrap(); }
    let properties = reader.properties().unwrap(); assert!(!properties.iter().any(|property| property.key == b"Client.ArcPort")); reader.finish().unwrap();
}
#[tokio::test]
async fn actual_original_tcp_tls_then_native_mpath_relay_delivers_both_rtp_directions_byte_exact_and_joins() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = fixture(&cert).await;
    let server = cert.server(&rustls::version::TLS13); let (send, recv) = oneshot::channel(); let (done, waited) = oneshot::channel();
    let downlink = rtp(9, 0xff, 173); let uplink = rtp(10, 0x80, 174); let expected = uplink.clone(); let inbound = downlink.clone();
    let serving = tokio::spawn(async move { bounded(async move {
        let mut peer = Peer::accept(listener, server).await; let request = peer.next_rpc().await.unwrap().unwrap();
        assert_eq!(request.kind, CallKind::Request); request_is_original_mpath(request.payload.expose_to_transport());
        peer.reply(true).await;
        let (packets, _) = tcp_mpath::encode_payload(0, 0x3fff, 1234, 0, &inbound).unwrap();
        peer.send_raw(packets[0].expose_to_transport()).await;
        let raw = peer.next_raw().await.unwrap().unwrap(); let mut receiver = tcp_mpath::Receiver::default();
        let Some(MpathInput::Payload { channel, payload, .. }) = receiver.on_packet(&raw, Instant::now()).unwrap() else { panic!("native uplink expected") };
        assert_eq!(channel, 0); assert_eq!(payload.expose_to_transport(), expected); send.send(()).unwrap();
        waited.await.unwrap(); peer.eof().await;
    }).await });
    let ready = bounded(&mut fixture.ready).await.unwrap(); let scope = fixture.handle.scope();
    assert!(ready.original(&fixture.ticket, &scope)); assert_eq!(ready.local(&fixture.ticket, &scope), Some(fixture.local));
    let mut received = [0; 4096]; let (count, peer) = bounded(fixture.asterisk.recv_from(&mut received)).await.unwrap();
    assert_eq!(&received[..count], downlink); assert_eq!(peer, fixture.local);
    fixture.asterisk.send_to(&uplink, fixture.local).await.unwrap(); bounded(recv).await.unwrap();
    let closed = bounded(fixture.handle.close()).await; assert!(closed.io_released); assert!(!scope.is_open()); assert!(fixture.account.is_open());
    assert!(ready.local(&fixture.ticket, &scope).is_none());
    let diagnostics = fixture.metrics.diagnostics();
    assert_eq!(diagnostics.active_relays, 0); assert_eq!(diagnostics.rtp_to_asterisk_packets, 1); assert_eq!(diagnostics.rtp_from_asterisk_packets, 1);
    assert_eq!(diagnostics.rtp_to_asterisk_bytes, downlink.len() as u64); assert_eq!(diagnostics.rtp_from_asterisk_bytes, uplink.len() as u64);
    done.send(()).unwrap(); bounded(serving).await.unwrap();
    let reused = UdpSocket::bind(fixture.local).await.unwrap(); assert_eq!(reused.local_addr().unwrap(), fixture.local);
}
#[tokio::test]
async fn native_balanced_mpath_halves_reassemble_before_delivering_one_original_audio_datagram() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = fixture(&cert).await;
    let server = cert.server(&rustls::version::TLS13); let payload = rtp(99, 0xff, 1461); let expected = payload.clone();
    let (done, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move { bounded(async move {
        let mut peer = Peer::accept(listener, server).await; request_is_original_mpath(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(true).await; let (packets, _) = tcp_mpath::encode_payload(0, 0x3fff, 9876, 0, &payload).unwrap();
        peer.send_raw(packets[1].expose_to_transport()).await; peer.send_raw(packets[0].expose_to_transport()).await;
        stopped.await.unwrap(); peer.eof().await;
    }).await });
    bounded(&mut fixture.ready).await.unwrap(); let mut bytes = [0; 4096]; let (count, _) = bounded(fixture.asterisk.recv_from(&mut bytes)).await.unwrap();
    assert_eq!(&bytes[..count], expected); assert!(bounded(fixture.handle.close()).await.io_released); done.send(()).unwrap(); bounded(serving).await.unwrap();
}
#[tokio::test]
async fn original_ticket_revocation_retires_actual_tcp_and_udp_before_late_audio_can_escape() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = fixture(&cert).await;
    let server = cert.server(&rustls::version::TLS13); let (done, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move { bounded(async move {
        let mut peer = Peer::accept(listener, server).await; request_is_original_mpath(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(true).await; stopped.await.unwrap(); peer.eof().await;
    }).await });
    let ready = bounded(&mut fixture.ready).await.unwrap(); fixture.ticket.revoke();
    assert!(!ready.original(&fixture.ticket, &fixture.handle.scope())); assert!(bounded(fixture.handle.close()).await.io_released);
    assert!(fixture.account.is_open()); let reused = UdpSocket::bind(fixture.local).await.unwrap(); assert_eq!(reused.local_addr().unwrap(), fixture.local);
    done.send(()).unwrap(); bounded(serving).await.unwrap();
}
#[tokio::test]
async fn false_native_endpoint_result_never_makes_audio_ready_and_closes_original_bound_ports() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = fixture(&cert).await;
    let server = cert.server(&rustls::version::TLS13);
    let serving = tokio::spawn(async move { bounded(async move {
        let mut peer = Peer::accept(listener, server).await; request_is_original_mpath(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(false).await; peer.eof().await;
    }).await });
    assert_eq!(bounded(&mut fixture.ready).await.err(), Some(MediaError::Rejected));
    assert!(bounded(fixture.handle.close()).await.io_released); assert!(fixture.account.is_open());
    assert!(tokio::time::timeout(Duration::from_millis(50), fixture.asterisk.recv_from(&mut [0; 4096])).await.is_err());
    let reused = UdpSocket::bind(fixture.local).await.unwrap(); assert_eq!(reused.local_addr().unwrap(), fixture.local); bounded(serving).await.unwrap();
}
#[tokio::test]
async fn unsupported_sdp_crypto_and_other_original_ticket_fail_before_audio_activation() {
    let (_rpc, _, account, ticket, _) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
    let (_other_rpc, _, _, other, _) = accepted_context(super::super::media_test_support::ENDPOINT, CONTENT);
    let peer: SocketAddr = "127.0.0.1:49152".parse().unwrap(); let offer = PlainAudioOffer::trunk(&sdp(peer), true).unwrap();
    let socket = BoundAudioSocket::bind(&ticket, "127.0.0.1".parse().unwrap()).await.unwrap();
    assert_eq!(socket.for_peer(&other, &offer, &offer, None).err(), Some(MediaError::Scope));
    let mut crypto = sdp(peer); crypto.extend_from_slice(b"a=crypto:1 fixture\r\n"); assert!(PlainAudioOffer::trunk(&crypto, true).is_err());
    ticket.revoke(); assert_eq!(BoundAudioSocket::bind(&ticket, "127.0.0.1".parse().unwrap()).await.err(), Some(MediaError::Scope)); assert!(account.is_open());
}
#[tokio::test]
async fn dropping_original_audio_waiter_before_endpoint_true_retires_only_its_destination_and_joins() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (fixture, listener) = fixture(&cert).await;
    let server = cert.server(&rustls::version::TLS13); let (seen, observed) = oneshot::channel();
    let serving = tokio::spawn(async move { bounded(async move {
        let mut peer = Peer::accept(listener, server).await; request_is_original_mpath(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        seen.send(()).unwrap(); peer.eof().await;
    }).await });
    bounded(observed).await.unwrap(); let Fixture { _rpc, account, ticket, handle, ready, asterisk: _, local, metrics: _ } = fixture;
    drop(ready); assert!(!handle.scope().is_open()); assert!(bounded(handle.close()).await.io_released); assert!(account.is_open()); assert!(ticket.is_live());
    let reused = UdpSocket::bind(local).await.unwrap(); assert_eq!(reused.local_addr().unwrap(), local); bounded(serving).await.unwrap();
}
#[tokio::test]
async fn another_udp_source_cannot_inject_audio_into_original_matched_tcp_call() {
    let cert = Certificates::generate("IP:127.0.0.1"); let (mut fixture, listener) = fixture(&cert).await;
    let server = cert.server(&rustls::version::TLS13); let (ready, seen) = oneshot::channel(); let (done, stopped) = oneshot::channel();
    let expected = rtp(22, 0xff, 173); let expected_peer = expected.clone();
    let serving = tokio::spawn(async move { bounded(async move {
        let mut peer = Peer::accept(listener, server).await; request_is_original_mpath(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());
        peer.reply(true).await;
        assert!(tokio::time::timeout(Duration::from_millis(150), peer.next_raw()).await.is_err(), "foreign UDP source produced native media");
        ready.send(()).unwrap();
        let raw = peer.next_raw().await.unwrap().unwrap(); let mut decoder = tcp_mpath::Receiver::default();
        let Some(MpathInput::Payload { channel: 0, payload, .. }) = decoder.on_packet(&raw, Instant::now()).unwrap() else { panic!("original audio required") };
        assert_eq!(payload.expose_to_transport(), expected_peer); stopped.await.unwrap(); peer.eof().await;
    }).await });
    bounded(&mut fixture.ready).await.unwrap();
    let intruder = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap(); intruder.send_to(&rtp(11, 0x80, 172), fixture.local).await.unwrap();
    bounded(seen).await.unwrap(); fixture.asterisk.send_to(&expected, fixture.local).await.unwrap();
    // A synchronization read of the metrics confirms only the exact original
    // source was admitted; teardown may wait until that one nonblocking write.
    bounded(async { while fixture.metrics.diagnostics().rtp_from_asterisk_packets != 1 { tokio::task::yield_now().await; } }).await;
    assert_eq!(fixture.metrics.diagnostics().rtp_from_asterisk_bytes, expected.len() as u64);
    assert!(bounded(fixture.handle.close()).await.io_released); done.send(()).unwrap(); bounded(serving).await.unwrap();
}
