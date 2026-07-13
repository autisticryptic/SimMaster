#![allow(dead_code)]

use std::{fmt, net::SocketAddr, sync::Arc};

use serde::Serialize;
use tokio::{net::UdpSocket, time::Duration};

use super::profiles::CarrierProfileMeta;

const IKE_NAT_T_NON_ESP_MARKER: [u8; 4] = [0, 0, 0, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyKind {
    Direct,
    Socks5UdpAssociate,
    ConnectUdpMasque,
    UdpRelay,
}

impl ProxyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProxyKind::Direct => "direct",
            ProxyKind::Socks5UdpAssociate => "socks5_udp_associate",
            ProxyKind::ConnectUdpMasque => "connect_udp_masque",
            ProxyKind::UdpRelay => "udp_relay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkRoutePolicy {
    pub kind: ProxyKind,
    pub policy_id: &'static str,
    pub note: &'static str,
}

impl Default for NetworkRoutePolicy {
    fn default() -> Self {
        Self {
            kind: ProxyKind::Direct,
            policy_id: "direct",
            note: "direct UDP path",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedEpdgEndpoint {
    pub host: String,
    pub port: u16,
    pub addresses: Vec<SocketAddr>,
    pub route_policy: NetworkRoutePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    DnsFailed(String),
    RouteUnavailable(String),
    UnsupportedProxy(String),
    Io(String),
    Timeout(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::DnsFailed(reason) => write!(f, "DNS resolution failed: {reason}"),
            TransportError::RouteUnavailable(reason) => {
                write!(f, "network route unavailable: {reason}")
            }
            TransportError::UnsupportedProxy(reason) => {
                write!(f, "unsupported proxy route: {reason}")
            }
            TransportError::Io(reason) => write!(f, "transport IO failed: {reason}"),
            TransportError::Timeout(reason) => write!(f, "transport timeout: {reason}"),
        }
    }
}

impl std::error::Error for TransportError {}

pub trait DnsResolver {
    fn resolve_epdg(
        &self,
        profile: &'static CarrierProfileMeta,
        host: &str,
        port: u16,
    ) -> impl std::future::Future<Output = Result<ResolvedEpdgEndpoint, TransportError>> + Send;
}

pub trait IkeDatagramTransport {
    fn send_ike_datagram(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send;

    fn recv_ike_datagram(
        &self,
    ) -> impl std::future::Future<Output = Result<(SocketAddr, Vec<u8>), TransportError>> + Send;
}

pub trait NatTPacketTransport {
    fn send_nat_t_packet(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> impl std::future::Future<Output = Result<(), TransportError>> + Send;

    fn recv_nat_t_packet(
        &self,
    ) -> impl std::future::Future<Output = Result<(SocketAddr, Vec<u8>), TransportError>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatagramPacketMetadata {
    pub remote: SocketAddr,
    pub bytes: usize,
    pub channel: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone)]
pub struct UdpSocketDatagramTransport {
    socket: Arc<UdpSocket>,
    recv_timeout: Duration,
    max_datagram_bytes: usize,
}

impl UdpSocketDatagramTransport {
    pub async fn bind(local: SocketAddr) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(local)
            .await
            .map_err(|err| TransportError::Io(err.kind().to_string()))?;
        Ok(Self::from_socket(socket))
    }

    pub fn from_socket(socket: UdpSocket) -> Self {
        Self {
            socket: Arc::new(socket),
            recv_timeout: Duration::from_secs(8),
            max_datagram_bytes: 4096,
        }
    }

    pub fn with_recv_timeout(mut self, recv_timeout: Duration) -> Self {
        self.recv_timeout = recv_timeout;
        self
    }

    pub fn with_max_datagram_bytes(mut self, max_datagram_bytes: usize) -> Self {
        self.max_datagram_bytes = max_datagram_bytes.max(1);
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.socket
            .local_addr()
            .map_err(|err| TransportError::Io(err.kind().to_string()))
    }

    async fn send_packet(
        &self,
        channel: &'static str,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<DatagramPacketMetadata, TransportError> {
        self.socket
            .send_to(payload, destination)
            .await
            .map_err(|err| TransportError::Io(err.kind().to_string()))?;

        Ok(DatagramPacketMetadata {
            remote: destination,
            bytes: payload.len(),
            channel,
            sensitive_values_policy: "packet_metadata_only_no_payload_or_spi_values",
        })
    }

    async fn recv_packet(
        &self,
        channel: &'static str,
    ) -> Result<(SocketAddr, Vec<u8>, DatagramPacketMetadata), TransportError> {
        let mut buffer = vec![0u8; self.max_datagram_bytes];
        let received = tokio::time::timeout(self.recv_timeout, self.socket.recv_from(&mut buffer))
            .await
            .map_err(|_| TransportError::Timeout(format!("{channel} receive timed out")))?
            .map_err(|err| TransportError::Io(err.kind().to_string()))?;
        let (bytes, remote) = received;
        buffer.truncate(bytes);

        let metadata = DatagramPacketMetadata {
            remote,
            bytes,
            channel,
            sensitive_values_policy: "packet_metadata_only_no_payload_or_spi_values",
        };
        Ok((remote, buffer, metadata))
    }

    pub async fn send_ike_metadata(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<DatagramPacketMetadata, TransportError> {
        self.send_packet("ike_udp_500", destination, payload).await
    }

    pub async fn recv_ike_metadata(
        &self,
    ) -> Result<(SocketAddr, Vec<u8>, DatagramPacketMetadata), TransportError> {
        self.recv_packet("ike_udp_500").await
    }

    pub async fn send_ike_message_metadata(
        &self,
        use_nat_t: bool,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<DatagramPacketMetadata, TransportError> {
        if use_nat_t {
            self.send_nat_t_metadata(destination, payload).await
        } else {
            self.send_ike_metadata(destination, payload).await
        }
    }

    pub async fn recv_ike_message_metadata(
        &self,
        use_nat_t: bool,
    ) -> Result<(SocketAddr, Vec<u8>, DatagramPacketMetadata), TransportError> {
        if use_nat_t {
            self.recv_nat_t_metadata().await
        } else {
            self.recv_ike_metadata().await
        }
    }

    pub async fn send_nat_t_metadata(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<DatagramPacketMetadata, TransportError> {
        let mut framed = Vec::with_capacity(IKE_NAT_T_NON_ESP_MARKER.len() + payload.len());
        framed.extend_from_slice(&IKE_NAT_T_NON_ESP_MARKER);
        framed.extend_from_slice(payload);
        self.send_packet("ike_nat_t_udp_4500", destination, &framed)
            .await
    }

    pub async fn send_esp_nat_t_metadata(
        &self,
        destination: SocketAddr,
        esp_frame: &[u8],
    ) -> Result<DatagramPacketMetadata, TransportError> {
        self.send_packet("esp_nat_t_udp_4500", destination, esp_frame)
            .await
    }

    pub async fn recv_nat_t_raw_metadata(
        &self,
    ) -> Result<(SocketAddr, Vec<u8>, DatagramPacketMetadata), TransportError> {
        self.recv_packet("nat_t_udp_4500_raw").await
    }

    pub async fn recv_nat_t_metadata(
        &self,
    ) -> Result<(SocketAddr, Vec<u8>, DatagramPacketMetadata), TransportError> {
        loop {
            let (remote, packet, metadata) = self.recv_packet("ike_nat_t_udp_4500").await?;
            if packet == [0xff] {
                continue;
            }
            if packet.len() >= IKE_NAT_T_NON_ESP_MARKER.len()
                && packet[..IKE_NAT_T_NON_ESP_MARKER.len()] == IKE_NAT_T_NON_ESP_MARKER
            {
                return Ok((
                    remote,
                    packet[IKE_NAT_T_NON_ESP_MARKER.len()..].to_vec(),
                    metadata,
                ));
            }
            continue;
        }
    }
}

impl IkeDatagramTransport for UdpSocketDatagramTransport {
    async fn send_ike_datagram(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        self.send_ike_metadata(destination, payload).await?;
        Ok(())
    }

    async fn recv_ike_datagram(&self) -> Result<(SocketAddr, Vec<u8>), TransportError> {
        let (remote, payload, _metadata) = self.recv_ike_metadata().await?;
        Ok((remote, payload))
    }
}

impl NatTPacketTransport for UdpSocketDatagramTransport {
    async fn send_nat_t_packet(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<(), TransportError> {
        self.send_nat_t_metadata(destination, payload).await?;
        Ok(())
    }

    async fn recv_nat_t_packet(&self) -> Result<(SocketAddr, Vec<u8>), TransportError> {
        let (remote, payload, _metadata) = self.recv_nat_t_metadata().await?;
        Ok((remote, payload))
    }
}

pub fn choose_route_policy(
    profile: &'static CarrierProfileMeta,
    epdg_host: &str,
    requested: Option<ProxyKind>,
) -> NetworkRoutePolicy {
    match requested.unwrap_or(ProxyKind::Direct) {
        ProxyKind::Direct => NetworkRoutePolicy::default(),
        ProxyKind::Socks5UdpAssociate => NetworkRoutePolicy {
            kind: ProxyKind::Socks5UdpAssociate,
            policy_id: "socks5_udp_by_profile",
            note: "SOCKS5 UDP ASSOCIATE path; plain HTTP CONNECT is not used for UDP",
        },
        ProxyKind::ConnectUdpMasque => NetworkRoutePolicy {
            kind: ProxyKind::ConnectUdpMasque,
            policy_id: "masque_by_profile",
            note: "HTTP CONNECT-UDP/MASQUE path; plain HTTP CONNECT is not equivalent",
        },
        ProxyKind::UdpRelay => NetworkRoutePolicy {
            kind: ProxyKind::UdpRelay,
            policy_id: if profile.country_iso2 == "us" && epdg_host.ends_with(".net") {
                "udp_relay_us_epdg"
            } else {
                "udp_relay_by_epdg"
            },
            note: "SimAdmin UDP relay path",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::vowifi::profiles::US_ATT_310410;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn direct_route_is_default() {
        let policy = choose_route_policy(&US_ATT_310410.meta, US_ATT_310410.epdg.host, None);

        assert_eq!(policy.kind, ProxyKind::Direct);
        assert_eq!(policy.policy_id, "direct");
    }

    #[test]
    fn masque_route_explicitly_distinguishes_connect_udp_from_plain_connect() {
        let policy = choose_route_policy(
            &US_ATT_310410.meta,
            US_ATT_310410.epdg.host,
            Some(ProxyKind::ConnectUdpMasque),
        );

        assert_eq!(policy.kind, ProxyKind::ConnectUdpMasque);
        assert!(policy.note.contains("CONNECT-UDP"));
        assert!(policy.note.contains("not equivalent"));
    }

    #[tokio::test]
    async fn udp_socket_transport_round_trips_ike_datagram_with_metadata_only() {
        let left =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind left");
        let right =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind right");
        let right_addr = right.local_addr().expect("right addr");

        let payload = [0x11, 0x22, 0x33, 0x44];
        let sent = left
            .send_ike_metadata(right_addr, &payload)
            .await
            .expect("send ike");
        let (remote, received, metadata) = right.recv_ike_metadata().await.expect("recv ike");

        assert_eq!(received, payload);
        assert_eq!(metadata.bytes, payload.len());
        assert_eq!(metadata.channel, "ike_udp_500");
        assert_eq!(sent.bytes, payload.len());
        assert_eq!(remote, left.local_addr().expect("left addr"));

        let json = serde_json::to_string(&metadata).expect("serialize metadata");
        for forbidden in ["payload", "spi", "key", "secret", "imsi", "iccid"] {
            assert!(!json
                .to_ascii_lowercase()
                .contains(&format!("\"{forbidden}\"")));
        }
    }

    #[tokio::test]
    async fn udp_socket_transport_wraps_ike_over_nat_t_non_esp_marker() {
        let left =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind left");
        let right =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind right");
        let right_addr = right.local_addr().expect("right addr");

        let payload = [0x21, 0x20, 0x00, 0x28];
        let sent = left
            .send_ike_message_metadata(true, right_addr, &payload)
            .await
            .expect("send nat-t ike");
        let (_remote, received, metadata) = right
            .recv_ike_message_metadata(true)
            .await
            .expect("recv nat-t ike");

        assert_eq!(received, payload);
        assert_eq!(sent.bytes, payload.len() + IKE_NAT_T_NON_ESP_MARKER.len());
        assert_eq!(metadata.channel, "ike_nat_t_udp_4500");
        assert_eq!(
            metadata.bytes,
            payload.len() + IKE_NAT_T_NON_ESP_MARKER.len()
        );
    }

    #[tokio::test]
    async fn nat_t_ike_receiver_skips_esp_packets_until_non_esp_marker_arrives() {
        let left =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind left");
        let right =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind right");
        let right_addr = right.local_addr().expect("right addr");

        left.send_packet("ike_nat_t_udp_4500", right_addr, &[0x12, 0x34, 0x56, 0x78])
            .await
            .expect("send esp-like packet");
        left.send_ike_message_metadata(true, right_addr, &[0x21, 0x20, 0x00, 0x28])
            .await
            .expect("send ike packet");

        let (_remote, received, _metadata) = right
            .recv_ike_message_metadata(true)
            .await
            .expect("recv nat-t ike after esp packet");

        assert_eq!(received, [0x21, 0x20, 0x00, 0x28]);
    }
    #[tokio::test]
    async fn udp_socket_transport_times_out_without_payload_material() {
        let transport =
            UdpSocketDatagramTransport::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .await
                .expect("bind")
                .with_recv_timeout(Duration::from_millis(1));

        let err = transport
            .recv_nat_t_packet()
            .await
            .expect_err("receive should time out");

        assert!(matches!(err, TransportError::Timeout(_)));
        let text = err.to_string();
        assert!(text.contains("ike_nat_t_udp_4500"));
        assert!(!text.contains("payload"));
    }
}
