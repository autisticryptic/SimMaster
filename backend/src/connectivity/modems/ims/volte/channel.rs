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

use crate::connectivity::core::{
    access::{ImsChannel, ImsRequeue},
    context::ImsRoute,
    ImsError,
};
use crate::services::ue_worker::{UeSocket, UeSocketSpec, UeWorkerHandle};

const MAX_SIP_DATAGRAM: usize = 65_535;

pub struct VolteSipChannel {
    send_socket: Option<UdpSocket>,
    receive_socket: Option<UdpSocket>,
    reserved_receive_socket: Option<ReservedReceiveSocket>,
    /// Complete frames set aside by a REGISTER transaction because they belong
    /// to a different dialog (NOTIFY/MESSAGE/MWI). They are handed back to the
    /// session loop after the transaction completes.
    requeued: ImsRequeue,
    route: ImsRoute,
    /// Port to advertise in Via/Contact once a security association is active.
    /// TS 24.229 §5.1.1.2.2 b)/c): a UDP request protected by an SA is sourced
    /// from the protected client port (port_uc) but must advertise the protected
    /// *server* port (port_us), because that is where the P-CSCF sends
    /// terminating requests. `None` means the channel is unprotected and the
    /// send port is also the right port to advertise.
    advertised_local_port: Option<u16>,
    interface: Option<String>,
    security_verify: Option<String>,
}

enum ReservedReceiveSocket {
    Host(Socket),
    Worker(UdpSocket),
}

impl VolteSipChannel {
    pub fn bind(
        route: ImsRoute,
        interface: Option<&str>,
        security_verify: Option<String>,
    ) -> Result<Self, ImsError> {
        let socket = build_socket(route.local_addr, route.pcscf_addr, interface)
            .map_err(|_| ImsError::new("volte_channel_bind_failed"))?;
        let mut route = route;
        route.local_addr = socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
        Ok(Self {
            send_socket: Some(socket),
            receive_socket: None,
            reserved_receive_socket: None,
            requeued: ImsRequeue::default(),
            route,
            advertised_local_port: None,
            interface: interface.map(ToOwned::to_owned),
            security_verify,
        })
    }

    /// Reserve a second local UDP port for protected packets sent by the
    /// P-CSCF.  The initial REGISTER socket remains the protected send socket.
    pub fn reserve_security_receive_port(&mut self) -> Result<u16, ImsError> {
        if let Some(socket) = self.reserved_receive_socket.as_ref() {
            return match socket {
                ReservedReceiveSocket::Host(socket) => socket_port(socket),
                ReservedReceiveSocket::Worker(socket) => socket
                    .local_addr()
                    .map(|addr| addr.port())
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed")),
            };
        }
        let local = SocketAddr::new(self.route.local_addr.ip(), 0);
        let socket = build_bound_socket(local, self.interface.as_deref())
            .map_err(|_| ImsError::new("volte_channel_receive_reserve_failed"))?;
        let port = socket_port(&socket)?;
        self.reserved_receive_socket = Some(ReservedReceiveSocket::Host(socket));
        Ok(port)
    }

    /// Create the initial SIP channel inside a per-line UE worker.  The
    /// bearer/QMI lifecycle remains in the parent; only the socket's kernel
    /// network namespace is moved.  Callers must retain the host path as a
    /// fallback because a worker cannot reach a bearer interface that has not
    /// yet been bridged or moved into its namespace.
    pub async fn bind_in_worker(
        route: ImsRoute,
        worker: &UeWorkerHandle,
        interface: Option<&str>,
        security_verify: Option<String>,
    ) -> Result<Self, ImsError> {
        let spec = UeSocketSpec::udp_connected(
            route.local_addr,
            route.pcscf_addr,
            interface.map(ToOwned::to_owned),
        );
        let socket = match worker.create_socket(spec).await {
            Ok(UeSocket::Udp(socket)) => socket,
            Ok(_) => return Err(ImsError::new("volte_channel_worker_socket_type")),
            Err(_) => return Err(ImsError::new("volte_channel_worker_socket_failed")),
        };
        let mut route = route;
        route.local_addr = socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
        Ok(Self {
            send_socket: Some(socket),
            receive_socket: None,
            reserved_receive_socket: None,
            requeued: ImsRequeue::default(),
            route,
            advertised_local_port: None,
            interface: interface.map(ToOwned::to_owned),
            security_verify,
        })
    }

    /// Reserve the protected receive port using the worker socket factory.
    pub async fn reserve_security_receive_port_in_worker(
        &mut self,
        worker: &UeWorkerHandle,
    ) -> Result<u16, ImsError> {
        if let Some(socket) = self.reserved_receive_socket.as_ref() {
            return match socket {
                ReservedReceiveSocket::Host(socket) => socket_port(socket),
                ReservedReceiveSocket::Worker(socket) => socket
                    .local_addr()
                    .map(|addr| addr.port())
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed")),
            };
        }
        let local = SocketAddr::new(self.route.local_addr.ip(), 0);
        let spec = UeSocketSpec::udp_bound(local, self.interface.clone());
        let socket = match worker.create_socket(spec).await {
            Ok(UeSocket::Udp(socket)) => socket,
            Ok(_) => return Err(ImsError::new("volte_channel_worker_socket_type")),
            Err(_) => return Err(ImsError::new("volte_channel_worker_socket_failed")),
        };
        let port = socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?
            .port();
        self.reserved_receive_socket = Some(ReservedReceiveSocket::Worker(socket));
        Ok(port)
    }

