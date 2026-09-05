//! Native VoLTE SIP channel over the dedicated IMS bearer.
//!
//! The socket is explicitly bound to the bearer interface so IMS traffic can
//! never escape through the host's normal Wi-Fi/default route. Once xfrm SAs
//! and policies are installed, the same UDP API transparently carries ESP-
//! protected SIP. A 401/407 sec-agree challenge may replace the socket with a
//! channel bound to the negotiated client port.

use std::{io, net::SocketAddr, time::Duration};

#[cfg(test)]
use socket2::{Domain, Protocol, Socket, Type};
#[cfg(test)]
use std::net::UdpSocket as StdUdpSocket;
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
    /// Protected UE-originated traffic uses a separately reserved port_c.
    /// Keeping this socket open from the initial SM1 until activation makes the
    /// Security-Client offer, XFRM selector, and actual UDP source identical.
    reserved_send_socket: Option<ReservedSendSocket>,
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
    /// The old channel remains alive until a challenged REGISTER completes.
    staged_security: Option<ChannelSecurity>,
    /// P-CSCF may still send on the old SA after 200 OK (TS 33.203 7.4.2a).
    /// Retain one previous association until the next registration procedure.
    retired_security: Option<ChannelSecurity>,
}

struct ChannelSecurity {
    send_socket: Option<UdpSocket>,
    receive_socket: Option<UdpSocket>,
    route: ImsRoute,
    advertised_local_port: Option<u16>,
    security_verify: Option<String>,
}

enum ReservedSendSocket {
    #[cfg(test)]
    Host(Socket),
    Worker(UdpSocket),
}

enum ReservedReceiveSocket {
    #[cfg(test)]
    Host(Socket),
    Worker(UdpSocket),
}

