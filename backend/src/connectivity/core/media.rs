//! Shared IMS media-plane primitives (RTP relay core + UDP lifecycle).
//!
//! Clean-room from RFC 3550 (RTP). The target device has no audio/video
//! capture, so media is never decoded locally — it is relayed at the packet
//! level between two endpoints:
//!   - the operator IMS media endpoint (negotiated via the SDP), and
//!   - the internal SIP UA (Linphone/Asterisk) media endpoint.
//!
//! This module provides the transport-agnostic relay *logic* (endpoint binding,
//! peer learning, forward-direction decision, packet/byte counters) as pure,
//! offline-testable code. The actual blocking UDP socket loop is a thin
//! `#[cfg(unix)]` layer that plugs this logic onto real sockets; it is compiled
//! out on Windows so the logic can still be unit-tested there.
//!
//! The relay has both a pure decision core and a Tokio UDP lifecycle used by
//! live Trunk dialogs. Transcoding (AMR ↔ G.711/Opus), RTCP interpretation and
//! jitter buffering remain out of scope: Asterisk owns media conversion while
//! the device forwards RTP plus strictly-framed RTCP-mux packets and rewrites
//! negotiated dynamic RTP payload types.
//!
//! This is access-agnostic: both the VoLTE/ViLTE live adapter and the VoWiFi
//! live adapter drive relays of this type against the same Trunk seam.

use std::{
    future::Future,
    io as std_io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::{net::UdpSocket, sync::watch, task::JoinHandle};

use crate::connectivity::core::voice::{MediaDirection, RtpPacket};

/// Per-relay media-plane metrics collected by a live adapter. Implemented by the
/// Trunk operator metrics so `core` never depends on the services layer.
pub trait MediaRelayMetrics: Send + Sync {
    fn relay_started(&self);
    fn relay_stopped(&self);
    fn record_rtp_to_asterisk(&self, bytes: usize);
    fn record_rtp_from_asterisk(&self, bytes: usize);
}

/// Which leg a datagram arrived on. The relay forwards A->B and B->A.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayLeg {
    /// Operator IMS media side (the SDP-negotiated RTP endpoint).
    Operator,
    /// Internal SIP UA side (Linphone/Asterisk).
    Internal,
}

/// RTP forwarding permissions negotiated for one relay. RTCP-mux remains
/// transparent in both directions, while RTP is gated by offer/answer media
/// directions so `sendonly`, `recvonly`, and `inactive` actually hold media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaRelayPolicy {
    pub operator_to_internal_rtp: bool,
    pub internal_to_operator_rtp: bool,
}

impl MediaRelayPolicy {
    pub const fn bidirectional() -> Self {
        Self {
            operator_to_internal_rtp: true,
            internal_to_operator_rtp: true,
        }
    }

    /// Build the allowed media flows from the two local SDP directions. Each
    /// direction is expressed from that endpoint's point of view.
    pub const fn from_directions(operator: MediaDirection, internal: MediaDirection) -> Self {
        Self {
            operator_to_internal_rtp: operator.allows_send() && internal.allows_receive(),
            internal_to_operator_rtp: internal.allows_send() && operator.allows_receive(),
        }
    }

    const fn allows_rtp_from(self, leg: RelayLeg) -> bool {
        match leg {
            RelayLeg::Operator => self.operator_to_internal_rtp,
            RelayLeg::Internal => self.internal_to_operator_rtp,
        }
    }
}

/// Datagram framing accepted by a relay. RTCP is only supported when it is
/// multiplexed on the same UDP port as RTP; the SDP and socket lifecycle do
/// not allocate separate RTCP ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDatagramKind {
    Rtp,
    RtcpMux,
    /// Used only by the explicit diagnostic/test escape hatch
    /// [`RtpRelayCore::with_require_rtp`]. Production relays leave strict
    /// framing enabled and never emit this variant.
    Opaque,
}

impl RelayLeg {
    /// The opposite leg (where a datagram received here must be forwarded).
    pub fn peer(self) -> RelayLeg {
        match self {
            RelayLeg::Operator => RelayLeg::Internal,
            RelayLeg::Internal => RelayLeg::Operator,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RelayLeg::Operator => "operator",
            RelayLeg::Internal => "internal",
        }
    }
}

/// One relayed leg's addressing + learned remote peer.
#[derive(Debug, Clone)]
pub struct LegEndpoint {
    /// Where we expect this leg's media to come from / go to (from SDP).
    /// May be updated by symmetric-RTP learning when the observed source
    /// differs (common behind NAT).
    pub remote: Option<SocketAddr>,
    /// Whether to trust and latch the first observed source address
    /// (symmetric RTP / latching, RFC 4961 spirit).
    pub latch: bool,
    pub packets: u64,
    pub bytes: u64,
}