    /// Activate the two protected UDP directions negotiated by sec-agree:
    /// UE send -> P-CSCF client port, and P-CSCF send -> UE receive port.
    pub fn activate_security(
        &mut self,
        send_route: ImsRoute,
        receive_local: SocketAddr,
        receive_remote: SocketAddr,
        security_verify: Option<String>,
    ) -> Result<(), ImsError> {
        let reserved = self
            .reserved_receive_socket
            .take()
            .ok_or_else(|| ImsError::new("volte_channel_receive_not_reserved"))?;
        let receive_socket = match reserved {
            ReservedReceiveSocket::Host(reserved) => {
                if socket_addr(&reserved)? != receive_local {
                    return Err(ImsError::new("volte_channel_receive_port_mismatch"));
                }
                connect_bound_socket(reserved, receive_remote)
                    .map_err(|_| ImsError::new("volte_channel_receive_connect_failed"))?
            }
            ReservedReceiveSocket::Worker(socket) => {
                let _ = (socket, receive_local, receive_remote);
                return Err(ImsError::new("volte_channel_worker_receive_requires_async"));
            }
        };

        // Release the initial connected socket before rebinding the same local
        // send port to the P-CSCF protected client port.
        self.send_socket.take();
        let send_socket = build_socket(
            send_route.local_addr,
            send_route.pcscf_addr,
            self.interface.as_deref(),
        )
        .map_err(|_| ImsError::new("volte_channel_protected_send_bind_failed"))?;
        let mut send_route = send_route;
        send_route.local_addr = send_socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
        self.send_socket = Some(send_socket);
        self.receive_socket = Some(receive_socket);
        self.route = send_route;
        // Terminating requests arrive on the protected server port, so that is
        // what Via/Contact must advertise from here on -- not the send port.
        self.advertised_local_port = Some(receive_local.port());
        self.security_verify = security_verify;
        Ok(())
    }

    /// Worker equivalent of [`Self::activate_security`].  The protected send
    /// socket is recreated in the worker after XFRM has been installed; the
    /// already-reserved receive socket is connected locally in the parent.
    pub async fn activate_security_in_worker(
        &mut self,
        send_route: ImsRoute,
        receive_local: SocketAddr,
        receive_remote: SocketAddr,
        security_verify: Option<String>,
        worker: &UeWorkerHandle,
    ) -> Result<(), ImsError> {
        let reserved = self
            .reserved_receive_socket
            .take()
            .ok_or_else(|| ImsError::new("volte_channel_receive_not_reserved"))?;
        let receive_socket = match reserved {
            ReservedReceiveSocket::Worker(socket) => {
                let local = socket
                    .local_addr()
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
                if local != receive_local {
                    return Err(ImsError::new("volte_channel_receive_port_mismatch"));
                }
                socket
                    .connect(receive_remote)
                    .await
                    .map_err(|_| ImsError::new("volte_channel_receive_connect_failed"))?;
                socket
            }
            ReservedReceiveSocket::Host(_) => {
                return Err(ImsError::new("volte_channel_worker_receive_mismatch"));
            }
        };
        let spec = UeSocketSpec::udp_connected(
            send_route.local_addr,
            send_route.pcscf_addr,
            self.interface.clone(),
        );
        let send_socket = match worker.create_socket(spec).await {
            Ok(UeSocket::Udp(socket)) => socket,
            Ok(_) => return Err(ImsError::new("volte_channel_worker_socket_type")),
            Err(_) => return Err(ImsError::new("volte_channel_worker_socket_failed")),
        };
        let mut send_route = send_route;
        send_route.local_addr = send_socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
        self.send_socket = Some(send_socket);
        self.receive_socket = Some(receive_socket);
        self.route = send_route;
        // Same rule as the host path: advertise the protected server port.
        self.advertised_local_port = Some(receive_local.port());
        self.security_verify = security_verify;
        Ok(())
    }

    /// Route to put in Via/Contact. Identical to [`Self::send_route`] until a
    /// security association is active, after which the local port becomes the
    /// protected server port while sends keep using the client port.
    ///
    /// This is what [`ImsChannel::route`] returns, because every consumer
    /// outside this module uses the route to build headers or to read the local
    /// IP -- sending goes through `send_socket`, which is already connected.
    pub fn advertised_route(&self) -> ImsRoute {
        let mut route = self.route;
        if let Some(port) = self.advertised_local_port {
            route.local_addr.set_port(port);
        }
        route
    }