impl VolteSipChannel {
    #[cfg(test)]
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
            reserved_send_socket: None,
            reserved_receive_socket: None,
            requeued: ImsRequeue::default(),
            route,
            advertised_local_port: None,
            interface: interface.map(ToOwned::to_owned),
            security_verify,
            staged_security: None,
            retired_security: None,
        })
    }

    #[cfg(test)]
    /// Reserve a second local UDP port for protected packets sent by the
    /// P-CSCF.  The initial REGISTER socket remains the protected send socket.
    pub fn reserve_security_receive_port(&mut self) -> Result<u16, ImsError> {
        self.reserve_security_receive_port_at(0)
    }

    #[cfg(test)]
    fn reserve_security_receive_port_at(&mut self, port: u16) -> Result<u16, ImsError> {
        if let Some(socket) = self.reserved_receive_socket.as_ref() {
            return match socket {
                ReservedReceiveSocket::Host(socket) => socket_port(socket),
                ReservedReceiveSocket::Worker(socket) => socket
                    .local_addr()
                    .map(|addr| addr.port())
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed")),
            };
        }
        let local = SocketAddr::new(self.route.local_addr.ip(), port);
        let socket = build_bound_socket_excluding(local, self.interface.as_deref(), 0)
            .map_err(|_| ImsError::new("volte_channel_receive_reserve_failed"))?;
        let port = socket_port(&socket)?;
        self.reserved_receive_socket = Some(ReservedReceiveSocket::Host(socket));
        Ok(port)
    }

    /// Use host sockets only in tests, but wrap the resulting descriptors just
    /// like worker-created sockets so production reservation/activation runs.
    #[cfg(test)]
    pub(crate) fn reserve_security_ports_for_test(&mut self, server_port: u16) -> (u16, u16) {
        let receive = self.reserve_security_receive_port_at(server_port).unwrap();
        let send = self.reserve_security_send_port(receive).unwrap();
        let Some(ReservedSendSocket::Host(send_socket)) = self.reserved_send_socket.take() else {
            panic!("expected host reservation");
        };
        let Some(ReservedReceiveSocket::Host(receive_socket)) = self.reserved_receive_socket.take()
        else {
            panic!("expected host reservation");
        };
        send_socket.set_nonblocking(true).unwrap();
        receive_socket.set_nonblocking(true).unwrap();
        self.reserved_send_socket = Some(ReservedSendSocket::Worker(
            UdpSocket::from_std(send_socket.into()).unwrap(),
        ));
        self.reserved_receive_socket = Some(ReservedReceiveSocket::Worker(
            UdpSocket::from_std(receive_socket.into()).unwrap(),
        ));
        (send, receive)
    }

    /// Create the initial SIP channel inside the mandatory per-line UE worker.
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
            reserved_send_socket: None,
            reserved_receive_socket: None,
            requeued: ImsRequeue::default(),
            route,
            advertised_local_port: None,
            interface: interface.map(ToOwned::to_owned),
            security_verify,
            staged_security: None,
            retired_security: None,
        })
    }

    /// Reserve the protected UE/client port (`port_uc`) before SM1 is sent.
    ///
    /// TS 33.203 requires the ports declared in Security-Client during the
    /// initial unprotected REGISTER to be the same ports used after the
    /// Security-Server negotiation. The plain 5060 socket is therefore kept
    /// for SM1, while this independent socket is held until activation.
    #[cfg(test)]
    pub fn reserve_security_send_port(&mut self, avoid_port: u16) -> Result<u16, ImsError> {
        if let Some(socket) = self.reserved_send_socket.as_ref() {
            return match socket {
                ReservedSendSocket::Host(socket) => socket_port(socket),
                ReservedSendSocket::Worker(socket) => socket
                    .local_addr()
                    .map(|addr| addr.port())
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed")),
            };
        }
        let local = SocketAddr::new(self.route.local_addr.ip(), 0);
        let socket = build_bound_socket_excluding(local, self.interface.as_deref(), avoid_port)
            .map_err(|_| ImsError::new("volte_channel_send_reserve_failed"))?;
        let port = socket_port(&socket)?;
        self.reserved_send_socket = Some(ReservedSendSocket::Host(socket));
        Ok(port)
    }

    /// Reserve the protected UE/client port (`port_uc`) inside the UE worker.
    pub async fn reserve_security_send_port_in_worker(
        &mut self,
        worker: &UeWorkerHandle,
        avoid_port: u16,
    ) -> Result<u16, ImsError> {
        self.reserve_security_send_port_in_worker_at(worker, 0, avoid_port)
            .await
    }

    /// Reuse exactly the port already advertised by this REGISTER procedure,
    /// including after an additional AKA challenge; never invent a new offer
    /// in response to Security-Server.
    pub async fn reserve_security_send_port_in_worker_at(
        &mut self,
        worker: &UeWorkerHandle,
        requested_port: u16,
        avoid_port: u16,
    ) -> Result<u16, ImsError> {
        if let Some(socket) = self.reserved_send_socket.as_ref() {
            return match socket {
                #[cfg(test)]
                ReservedSendSocket::Host(socket) => socket_port(socket),
                ReservedSendSocket::Worker(socket) => socket
                    .local_addr()
                    .map(|addr| addr.port())
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed")),
            };
        }
        let attempts = if requested_port == 0 { 8 } else { 1 };
        for _ in 0..attempts {
            let local = SocketAddr::new(self.route.local_addr.ip(), requested_port);
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
            if port != 5060
                && port != 5061
                && port != avoid_port
                && port != self.route.local_addr.port()
            {
                self.reserved_send_socket = Some(ReservedSendSocket::Worker(socket));
                return Ok(port);
            }
        }
        Err(ImsError::new("volte_channel_send_reserve_invalid_port"))
    }

    /// Reserve the protected receive port using the worker socket factory.
    pub async fn reserve_security_receive_port_in_worker(
        &mut self,
        worker: &UeWorkerHandle,
    ) -> Result<u16, ImsError> {
        self.reserve_security_receive_port_in_worker_at(worker, 0)
            .await
    }

    /// Reserve the protected server port selected by a previous security
    /// association. TS 33.203 keeps `port_us` stable across authenticated
    /// re-registration, so recovery must be able to bind that exact port
    /// instead of silently allocating a new one.
    pub async fn reserve_security_receive_port_in_worker_at(
        &mut self,
        worker: &UeWorkerHandle,
        requested_port: u16,
    ) -> Result<u16, ImsError> {
        if let Some(socket) = self.reserved_receive_socket.as_ref() {
            return match socket {
                #[cfg(test)]
                ReservedReceiveSocket::Host(socket) => socket_port(socket),
                ReservedReceiveSocket::Worker(socket) => socket
                    .local_addr()
                    .map(|addr| addr.port())
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed")),
            };
        }
        if requested_port != 0 && (requested_port == 5060 || requested_port == 5061) {
            return Err(ImsError::new("volte_channel_receive_reserved_sip_port"));
        }
        let attempts = if requested_port == 0 { 8 } else { 1 };
        for _ in 0..attempts {
            let local = SocketAddr::new(self.route.local_addr.ip(), requested_port);
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
            if port != 5060 && port != 5061 {
                self.reserved_receive_socket = Some(ReservedReceiveSocket::Worker(socket));
                return Ok(port);
            }
        }
        Err(ImsError::new("volte_channel_receive_reserve_invalid_port"))
    }

    #[cfg(test)]
    /// Activate the two protected UDP directions negotiated by sec-agree:
    /// UE client -> P-CSCF server, and P-CSCF client -> UE server.
    pub fn activate_security(
        &mut self,
        send_route: ImsRoute,
        receive_local: SocketAddr,
        receive_remote: SocketAddr,
        security_verify: Option<String>,
    ) -> Result<(), ImsError> {
        let reserved_send = self
            .reserved_send_socket
            .take()
            .ok_or_else(|| ImsError::new("volte_channel_send_not_reserved"))?;
        let send_socket = match reserved_send {
            ReservedSendSocket::Host(socket) => {
                if socket_addr(&socket)? != send_route.local_addr {
                    return Err(ImsError::new("volte_channel_send_port_mismatch"));
                }
                connect_bound_socket(socket, send_route.pcscf_addr)
                    .map_err(|_| ImsError::new("volte_channel_send_connect_failed"))?
            }
            ReservedSendSocket::Worker(_) => {
                return Err(ImsError::new("volte_channel_worker_send_requires_async"));
            }
        };
        let reserved_receive = self
            .reserved_receive_socket
            .take()
            .ok_or_else(|| ImsError::new("volte_channel_receive_not_reserved"))?;
        let receive_socket = match reserved_receive {
            ReservedReceiveSocket::Host(socket) => {
                if socket_addr(&socket)? != receive_local {
                    return Err(ImsError::new("volte_channel_receive_port_mismatch"));
                }
                connect_bound_socket(socket, receive_remote)
                    .map_err(|_| ImsError::new("volte_channel_receive_connect_failed"))?
            }
            ReservedReceiveSocket::Worker(_) => {
                return Err(ImsError::new("volte_channel_worker_receive_requires_async"));
            }
        };

        self.stage_security(
            send_socket,
            receive_socket,
            send_route,
            receive_local.port(),
            security_verify,
        )
    }

    /// Worker equivalent of [`Self::activate_security`]. Both protected sockets
    /// were reserved before SM1; activation only connects them to the two
    /// P-CSCF ports after XFRM has been installed.
    pub async fn activate_security_in_worker(
        &mut self,
        send_route: ImsRoute,
        receive_local: SocketAddr,
        receive_remote: SocketAddr,
        security_verify: Option<String>,
        _worker: &UeWorkerHandle,
    ) -> Result<(), ImsError> {
        let reserved_send = self
            .reserved_send_socket
            .take()
            .ok_or_else(|| ImsError::new("volte_channel_send_not_reserved"))?;
        let send_socket = match reserved_send {
            ReservedSendSocket::Worker(socket) => {
                let local = socket
                    .local_addr()
                    .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
                if local != send_route.local_addr {
                    return Err(ImsError::new("volte_channel_send_port_mismatch"));
                }
                socket
                    .connect(send_route.pcscf_addr)
                    .await
                    .map_err(|_| ImsError::new("volte_channel_send_connect_failed"))?;
                socket
            }
            #[cfg(test)]
            ReservedSendSocket::Host(_) => {
                return Err(ImsError::new("volte_channel_worker_send_mismatch"));
            }
        };
        let reserved_receive = self
            .reserved_receive_socket
            .take()
            .ok_or_else(|| ImsError::new("volte_channel_receive_not_reserved"))?;
        let receive_socket = match reserved_receive {
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
            #[cfg(test)]
            ReservedReceiveSocket::Host(_) => {
                return Err(ImsError::new("volte_channel_worker_receive_mismatch"));
            }
        };

        self.stage_security(
            send_socket,
            receive_socket,
            send_route,
            receive_local.port(),
            security_verify,
        )
    }

    fn stage_security(
        &mut self,
        send_socket: UdpSocket,
        receive_socket: UdpSocket,
        mut route: ImsRoute,
        advertised_port: u16,
        security_verify: Option<String>,
    ) -> Result<(), ImsError> {
        if self.staged_security.is_some() {
            return Err(ImsError::new("volte_channel_security_update_pending"));
        }
        route.local_addr = send_socket
            .local_addr()
            .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?;
        self.staged_security = Some(ChannelSecurity {
            send_socket: self.send_socket.take(),
            receive_socket: self.receive_socket.take(),
            route: self.route,
            advertised_local_port: self.advertised_local_port,
            security_verify: self.security_verify.take(),
        });
        self.send_socket = Some(send_socket);
        self.receive_socket = Some(receive_socket);
        self.route = route;
        self.advertised_local_port = Some(advertised_port);
        self.security_verify = security_verify;
        Ok(())
    }

    /// Commit only after the matching final 2xx. Keep the preceding protected
    /// channel readable: the P-CSCF may still use it until it sees further
    /// traffic on the new SA. Its XFRM plan has the same retirement lifetime.
    pub fn commit_security(&mut self) {
        if let Some(previous) = self.staged_security.take() {
            self.retired_security = previous.security_verify.is_some().then_some(previous);
        }
        self.discard_reserved_security_ports();
    }

    /// Restore sockets AND headers together after a failed tentative exchange.
    /// The caller removes only the tentative XFRM plan, never the active plan.
    pub fn rollback_security(&mut self) {
        if let Some(previous) = self.staged_security.take() {
            self.send_socket = previous.send_socket;
            self.receive_socket = previous.receive_socket;
            self.route = previous.route;
            self.advertised_local_port = previous.advertised_local_port;
            self.security_verify = previous.security_verify;
        }
        self.discard_reserved_security_ports();
    }

    pub fn discard_retired_security(&mut self) {
        self.retired_security = None;
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

    /// Restore the local port used in Via/Contact after sec-agree from the
    /// binding that was negotiated for this live session.
    ///
    /// The protected receive socket is deliberately kept separate from the
    /// client/send socket. A refresh must continue to advertise `port_us`,
    /// even when a worker/socket lifecycle path has lost the in-memory
    /// advertised-port marker. This does not touch the bearer, sockets, or
    /// XFRM state; it only repairs the SIP header route used by this channel.
    pub fn sync_protected_advertised_port(&mut self, port: u16) {
        let observed = self.advertised_route().local_addr.port();
        if observed != port {
            tracing::warn!(
                observed_local_port = observed,
                negotiated_server_port = port,
                "VoLTE protected SIP advertised port drift detected; correcting refresh route"
            );
        }
        self.advertised_local_port = Some(port);
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

    /// Release ports reserved for a standards-compliant protected refresh that
    /// completed without an AKA challenge. In that case the existing SA stays
    /// active and the new Security-Client offer is discarded.
    pub fn discard_reserved_security_ports(&mut self) {
        self.reserved_send_socket = None;
        self.reserved_receive_socket = None;
    }

    pub fn interface(&self) -> Option<&str> {
        self.interface.as_deref()
    }

    async fn recv_fresh(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        let current = recv_socket_pair(
            self.receive_socket.as_ref(),
            self.send_socket.as_ref(),
            timeout,
        );
        let Some(previous) = self
            .staged_security
            .as_ref()
            .or(self.retired_security.as_ref())
        else {
            return current.await;
        };
        let old = recv_socket_pair(
            previous.receive_socket.as_ref(),
            previous.send_socket.as_ref(),
            timeout,
        );
        // Failure responses during AKA use the old SA. Old non-REGISTER frames
        // must also remain readable so the transaction driver can requeue them.
        // A transient ICMP error on either association must not cancel the other.
        tokio::pin!(current, old);
        let deadline = tokio::time::Instant::now() + timeout;
        tokio::select! {
            result = &mut current => match result {
                Ok(frame) => Ok(frame),
                Err(error) => match tokio::time::timeout_at(deadline, &mut old).await {
                    Ok(Ok(frame)) => Ok(frame),
                    _ => Err(error),
                },
            },
            result = &mut old => match result {
                Ok(frame) => Ok(frame),
                Err(_) => tokio::time::timeout_at(deadline, &mut current).await
                    .map_err(|_| ImsError::new("volte_channel_read_timeout"))?,
            },
        }
    }
}

async fn recv_one(socket: &UdpSocket, timeout: Duration) -> Result<Vec<u8>, ImsError> {
    let mut frame = vec![0u8; MAX_SIP_DATAGRAM];
    let read = tokio::time::timeout(timeout, socket.recv(&mut frame))
        .await
        .map_err(|_| ImsError::new("volte_channel_read_timeout"))?
        .map_err(|error| map_socket_read_error("single", error))?;
    frame.truncate(read);
    Ok(frame)
}

/// A connected UDP socket can report an ICMP error from the previous send as
/// an immediate `recv` error.  That is not a SIP transaction failure: the next
/// identical REGISTER retransmission may still receive the response (and this
/// is exactly the situation covered by RFC 3261's UDP Timer E retransmission).
/// Keep this distinction in the channel error code so the shared REGISTER
/// driver can wait for the next retransmission instead of aborting after a few
/// milliseconds.
fn map_socket_read_error(path: &'static str, error: io::Error) -> ImsError {
    let transient = matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::NetworkDown
            | io::ErrorKind::NotConnected
    );
    tracing::debug!(
        receive_path = path,
        error_kind = ?error.kind(),
        error = %error,
        transient,
        "VoLTE SIP socket read returned an error"
    );
    if transient {
        ImsError::new("volte_channel_read_retryable")
    } else {
        ImsError::new("volte_channel_read_failed")
    }
}