impl LegEndpoint {
    pub fn new(remote: Option<SocketAddr>, latch: bool) -> Self {
        Self {
            remote,
            latch,
            packets: 0,
            bytes: 0,
        }
    }
}

/// A forwarding decision produced by the pure relay core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardDecision {
    /// Leg to send the datagram out on.
    pub to: RelayLeg,
    /// Destination address (the peer leg's learned or configured remote).
    pub dest: SocketAddr,
    /// Framing of the forwarded datagram. Only RTP packets can have their
    /// payload type rewritten.
    pub kind: MediaDatagramKind,
    /// Optional RTP payload type expected by the destination leg. This is used
    /// for dynamic codecs such as RFC 4733 telephone-event when the two SIP
    /// dialogs negotiated different payload numbers.
    pub rewrite_payload_type: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadTypeMapping {
    pub operator: u8,
    pub internal: u8,
}

/// Errors from the relay core (pure logic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayError {
    /// The peer leg has no known destination yet (no SDP remote, no latch).
    PeerAddressUnknown,
    /// The datagram was neither plausible RTP nor a complete RTCP-mux packet.
    NotRtp,
}

/// The transport-agnostic RTP/RTCP-mux relay core: two legs, symmetric
/// forwarding, peer learning, and counters. No sockets — feed it `(leg, src,
/// datagram)` and it tells you where to forward.
#[derive(Debug, Clone)]
pub struct RtpRelayCore {
    operator: LegEndpoint,
    internal: LegEndpoint,
    /// If true, only forward RTP v2 or complete RTCP-mux packets (drop
    /// stray/noise).
    require_rtp: bool,
    payload_type_mappings: Vec<PayloadTypeMapping>,
}

impl RtpRelayCore {
    pub fn new(operator: LegEndpoint, internal: LegEndpoint) -> Self {
        Self {
            operator,
            internal,
            require_rtp: true,
            payload_type_mappings: Vec::new(),
        }
    }

    /// Allow forwarding arbitrary datagrams without framing validation.
    ///
    /// Production relays keep the default strict behavior, which accepts RTP
    /// and valid RTCP-mux compound packets. This is retained only for explicit
    /// diagnostic/test use; it must not be used to emulate a separate RTCP
    /// port, because this relay does not allocate one.
    pub fn with_require_rtp(mut self, require: bool) -> Self {
        self.require_rtp = require;
        self
    }

    /// Add a dynamic RTP payload mapping between the operator and internal
    /// dialogs. Marker/sequence/timestamp/SSRC and payload bytes remain intact;
    /// only the 7-bit PT field is rewritten when required.
    pub fn with_payload_type_mapping(mut self, operator: u8, internal: u8) -> Self {
        if operator <= 0x7f && internal <= 0x7f {
            self.payload_type_mappings
                .push(PayloadTypeMapping { operator, internal });
        }
        self
    }

    fn leg(&self, leg: RelayLeg) -> &LegEndpoint {
        match leg {
            RelayLeg::Operator => &self.operator,
            RelayLeg::Internal => &self.internal,
        }
    }

    fn leg_mut(&mut self, leg: RelayLeg) -> &mut LegEndpoint {
        match leg {
            RelayLeg::Operator => &mut self.operator,
            RelayLeg::Internal => &mut self.internal,
        }
    }

    /// The learned/configured remote for a leg.
    pub fn remote(&self, leg: RelayLeg) -> Option<SocketAddr> {
        self.leg(leg).remote
    }

    pub fn counters(&self, leg: RelayLeg) -> (u64, u64) {
        let e = self.leg(leg);
        (e.packets, e.bytes)
    }

