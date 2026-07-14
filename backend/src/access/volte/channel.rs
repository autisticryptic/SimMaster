//! Native VoLTE SIP channel over the dedicated IMS bearer.
//!
//! The socket is explicitly bound to the bearer interface so IMS traffic can
//! never escape through the host's normal Wi-Fi/default route. Once xfrm SAs
//! and policies are installed, the same UDP API transparently carries ESP-
//! protected SIP. A 401/407 sec-agree challenge may replace the socket with a
//! channel bound to the negotiated client port.

use std::{
    io,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    time::Duration,
};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use crate::ims::{access::ImsChannel, context::ImsRoute, ImsError};

const MAX_SIP_DATAGRAM: usize = 65_535;

pub struct VolteSipChannel {
    socket: UdpSocket,
    route: ImsRoute,
    interface: Option<String>,
    security_verify: Option<String>,
}

impl VolteSipChannel {
    pub fn bind(
        route: ImsRoute,
        interface: Option<&str>,
        security_verify: Option<String>,
    ) -> Result<Self, ImsError> {
        let socket = build_socket(route.local_addr, route.pcscf_addr, interface)
            .map_err(|_| ImsError::new("volte_channel_bind_failed"))?;
        Ok(Self {
            socket,
            route,
            interface: interface.map(ToOwned::to_owned),
            security_verify,
        })
    }

    /// Replace the UDP socket after a Security-Server challenge selected new
    /// protected ports. The old socket is dropped only after the replacement
    /// was created successfully.
    pub fn rebind(
        &mut self,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Result<(), ImsError> {
        let replacement = Self::bind(route, self.interface.as_deref(), security_verify)?;
        *self = replacement;
        Ok(())
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ImsError> {
        self.socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))
    }

    pub fn interface(&self) -> Option<&str> {
        self.interface.as_deref()
    }
}

impl ImsChannel for VolteSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        let written = self
            .socket
            .send(frame)
            .await
            .map_err(|_| ImsError::new("volte_channel_send_failed"))?;
        if written != frame.len() {
            return Err(ImsError::new("volte_channel_short_send"));
        }
        Ok(())
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        let mut frame = vec![0u8; MAX_SIP_DATAGRAM];
        let read = tokio::time::timeout(timeout, self.socket.recv(&mut frame))
            .await
            .map_err(|_| ImsError::new("volte_channel_read_timeout"))?
            .map_err(|_| ImsError::new("volte_channel_read_failed"))?;
        frame.truncate(read);
        Ok(frame)
    }

    fn route(&self) -> ImsRoute {
        self.route
    }

    fn security_verify(&self) -> Option<&str> {
        self.security_verify.as_deref()
    }
}

fn build_socket(
    local: SocketAddr,
    remote: SocketAddr,
    interface: Option<&str>,
) -> io::Result<UdpSocket> {
    if local.is_ipv4() != remote.is_ipv4() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "IMS local and P-CSCF address families differ",
        ));
    }
    let socket = Socket::new(
        Domain::for_address(local),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    bind_to_interface(&socket, interface)?;
    socket.bind(&local.into())?;
    socket.connect(&remote.into())?;
    socket.set_nonblocking(true)?;
    let std_socket: StdUdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

#[cfg(target_os = "linux")]
fn bind_to_interface(socket: &Socket, interface: Option<&str>) -> io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let Some(interface) = interface else {
        return Ok(());
    };
    let name = CString::new(interface)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interface contains NUL"))?;
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            (name.as_bytes_with_nul().len()) as libc::socklen_t,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_to_interface(_socket: &Socket, interface: Option<&str>) -> io::Result<()> {
    if interface.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SO_BINDTODEVICE is Linux-only",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ims::context::SipTransport;
    use std::net::{IpAddr, Ipv4Addr};

    #[tokio::test]
    async fn udp_channel_round_trips_sip_datagrams() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let route = ImsRoute {
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            pcscf_addr: server_addr,
            transport: SipTransport::Udp,
        };
        let mut channel = VolteSipChannel::bind(route, None, None).unwrap();
        let client_addr = channel.local_addr().unwrap();

        channel.send_sip(b"REGISTER sip:ims.example SIP/2.0\r\n\r\n").await.unwrap();
        let mut request = [0u8; 256];
        let (read, peer) = server.recv_from(&mut request).await.unwrap();
        assert_eq!(peer, client_addr);
        assert!(request[..read].starts_with(b"REGISTER "));

        server
            .send_to(b"SIP/2.0 200 OK\r\nContent-Length: 0\r\n\r\n", peer)
            .await
            .unwrap();
        let response = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        assert!(response.starts_with(b"SIP/2.0 200"));
    }

    #[test]
    fn rejects_mismatched_address_families() {
        let route = ImsRoute {
            local_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            pcscf_addr: "[::1]:5060".parse().unwrap(),
            transport: SipTransport::Udp,
        };
        let error = VolteSipChannel::bind(route, None, None).err().unwrap();
        assert_eq!(error.code(), "volte_channel_bind_failed");
    }
}