/// A protected IMS association has two receive-capable tuples. Responses to
/// UE-originated transactions can return to the client/send port, while
/// terminating requests arrive on the server/receive port.
async fn recv_socket_pair(
    receive: Option<&UdpSocket>,
    send: Option<&UdpSocket>,
    timeout: Duration,
) -> Result<Vec<u8>, ImsError> {
    match (receive, send) {
        (Some(receive), Some(send)) => recv_protected(receive, send, timeout).await,
        (Some(socket), None) | (None, Some(socket)) => recv_one(socket, timeout).await,
        (None, None) => Err(ImsError::new("volte_channel_receive_socket_missing")),
    }
}

async fn recv_protected(
    receive_socket: &UdpSocket,
    send_socket: &UdpSocket,
    timeout: Duration,
) -> Result<Vec<u8>, ImsError> {
    let mut receive_frame = vec![0u8; MAX_SIP_DATAGRAM];
    let mut send_frame = vec![0u8; MAX_SIP_DATAGRAM];
    let (read, from_server) = tokio::time::timeout(timeout, async {
        tokio::select! {
            read = receive_socket.recv(&mut receive_frame) => {
                (read, true)
            }
            read = send_socket.recv(&mut send_frame) => {
                (read, false)
            }
        }
    })
    .await
    .map_err(|_| ImsError::new("volte_channel_read_timeout"))?;
    let read = read.map_err(|error| {
        map_socket_read_error(
            if from_server {
                "protected_server"
            } else {
                "protected_client"
            },
            error,
        )
    })?;
    let (mut frame, receive_path) = if from_server {
        (receive_frame, "protected_server")
    } else {
        (send_frame, "protected_client")
    };
    frame.truncate(read);
    tracing::debug!(
        receive_path,
        frame_bytes = read,
        "VoLTE protected SIP frame received"
    );
    Ok(frame)
}