    /// Process one inbound datagram received on `leg` from `src`. Returns the
    /// forward decision (where to send it), updating peer-learning + counters.
    ///
    /// Symmetric-RTP latching: if the receiving leg is configured to latch and
    /// its remote is unknown or differs, it learns `src` as that leg's remote
    /// (so return traffic flows back to where media actually came from).
    pub fn ingest(
        &mut self,
        leg: RelayLeg,
        src: SocketAddr,
        datagram: &[u8],
    ) -> Result<ForwardDecision, RelayError> {
        // RFC 5761 RTCP-mux reserves the 192..=223 second-octet range for
        // RTCP. Classify it before RTP parsing: an RTCP report can otherwise
        // superficially resemble an RTP packet with a long payload. We only
        // accept complete RTCP compound packets and pass them through without
        // interpreting or rewriting their contents.
        let kind = if is_rtcp_mux_datagram(datagram) {
            MediaDatagramKind::RtcpMux
        } else if RtpPacket::parse(datagram).is_some() {
            MediaDatagramKind::Rtp
        } else if self.require_rtp {
            return Err(RelayError::NotRtp);
        } else {
            MediaDatagramKind::Opaque
        };

        // Symmetric-RTP learning on the receiving leg.
        {
            let recv = self.leg_mut(leg);
            if recv.latch && recv.remote != Some(src) {
                recv.remote = Some(src);
            }
            recv.packets = recv.packets.saturating_add(1);
            recv.bytes = recv.bytes.saturating_add(datagram.len() as u64);
        }

        let peer = leg.peer();
        let dest = self
            .leg(peer)
            .remote
            .ok_or(RelayError::PeerAddressUnknown)?;
        let rewrite_payload_type = (kind == MediaDatagramKind::Rtp)
            .then(|| datagram[1] & 0x7f)
            .and_then(|current| {
                self.payload_type_mappings
                    .iter()
                    .find_map(|mapping| match leg {
                        RelayLeg::Operator if current == mapping.operator => Some(mapping.internal),
                        RelayLeg::Internal if current == mapping.internal => Some(mapping.operator),
                        _ => None,
                    })
            });
        Ok(ForwardDecision {
            to: peer,
            dest,
            kind,
            rewrite_payload_type,
        })
    }
}

/// Return whether `datagram` is a complete RFC 5761 RTCP-mux packet or
/// compound packet. The relay does not interpret RTCP reports; validating the
/// version, reserved packet-type range, and every length field is sufficient
/// to keep malformed traffic from being relayed as control-plane media.
fn is_rtcp_mux_datagram(datagram: &[u8]) -> bool {
    let mut offset = 0usize;
    while offset < datagram.len() {
        let Some(header) = datagram.get(offset..offset.saturating_add(4)) else {
            return false;
        };
        if header[0] >> 6 != 2 || !(192..=223).contains(&header[1]) {
            return false;
        }
        let words = usize::from(u16::from_be_bytes([header[2], header[3]]));
        let Some(packet_len) = words.checked_add(1).and_then(|words| words.checked_mul(4)) else {
            return false;
        };
        let Some(next) = offset.checked_add(packet_len) else {
            return false;
        };
        if next > datagram.len() {
            return false;
        }
        offset = next;
    }
    !datagram.is_empty()
}

pub fn rewrite_rtp_payload_type(datagram: &[u8], payload_type: u8) -> Option<Vec<u8>> {
    if payload_type > 0x7f {
        return None;
    }
    RtpPacket::parse(datagram)?;
    let mut rewritten = datagram.to_vec();
    rewritten[1] = (rewritten[1] & 0x80) | payload_type;
    Some(rewritten)
}

/// Bound sockets allocated before SIP offer/answer exchange. Their local
/// addresses are advertised independently to IMS and Asterisk; forwarding is
/// activated only after both remote SDP endpoints are known.
pub struct PendingRtpRelay {
    operator_socket: Arc<UdpSocket>,
    internal_socket: Arc<UdpSocket>,
    /// Keeps a UE worker (if any) alive for the lifetime of the relay so the
    /// operator-side socket stays bound to its namespace-owned fd.
    _operator_creator: Option<Arc<dyn OperatorSocketCreator>>,
}

/// Creates the operator-facing UDP socket on behalf of a media relay.
///
/// The host path binds in the current namespace; the per-UE isolation path
/// asks the line's UE worker to create the socket inside the UE network
/// namespace and returns the fd. Keeping the creator in the relay prevents
/// the worker handle from being dropped while the socket is still in use.
pub trait OperatorSocketCreator: Send + Sync {
    fn create_udp<'a>(
        &'a self,
        local: SocketAddr,
        bind_to_device: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = std_io::Result<UdpSocket>> + Send + 'a>>;
}

impl PendingRtpRelay {
    pub async fn bind(operator_ip: IpAddr, internal_ip: IpAddr) -> std_io::Result<Self> {
        Self::bind_with_operator_interface(operator_ip, internal_ip, None).await
    }

    /// Bind the operator-facing socket to the access interface as well as its
    /// local address. The address alone is not a unique selector when two
    /// modem interfaces receive the same private IP, so Linux must carry the
    /// interface identity on the socket itself.
    pub async fn bind_with_operator_interface(
        operator_ip: IpAddr,
        internal_ip: IpAddr,
        operator_interface: Option<&str>,
    ) -> std_io::Result<Self> {
        Self::bind_with_operator_source(operator_ip, internal_ip, operator_interface, None).await
    }

