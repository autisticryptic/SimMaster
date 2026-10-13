//! Cellular reliable transport on worker-owned TCP/TLS sockets. The UDP
//! implementation stays separate; no connection or security downgrade fallback.
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::{TcpListener, TcpSocket};
use crate::{connectivity::core::{access::ImsRequeue, context::{ImsRoute, SipTransport},
    ims_access::ImsAccess, outbound::OutboundFlow, reg_event::RegEventBinding,
    register_response::RegisterArtifacts, ImsError}, services::ue_worker::{UeSocket, UeSocketSpec, UeWorkerBinding, UeWorkerHandle}};
use super::super::errors::code;
use super::sip_stream::{ResponseRoutes, SipStream, StreamAssociation, StreamFrame};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

struct Association {
    streams: StreamAssociation,
    route: ImsRoute,
    listener: Option<Arc<TcpListener>>,
    verify: Option<String>,
    advertised_port: Option<u16>,
    outbound: OutboundFlow,
}

enum ReservedReceive {
    Bound(TcpSocket),
    Existing(Arc<TcpListener>),
}
impl ReservedReceive {
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        match self { Self::Bound(socket) => socket.local_addr(), Self::Existing(listener) => listener.local_addr() }
    }
    fn listen(self) -> Result<Arc<TcpListener>, ImsError> {
        match self {
            Self::Bound(socket) => socket.listen(8).map(Arc::new)
                .map_err(|_| ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED)),
            Self::Existing(listener) => Ok(listener),
        }
    }
}

pub(super) struct CellularStreamTransport {
    current: Association,
    staged: Option<Association>,
    retired: Option<Association>,
    reserved_send: Option<TcpSocket>,
    reserved_receive: Option<ReservedReceive>,
    interface: String,
    worker: UeWorkerBinding,
    responses: ResponseRoutes,
    requeued: ImsRequeue,
}

