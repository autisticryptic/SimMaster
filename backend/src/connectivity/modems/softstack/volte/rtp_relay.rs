//! VoLTE gateway RTP relay (skeleton).
//!
//! Clean-room from RFC 3550 (RTP). The target device has no audio hardware, so
//! voice media is never decoded locally — it is relayed at the packet level
//! between two endpoints:
//!   - the operator IMS media endpoint (negotiated via the VoLTE SDP), and
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
//! the device forwards packets and rewrites negotiated dynamic payload types.

use std::{
    io as std_io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use tokio::{net::UdpSocket, sync::watch, task::JoinHandle};

use crate::connectivity::modems::softstack::vowifi::voice::RtpPacket;
use crate::services::trunk::operator::OperatorMediaMetrics;

/// Which leg a datagram arrived on. The relay forwards A->B and B->A.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayLeg {
    /// Operator IMS media side (the VoLTE-negotiated RTP endpoint).
    Operator,
    /// Internal SIP UA side (Linphone/Asterisk).
    Internal,
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
    /// The datagram was not a plausible RTP packet.
    NotRtp,
}

/// The transport-agnostic RTP relay core: two legs, symmetric forwarding,
/// peer learning, and counters. No sockets — feed it `(leg, src, datagram)`
/// and it tells you where to forward.
#[derive(Debug, Clone)]
pub struct RtpRelayCore {
    operator: LegEndpoint,
    internal: LegEndpoint,
    /// If true, only forward payloads that parse as RTP v2 (drop stray/noise).
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

    /// Allow forwarding non-RTP datagrams (e.g. RTCP) without RTP validation.
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
        if self.require_rtp && RtpPacket::parse(datagram).is_none() {
            return Err(RelayError::NotRtp);
        }

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
        let current_payload_type = datagram.get(1).map(|byte| byte & 0x7f);
        let rewrite_payload_type = current_payload_type.and_then(|current| {
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
            rewrite_payload_type,
        })
    }
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
}

impl PendingRtpRelay {
    pub async fn bind(operator_ip: IpAddr, internal_ip: IpAddr) -> std_io::Result<Self> {
        let operator_socket = Arc::new(UdpSocket::bind(SocketAddr::new(operator_ip, 0)).await?);
        let internal_socket = Arc::new(UdpSocket::bind(SocketAddr::new(internal_ip, 0)).await?);
        Ok(Self {
            operator_socket,
            internal_socket,
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
        metrics: Option<Arc<OperatorMediaMetrics>>,
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

pub struct ActiveRtpRelay {
    stop: watch::Sender<bool>,
    first_operator_rtp: watch::Receiver<bool>,
    task: JoinHandle<std_io::Result<()>>,
    metrics: Option<Arc<OperatorMediaMetrics>>,
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
    mut stop: watch::Receiver<bool>,
    first_operator_rtp: watch::Sender<bool>,
    metrics: Option<Arc<OperatorMediaMetrics>>,
) -> std_io::Result<()> {
    let mut operator_buf = vec![0u8; 65_535];
    let mut internal_buf = vec![0u8; 65_535];
    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Ok(());
                }
            }
            received = operator_socket.recv_from(&mut operator_buf) => {
                let (len, source) = received?;
                if forward_async(
                    &mut core,
                    RelayLeg::Operator,
                    source,
                    &operator_buf[..len],
                    &internal_socket,
                ).await {
                    if let Some(metrics) = metrics.as_ref() {
                        metrics.record_rtp_to_asterisk(len);
                    }
                    let _ = first_operator_rtp.send(true);
                }
            }
            received = internal_socket.recv_from(&mut internal_buf) => {
                let (len, source) = received?;
                if forward_async(
                    &mut core,
                    RelayLeg::Internal,
                    source,
                    &internal_buf[..len],
                    &operator_socket,
                ).await {
                    if let Some(metrics) = metrics.as_ref() {
                        metrics.record_rtp_from_asterisk(len);
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
) -> bool {
    let Ok(decision) = core.ingest(leg, source, datagram) else {
        return false;
    };
    if let Some(payload_type) = decision.rewrite_payload_type {
        if let Some(rewritten) = rewrite_rtp_payload_type(datagram, payload_type) {
            let _ = send_socket.send_to(&rewritten, decision.dest).await;
        }
    } else {
        let _ = send_socket.send_to(datagram, decision.dest).await;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::modems::softstack::vowifi::voice::RtpPacket;
    use std::net::{IpAddr, Ipv4Addr};

    fn addr(last: u8, port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)), port)
    }

    fn rtp_datagram() -> Vec<u8> {
        RtpPacket {
            payload_type: 96,
            marker: false,
            sequence: 1,
            timestamp: 160,
            ssrc: 0xdead_beef,
            payload: vec![0xaa, 0xbb, 0xcc],
        }
        .encode()
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
            );
            relay_once(
                RelayLeg::Internal,
                internal_sock,
                operator_sock,
                core,
                &mut buf,
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
    ) {
        match recv_sock.recv_from(buf) {
            Ok((n, src)) => {
                if let Ok(decision) = core.ingest(leg, src, &buf[..n]) {
                    // Forward out the peer leg's socket to the learned dest.
                    if let Some(payload_type) = decision.rewrite_payload_type {
                        if let Some(rewritten) = rewrite_rtp_payload_type(&buf[..n], payload_type) {
                            let _ = send_sock.send_to(&rewritten, decision.dest);
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