    /// Bind the operator-facing socket either in this namespace or inside the
    /// UE namespace through `operator_creator`. The internal (Asterisk/Trunk)
    /// socket always stays in the host namespace.
    pub async fn bind_with_operator_source(
        operator_ip: IpAddr,
        internal_ip: IpAddr,
        operator_interface: Option<&str>,
        operator_creator: Option<Arc<dyn OperatorSocketCreator>>,
    ) -> std_io::Result<Self> {
        let operator_socket = Arc::new(match &operator_creator {
            Some(creator) => {
                creator
                    .create_udp(SocketAddr::new(operator_ip, 0), operator_interface)
                    .await?
            }
            None => bind_udp_socket(SocketAddr::new(operator_ip, 0), operator_interface)?,
        });
        let internal_socket = Arc::new(UdpSocket::bind(SocketAddr::new(internal_ip, 0)).await?);
        Ok(Self {
            operator_socket,
            internal_socket,
            _operator_creator: operator_creator,
        })
    }

    pub fn operator_local_addr(&self) -> std_io::Result<SocketAddr> {
        self.operator_socket.local_addr()
    }

    pub fn internal_local_addr(&self) -> std_io::Result<SocketAddr> {
        self.internal_socket.local_addr()
    }

    pub fn activate(
        self,
        operator_remote: SocketAddr,
        internal_remote: SocketAddr,
        payload_mappings: impl IntoIterator<Item = PayloadTypeMapping>,
    ) -> ActiveRtpRelay {
        self.activate_with_metrics(operator_remote, internal_remote, payload_mappings, None)
    }

    pub fn activate_with_metrics(
        self,
        operator_remote: SocketAddr,
        internal_remote: SocketAddr,
        payload_mappings: impl IntoIterator<Item = PayloadTypeMapping>,
        metrics: Option<Arc<dyn MediaRelayMetrics>>,
    ) -> ActiveRtpRelay {
        self.activate_with_metrics_and_policy(
            operator_remote,
            internal_remote,
            payload_mappings,
            MediaRelayPolicy::bidirectional(),
            metrics,
        )
    }

    pub fn activate_with_metrics_and_policy(
        self,
        operator_remote: SocketAddr,
        internal_remote: SocketAddr,
        payload_mappings: impl IntoIterator<Item = PayloadTypeMapping>,
        policy: MediaRelayPolicy,
        metrics: Option<Arc<dyn MediaRelayMetrics>>,
    ) -> ActiveRtpRelay {
        let mut core = RtpRelayCore::new(
            LegEndpoint::new(Some(operator_remote), true),
            LegEndpoint::new(Some(internal_remote), true),
        );
        for mapping in payload_mappings {
            core = core.with_payload_type_mapping(mapping.operator, mapping.internal);
        }
        let (stop, stop_rx) = watch::channel(false);
        let (first_operator_rtp, first_operator_rtp_rx) = watch::channel(false);
        let task = tokio::spawn(run_async_relay(
            self.operator_socket,
            self.internal_socket,
            core,
            policy,
            stop_rx,
            first_operator_rtp,
            metrics.clone(),
        ));
        if let Some(metrics) = metrics.as_ref() {
            metrics.relay_started();
        }
        ActiveRtpRelay {
            stop,
            first_operator_rtp: first_operator_rtp_rx,
            task,
            metrics,
        }
    }
}