impl CellularStreamTransport {
    pub(super) async fn bind(route: ImsRoute, authority: Option<&str>, interface: &str, worker: &UeWorkerHandle) -> Result<Self, ImsError> {
        let binding = worker.bind();
        ensure_current(&binding)?;
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        let operation = async {
            let spec = UeSocketSpec::tcp_connected(route.local_addr, route.pcscf_addr, Some(interface.into()), CONNECT_TIMEOUT.as_secs());
            let socket = match worker.create_socket(spec).await { Ok(UeSocket::Tcp(socket)) => socket,
                _ => return Err(ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED)) };
            ensure_current(&binding)?;
            let client = if route.transport == SipTransport::Tls {
                SipStream::tls(socket, authority.ok_or_else(|| ImsError::new(code::TLS_AUTHORITY_INVALID))?,
                    deadline.saturating_duration_since(tokio::time::Instant::now())).await?
            } else { SipStream::plain(socket)? };
            ensure_current(&binding)?;
            let route = ImsRoute { local_addr: client.local_addr(), ..route };
            Ok(Self { current: Association { streams: StreamAssociation { client, server: None, expected_server: None },
                route, listener: None, verify: None, advertised_port: None, outbound: OutboundFlow::default() },
                staged: None, retired: None, reserved_send: None, reserved_receive: None,
                interface: interface.into(), worker: binding.clone(), responses: ResponseRoutes::default(), requeued: ImsRequeue::default() })
        };
        tokio::time::timeout_at(deadline, operation).await.map_err(|_| ImsError::new(
            if route.transport == SipTransport::Tls { code::TLS_HANDSHAKE_TIMEOUT } else { code::CHANNEL_STREAM_CONNECT_FAILED }
        ))?
    }

    #[cfg(test)]
    pub(super) fn from_tcp(socket: tokio::net::TcpStream, route: ImsRoute, worker: UeWorkerBinding) -> Result<Self, ImsError> {
        let client = SipStream::plain(socket)?;
        let route = ImsRoute { local_addr: client.local_addr(), ..route };
        Ok(Self { current: Association { streams: StreamAssociation { client, server: None, expected_server: None },
            route, listener: None, verify: None, advertised_port: None, outbound: OutboundFlow::default() },
            staged: None, retired: None, reserved_send: None, reserved_receive: None,
            interface: "lo".into(), worker, responses: ResponseRoutes::default(), requeued: ImsRequeue::default() })
    }

    #[cfg(test)]
    pub(super) async fn reserve_for_test(&mut self) -> (u16, u16) {
        let receive = match &self.current.listener {
            Some(listener) => ReservedReceive::Existing(listener.clone()),
            None => {
                let socket = if self.current.route.local_addr.is_ipv4() { TcpSocket::new_v4().unwrap() } else { TcpSocket::new_v6().unwrap() };
                socket.set_reuseaddr(true).unwrap();
                socket.bind(SocketAddr::new(self.current.route.local_addr.ip(), 0)).unwrap();
                ReservedReceive::Bound(socket)
            }
        };
        let server_port = receive.local_addr().unwrap().port();
        let socket = if self.current.route.local_addr.is_ipv4() { TcpSocket::new_v4().unwrap() } else { TcpSocket::new_v6().unwrap() };
        socket.set_reuseaddr(true).unwrap();
        socket.bind(SocketAddr::new(self.current.route.local_addr.ip(), 0)).unwrap();
        let client_port = socket.local_addr().unwrap().port();
        self.reserved_send = Some(socket); self.reserved_receive = Some(receive);
        (client_port, server_port)
    }

    pub(super) fn route(&self) -> ImsRoute {
        let mut route = self.current.route;
        if let Some(port) = self.current.advertised_port { route.local_addr.set_port(port); }
        route
    }
    pub(super) fn send_route(&self) -> ImsRoute { self.current.route }
    pub(super) fn security_verify(&self) -> Option<&str> { self.current.verify.as_deref() }
    pub(super) fn sync_advertised_port(&mut self, port: u16) { self.current.advertised_port = Some(port); }
    pub(super) fn configure_outbound(&mut self, line: &str, instance: &str, enabled: bool) {
        self.current.outbound.configure(line, ImsAccess::Cellular, instance, enabled);
    }
    pub(super) fn registered(&mut self, response: &[u8], expires: u32) -> Result<RegisterArtifacts, ImsError> {
        ensure_current(&self.worker)?;
        if !self.current.streams.client.is_usable() { return Err(ImsError::new(code::CHANNEL_STREAM_CLOSED)); }
        self.current.outbound.registered(response, true, expires)
    }
    pub(super) fn reg_event_binding(&self) -> Option<RegEventBinding> { self.current.outbound.reg_event_binding() }
    pub(super) fn invalidate_registration(&mut self) { self.current.outbound.disable(); }

    pub(super) async fn reserve_send(&mut self, requested_port: u16, avoid: u16) -> Result<u16, ImsError> {
        ensure_current(&self.worker)?;
        if let Some(socket) = &self.reserved_send {
            let port = socket.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?.port();
            if port == avoid || requested_port != 0 && port != requested_port { return Err(ImsError::new(code::CHANNEL_SEND_PORT_MISMATCH)); }
            return Ok(port);
        }
        for _ in 0..if requested_port == 0 { 8 } else { 1 } {
            let local = SocketAddr::new(self.current.route.local_addr.ip(), requested_port);
            let spec = UeSocketSpec::tcp_bound(local, Some(self.interface.clone()));
            let socket = match self.worker.worker().create_socket(spec).await { Ok(UeSocket::TcpBound(socket)) => socket,
                _ => return Err(ImsError::new(code::CHANNEL_WORKER_SOCKET_FAILED)) };
            ensure_current(&self.worker)?;
            let port = socket.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?.port();
            if port != avoid && port != self.current.route.local_addr.port() && !matches!(port, 5060 | 5061) {
                self.reserved_send = Some(socket); return Ok(port);
            }
        }
        Err(ImsError::new(code::CHANNEL_SEND_RESERVE_INVALID_PORT))
    }

    pub(super) async fn reserve_receive(&mut self, requested_port: u16) -> Result<u16, ImsError> {
        ensure_current(&self.worker)?;
        if let Some(listener) = &self.reserved_receive {
            let port = listener.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?.port();
            if requested_port != 0 && port != requested_port { return Err(ImsError::new(code::CHANNEL_RECEIVE_PORT_MISMATCH)); }
            return Ok(port);
        }
        if let Some(listener) = &self.current.listener {
            let port = listener.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?.port();
            if requested_port != 0 && requested_port != port { return Err(ImsError::new(code::CHANNEL_RECEIVE_PORT_MISMATCH)); }
            // port_us is stable across refresh. Share the exact listener, never
            // SO_REUSEPORT a second listener which could steal the old flow.
            self.reserved_receive = Some(ReservedReceive::Existing(listener.clone())); return Ok(port);
        }
        for _ in 0..if requested_port == 0 { 8 } else { 1 } {
            let local = SocketAddr::new(self.current.route.local_addr.ip(), requested_port);
            // Reserve but DO NOT listen before the peer challenge authorizes
            // TCP XFRM. Otherwise pre-SA plaintext could be queued and read
            // after activation as if it came through the protected listener.
            let spec = UeSocketSpec::tcp_bound(local, Some(self.interface.clone()));
            let socket = match self.worker.worker().create_socket(spec).await { Ok(UeSocket::TcpBound(socket)) => socket,
                _ => return Err(ImsError::new(code::CHANNEL_WORKER_SOCKET_FAILED)) };
            ensure_current(&self.worker)?;
            let port = socket.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?.port();
            if !matches!(port, 5060 | 5061) { self.reserved_receive = Some(ReservedReceive::Bound(socket)); return Ok(port); }
        }
        Err(ImsError::new(code::CHANNEL_RECEIVE_RESERVE_INVALID_PORT))
    }

    pub(super) async fn activate(&mut self, send_route: ImsRoute, receive_local: SocketAddr, receive_remote: SocketAddr, verify: Option<String>) -> Result<(), ImsError> {
        ensure_current(&self.worker)?;
        if self.current.route.transport != SipTransport::Tcp || send_route.transport != SipTransport::Tcp {
            return Err(ImsError::new(code::TLS_IPSEC_UNSUPPORTED));
        }
        if self.staged.is_some() { return Err(ImsError::new(code::CHANNEL_SECURITY_UPDATE_PENDING)); }
        let socket = self.reserved_send.take().ok_or_else(|| ImsError::new(code::CHANNEL_SEND_NOT_RESERVED))?;
        let listener = self.reserved_receive.take().ok_or_else(|| ImsError::new(code::CHANNEL_RECEIVE_NOT_RESERVED))?;
        if socket.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))? != send_route.local_addr {
            return Err(ImsError::new(code::CHANNEL_SEND_PORT_MISMATCH));
        }
        if listener.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))? != receive_local {
            return Err(ImsError::new(code::CHANNEL_RECEIVE_PORT_MISMATCH));
        }
        // Both listen and connect happen only AFTER the authenticator installs
        // the negotiated selectors. The bound socket keeps the frozen SM1 port
        // and its worker namespace; listen does not create a host socket.
        let listener = listener.listen()?;
        let socket = tokio::time::timeout(CONNECT_TIMEOUT, socket.connect(send_route.pcscf_addr)).await
            .map_err(|_| ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED))?
            .map_err(|_| ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED))?;
        ensure_current(&self.worker)?;
        let client = SipStream::plain(socket)?;
        let next = Association { streams: StreamAssociation { client, server: None, expected_server: Some(receive_remote) },
            route: send_route, listener: Some(listener), verify, advertised_port: Some(receive_local.port()),
            outbound: self.current.outbound.replacement() };
        self.staged = Some(std::mem::replace(&mut self.current, next));
        Ok(())
    }

    pub(super) fn commit(&mut self) {
        if let Some(mut previous) = self.staged.take() {
            previous.outbound.disable();
            self.retired = previous.verify.is_some().then_some(previous);
        }
        self.discard_reserved();
    }
    pub(super) fn rollback(&mut self) {
        if let Some(previous) = self.staged.take() { self.current.outbound.disable(); self.current = previous; }
        self.discard_reserved();
    }
    pub(super) fn discard_retired(&mut self) { self.retired = None; }
    pub(super) fn discard_reserved(&mut self) { self.reserved_send = None; self.reserved_receive = None; }

    fn connection(&mut self, id: u64) -> Option<&mut SipStream> {
        if self.current.streams.connection(id).is_some() { return self.current.streams.connection(id); }
        if let Some(old) = self.staged.as_mut() { if let Some(stream) = old.streams.connection(id) { return Some(stream); } }
        self.retired.as_mut().and_then(|old| old.streams.connection(id))
    }

    pub(super) async fn send(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        ensure_current(&self.worker)?;
        let prepared = self.current.outbound.prepare(frame)?;
        if let Some(id) = self.responses.response_connection(&prepared)? {
            self.connection(id).ok_or_else(|| ImsError::new(code::CHANNEL_STREAM_RESPONSE_OWNER_MISSING))?.send(&prepared).await
        } else {
            let result = self.current.streams.client.send(&prepared).await;
            if result.is_err() && self.staged.is_none() {
                return Err(self.current.outbound.fail("ims_outbound_flow_failed"));
            }
            result
        }
    }

    async fn maintain(&mut self) -> Result<(), ImsError> {
        ensure_current(&self.worker)?;
        if let Some(packet) = self.current.outbound.poll(std::time::Instant::now())? {
            if self.current.streams.client.send(&packet).await.is_err() {
                return Err(self.current.outbound.fail("ims_outbound_keepalive_send_failed"));
            }
        }
        Ok(())
    }

    pub(super) async fn receive(&mut self, timeout: Duration, fresh: bool) -> Result<Vec<u8>, ImsError> {
        self.maintain().await?;
        if !fresh { if let Some(frame) = self.requeued.pop_front() { return Ok(frame); } }
        let deadline = tokio::time::Instant::now() + timeout;
        let mut old_enabled = true;
        let mut current_error = None;
        let mut rejected_connections = 0;
        loop {
            self.maintain().await?;
            let now = tokio::time::Instant::now();
            if now >= deadline { return Err(current_error.unwrap_or_else(|| ImsError::new(code::CHANNEL_READ_TIMEOUT))); }
            let can_read_old = old_enabled && (self.staged.is_some() || self.retired.is_some());
            if let Some(error) = current_error {
                if !can_read_old { return Err(error); }
            }
            let wake = self.current.outbound.deadline().map(tokio::time::Instant::from_std)
                .map_or(deadline, |due| due.min(deadline));
            let listener = self.current.listener.clone();
            let expect_pong = self.current.outbound.awaiting_tcp_pong();
            let current = &mut self.current.streams;
            let previous = self.staged.as_mut().or(self.retired.as_mut()).filter(|_| old_enabled);
            enum Input { Current(Result<(u64, StreamFrame), ImsError>), Old(Result<(u64, StreamFrame), ImsError>), Accept(std::io::Result<(tokio::net::TcpStream, SocketAddr)>), Wake }
            let input = tokio::select! {
                frame = current.receive(expect_pong), if current_error.is_none() => Input::Current(frame),
                frame = async { match previous { Some(old) => old.streams.receive(false).await, None => std::future::pending().await } } => Input::Old(frame),
                incoming = async { match listener { Some(listener) => listener.accept().await, None => std::future::pending().await } } => Input::Accept(incoming),
                _ = tokio::time::sleep_until(wake) => Input::Wake,
            };
            ensure_current(&self.worker)?;
            let (id, frame) = match input {
                Input::Wake => continue,
                Input::Current(Ok(frame)) => frame,
                Input::Current(Err(error)) => {
                    // A dead established reliable flow is not a retryable UDP
                    // read gap. A tentative stream may still receive its final
                    // rejection on the retained association, which is rolled
                    // back by the REGISTER owner on failure.
                    if self.staged.is_none() && error.code() == code::CHANNEL_STREAM_CLOSED {
                        return Err(self.current.outbound.fail("ims_outbound_flow_failed"));
                    }
                    if !can_read_old { return Err(error); }
                    current_error = Some(error);
                    continue;
                }
                Input::Old(Ok(frame)) => frame,
                Input::Old(Err(_)) => { old_enabled = false; continue; },
                Input::Accept(Err(_)) => return Err(ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED)),
                Input::Accept(Ok((socket, peer))) => {
                    let target = if self.current.streams.expected_server == Some(peer) && self.current.streams.server.is_none() {
                        Some(&mut self.current.streams)
                    } else {
                        self.staged.as_mut().or(self.retired.as_mut()).map(|old| &mut old.streams)
                            .filter(|streams| streams.expected_server == Some(peer) && streams.server.is_none())
                    };
                    if let Some(target) = target { target.server = Some(SipStream::plain(socket)?); }
                    else {
                        drop(socket); rejected_connections += 1;
                        // Unrelated peers are not evidence that the admitted
                        // connection failed. Bound work per receive call and
                        // yield without turning a SYN flood into a fatal flow
                        // error or accepting any of those peers' SIP frames.
                        if rejected_connections >= 8 { return Err(ImsError::new(code::CHANNEL_READ_TIMEOUT)); }
                    }
                    continue;
                }
            };
            match frame {
                StreamFrame::Ping => {
                    self.connection(id).ok_or_else(|| ImsError::new(code::CHANNEL_STREAM_RESPONSE_OWNER_MISSING))?.send(b"\r\n").await?;
                }
                StreamFrame::Pong => {
                    if self.current.streams.client.id() == id { self.current.outbound.pong(); }
                }
                StreamFrame::Sip(frame) => {
                    self.current.outbound.received_sip(&frame)?;
                    self.responses.received(&frame, id)?;
                    return Ok(frame);
                }
            }
        }
    }

    pub(super) fn requeue(&mut self, frame: Vec<u8>) { self.requeued.push_back(frame); }
}

fn ensure_current(binding: &UeWorkerBinding) -> Result<(), ImsError> {
    if binding.is_current() { Ok(()) } else { Err(ImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)) }
}

#[cfg(test)]
#[path = "stream_channel_tests.rs"]
mod tests;
