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

use crate::connectivity::core::{access::ImsChannel, context::ImsRoute, sip_frame, ImsError};

const MAX_PENDING_BYTES: usize = 64 * 1024;

pub struct EpdgSipChannel {
    stream: TcpStream,
    pending: Vec<u8>,
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
            route,
            security_verify,
        }
    }

    pub fn into_parts(self) -> (TcpStream, Vec<u8>) {
        (self.stream, self.pending)
    }

    pub async fn send_keepalive(&mut self) -> Result<(), ImsError> {
        self.stream
            .write_all(b"\r\n\r\n")
            .await
            .map_err(|_| ImsError::new("ims_channel_keepalive_write_failed"))?;
        self.stream
            .flush()
            .await
            .map_err(|_| ImsError::new("ims_channel_keepalive_flush_failed"))
    }

    fn discard_keepalive_frames(&mut self) {
        while self.pending.starts_with(b"\r\n") {
            self.pending.drain(..2);
        }
        while self.pending.starts_with(b"\n") {
            self.pending.drain(..1);
        }
    }
}

impl ImsChannel for EpdgSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
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
        self.discard_keepalive_frames();
        if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
            return Ok(self.pending.drain(..frame_len).collect());
        }

        tokio::time::timeout(timeout, async {
            loop {
                let mut chunk = [0u8; 2048];
                let read = self
                    .stream
                    .read(&mut chunk)
                    .await
                    .map_err(|_| ImsError::new("ims_channel_read_failed"))?;
                if read == 0 {
                    return Err(ImsError::new("ims_channel_closed"));
                }
                self.pending.extend_from_slice(&chunk[..read]);
                self.discard_keepalive_frames();
                if self.pending.len() > MAX_PENDING_BYTES {
                    return Err(ImsError::new("ims_channel_frame_too_large"));
                }
                if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
                    return Ok(self.pending.drain(..frame_len).collect());
                }
            }
        })
        .await
        .map_err(|_| ImsError::new("ims_channel_read_timeout"))?
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
            route,
            security_verify,
        }
    }

    pub fn into_parts(self) -> (UdpSocket, Vec<u8>) {
        // The dedicated receive socket is only needed while the protected
        // transaction is in flight; after registration the channel is kept for
        // outbound MESSAGE/INVITE traffic and the response path is not used.
        drop(self.receive_socket);
        (self.socket, self.pending)
    }

    pub async fn send_keepalive(&mut self) -> Result<(), ImsError> {
        // UDP has no CRLF keepalive. NAT binding is kept fresh by the SIP
        // OPTIONS ping timer, so a no-op is the safe behaviour here.
        Ok(())
    }

    fn discard_keepalive_frames(&mut self) {
        while self.pending.starts_with(b"\r\n") {
            self.pending.drain(..2);
        }
        while self.pending.starts_with(b"\n") {
            self.pending.drain(..1);
        }
    }

    /// Chunked read for the live flows. A UDP datagram may exceed the caller's
    /// chunk size; the remainder is buffered internally and drained on the next
    /// call so no bytes are lost.
    async fn recv_chunk(&mut self, buf: &mut [u8]) -> Result<usize, ImsError> {
        if !self.pending.is_empty() {
            let take = buf.len().min(self.pending.len());
            buf[..take].copy_from_slice(&self.pending[..take]);
            self.pending.drain(..take);
            return Ok(take);
        }
        let mut scratch = vec![0u8; MAX_PENDING_BYTES];
        let socket = self.receive_socket.as_ref().unwrap_or(&self.socket);
        let read = socket
            .recv(&mut scratch)
            .await
            .map_err(|_| ImsError::new("ims_channel_read_failed"))?;
        if read <= buf.len() {
            buf[..read].copy_from_slice(&scratch[..read]);
            Ok(read)
        } else {
            buf.copy_from_slice(&scratch[..buf.len()]);
            self.pending.extend_from_slice(&scratch[buf.len()..read]);
            Ok(buf.len())
        }
    }
}

impl ImsChannel for UdpSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        self.socket
            .send(frame)
            .await
            .map(|_| ())
            .map_err(|_| ImsError::new("ims_channel_write_failed"))
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.discard_keepalive_frames();
        if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
            return Ok(self.pending.drain(..frame_len).collect());
        }

        tokio::time::timeout(timeout, async {
            loop {
                let mut scratch = vec![0u8; MAX_PENDING_BYTES];
                let read = self
                    .socket
                    .recv(&mut scratch)
                    .await
                    .map_err(|_| ImsError::new("ims_channel_read_failed"))?;
                self.pending.extend_from_slice(&scratch[..read]);
                self.discard_keepalive_frames();
                if self.pending.len() > MAX_PENDING_BYTES {
                    return Err(ImsError::new("ims_channel_frame_too_large"));
                }
                if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
                    return Ok(self.pending.drain(..frame_len).collect());
                }
            }
        })
        .await
        .map_err(|_| ImsError::new("ims_channel_read_timeout"))?
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
        }
    }

    pub fn abort(self) {
        match self {
            Self::Tcp(stream) => abort_tcp(stream),
            Self::Udp(_) => drop(self),
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
                let (socket, pending) = channel.into_parts();
                (SipChannelSocket::Udp(socket), pending)
            }
        }
    }

    pub async fn send_all(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        match self {
            Self::Tcp(channel) => {
                channel
                    .stream
                    .write_all(frame)
                    .await
                    .map_err(|_| ImsError::new("ims_channel_write_failed"))?;
                channel
                    .stream
                    .flush()
                    .await
                    .map_err(|_| ImsError::new("ims_channel_flush_failed"))
            }
            Self::Udp(channel) => channel
                .socket
                .send(frame)
                .await
                .map(|_| ())
                .map_err(|_| ImsError::new("ims_channel_write_failed")),
        }
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

    #[tokio::test]
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