fn bind_udp_socket(local: SocketAddr, interface: Option<&str>) -> std_io::Result<UdpSocket> {
    let socket = Socket::new(Domain::for_address(local), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    bind_socket_to_interface(&socket, interface)?;
    socket.bind(&local.into())?;
    socket.set_nonblocking(true)?;
    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

#[cfg(target_os = "linux")]
fn bind_socket_to_interface(socket: &Socket, interface: Option<&str>) -> std_io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let Some(interface) = interface.filter(|name| !name.trim().is_empty()) else {
        return Ok(());
    };
    let name = CString::new(interface).map_err(|_| {
        std_io::Error::new(std_io::ErrorKind::InvalidInput, "interface contains NUL")
    })?;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.as_bytes_with_nul().len() as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std_io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_socket_to_interface(_socket: &Socket, interface: Option<&str>) -> std_io::Result<()> {
    if interface.is_some() {
        return Err(std_io::Error::new(
            std_io::ErrorKind::Unsupported,
            "SO_BINDTODEVICE is Linux-only",
        ));
    }
    Ok(())
}

pub struct ActiveRtpRelay {
    stop: watch::Sender<bool>,
    first_operator_rtp: watch::Receiver<bool>,
    task: JoinHandle<std_io::Result<()>>,
    metrics: Option<Arc<dyn MediaRelayMetrics>>,
}

impl ActiveRtpRelay {
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Subscribe to the first valid RTP packet observed on the operator leg.
    /// A watch channel retains the event if the packet arrives before the
    /// caller starts waiting.
    pub fn subscribe_first_operator_rtp(&self) -> watch::Receiver<bool> {
        self.first_operator_rtp.clone()
    }
}

impl Drop for ActiveRtpRelay {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        self.task.abort();
        if let Some(metrics) = self.metrics.as_ref() {
            metrics.relay_stopped();
        }
    }
}

async fn run_async_relay(
    operator_socket: Arc<UdpSocket>,
    internal_socket: Arc<UdpSocket>,
    mut core: RtpRelayCore,
    policy: MediaRelayPolicy,
    mut stop: watch::Receiver<bool>,
    first_operator_rtp: watch::Sender<bool>,
    metrics: Option<Arc<dyn MediaRelayMetrics>>,
) -> std_io::Result<()> {
    let mut operator_buf = vec![0u8; 65_535];
    let mut internal_buf = vec![0u8; 65_535];
    let mut operator_send_failed = false;
    let mut internal_send_failed = false;
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Ok(());
                }
            }
            received = operator_socket.recv_from(&mut operator_buf) => {
                let (len, source) = received?;
                match forward_async(
                    &mut core,
                    RelayLeg::Operator,
                    source,
                    &operator_buf[..len],
                    &internal_socket,
                    policy.allows_rtp_from(RelayLeg::Operator),
                )
                .await {
                    Ok(Some(MediaDatagramKind::Rtp)) => {
                        internal_send_failed = false;
                        if let Some(metrics) = metrics.as_ref() {
                            metrics.record_rtp_to_asterisk(len);
                        }
                        let _ = first_operator_rtp.send(true);
                    }
                    Ok(_) => internal_send_failed = false,
                    Err(error) => {
                        if !internal_send_failed {
                            tracing::warn!(
                                from_leg = RelayLeg::Operator.as_str(),
                                %error,
                                "IMS media relay UDP send failed"
                            );
                        }
                        internal_send_failed = true;
                    }
                }
            }
            received = internal_socket.recv_from(&mut internal_buf) => {
                let (len, source) = received?;
                match forward_async(
                    &mut core,
                    RelayLeg::Internal,
                    source,
                    &internal_buf[..len],
                    &operator_socket,
                    policy.allows_rtp_from(RelayLeg::Internal),
                )
                .await {
                    Ok(Some(MediaDatagramKind::Rtp)) => {
                        operator_send_failed = false;
                        if let Some(metrics) = metrics.as_ref() {
                            metrics.record_rtp_from_asterisk(len);
                        }
                    }
                    Ok(_) => operator_send_failed = false,
                    Err(error) => {
                        if !operator_send_failed {
                            tracing::warn!(
                                from_leg = RelayLeg::Internal.as_str(),
                                %error,
                                "IMS media relay UDP send failed"
                            );
                        }
                        operator_send_failed = true;
                    }
                }
            }
        }
    }
}

