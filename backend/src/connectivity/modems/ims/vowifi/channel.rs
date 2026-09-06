//! VoWiFi protected SIP channel.
//!
//! The TUN/ePDG stack has already decrypted ESP before this stream/socket is
//! used. The channel adapter owns transport framing (TCP stream or UDP
//! datagrams) and exposes the transport-neutral [`ImsChannel`] contract to
//! shared REGISTER/MESSAGE logic.
//!
//! SIP-over-UDP is the default for VoWiFi (3GPP TS 24.229 §4.2A); TCP remains
//! available for carriers that explicitly configure it.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
};

use crate::connectivity::core::{
    access::{ImsChannel, ImsRequeue},
    context::ImsRoute,
    ims_access::ImsAccess,
    outbound::OutboundFlow,
    register_response::RegisterArtifacts,
    sip_frame, ImsError,
};

const MAX_PENDING_BYTES: usize = 64 * 1024;

pub struct EpdgSipChannel {
    stream: TcpStream,
    pending: Vec<u8>,
    /// Complete frames set aside by a REGISTER transaction (other dialogs).
    requeued: ImsRequeue,
    outbound: OutboundFlow,
    route: ImsRoute,
    security_verify: Option<String>,
}

impl EpdgSipChannel {
    pub fn new(
        stream: TcpStream,
        pending: Vec<u8>,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Self {
        Self {
            stream,
            pending,
            requeued: ImsRequeue::default(),
            outbound: OutboundFlow::default(),
            route,
            security_verify,
        }
    }

    pub fn into_parts(self) -> (TcpStream, Vec<u8>) {
        (self.stream, merge_requeued(self.pending, self.requeued))
    }

    pub async fn send_keepalive(&mut self) -> Result<(), ImsError> {
        if self.outbound.active() {
            return Ok(());
        }
        self.stream
            .write_all(b"\r\n\r\n")
            .await
            .map_err(|_| ImsError::new("ims_channel_keepalive_write_failed"))?;
        self.stream
            .flush()
            .await
            .map_err(|_| ImsError::new("ims_channel_keepalive_flush_failed"))
    }

    async fn maintain_outbound(&mut self) -> Result<(), ImsError> {
        if let Some(packet) = self.outbound.poll(std::time::Instant::now())? {
            if self.stream.write_all(&packet).await.is_err() {
                return Err(self.outbound.fail("ims_outbound_keepalive_send_failed"));
            }
        }
        Ok(())
    }

    async fn recv_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            self.maintain_outbound().await?;
            self.discard_keepalive_frames();
            if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
                let frame = self.pending.drain(..frame_len).collect::<Vec<_>>();
                self.outbound.received_sip(&frame)?;
                return Ok(frame);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return Err(ImsError::new("ims_channel_read_timeout"));
            }
            let wake = self
                .outbound
                .deadline()
                .map_or(deadline, |due| due.min(deadline));
            let mut chunk = [0u8; 2048];
            match tokio::time::timeout(
                wake.saturating_duration_since(now),
                self.stream.read(&mut chunk),
            )
            .await
            {
                Ok(Ok(0)) => return Err(ImsError::new("ims_channel_closed")),
                Ok(Ok(n)) => self.pending.extend_from_slice(&chunk[..n]),
                Ok(Err(_)) => return Err(ImsError::new("ims_channel_read_failed")),
                Err(_) if wake < deadline => continue,
                Err(_) => return Err(ImsError::new("ims_channel_read_timeout")),
            }
            if self.pending.len() > MAX_PENDING_BYTES {
                return Err(ImsError::new("ims_channel_frame_too_large"));
            }
        }
    }

    fn discard_keepalive_frames(&mut self) {
        while self.pending.starts_with(b"\r\n") {
            self.outbound.pong();
            self.pending.drain(..2);
        }
        while self.pending.starts_with(b"\n") {
            self.pending.drain(..1);
        }
    }
}

impl ImsChannel for EpdgSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        let prepared = self.outbound.prepare(frame)?;
        let frame = prepared.as_slice();
        self.stream
            .write_all(frame)
            .await
            .map_err(|_| ImsError::new("ims_channel_write_failed"))?;
        self.stream
            .flush()
            .await
            .map_err(|_| ImsError::new("ims_channel_flush_failed"))
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.maintain_outbound().await?;
        if let Some(frame) = self.requeued.pop_front() {
            return Ok(frame);
        }
        self.recv_fresh(timeout).await
    }

    async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.recv_fresh(timeout).await
    }

    fn requeue(&mut self, frame: Vec<u8>) {
        retain_requeued_frame(&mut self.requeued, frame, "TCP");
    }

    fn route(&self) -> ImsRoute {
        self.route
    }

    fn security_verify(&self) -> Option<&str> {
        self.security_verify.as_deref()
    }
}