fn header_parameter_port(frame: &[u8], header_name: &str, parameter_name: &str) -> Option<u16> {
    for value in super::sip::header_values(frame, header_name) {
        for parameter in value.split(';') {
            let Some((name, value)) = parameter.trim().split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case(parameter_name) {
                if let Ok(port) = value.trim().trim_matches('"').parse() {
                    return Some(port);
                }
            }
        }
    }
    None
}

impl ImsChannel for VolteSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        // Log the actual REGISTER transmit tuple, not only the route used to
        // construct Via/Contact.  In a protected VoLTE session these are
        // intentionally different: headers advertise port_us while the UDP
        // packet leaves port_uc.  Keeping both values in one log record makes
        // an unanswered refresh distinguishable from a SIP/header mismatch.
        if super::sip::is_request(frame, "REGISTER") {
            let advertised = self.advertised_route();
            let send_route = self.send_route();
            let actual_send = self
                .send_socket
                .as_ref()
                .and_then(|socket| socket.local_addr().ok());
            tracing::info!(
                register_cseq = super::sip::header_value(frame, "CSeq"),
                advertised_port = advertised.local_addr.port(),
                actual_send_port = actual_send.map(|address| address.port()),
                send_route_port = send_route.local_addr.port(),
                advertised_pcscf = %advertised.pcscf_addr,
                send_pcscf = %send_route.pcscf_addr,
                security_client_port_c = header_parameter_port(frame, "Security-Client", "port-c"),
                security_client_port_s = header_parameter_port(frame, "Security-Client", "port-s"),
                security_verify_port_c = header_parameter_port(frame, "Security-Verify", "port-c"),
                security_verify_port_s = header_parameter_port(frame, "Security-Verify", "port-s"),
                security_client_present = !super::sip::header_values(frame, "Security-Client").is_empty(),
                security_verify_present = !super::sip::header_values(frame, "Security-Verify").is_empty(),
                protected_channel = self.security_verify.is_some(),
                "VoLTE REGISTER transmit path"
            );
        }
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