    /// The route packets are actually sourced from: after sec-agree the
    /// protected client port (port_uc). Only the security offer needs this.
    pub fn send_route(&self) -> ImsRoute {
        self.route
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ImsError> {
        self.send_socket
            .as_ref()
            .ok_or_else(|| ImsError::new("volte_channel_send_socket_missing"))?
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))
    }

    pub fn interface(&self) -> Option<&str> {
        self.interface.as_deref()
    }

    async fn recv_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        let mut frame = vec![0u8; MAX_SIP_DATAGRAM];
        let socket = self
            .receive_socket
            .as_ref()
            .or(self.send_socket.as_ref())
            .ok_or_else(|| ImsError::new("volte_channel_receive_socket_missing"))?;
        let read = tokio::time::timeout(timeout, socket.recv(&mut frame))
            .await
            .map_err(|_| ImsError::new("volte_channel_read_timeout"))?
            .map_err(|_| ImsError::new("volte_channel_read_failed"))?;
        frame.truncate(read);
        Ok(frame)
    }
}

impl ImsChannel for VolteSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        let written = self
            .send_socket
            .as_ref()
            .ok_or_else(|| ImsError::new("volte_channel_send_socket_missing"))?
            .send(frame)
            .await
            .map_err(|_| ImsError::new("volte_channel_send_failed"))?;
        if written != frame.len() {
            return Err(ImsError::new("volte_channel_short_send"));
        }
        Ok(())
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        if let Some(frame) = self.requeued.pop_front() {
            return Ok(frame);
        }
        self.recv_fresh(timeout).await
    }

    async fn recv_sip_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.recv_fresh(timeout).await
    }

    fn requeue(&mut self, frame: Vec<u8>) {
        let frame_bytes = frame.len();
        if !self.requeued.push_back(frame) {
            tracing::warn!(
                frame_bytes,
                queued_frames = self.requeued.len(),
                queued_bytes = self.requeued.bytes(),
                "VoLTE SIP requeue full; dropping newest frame"
            );
        }
    }

    fn route(&self) -> ImsRoute {
        self.advertised_route()
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
    let socket = build_bound_socket(local, interface)?;
    connect_bound_socket(socket, remote)
}

fn build_bound_socket(local: SocketAddr, interface: Option<&str>) -> io::Result<Socket> {
    let socket = Socket::new(Domain::for_address(local), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    bind_to_interface(&socket, interface)?;
    socket.bind(&local.into())?;
    Ok(socket)
}

fn connect_bound_socket(socket: Socket, remote: SocketAddr) -> io::Result<UdpSocket> {
    socket.connect(&remote.into())?;
    socket.set_nonblocking(true)?;
    let std_socket: StdUdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

fn socket_addr(socket: &Socket) -> Result<SocketAddr, ImsError> {
    socket
        .local_addr()
        .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?
        .as_socket()
        .ok_or_else(|| ImsError::new("volte_channel_local_addr_failed"))
}

fn socket_port(socket: &Socket) -> Result<u16, ImsError> {
    Ok(socket_addr(socket)?.port())
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
    use crate::connectivity::core::context::SipTransport;
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

        channel
            .send_sip(b"REGISTER sip:ims.example SIP/2.0\r\n\r\n")
            .await
            .unwrap();
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

    #[tokio::test]
    async fn protected_channel_uses_distinct_send_and_receive_ports() {
        let pcscf_client = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let pcscf_send = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = ImsRoute {
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            pcscf_addr: pcscf_client.local_addr().unwrap(),
            transport: SipTransport::Udp,
        };
        let mut channel = VolteSipChannel::bind(route, None, None).unwrap();
        let local_send = channel.local_addr().unwrap();
        let local_receive = SocketAddr::new(
            local_send.ip(),
            channel.reserve_security_receive_port().unwrap(),
        );
        assert_ne!(local_send.port(), local_receive.port());

        channel
            .activate_security(
                ImsRoute {
                    local_addr: local_send,
                    pcscf_addr: pcscf_client.local_addr().unwrap(),
                    transport: SipTransport::Udp,
                },
                local_receive,
                pcscf_send.local_addr().unwrap(),
                Some("ipsec-3gpp".to_string()),
            )
            .unwrap();
        channel.send_sip(b"protected register").await.unwrap();
        let mut request = [0u8; 64];
        let (read, peer) = pcscf_client.recv_from(&mut request).await.unwrap();
        assert_eq!(&request[..read], b"protected register");
        assert_eq!(peer, local_send);

        pcscf_send
            .send_to(b"protected response", local_receive)
            .await
            .unwrap();
        let response = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        assert_eq!(response, b"protected response");
    }

    #[tokio::test]
    async fn requeued_datagrams_are_delivered_fifo_before_fresh_transport_data() {
        let server = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let route = ImsRoute {
            local_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            pcscf_addr: server.local_addr().unwrap(),
            transport: SipTransport::Udp,
        };
        let mut channel = VolteSipChannel::bind(route, None, None).unwrap();
        let client_addr = channel.local_addr().unwrap();

        channel.requeue(b"first".to_vec());
        channel.requeue(b"second".to_vec());
        server.send_to(b"fresh", client_addr).await.unwrap();

        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            b"first"
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            b"second"
        );
        assert_eq!(
            channel.recv_sip(Duration::from_secs(1)).await.unwrap(),
            b"fresh"
        );
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