/// UDP transport for a protected SIP channel. Each SIP message travels as one
/// (or a few) datagrams; framing reuses the same Content-Length de-coalescing
/// as TCP.
pub struct UdpSipChannel {
    socket: UdpSocket,
    receive_socket: Option<UdpSocket>,
    pending: Vec<u8>,
    /// Complete frames set aside by a REGISTER transaction (other dialogs).
    requeued: ImsRequeue,
    outbound: OutboundFlow,
    route: ImsRoute,
    security_verify: Option<String>,
}

impl UdpSipChannel {
    pub fn new(
        socket: UdpSocket,
        pending: Vec<u8>,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Self {
        Self {
            socket,
            receive_socket: None,
            pending,
            requeued: ImsRequeue::default(),
            outbound: OutboundFlow::default(),
            route,
            security_verify,
        }
    }

    /// Protected UDP channel with a dedicated receive socket.
    ///
    /// TS 33.203 §7.1: for UDP the P-CSCF sends responses to the UE's
    /// protected server port (port_us) from its protected client port
    /// (port_pc), which is a different socket than the one used to send the
    /// REGISTER (port_uc -> port_ps). Without this listener the kernel drops
    /// the 200 OK even when the P-CSCF accepted the registration.
    pub fn new_with_receive_socket(
        socket: UdpSocket,
        receive_socket: UdpSocket,
        pending: Vec<u8>,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Self {
        Self {
            socket,
            receive_socket: Some(receive_socket),
            pending,
            requeued: ImsRequeue::default(),
            outbound: OutboundFlow::default(),
            route,
            security_verify,
        }
    }

    pub fn into_parts(self) -> (UdpSocket, Option<UdpSocket>, Vec<u8>) {
        (
            self.socket,
            self.receive_socket,
            merge_requeued(self.pending, self.requeued),
        )
    }

    pub async fn send_keepalive(&mut self) -> Result<(), ImsError> {
        // Negotiated UDP outbound uses STUN on this socket. SIP OPTIONS is
        // not a substitute for flow maintenance.
        self.maintain_outbound().await
    }

    fn discard_keepalive_frames(&mut self) {
        while self.pending.starts_with(b"\r\n") {
            self.pending.drain(..2);
        }
        while self.pending.starts_with(b"\n") {
            self.pending.drain(..1);
        }
    }

    async fn maintain_outbound(&mut self) -> Result<(), ImsError> {
        if let Some(packet) = self.outbound.poll(std::time::Instant::now())? {
            if !matches!(self.socket.send(&packet).await, Ok(n) if n == packet.len()) {
                return Err(self.outbound.fail("ims_outbound_keepalive_send_failed"));
            }
        }
        Ok(())
    }

    /// Read BOTH protected tuples. Responses to client-flow STUN (and some
    /// REGISTER responses) return on port_uc, not the advertised port_us.
    async fn recv_datagram(&mut self) -> Result<Vec<u8>, ImsError> {
        loop {
            self.maintain_outbound().await?;
            let mut client = vec![0u8; MAX_PENDING_BYTES];
            let mut server = vec![0u8; MAX_PENDING_BYTES];
            let due = self.outbound.deadline();
            let wait = async {
                match due {
                    Some(due) => tokio::time::sleep_until(due.into()).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let read = async {
                if let Some(receive) = &self.receive_socket {
                    tokio::select! {
                        result = self.socket.recv(&mut client) => result.map(|n| { client.truncate(n); client }),
                        result = receive.recv(&mut server) => result.map(|n| { server.truncate(n); server }),
                    }
                } else {
                    self.socket.recv(&mut client).await.map(|n| {
                        client.truncate(n);
                        client
                    })
                }
            };
            let packet = tokio::select! {
                result = read => result.map_err(|_| ImsError::new("ims_channel_read_failed"))?,
                _ = wait => continue,
            };
            if !self.outbound.receive_stun(&packet)? {
                return Ok(packet);
            }
        }
    }

    /// Chunk reads are also used during REGISTER; never mix binary STUN into
    /// the SIP pending buffer. Datagram remainder remains owned by the channel.
    async fn recv_chunk(&mut self, buf: &mut [u8]) -> Result<usize, ImsError> {
        if self.pending.is_empty() {
            self.pending = self.recv_datagram().await?;
        }
        let take = buf.len().min(self.pending.len());
        buf[..take].copy_from_slice(&self.pending[..take]);
        self.pending.drain(..take);
        Ok(take)
    }

    async fn recv_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        tokio::time::timeout(timeout, async {
            loop {
                self.discard_keepalive_frames();
                if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
                    let frame = self.pending.drain(..frame_len).collect::<Vec<_>>();
                    self.outbound.received_sip(&frame)?;
                    return Ok(frame);
                }
                let packet = self.recv_datagram().await?;
                self.pending.extend_from_slice(&packet);
                if self.pending.len() > MAX_PENDING_BYTES {
                    return Err(ImsError::new("ims_channel_frame_too_large"));
                }
            }
        })
        .await
        .map_err(|_| ImsError::new("ims_channel_read_timeout"))?
    }
}

impl ImsChannel for UdpSipChannel {
    fn requeue(&mut self, frame: Vec<u8>) {
        retain_requeued_frame(&mut self.requeued, frame, "UDP");
    }

    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        let prepared = self.outbound.prepare(frame)?;
        let frame = prepared.as_slice();
        // TS 33.203 protected UDP uses two flows. UE-originated requests use
        // port_uc -> port_ps, while responses to network-originated requests
        // must use port_us -> port_pc. The latter is the dedicated receive
        // socket kept after REGISTER succeeds.
        let socket = if frame.starts_with(b"SIP/2.0") {
            self.receive_socket.as_ref().unwrap_or(&self.socket)
        } else {
            &self.socket
        };
        socket
            .send(frame)
            .await
            .map(|_| ())
            .map_err(|_| ImsError::new("ims_channel_write_failed"))
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.maintain_outbound().await?;
        if let Some(frame) = self.requeued.pop_front() {
            return Ok(frame);
        }
        self.recv_fresh(timeout).await
    }

    async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.recv_fresh(timeout).await
    }