#[cfg(test)]
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

#[cfg(test)]
fn build_bound_socket_excluding(
    local: SocketAddr,
    interface: Option<&str>,
    avoid_port: u16,
) -> io::Result<Socket> {
    for _ in 0..8 {
        let socket = build_bound_socket(local, interface)?;
        let port = socket
            .local_addr()?
            .as_socket()
            .map(|addr| addr.port())
            .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "missing local port"))?;
        if port != 5060 && port != 5061 && port != avoid_port {
            return Ok(socket);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "kernel repeatedly selected a reserved SIP port",
    ))
}

#[cfg(test)]
fn build_bound_socket(local: SocketAddr, interface: Option<&str>) -> io::Result<Socket> {
    let socket = Socket::new(Domain::for_address(local), Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    bind_to_interface(&socket, interface)?;
    socket.bind(&local.into())?;
    Ok(socket)
}

#[cfg(test)]
fn connect_bound_socket(socket: Socket, remote: SocketAddr) -> io::Result<UdpSocket> {
    socket.connect(&remote.into())?;
    socket.set_nonblocking(true)?;
    let std_socket: StdUdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}

#[cfg(test)]
fn socket_addr(socket: &Socket) -> Result<SocketAddr, ImsError> {
    socket
        .local_addr()
        .map_err(|_| ImsError::new("volte_channel_local_addr_failed"))?
        .as_socket()
        .ok_or_else(|| ImsError::new("volte_channel_local_addr_failed"))
}

#[cfg(test)]
fn socket_port(socket: &Socket) -> Result<u16, ImsError> {
    Ok(socket_addr(socket)?.port())
}

#[cfg(all(test, target_os = "linux"))]
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

#[cfg(all(test, not(target_os = "linux")))]
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
        let plain_send = channel.local_addr().unwrap();
        let local_receive = SocketAddr::new(
            plain_send.ip(),
            channel.reserve_security_receive_port().unwrap(),
        );
        let local_send = SocketAddr::new(
            plain_send.ip(),
            channel
                .reserve_security_send_port(local_receive.port())
                .unwrap(),
        );
        assert_ne!(plain_send.port(), local_send.port());
        assert_ne!(local_send.port(), local_receive.port());
        assert_ne!(local_send.port(), 5060);
        assert_ne!(local_send.port(), 5061);

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
            .send_to(b"protected incoming request", local_receive)
            .await
            .unwrap();
        let incoming = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        assert_eq!(incoming, b"protected incoming request");

        // Responses to UE-originated transactions return to port_uc.
        pcscf_client
            .send_to(b"protected register response", local_send)
            .await
            .unwrap();
        let response = channel.recv_sip(Duration::from_secs(1)).await.unwrap();
        assert_eq!(response, b"protected register response");

        // A refresh can repair only the SIP advertisement without replacing
        // either protected socket. Via/Contact use port_us, while the actual
        // datagram must still leave the original port_uc send socket.
        channel.sync_protected_advertised_port(local_receive.port());
        assert_eq!(channel.route().local_addr, local_receive);
        assert_eq!(channel.send_route().local_addr, local_send);
        assert_eq!(channel.local_addr().unwrap(), local_send);
        channel.send_sip(b"protected refresh").await.unwrap();
        let (read, peer) = pcscf_client.recv_from(&mut request).await.unwrap();
        assert_eq!(&request[..read], b"protected refresh");
        assert_eq!(peer, local_send);
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
