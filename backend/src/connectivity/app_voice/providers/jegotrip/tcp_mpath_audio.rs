//! Original Endpoint TCP/TLS signaling plus its separate native TCP-Mpath
//! media stream. SDK credentials/RPC never leave TLS. Plain RTP media is an
//! explicitly enabled, SDP-validated profile, not a certificate downgrade.
//! No Client.Host/ArcPort, UDP/P2P route, address or media success is guessed.
use super::{
    call_control::CallControlTicket,
    media_endpoint::{MediaEndpointPlan, MediaError, OwnedMediaEndpointReply},
    media_endpoint_runtime::{check_deadline, connect_original, deadline, earlier, io_deadline, read_original, send_packets, tls_error, write_frames},
    native_audio_sdp::PlainAudioOffer,
    packet_stream::{encode_packet_stream_frame, PacketStreamDecoder},
    packet_transport::PacketRoute,
    raw_arc_audio::{audio_kind, AudioPacketKind},
    rpc_pending::RpcConnectionScope,
    runtime_connector::{BoxRuntimeIo, RuntimeConnector},
    tcp_mpath::{self, Input as MpathInput},
    tls_engine::{TlsEngine, TlsParameters, TlsPhase},
    transport_packets::{encode_call_packets, CallAssembler, CallKind, TLS_FRAGMENT_THRESHOLD},
};
use crate::connectivity::core::media::MediaRelayMetrics;
use std::{fmt, future::{poll_fn, Future}, net::{IpAddr, SocketAddr}, pin::Pin, sync::Arc, task::Poll, time::{Duration, Instant}};
use tokio::{io::{AsyncWrite, ReadBuf}, net::UdpSocket, sync::{oneshot, Mutex}, task::JoinHandle};
use zeroize::Zeroizing;