    fn route(&self) -> ImsRoute {
        self.route
    }

    fn security_verify(&self) -> Option<&str> {
        self.security_verify.as_deref()
    }
}

/// Raw transport socket of a protected SIP channel (inside the ePDG tunnel).
pub enum SipChannelSocket {
    Tcp(TcpStream),
    Udp(UdpSocket),
    UdpPair { send: UdpSocket, receive: UdpSocket },
}

impl SipChannelSocket {
    pub fn local_addr(&self) -> Result<SocketAddr, ImsError> {
        match self {
            Self::Tcp(stream) => stream
                .local_addr()
                .map_err(|_| ImsError::new("ims_channel_local_addr_failed")),
            Self::Udp(socket) => socket
                .local_addr()
                .map_err(|_| ImsError::new("ims_channel_local_addr_failed")),
            Self::UdpPair { send, .. } => send
                .local_addr()
                .map_err(|_| ImsError::new("ims_channel_local_addr_failed")),
        }
    }

    pub fn abort(self) {
        match self {
            Self::Tcp(stream) => abort_tcp(stream),
            Self::Udp(_) => drop(self),
            Self::UdpPair { .. } => drop(self),
        }
    }
}

/// Closed-set protected SIP channel: either TCP or UDP inside the ePDG tunnel.
/// The live flows choose the variant from `profile.ims.transport`; shared
/// REGISTER/MESSAGE logic only sees the [`ImsChannel`] contract.
pub enum SipChannel {
    Tcp(EpdgSipChannel),
    Udp(UdpSipChannel),
}