async fn forward_async(
    core: &mut RtpRelayCore,
    leg: RelayLeg,
    source: SocketAddr,
    datagram: &[u8],
    send_socket: &UdpSocket,
    allow_rtp: bool,
) -> std_io::Result<Option<MediaDatagramKind>> {
    let Ok(decision) = core.ingest(leg, source, datagram) else {
        return Ok(None);
    };
    if decision.kind == MediaDatagramKind::Rtp && !allow_rtp {
        return Ok(None);
    }
    if decision.kind == MediaDatagramKind::Rtp {
        if let Some(payload_type) = decision.rewrite_payload_type {
            if let Some(rewritten) = rewrite_rtp_payload_type(datagram, payload_type) {
                send_socket.send_to(&rewritten, decision.dest).await?;
            }
        } else {
            send_socket.send_to(datagram, decision.dest).await?;
        }
    } else {
        send_socket.send_to(datagram, decision.dest).await?;
    }
    Ok(Some(decision.kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::voice::RtpPacket;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr(last: u8, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)), port)
    }

    fn rtp_datagram() -> Vec<u8> {
        rtp_datagram_with(96, 0xdead_beef)
    }

    fn rtp_datagram_with(payload_type: u8, ssrc: u32) -> Vec<u8> {
        RtpPacket {
            payload_type,
            marker: false,
            sequence: 1,
            timestamp: 160,
            ssrc,
            payload: vec![0xaa, 0xbb, 0xcc],
        }
        .encode()
    }

    /// A minimal RTCP receiver report: V=2, RC=0, PT=201, length=1 word
    /// after the header, followed by the reporter SSRC.
    fn rtcp_receiver_report(ssrc: u32) -> Vec<u8> {
        let mut packet = vec![0x80, 201, 0x00, 0x01];
        packet.extend_from_slice(&ssrc.to_be_bytes());
        packet
    }

    #[test]
    fn leg_peer_is_symmetric() {
        assert_eq!(RelayLeg::Operator.peer(), RelayLeg::Internal);
        assert_eq!(RelayLeg::Internal.peer(), RelayLeg::Operator);
    }

    #[test]
    fn forwards_operator_to_internal_with_known_remotes() {
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(Some(addr(2, 40000)), false),
        );
        let d = relay
            .ingest(RelayLeg::Operator, addr(1, 5004), &rtp_datagram())
            .expect("forward");
        assert_eq!(d.to, RelayLeg::Internal);
        assert_eq!(d.dest, addr(2, 40000));
        assert_eq!(d.rewrite_payload_type, None);
        // Counters incremented on the receiving (operator) leg.
        let (pkts, bytes) = relay.counters(RelayLeg::Operator);
        assert_eq!(pkts, 1);
        assert!(bytes >= 12);
    }

    #[test]
    fn symmetric_latch_learns_source() {
        // Internal remote unknown; latch on so we learn it from first packet.
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(None, true),
        );
        // Operator -> Internal fails first (internal dest unknown).
        let err = relay
            .ingest(RelayLeg::Operator, addr(1, 5004), &rtp_datagram())
            .unwrap_err();
        assert_eq!(err, RelayError::PeerAddressUnknown);
        // Internal sends a packet from an unexpected port; latch learns it.
        let d = relay
            .ingest(RelayLeg::Internal, addr(2, 51234), &rtp_datagram())
            .expect("internal->operator forwards");
        assert_eq!(d.dest, addr(1, 5004));
        assert_eq!(relay.remote(RelayLeg::Internal), Some(addr(2, 51234)));
        // Now operator->internal can forward to the learned address.
        let d2 = relay
            .ingest(RelayLeg::Operator, addr(1, 5004), &rtp_datagram())
            .expect("now forwards");
        assert_eq!(d2.dest, addr(2, 51234));
    }

    #[test]
    fn non_rtp_datagram_rejected_when_required() {
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(Some(addr(2, 40000)), false),
        );
        assert_eq!(
            relay.ingest(RelayLeg::Operator, addr(1, 5004), &[0x00, 0x01]),
            Err(RelayError::NotRtp)
        );
    }

    #[test]
    fn non_rtp_allowed_when_not_required() {
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(Some(addr(2, 40000)), false),
        )
        .with_require_rtp(false);
        // A short (RTCP-like/noise) datagram still forwards.
        let d = relay
            .ingest(
                RelayLeg::Internal,
                addr(2, 40000),
                &[0x80, 0xc8, 0x00, 0x06],
            )
            .expect("forward without rtp check");
        assert_eq!(d.to, RelayLeg::Operator);
        assert_eq!(d.kind, MediaDatagramKind::Opaque);
    }

    #[test]
    fn valid_rtcp_mux_is_transparent_and_never_payload_type_rewritten() {
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(Some(addr(2, 40000)), false),
        )
        .with_payload_type_mapping(96, 101);
        let report = rtcp_receiver_report(0x0102_0304);
        let decision = relay
            .ingest(RelayLeg::Operator, addr(1, 5004), &report)
            .expect("RTCP-mux forwards");
        assert_eq!(decision.kind, MediaDatagramKind::RtcpMux);
        assert_eq!(decision.to, RelayLeg::Internal);
        assert_eq!(decision.dest, addr(2, 40000));
        assert_eq!(decision.rewrite_payload_type, None);
    }

    #[test]
    fn malformed_rtcp_mux_is_rejected_in_strict_mode() {
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(Some(addr(2, 40000)), false),
        );
        // RTCP header says 28 bytes, but only the four-byte header is present.
        assert_eq!(
            relay.ingest(RelayLeg::Operator, addr(1, 5004), &[0x80, 200, 0, 6]),
            Err(RelayError::NotRtp)
        );
    }

    #[test]
    fn telephone_event_payload_type_is_rewritten_between_dialogs() {
        let mut relay = RtpRelayCore::new(
            LegEndpoint::new(Some(addr(1, 5004)), false),
            LegEndpoint::new(Some(addr(2, 40000)), false),
        )
        .with_payload_type_mapping(96, 101);
        let mut packet = RtpPacket {
            payload_type: 101,
            marker: true,
            sequence: 9,
            timestamp: 800,
            ssrc: 0x1234,
            // RFC 4733 event=5, end=0, volume=10, duration=160 samples.
            payload: vec![5, 10, 0, 160],
        }
        .encode();
        let decision = relay
            .ingest(RelayLeg::Internal, addr(2, 40000), &packet)
            .unwrap();
        assert_eq!(decision.rewrite_payload_type, Some(96));
        packet = rewrite_rtp_payload_type(&packet, 96).unwrap();
        assert_eq!(packet[1], 0x80 | 96, "marker bit must be preserved");
        assert!(rewrite_rtp_payload_type(&packet, 128).is_none());
        assert_eq!(
            RtpPacket::parse(&packet).unwrap().payload,
            vec![5, 10, 0, 160]
        );
    }

    #[tokio::test]
    async fn async_relay_forwards_both_legs_and_rewrites_payload_type() {
        let operator_remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let internal_remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let pending = PendingRtpRelay::bind(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await
        .unwrap();
        let operator_local = pending.operator_local_addr().unwrap();
        let internal_local = pending.internal_local_addr().unwrap();
        let relay = pending.activate(
            operator_remote.local_addr().unwrap(),
            internal_remote.local_addr().unwrap(),
            [PayloadTypeMapping {
                operator: 96,
                internal: 101,
            }],
        );
        let mut first_operator_rtp = relay.subscribe_first_operator_rtp();

        let mut packet = rtp_datagram();
        packet[1] = 96;
        operator_remote
            .send_to(&packet, operator_local)
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            first_operator_rtp.changed(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(*first_operator_rtp.borrow());
        let mut received = [0u8; 256];
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            internal_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(received[1] & 0x7f, 101);
        assert_eq!(&received[2..len], &packet[2..]);

        packet[1] = 101;
        internal_remote
            .send_to(&packet, internal_local)
            .await
            .unwrap();
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            operator_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(received[1] & 0x7f, 96);
        assert_eq!(&received[2..len], &packet[2..]);
        relay.stop();
    }

    #[tokio::test]
    async fn direction_policy_holds_rtp_but_keeps_rtcp_mux_available() {
        use crate::connectivity::core::voice::MediaDirection;

        let operator_remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let internal_remote = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let pending = PendingRtpRelay::bind(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await
        .unwrap();
        let operator_local = pending.operator_local_addr().unwrap();
        let internal_local = pending.internal_local_addr().unwrap();
        // The operator offers sendonly while the internal endpoint is
        // recvonly: RTP can travel only from operator to internal.
        let relay = pending.activate_with_metrics_and_policy(
            operator_remote.local_addr().unwrap(),
            internal_remote.local_addr().unwrap(),
            std::iter::empty::<PayloadTypeMapping>(),
            MediaRelayPolicy::from_directions(MediaDirection::SendOnly, MediaDirection::RecvOnly),
            None,
        );
        let packet = rtp_datagram();
        let mut received = [0u8; 256];

        operator_remote
            .send_to(&packet, operator_local)
            .await
            .unwrap();
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            internal_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&received[..len], packet.as_slice());

        internal_remote
            .send_to(&packet, internal_local)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                operator_remote.recv_from(&mut received)
            )
            .await
            .is_err(),
            "held RTP direction must not leak from internal to operator"
        );

        let report = rtcp_receiver_report(0x1234_5678);
        internal_remote
            .send_to(&report, internal_local)
            .await
            .unwrap();
        let (len, _) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            operator_remote.recv_from(&mut received),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&received[..len], report.as_slice());
        relay.stop();
    }

    #[tokio::test]
    async fn independent_relays_isolate_sockets_ssrc_payload_types_and_rtcp_mux() {
        use std::collections::HashSet;
        use std::time::Duration;

        let operator_a = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let internal_a = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let operator_b = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let internal_b = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let pending_a = PendingRtpRelay::bind(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await
        .unwrap();
        let pending_b = PendingRtpRelay::bind(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
        )
        .await
        .unwrap();
        let operator_a_local = pending_a.operator_local_addr().unwrap();
        let internal_a_local = pending_a.internal_local_addr().unwrap();
        let operator_b_local = pending_b.operator_local_addr().unwrap();
        let internal_b_local = pending_b.internal_local_addr().unwrap();
        let local_ports = [
            operator_a_local.port(),
            internal_a_local.port(),
            operator_b_local.port(),
            internal_b_local.port(),
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        assert_eq!(local_ports.len(), 4, "every relay leg needs its own socket");

        let relay_a = pending_a.activate(
            operator_a.local_addr().unwrap(),
            internal_a.local_addr().unwrap(),
            [PayloadTypeMapping {
                operator: 96,
                internal: 101,
            }],
        );
        let relay_b = pending_b.activate(
            operator_b.local_addr().unwrap(),
            internal_b.local_addr().unwrap(),
            [PayloadTypeMapping {
                operator: 97,
                internal: 102,
            }],
        );
        let mut first_operator_rtp = relay_a.subscribe_first_operator_rtp();
        let mut received = [0u8; 256];

        let rtcp_a = rtcp_receiver_report(0x1111_1111);
        operator_a.send_to(&rtcp_a, operator_a_local).await.unwrap();
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(1), internal_a.recv_from(&mut received))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&received[..len], rtcp_a.as_slice());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                internal_b.recv_from(&mut received)
            )
            .await
            .is_err(),
            "RTCP-mux must retain the relay/line boundary"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), first_operator_rtp.changed())
                .await
                .is_err(),
            "RTCP must not trigger an RTP-only answered/media event"
        );

        let rtp_a = rtp_datagram_with(96, 0x1111_1111);
        operator_a.send_to(&rtp_a, operator_a_local).await.unwrap();
        let (_len, _) =
            tokio::time::timeout(Duration::from_secs(1), internal_a.recv_from(&mut received))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(received[1] & 0x7f, 101);
        assert_eq!(
            u32::from_be_bytes(received[8..12].try_into().unwrap()),
            0x1111_1111
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                internal_b.recv_from(&mut received)
            )
            .await
            .is_err(),
            "a packet from line A must not reach line B"
        );
        tokio::time::timeout(Duration::from_secs(1), first_operator_rtp.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(*first_operator_rtp.borrow());

        relay_a.stop();
        relay_b.stop();
    }
}