pub(super) struct BoundAudioSocket { ticket: CallControlTicket, socket: UdpSocket, local: SocketAddr }
impl fmt::Debug for BoundAudioSocket { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("BoundAudioSocket(<redacted>)") } }
impl BoundAudioSocket {
    /// Local Asterisk-side port, NOT a Jegotrip logical ARC port/address. Bind
    /// polls are serialized with the ORIGINAL call/account retirement gate.
    pub(super) async fn bind(ticket: &CallControlTicket, local_ip: IpAddr) -> Result<Self, MediaError> {
        if !ticket.is_live() { return Err(MediaError::Scope); }
        if local_ip.is_unspecified() || local_ip.is_multicast() { return Err(MediaError::Configuration); }
        let bind = UdpSocket::bind(SocketAddr::new(local_ip, 0)); tokio::pin!(bind);
        let guarded = poll_fn(|cx| match ticket.with_media_gate(|| bind.as_mut().poll(cx)) {
            Some(poll) => poll.map(|result| result.map_err(|_| MediaError::Io)), None => Poll::Ready(Err(MediaError::Scope)),
        });
        let socket = tokio::select! { biased;
            _ = ticket.revoked() => return Err(MediaError::Scope),
            result = tokio::time::timeout(Duration::from_secs(2), guarded) => result.map_err(|_| MediaError::Timeout)??,
        };
        if !ticket.is_live() { return Err(MediaError::Scope); }
        let local = socket.local_addr().map_err(|_| MediaError::Io)?;
        Ok(Self { ticket: ticket.captured_copy(), socket, local })
    }
    pub(super) fn address(&self, ticket: &CallControlTicket) -> Option<SocketAddr> {
        (self.ticket.same_call(ticket) && ticket.is_live()).then_some(self.local)
    }
    pub(super) fn for_peer(self, ticket: &CallControlTicket, native: &PlainAudioOffer, answer: &PlainAudioOffer,
        metrics: Option<Arc<dyn MediaRelayMetrics>>) -> Result<AudioRelay, MediaError> {
        if self.address(ticket).is_none() { return Err(MediaError::Scope); }
        let peer = answer.endpoint()?;
        if peer.is_ipv4() != self.local.is_ipv4() { return Err(MediaError::Configuration); }
        let payloads = native.matching_payloads(answer)?;
        Ok(AudioRelay { bound: self, peer, payloads, metrics })
    }
}
pub(super) struct AudioRelay { bound: BoundAudioSocket, peer: SocketAddr, payloads: Vec<u8>, metrics: Option<Arc<dyn MediaRelayMetrics>> }
impl fmt::Debug for AudioRelay { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AudioRelay(<redacted>)") } }
impl AudioRelay {
    fn matches(&self, plan: &MediaEndpointPlan) -> bool { self.bound.ticket.same_call(&plan.ticket()) && plan.is_mpath() && plan.live() }
    fn accepts(&self, bytes: &[u8]) -> bool {
        match audio_kind(bytes) {
            Ok(AudioPacketKind::Rtp) => self.payloads.contains(&(bytes[1] & 0x7f)), Ok(AudioPacketKind::Rtcp) => true, Err(_) => false,
        }
    }
}
struct MetricsGuard { metrics: Option<Arc<dyn MediaRelayMetrics>>, started: bool }
impl Drop for MetricsGuard { fn drop(&mut self) { if self.started { if let Some(metrics) = &self.metrics { metrics.relay_stopped(); } } } }

pub(super) struct AudioReady { endpoint: OwnedMediaEndpointReply, scope: RpcConnectionScope, ticket: CallControlTicket, local: SocketAddr }
impl fmt::Debug for AudioReady { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AudioReady(<redacted>)") } }
impl AudioReady {
    pub(super) fn original(&self, ticket: &CallControlTicket, scope: &RpcConnectionScope) -> bool {
        self.ticket.same_call(ticket) && self.scope.matches(scope) && scope.is_open() && ticket.is_live()
    }
    pub(super) fn local(&self, ticket: &CallControlTicket, scope: &RpcConnectionScope) -> Option<SocketAddr> { self.original(ticket, scope).then_some(self.local) }
    pub(super) fn endpoint(&self) -> &OwnedMediaEndpointReply { &self.endpoint }
}
pub(super) struct AudioWaiter { scope: RpcConnectionScope, ticket: CallControlTicket, receiver: oneshot::Receiver<Result<AudioReady, MediaError>>, delivered: bool }
impl Future for AudioWaiter {
    type Output = Result<AudioReady, MediaError>;
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.receiver).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let mut result = result.unwrap_or(Err(MediaError::Closed));
                if result.is_ok() && (!this.scope.is_open() || !this.ticket.is_live()) { result = Err(MediaError::Scope); }
                if result.is_err() { this.scope.close(); }
                this.delivered = true; Poll::Ready(result)
            }
        }
    }
}
impl Drop for AudioWaiter { fn drop(&mut self) { if !self.delivered { self.scope.close(); } } }
pub(super) struct AudioHandle { scope: RpcConnectionScope, join: Mutex<Option<JoinHandle<Result<(), MediaError>>>> }
pub(super) struct AudioClosed { pub io_released: bool, pub reason: Option<MediaError> }
impl fmt::Debug for AudioHandle { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AudioHandle(<redacted>)") } }
impl AudioHandle {
    pub(super) fn scope(&self) -> RpcConnectionScope { self.scope.clone() }
    pub(super) fn retire(&self) { self.scope.close(); }
    pub(super) async fn close(&self) -> AudioClosed {
        self.scope.close(); let mut join = self.join.lock().await;
        let Some(original) = join.as_mut() else { return AudioClosed { io_released: true, reason: None }; };
        let result = original.await; join.take();
        AudioClosed { io_released: true, reason: match result { Ok(Ok(())) => None, Ok(Err(error)) => Some(error), Err(_) => Some(MediaError::Protocol) } }
    }
}
impl Drop for AudioHandle { fn drop(&mut self) { self.scope.close(); } }
struct Retirement(RpcConnectionScope);
impl Drop for Retirement { fn drop(&mut self) { self.0.close(); } }
pub(super) fn spawn(plan: MediaEndpointPlan, connector: Arc<dyn RuntimeConnector>, relay: AudioRelay)
    -> Result<(AudioHandle, AudioWaiter), MediaError> {
    if !relay.matches(&plan) { return Err(MediaError::Scope); }
    let scope = plan.scope(); let ticket = plan.ticket(); let original_scope = scope.clone(); let (sender, receiver) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _retirement = Retirement(original_scope); let mut sender = Some(sender);
        let result = run(plan, connector, relay, &mut sender).await;
        if let Some(sender) = sender.take() { let _ = sender.send(Err(result.as_ref().err().copied().unwrap_or(MediaError::Closed))); }
        result
    });
    Ok((AudioHandle { scope: scope.clone(), join: Mutex::new(Some(join)) }, AudioWaiter { scope, ticket, receiver, delivered: false }))
}
async fn write_raw(plan: &MediaEndpointPlan, io: &mut BoxRuntimeIo, packet: &[u8], absolute: Option<Instant>) -> Result<(), MediaError> {
    if !plan.is_mpath() { return Err(MediaError::Configuration); }
    let frame = encode_packet_stream_frame(packet).map_err(|_| MediaError::Protocol)?;
    let bytes = frame.expose_to_transport(); let limit = io_deadline(plan, absolute)?; let ticket = plan.ticket(); let scope = plan.scope();
    let write = async {
        let mut offset = 0;
        while offset < bytes.len() {
            let count = poll_fn(|cx| {
                if let Err(error) = check_deadline(limit) { return Poll::Ready(Err(error)); }
                match plan.with_io_gate(|| Pin::new(&mut *io).poll_write(cx, &bytes[offset..])) {
                    Some(poll) => poll.map(|result| result.map_err(|_| MediaError::Io)), None => Poll::Ready(Err(MediaError::Scope)),
                }
            }).await?;
            if count == 0 || count > bytes.len() - offset { return Err(MediaError::Io); } offset += count;
        }
        poll_fn(|cx| {
            if let Err(error) = check_deadline(limit) { return Poll::Ready(Err(error)); }
            match plan.with_io_gate(|| Pin::new(&mut *io).poll_flush(cx)) {
                Some(poll) => poll.map(|result| result.map_err(|_| MediaError::Io)), None => Poll::Ready(Err(MediaError::Scope)),
            }
        }).await
    };
    tokio::select! { biased; _ = ticket.revoked() => Err(MediaError::Scope), _ = scope.closed() => Err(MediaError::Closed),
        _ = deadline(limit) => Err(MediaError::Timeout), result = write => result }
}
async fn read_asterisk(plan: &MediaEndpointPlan, relay: &AudioRelay, bytes: &mut [u8]) -> Result<(usize, SocketAddr), MediaError> {
    poll_fn(|cx| {
        let mut buffer = ReadBuf::new(bytes);
        match plan.with_io_gate(|| relay.bound.socket.poll_recv_from(cx, &mut buffer)) {
            Some(Poll::Ready(Ok(peer))) => Poll::Ready(Ok((buffer.filled().len(), peer))),
            Some(Poll::Ready(Err(_))) => Poll::Ready(Err(MediaError::Io)), Some(Poll::Pending) => Poll::Pending,
            None => Poll::Ready(Err(MediaError::Scope)),
        }
    }).await
}
async fn send_asterisk(plan: &MediaEndpointPlan, relay: &AudioRelay, bytes: &[u8], absolute: Option<Instant>) -> Result<(), MediaError> {
    let limit = io_deadline(plan, absolute)?; let ticket = plan.ticket(); let scope = plan.scope();
    let send = poll_fn(|cx| {
        if let Err(error) = check_deadline(limit) { return Poll::Ready(Err(error)); }
        match plan.with_io_gate(|| relay.bound.socket.poll_send_to(cx, bytes, relay.peer)) {
            Some(poll) => poll.map(|result| result.map_err(|_| MediaError::Io)), None => Poll::Ready(Err(MediaError::Scope)),
        }
    });
    let count = tokio::select! { biased; _ = ticket.revoked() => return Err(MediaError::Scope), _ = scope.closed() => return Err(MediaError::Closed),
        _ = deadline(limit) => return Err(MediaError::Timeout), result = send => result? };
    if count != bytes.len() { return Err(MediaError::Io); } Ok(())
}
enum Step { Tcp(Result<usize, MediaError>), Asterisk(Result<(usize, SocketAddr), MediaError>), Expire, Cancelled, Tick }
async fn run(mut plan: MediaEndpointPlan, connector: Arc<dyn RuntimeConnector>, relay: AudioRelay, sender: &mut Option<oneshot::Sender<Result<AudioReady, MediaError>>>)
    -> Result<(), MediaError> {
    let owner = plan.owner().clone(); let scope = plan.scope(); let ticket = plan.ticket(); let input = plan.connection()?;
    let mut tls = TlsEngine::for_arc_session(owner.clone(), scope.clone(), PacketRoute::DirectTlsCarrier, TlsParameters {
        authority: input.authority, root_der_base64: input.root_der_base64, enable_sni: input.enable_sni, tls_only: true,
        handshake_timeout: plan.profile().handshake_timeout, partial_timeout: plan.profile().partial_timeout,
    }, Instant::now()).map_err(|_| MediaError::Configuration)?;
    let mut io = connect_original(&plan, connector.as_ref(), tls.next_deadline()).await?;
    let first = tls.take_wire_output(&owner, &scope, None, Instant::now()).map_err(tls_error)?;
    write_frames(&plan, &mut io, first, tls.next_deadline()).await?;
    let mut network = PacketStreamDecoder::default(); let mut calls = CallAssembler::default(); let mut mpath = tcp_mpath::Receiver::default();
    let mut tcp = Zeroizing::new(vec![0; 16_384]); let mut rtp = Zeroizing::new(vec![0; tcp_mpath::MAX_MPATH_PAYLOAD + 1]);
    let mut rpc_deadline = None; let mut partial_deadline = None; let mut submitted = false; let mut connected = false; let mut sequence = 0u16;
    let began = Instant::now(); let mut ticks = tokio::time::interval(Duration::from_secs(1)); ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut metrics = MetricsGuard { metrics: relay.metrics.clone(), started: false };
    loop {
        if !plan.live() || !relay.matches(&plan) { return Err(MediaError::Scope); }
        let expiry = earlier(earlier(tls.next_deadline(), rpc_deadline), partial_deadline);
        let step = {
            let cancelled = async { if let Some(sender) = sender.as_mut() { sender.closed().await; } else { std::future::pending::<()>().await; } };
            tokio::select! { biased;
                _ = ticket.revoked() => Step::Cancelled, _ = scope.closed() => Step::Cancelled, _ = cancelled => Step::Cancelled,
                _ = deadline(expiry) => Step::Expire,
                value = read_original(&plan, &mut io, &mut tcp) => Step::Tcp(value),
                value = read_asterisk(&plan, &relay, &mut rtp) => Step::Asterisk(value),
                _ = ticks.tick() => Step::Tick,
            }
        };
        match step {
            Step::Cancelled => return Err(MediaError::Closed), Step::Expire => return Err(MediaError::Timeout),
            Step::Tick => { mpath.expire(Instant::now()).map_err(|_| MediaError::Protocol)?; },
            Step::Asterisk(result) => {
                let (count, peer) = result?;
                if !connected || peer != relay.peer { continue; }
                if count == 0 || count > tcp_mpath::MAX_MPATH_PAYLOAD || !relay.accepts(&rtp[..count]) { return Err(MediaError::Protocol); }
                let tick = began.elapsed().as_millis() as u16;
                let (packets, next) = tcp_mpath::encode_payload(0, sequence, tick, 0, &rtp[..count]).map_err(|_| MediaError::Protocol)?;
                for packet in packets { write_raw(&plan, &mut io, packet.expose_to_transport(), earlier(tls.next_deadline(), partial_deadline)).await?; }
                sequence = next;
                if let Some(metrics) = &relay.metrics { metrics.record_rtp_from_asterisk(count); }
            }
            Step::Tcp(result) => {
                let count = result?; if count == 0 { return Err(MediaError::Closed); }
                check_deadline(expiry)?;
                let packets = network.on_bytes(&tcp[..count]).map_err(|_| MediaError::Protocol)?;
                if network.has_partial() {
                    // A completed original frame retires its partial budget.
                    // A NEW incomplete frame in the same read gets a new clock;
                    // drips completing no frame cannot renew the old one.
                    if partial_deadline.is_none() || !packets.is_empty() { partial_deadline = Some(Instant::now().checked_add(plan.profile().partial_timeout).ok_or(MediaError::Configuration)?); }
                } else { partial_deadline = None; }
                for packet in packets {
                    check_deadline(earlier(earlier(tls.next_deadline(), rpc_deadline), partial_deadline))?;
                    let raw = packet.expose_to_transport();
                    if raw.starts_with(&[0xff, 0xff, 6]) {
                        let frame = encode_packet_stream_frame(raw).map_err(|_| MediaError::Protocol)?;
                        let progress = tls.on_wire_bytes(&owner, &scope, frame.expose_to_transport(), None, Instant::now()).map_err(tls_error)?.ok_or(MediaError::Scope)?;
                        write_frames(&plan, &mut io, progress.wire, earlier(earlier(tls.next_deadline(), rpc_deadline), partial_deadline)).await?;
                        if !submitted && tls.phase() == TlsPhase::Verified {
                            let request = encode_call_packets(CallKind::Request, plan.request()?, TLS_FRAGMENT_THRESHOLD).map_err(|_| MediaError::Protocol)?;
                            rpc_deadline = Some(Instant::now().checked_add(plan.profile().rpc_timeout).ok_or(MediaError::Configuration)?);
                            send_packets(&plan, &mut io, &mut tls, &request, earlier(rpc_deadline, partial_deadline)).await?; submitted = true;
                        }
                        for packet in progress.plaintext.packets_for(&owner, &scope).ok_or(MediaError::Scope)? {
                            let Some(message) = calls.on_packet(packet.expose_to_transport()).map_err(|_| MediaError::Protocol)? else { continue; };
                            match message.kind {
                                CallKind::Reply => {
                                    if !submitted { return Err(MediaError::Protocol); }
                                    if let Some(endpoint) = plan.on_reply(&owner, &scope, message.payload.expose_to_transport())? {
                                        let publication_deadline = earlier(earlier(tls.next_deadline(), rpc_deadline), partial_deadline);
                                        check_deadline(publication_deadline)?;
                                        let result = AudioReady { endpoint, scope: scope.clone(), ticket: ticket.captured_copy(), local: relay.bound.local };
                                        let completion = sender.take().ok_or(MediaError::Protocol)?;
                                        plan.with_io_gate(|| { check_deadline(publication_deadline)?; completion.send(Ok(result)).map_err(|_| MediaError::Closed) }).ok_or(MediaError::Scope)??;
                                        rpc_deadline = None; connected = true;
                                        if let Some(metrics) = &metrics.metrics { metrics.relay_started(); } metrics.started = true;
                                    }
                                }
                                CallKind::Ping => { let pong = encode_call_packets(CallKind::Pong, &[], TLS_FRAGMENT_THRESHOLD).map_err(|_| MediaError::Protocol)?;
                                    send_packets(&plan, &mut io, &mut tls, &pong, earlier(rpc_deadline, partial_deadline)).await?; },
                                CallKind::Pong => {}, CallKind::Release => return Err(MediaError::Closed), CallKind::Request => return Err(MediaError::Configuration),
                            }
                        }
                    } else {
                        if !connected || tls.phase() != TlsPhase::Verified { return Err(MediaError::Protocol); }
                        let raw = if raw.starts_with(&[0xff, 0xff, 0xff]) { &raw[1..] } else {
                            if raw.starts_with(&[0xff, 0xff]) { return Err(MediaError::Protocol); } raw
                        };
                        match mpath.on_packet(raw, Instant::now()).map_err(|_| MediaError::Protocol)? {
                            Some(MpathInput::Payload { header, channel: 0, payload }) if header.path <= 1 => {
                                let bytes = payload.expose_to_transport();
                                if !relay.accepts(bytes) { return Err(MediaError::Protocol); }
                                send_asterisk(&plan, &relay, bytes, earlier(tls.next_deadline(), partial_deadline)).await?;
                                if let Some(metrics) = &relay.metrics { metrics.record_rtp_to_asterisk(bytes.len()); }
                            }
                            // Unknown media channels and optional bounded native
                            // sync/metrics do not create another route or ready.
                            Some(MpathInput::Control { .. }) | Some(MpathInput::Payload { .. }) | None => {},
                        }
                    }
                }
            }
        }
    }
}
#[cfg(test)]
#[path = "tcp_mpath_audio_tests.rs"]
pub(crate) mod tests;