impl SipChannel {
    pub fn new(
        socket: SipChannelSocket,
        pending: Vec<u8>,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Self {
        match socket {
            SipChannelSocket::Tcp(stream) => {
                Self::Tcp(EpdgSipChannel::new(stream, pending, route, security_verify))
            }
            SipChannelSocket::Udp(socket) => {
                Self::Udp(UdpSipChannel::new(socket, pending, route, security_verify))
            }
            SipChannelSocket::UdpPair { send, receive } => {
                Self::Udp(UdpSipChannel::new_with_receive_socket(
                    send,
                    receive,
                    pending,
                    route,
                    security_verify,
                ))
            }
        }
    }

    pub fn new_udp_pair(
        socket: UdpSocket,
        receive_socket: UdpSocket,
        pending: Vec<u8>,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Self {
        Self::Udp(UdpSipChannel::new_with_receive_socket(
            socket,
            receive_socket,
            pending,
            route,
            security_verify,
        ))
    }

    pub fn into_parts(self) -> (SipChannelSocket, Vec<u8>) {
        match self {
            Self::Tcp(channel) => {
                let (stream, pending) = channel.into_parts();
                (SipChannelSocket::Tcp(stream), pending)
            }
            Self::Udp(channel) => {
                let (send, receive, pending) = channel.into_parts();
                let socket = match receive {
                    Some(receive) => SipChannelSocket::UdpPair { send, receive },
                    None => SipChannelSocket::Udp(send),
                };
                (socket, pending)
            }
        }
    }

    pub fn configure_outbound(&mut self, line_id: &str, instance: &str, enabled: bool) {
        let flow = match self {
            Self::Tcp(c) => &mut c.outbound,
            Self::Udp(c) => &mut c.outbound,
        };
        flow.configure(line_id, ImsAccess::Wlan, instance, enabled);
    }

    pub fn inherit_outbound_path_observation(&mut self, previous: &Self) {
        let previous = match previous {
            Self::Tcp(c) => &c.outbound,
            Self::Udp(c) => &c.outbound,
        };
        match self {
            Self::Tcp(c) => c.outbound.inherit_path_observation(previous),
            Self::Udp(c) => c.outbound.inherit_path_observation(previous),
        }
    }

    /// All frame readers, including the adapter-owned protected exchange,
    /// report explicit outbound refusal using the same transaction key.
    pub fn observe_outbound_response(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        match self {
            Self::Tcp(c) => c.outbound.received_sip(frame),
            Self::Udp(c) => c.outbound.received_sip(frame),
        }
    }

    /// Keep the flow/keepalive owner when lending a registered channel to the
    /// legacy SMS path; reconstructing it from sockets discards its live lease.
    pub fn update_context(&mut self, route: ImsRoute, security_verify: Option<String>) {
        match self {
            Self::Tcp(c) => {
                c.route = route;
                c.security_verify = security_verify;
            }
            Self::Udp(c) => {
                c.route = route;
                c.security_verify = security_verify;
            }
        }
    }

    pub fn prepend_pending(&mut self, bytes: Vec<u8>) -> Result<(), ImsError> {
        let pending = match self {
            Self::Tcp(c) => &mut c.pending,
            Self::Udp(c) => &mut c.pending,
        };
        if bytes.len().saturating_add(pending.len()) > MAX_PENDING_BYTES {
            return Err(ImsError::new("ims_channel_frame_too_large"));
        }
        pending.splice(..0, bytes);
        Ok(())
    }

    pub fn outbound_registered(
        &mut self,
        response: &[u8],
        expires: u32,
    ) -> Result<RegisterArtifacts, ImsError> {
        match self {
            Self::Tcp(c) => c.outbound.registered(response, true, expires),
            Self::Udp(c) => c.outbound.registered(response, false, expires),
        }
    }

    pub fn outbound_active(&self) -> bool {
        match self {
            Self::Tcp(c) => c.outbound.active(),
            Self::Udp(c) => c.outbound.active(),
        }
    }

    pub async fn send_all(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        self.send_sip(frame).await
    }

    /// Chunked read used by the live buffered-framing helpers. For UDP the
    /// remainder of an oversized datagram is buffered inside the channel and
    /// drained on the next call.
    pub async fn recv_chunk(&mut self, buf: &mut [u8]) -> Result<usize, ImsError> {
        match self {
            Self::Tcp(channel) => channel
                .stream
                .read(buf)
                .await
                .map_err(|_| ImsError::new("ims_channel_read_failed")),
            Self::Udp(channel) => channel.recv_chunk(buf).await,
        }
    }

    /// Whether the underlying transport is a byte stream (TCP). UDP returns
    /// false so read loops treat a zero-length datagram as a keepalive instead
    /// of end-of-stream.
    pub fn is_tcp(&self) -> bool {
        matches!(self, Self::Tcp(_))
    }

    pub fn route(&self) -> ImsRoute {
        match self {
            Self::Tcp(channel) => channel.route(),
            Self::Udp(channel) => channel.route(),
        }
    }

    pub async fn send_keepalive(&mut self) -> Result<(), ImsError> {
        match self {
            Self::Tcp(channel) => channel.send_keepalive().await,
            Self::Udp(channel) => channel.send_keepalive().await,
        }
    }

    /// Close the transport side of an in-progress exchange. For TCP this sends
    /// FIN before the protected leg takes over; UDP has no connection state.
    pub async fn shutdown(&mut self) -> Result<(), ImsError> {
        match self {
            Self::Tcp(channel) => channel
                .stream
                .shutdown()
                .await
                .map_err(|_| ImsError::new("ims_channel_shutdown_failed")),
            Self::Udp(_) => Ok(()),
        }
    }

    /// Tear down the socket immediately (RST for TCP, drop for UDP).
    pub fn abort(self) {
        match self {
            Self::Tcp(channel) => abort_tcp(channel.stream),
            Self::Udp(_) => drop(self),
        }
    }
}

impl ImsChannel for SipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        match self {
            Self::Tcp(channel) => channel.send_sip(frame).await,
            Self::Udp(channel) => channel.send_sip(frame).await,
        }
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        match self {
            Self::Tcp(channel) => channel.recv_sip(timeout).await,
            Self::Udp(channel) => channel.recv_sip(timeout).await,
        }
    }

