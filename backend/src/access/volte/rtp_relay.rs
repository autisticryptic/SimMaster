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
//! Scope note (per plan stage E): this is the relay **skeleton** — symmetric
//! two-leg forwarding with counters. Transcoding (AMR ↔ G.711/opus), RTCP
//! handling, and jitter buffering are explicitly out of scope here (a pure
//! relay defers transcoding to the PBX, and the device only shuffles UDP).

use std::net::SocketAddr;

use crate::access::vowifi::voice::RtpPacket;

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
}

impl RtpRelayCore {
    pub fn new(operator: LegEndpoint, internal: LegEndpoint) -> Self {
        Self {
            operator,
            internal,
            require_rtp: true,
        }
    }

    /// Allow forwarding non-RTP datagrams (e.g. RTCP) without RTP validation.
    pub fn with_require_rtp(mut self, require: bool) -> Self {
        self.require_rtp = require;
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
        Ok(ForwardDecision { to: peer, dest })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::vowifi::voice::RtpPacket;
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
            .ingest(RelayLeg::Internal, addr(2, 40000), &[0x80, 0xc8, 0x00, 0x06])
            .expect("forward without rtp check");
        assert_eq!(d.to, RelayLeg::Operator);
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
            relay_once(RelayLeg::Operator, operator_sock, internal_sock, core, &mut buf);
            relay_once(RelayLeg::Internal, internal_sock, operator_sock, core, &mut buf);
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
                    let _ = send_sock.send_to(&buf[..n], decision.dest);
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => {}
        }
    }
}
