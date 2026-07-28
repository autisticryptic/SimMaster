//! Minimal connected UDP transport for one Asterisk peer.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use tokio::{net::UdpSocket, time::timeout};

pub struct TrunkUdpTransport {
    socket: UdpSocket,
    peer: SocketAddr,
}

impl TrunkUdpTransport {
    pub async fn connect(peer: SocketAddr, local_port: u16) -> Result<Self, String> {
        let bind_addr = match peer.ip() {
            IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), local_port),
            IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), local_port),
        };
        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|error| format!("trunk_udp_bind_failed:{error}"))?;
        socket
            .connect(peer)
            .await
            .map_err(|error| format!("trunk_udp_connect_failed:{error}"))?;
        Ok(Self { socket, peer })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, String> {
        self.socket
            .local_addr()
            .map_err(|error| format!("trunk_udp_local_addr_failed:{error}"))
    }

    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    pub async fn send(&self, frame: &[u8]) -> Result<(), String> {
        self.socket
            .send(frame)
            .await
            .map(|_| ())
            .map_err(|error| format!("trunk_udp_send_failed:{error}"))
    }

    pub async fn recv(&self, wait: Duration) -> Result<Vec<u8>, String> {
        let mut frame = vec![0u8; 65_535];
        let read = timeout(wait, self.socket.recv(&mut frame))
            .await
            .map_err(|_| "trunk_udp_receive_timeout".to_string())?
            .map_err(|error| format!("trunk_udp_receive_failed:{error}"))?;
        frame.truncate(read);
        Ok(frame)
    }
}

pub async fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let mut addresses = tokio::net::lookup_host((host.trim().trim_matches(['[', ']']), port))
        .await
        .map_err(|error| format!("trunk_dns_failed:{error}"))?
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err("trunk_dns_empty".to_string());
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connected_udp_round_trips_with_peer() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let transport = TrunkUdpTransport::connect(server.local_addr().unwrap(), 0)
            .await
            .unwrap();
        transport.send(b"OPTIONS").await.unwrap();
        let mut request = [0u8; 64];
        let (read, peer) = server.recv_from(&mut request).await.unwrap();
        assert_eq!(&request[..read], b"OPTIONS");
        server.send_to(b"SIP/2.0 200 OK", peer).await.unwrap();
        assert_eq!(
            transport.recv(Duration::from_secs(1)).await.unwrap(),
            b"SIP/2.0 200 OK"
        );
    }
}