    async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        match self {
            Self::Tcp(channel) => channel.recv_sip_fresh(timeout).await,
            Self::Udp(channel) => channel.recv_sip_fresh(timeout).await,
        }
    }

    fn requeue(&mut self, frame: Vec<u8>) {
        match self {
            Self::Tcp(channel) => channel.requeue(frame),
            Self::Udp(channel) => channel.requeue(frame),
        }
    }

    fn route(&self) -> ImsRoute {
        match self {
            Self::Tcp(channel) => channel.route(),
            Self::Udp(channel) => channel.route(),
        }
    }

    fn security_verify(&self) -> Option<&str> {
        match self {
            Self::Tcp(channel) => channel.security_verify(),
            Self::Udp(channel) => channel.security_verify(),
        }
    }
}

/// Merge complete frames that a REGISTER transaction set aside into the
/// transport's byte buffer so `into_parts` never loses them. Requeued frames
/// are prepended in arrival order ahead of any partial transport bytes.
fn merge_requeued(mut pending: Vec<u8>, requeued: ImsRequeue) -> Vec<u8> {
    for frame in requeued.into_frames().into_iter().rev() {
        pending.splice(..0, frame);
    }
    pending
}

fn retain_requeued_frame(requeued: &mut ImsRequeue, frame: Vec<u8>, transport: &'static str) {
    let frame_bytes = frame.len();
    if !requeued.push_back(frame) {
        tracing::warn!(
            transport,
            frame_bytes,
            queued_frames = requeued.len(),
            queued_bytes = requeued.bytes(),
            "VoWiFi SIP requeue full; dropping newest frame"
        );
    }
}