// ---------------------------------------------------------------------------
// Real UDP relay loop (unix-only IO layer)
// ---------------------------------------------------------------------------

/// Blocking bidirectional UDP relay between two bound sockets, driving the pure
/// [`RtpRelayCore`]. Compiled only on unix (the target device); on Windows the
/// logic above is still unit-tested.
#[cfg(unix)]
pub mod io {
    use super::*;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Run a relay loop until `stop` is set. `operator_sock`/`internal_sock` must
    /// already be bound to the local RTP ports advertised in the respective SDP.
    ///
    /// This is intentionally simple (single-threaded, `recv_from` with a read
    /// timeout). Real deployments may want per-leg threads; kept minimal here as
    /// the skeleton the plan calls for.
    pub fn run_relay(
        operator_sock: &UdpSocket,
        internal_sock: &UdpSocket,
        core: &mut RtpRelayCore,
        stop: Arc<AtomicBool>,
    ) -> std::io::Result<()> {
        run_relay_with_policy(
            operator_sock,
            internal_sock,
            core,
            stop,
            MediaRelayPolicy::bidirectional(),
        )
    }

    /// Unix relay variant used by callers that have negotiated hold/resume
    /// directions. RTCP-mux is still forwarded even when RTP is held.
    pub fn run_relay_with_policy(
        operator_sock: &UdpSocket,
        internal_sock: &UdpSocket,
        core: &mut RtpRelayCore,
        stop: Arc<AtomicBool>,
        policy: MediaRelayPolicy,
    ) -> std::io::Result<()> {
        let mut buf = [0u8; 2048];
        operator_sock.set_nonblocking(false)?;
        internal_sock.set_nonblocking(false)?;
        operator_sock.set_read_timeout(Some(std::time::Duration::from_millis(200)))?;
        internal_sock.set_read_timeout(Some(std::time::Duration::from_millis(200)))?;

        while !stop.load(Ordering::Relaxed) {
            relay_once(
                RelayLeg::Operator,
                operator_sock,
                internal_sock,
                core,
                &mut buf,
                policy.operator_to_internal_rtp,
            );
            relay_once(
                RelayLeg::Internal,
                internal_sock,
                operator_sock,
                core,
                &mut buf,
                policy.internal_to_operator_rtp,
            );
        }
        Ok(())
    }

    fn relay_once(
        leg: RelayLeg,
        recv_sock: &UdpSocket,
        send_sock: &UdpSocket,
        core: &mut RtpRelayCore,
        buf: &mut [u8],
        allow_rtp: bool,
    ) {
        match recv_sock.recv_from(buf) {
            Ok((n, src)) => {
                if let Ok(decision) = core.ingest(leg, src, &buf[..n]) {
                    // Forward out the peer leg's socket to the learned dest.
                    if decision.kind == MediaDatagramKind::Rtp && !allow_rtp {
                        return;
                    }
                    if decision.kind == MediaDatagramKind::Rtp {
                        if let Some(payload_type) = decision.rewrite_payload_type {
                            if let Some(rewritten) =
                                rewrite_rtp_payload_type(&buf[..n], payload_type)
                            {
                                let _ = send_sock.send_to(&rewritten, decision.dest);
                            }
                        } else {
                            let _ = send_sock.send_to(&buf[..n], decision.dest);
                        }
                    } else {
                        let _ = send_sock.send_to(&buf[..n], decision.dest);
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => {}
        }
    }
}