fn abort_tcp(stream: TcpStream) {
    #[cfg(unix)]
    {
        use std::mem;
        use std::os::fd::AsRawFd;

        let linger = libc::linger {
            l_onoff: 1,
            l_linger: 0,
        };
        unsafe {
            let _ = libc::setsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_LINGER,
                &linger as *const _ as *const libc::c_void,
                mem::size_of::<libc::linger>() as libc::socklen_t,
            );
        }
    }
    drop(stream);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::context::SipTransport;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    const REQUEUED_FIRST: &[u8] =
        b"NOTIFY sip:user@ims.example SIP/2.0\r\nContent-Length: 0\r\n\r\n";
    const REQUEUED_SECOND: &[u8] =
        b"MESSAGE sip:user@ims.example SIP/2.0\r\nContent-Length: 0\r\n\r\n";
    const PARTIAL_PREFIX: &[u8] =
        b"INVITE sip:user@ims.example SIP/2.0\r\nContent-Length: 4\r\n\r\nbo";
    const PARTIAL_SUFFIX: &[u8] = b"dy";

    async fn udp_pair() -> (UdpSocket, UdpSocket, SocketAddr) {
        let peer = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let local = UdpSocket::bind(("127.0.0.1", 0)).await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        local.connect(peer_addr).await.unwrap();
        (peer, local, peer_addr)
    }

    fn udp_route(local: SocketAddr, remote: SocketAddr) -> ImsRoute {
        ImsRoute {
            local_addr: local,
            pcscf_addr: remote,
            transport: SipTransport::Udp,
        }
    }

    fn expected_merged_pending() -> Vec<u8> {
        [REQUEUED_FIRST, REQUEUED_SECOND, PARTIAL_PREFIX].concat()
    }

    #[tokio::test]
    async fn outbound_udp_server_tuple_pong_is_consumed_without_corrupting_sip() {
        use crate::connectivity::core::{
            ims_access::ConcurrentRegistrationSupport,
            ims_registration_coordinator,
            outbound::tests::{register_request, register_success, success, INSTANCE},
        };
        let (send_peer, send, send_peer_addr) = udp_pair().await;
        let (receive_peer, receive, _) = udp_pair().await;
        let receive_addr = receive.local_addr().unwrap();
        let local = send.local_addr().unwrap();
        let mut channel = UdpSipChannel::new_with_receive_socket(
            send,
            receive,
            Vec::new(),
            udp_route(local, send_peer_addr),
            Some("ipsec-3gpp".into()),
        );
        let coordinator = ims_registration_coordinator::for_line("outbound-vowifi-udp");
        channel
            .outbound
            .configure("outbound-vowifi-udp", ImsAccess::Wlan, INSTANCE, true);
        channel
            .send_sip(&register_request("wlan", 1))
            .await
            .unwrap();
        let mut buf = [0u8; 2048];
        let (n, _) = send_peer.recv_from(&mut buf).await.unwrap();
        receive_peer
            .send_to(&register_success(&buf[..n], 1800), receive_addr)
            .await
            .unwrap();
        let response = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        channel.outbound.registered(&response, false, 3600).unwrap();
        let responder = tokio::spawn(async move {
            let mut packet = [0u8; 64];
            let (n, peer) = send_peer.recv_from(&mut packet).await.unwrap();
            assert_eq!(n, 28);
            let id: [u8; 12] = packet[8..20].try_into().unwrap();
            let mut wrong_id = id;
            wrong_id[0] ^= 1;
            receive_peer
                .send_to(&success(wrong_id, peer), receive_addr)
                .await
                .unwrap();
            receive_peer
                .send_to(&success(id, peer), receive_addr)
                .await
                .unwrap();
            receive_peer
                .send_to(REQUEUED_FIRST, receive_addr)
                .await
                .unwrap();
        });
        assert_eq!(
            channel.recv_sip(Duration::from_secs(2)).await.unwrap(),
            REQUEUED_FIRST
        );
        responder.await.unwrap();
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );
    }

    #[tokio::test]
    async fn outbound_tcp_split_pong_preserves_coalesced_sip_frames() {
        use crate::connectivity::core::{
            ims_access::ConcurrentRegistrationSupport,
            ims_registration_coordinator,
            outbound::tests::{register_request, register_success, INSTANCE},
        };
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stream, accepted) = tokio::join!(TcpStream::connect(addr), listener.accept());
        let stream = stream.unwrap();
        let (mut peer, _) = accepted.unwrap();
        let route = ImsRoute {
            local_addr: stream.local_addr().unwrap(),
            pcscf_addr: addr,
            transport: SipTransport::Tcp,
        };
        let mut channel = EpdgSipChannel::new(stream, Vec::new(), route, None);
        let coordinator = ims_registration_coordinator::for_line("outbound-vowifi-tcp");
        channel
            .outbound
            .configure("outbound-vowifi-tcp", ImsAccess::Wlan, INSTANCE, true);
        channel.send_sip(&register_request("tcp", 1)).await.unwrap();
        let mut request = Vec::new();
        while sip_frame::complete_frame_len(&request).is_none() {
            let mut chunk = [0u8; 512];
            let n = peer.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&chunk[..n]);
        }
        peer.write_all(&register_success(&request, 1800))
            .await
            .unwrap();
        let response = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        channel.outbound.registered(&response, true, 3600).unwrap();
        let responder = tokio::spawn(async move {
            let mut ping = [0u8; 4];
            peer.read_exact(&mut ping).await.unwrap();
            assert_eq!(&ping, b"\r\n\r\n");
            peer.write_all(b"\r").await.unwrap();
            tokio::task::yield_now().await;
            peer.write_all(&[b"\n".as_slice(), REQUEUED_FIRST, REQUEUED_SECOND].concat())
                .await
                .unwrap();
        });
        assert_eq!(
            channel.recv_sip(Duration::from_secs(2)).await.unwrap(),
            REQUEUED_FIRST
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_SECOND
        );
        responder.await.unwrap();
        assert_eq!(
            coordinator.concurrent_support(),
            ConcurrentRegistrationSupport::Negotiated
        );
    }

    #[tokio::test]
    async fn tcp_into_parts_preserves_requeue_fifo_and_partial_frame() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let listener_addr = listener.local_addr().unwrap();
        let connect = TcpStream::connect(listener_addr);
        let (accepted, client) = tokio::join!(listener.accept(), connect);
        let (mut peer, _) = accepted.unwrap();
        let client = client.unwrap();
        let route = ImsRoute {
            local_addr: client.local_addr().unwrap(),
            pcscf_addr: listener_addr,
            transport: SipTransport::Tcp,
        };
        let mut channel = EpdgSipChannel::new(client, PARTIAL_PREFIX.to_vec(), route, None);
        channel.requeue(REQUEUED_FIRST.to_vec());
        channel.requeue(REQUEUED_SECOND.to_vec());

        let (stream, pending) = channel.into_parts();
        assert_eq!(pending, expected_merged_pending());
        let mut channel = EpdgSipChannel::new(stream, pending, route, None);
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_FIRST
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_SECOND
        );
        peer.write_all(PARTIAL_SUFFIX).await.unwrap();
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            [PARTIAL_PREFIX, PARTIAL_SUFFIX].concat()
        );
    }

    #[tokio::test]
    async fn udp_into_parts_preserves_requeue_fifo_and_partial_frame() {
        let (peer, local, peer_addr) = udp_pair().await;
        let local_addr = local.local_addr().unwrap();
        let route = udp_route(local_addr, peer_addr);
        let mut channel = UdpSipChannel::new(local, PARTIAL_PREFIX.to_vec(), route, None);
        channel.requeue(REQUEUED_FIRST.to_vec());
        channel.requeue(REQUEUED_SECOND.to_vec());

        let (socket, receive, pending) = channel.into_parts();
        assert!(receive.is_none());
        assert_eq!(pending, expected_merged_pending());
        let mut channel = UdpSipChannel::new(socket, pending, route, None);
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_FIRST
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_SECOND
        );
        peer.send_to(PARTIAL_SUFFIX, local_addr).await.unwrap();
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            [PARTIAL_PREFIX, PARTIAL_SUFFIX].concat()
        );
    }

    #[tokio::test]
    async fn protected_udp_into_parts_preserves_requeue_fifo_and_partial_frame() {
        let (_send_peer, send, send_peer_addr) = udp_pair().await;
        let (receive_peer, receive, _) = udp_pair().await;
        let send_local_addr = send.local_addr().unwrap();
        let receive_local_addr = receive.local_addr().unwrap();
        let route = udp_route(send_local_addr, send_peer_addr);
        let mut channel = UdpSipChannel::new_with_receive_socket(
            send,
            receive,
            PARTIAL_PREFIX.to_vec(),
            route,
            Some("ipsec-3gpp".to_string()),
        );
        channel.requeue(REQUEUED_FIRST.to_vec());
        channel.requeue(REQUEUED_SECOND.to_vec());

        let (send, receive, pending) = channel.into_parts();
        assert_eq!(pending, expected_merged_pending());
        let mut channel = UdpSipChannel::new_with_receive_socket(
            send,
            receive.expect("protected receive socket"),
            pending,
            route,
            Some("ipsec-3gpp".to_string()),
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_FIRST
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            REQUEUED_SECOND
        );
        receive_peer
            .send_to(PARTIAL_SUFFIX, receive_local_addr)
            .await
            .unwrap();
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            [PARTIAL_PREFIX, PARTIAL_SUFFIX].concat()
        );
    }

    #[tokio::test]
    async fn udp_channel_round_trips_sip_frames() {
        let (peer, local, peer_addr) = udp_pair().await;
        let local_addr = local.local_addr().unwrap();
        let mut channel =
            UdpSipChannel::new(local, Vec::new(), udp_route(local_addr, peer_addr), None);

        let request =
            b"REGISTER sip:example.com SIP/2.0\r\nVia: SIP/2.0/UDP 192.0.2.1:5060\r\nContent-Length: 0\r\n\r\n";
        channel.send_sip(request).await.unwrap();

        let mut buf = vec![0u8; 4096];
        let (len, from) = peer.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..len], request);

        let response =
            b"SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"ims.example\"\r\nContent-Length: 0\r\n\r\n";
        peer.send_to(response, from).await.unwrap();
        let got = channel.recv_sip(Duration::from_secs(2)).await.unwrap();
        assert_eq!(&got[..], response);
    }

    /// Hangs forever under WSL2, so it is gated rather than left to wedge the
    /// suite. The point of the test is a datagram larger than the MTU, and on
    /// kernel 6.18.33.2-microsoft-standard-WSL2 a UDP datagram that needs IP
    /// fragmentation is never delivered over loopback. Measured with a plain
    /// Python socket and no SimAdmin code involved: 1024 and 1472 bytes arrive,
    /// 2048 and above never do. 1472 is exactly 1500 - 28 (IP 20 + UDP 8), so
    /// the cutoff is fragmentation, not a buffer limit (`SO_RCVBUF` is 212992).
    ///
    /// `recv_chunk` itself is correct: a 2940-byte frame read in 1024-byte
    /// chunks drains as 1024/1024/892, and the real IMS path on the 410 does
    /// reassemble oversized responses.
    /// Run it on real Linux with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "loopback UDP over the MTU is never delivered under WSL2"]
    async fn udp_channel_recv_chunk_reassembles_oversized_datagram() {
        let (peer, local, peer_addr) = udp_pair().await;
        let local_addr = local.local_addr().unwrap();
        let mut channel =
            UdpSipChannel::new(local, Vec::new(), udp_route(local_addr, peer_addr), None);

        // A 3 KiB response with a real body and Content-Length.
        let mut frame = b"SIP/2.0 200 OK\r\nContent-Length: 2900\r\n\r\n".to_vec();
        frame.extend(std::iter::repeat(b'x').take(2900));
        peer.send_to(&frame, local_addr).await.unwrap();

        let mut collected = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = channel.recv_chunk(&mut chunk).await.unwrap();
            collected.extend_from_slice(&chunk[..n]);
            if collected.len() >= frame.len() {
                break;
            }
        }
        assert_eq!(collected, frame);
    }

    #[tokio::test]
    async fn protected_udp_pair_keeps_server_flow_for_inbound_sip() {
        let (client_peer, client_socket, client_peer_addr) = udp_pair().await;
        let (server_peer, server_socket, _) = udp_pair().await;
        let client_local_addr = client_socket.local_addr().unwrap();
        let server_local_addr = server_socket.local_addr().unwrap();
        let mut channel = UdpSipChannel::new_with_receive_socket(
            client_socket,
            server_socket,
            Vec::new(),
            udp_route(client_local_addr, client_peer_addr),
            Some("ipsec-3gpp".to_string()),
        );

        let options = b"OPTIONS sip:ims.example SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        channel.send_sip(options).await.unwrap();
        let mut scratch = vec![0u8; 4096];
        let (len, _) = client_peer.recv_from(&mut scratch).await.unwrap();
        assert_eq!(&scratch[..len], options);

        let incoming = b"MESSAGE sip:user@ims.example SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        server_peer
            .send_to(incoming, server_local_addr)
            .await
            .unwrap();
        assert_eq!(
            channel.recv_sip(Duration::from_secs(2)).await.unwrap(),
            incoming
        );

        let response = b"SIP/2.0 200 OK\r\nContent-Length: 0\r\n\r\n";
        channel.send_sip(response).await.unwrap();
        let (len, _) = server_peer.recv_from(&mut scratch).await.unwrap();
        assert_eq!(&scratch[..len], response);

        let (_, receive_socket, pending) = channel.into_parts();
        assert!(receive_socket.is_some());
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn sip_channel_enum_dispatches_udp_and_keepalive_is_noop() {
        let (peer, local, peer_addr) = udp_pair().await;
        let local_addr = local.local_addr().unwrap();
        let mut channel = SipChannel::new(
            SipChannelSocket::Udp(local),
            Vec::new(),
            udp_route(local_addr, peer_addr),
            Some("ipsec-3gpp".to_string()),
        );
        assert!(!channel.is_tcp());
        assert_eq!(channel.route().transport, SipTransport::Udp);
        assert_eq!(channel.security_verify(), Some("ipsec-3gpp"));
        channel.send_keepalive().await.unwrap();

        let frame = b"SIP/2.0 200 OK\r\nContent-Length: 0\r\n\r\n";
        peer.send_to(frame, local_addr).await.unwrap();
        let got = channel.recv_sip(Duration::from_secs(2)).await.unwrap();
        assert_eq!(&got[..], frame);

        let (socket_out, pending) = channel.into_parts();
        assert!(pending.is_empty());
        assert!(matches!(socket_out, SipChannelSocket::Udp(_)));
    }
}
