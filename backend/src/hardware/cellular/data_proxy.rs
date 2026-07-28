//! Per-line HTTP/SOCKS5 proxy bound to a cellular data interface.

use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf},
    net::{lookup_host, TcpListener, TcpSocket, TcpStream},
    sync::{oneshot, Mutex},
    task::{JoinHandle, JoinSet},
};

use crate::platform::config::LineDataProxyConfig;

const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;

/// Live byte/connection counters for one line's proxy.
///
/// Counting happens as bytes flow rather than when a connection closes, so a
/// long-lived tunnel still shows up on the web page while it is running.
/// Direction is from the SIM's point of view: `uplink` leaves the device
/// through this SIM, `downlink` arrives on it.
#[derive(Debug, Default)]
pub struct DataProxyCounters {
    uplink_bytes: AtomicU64,
    downlink_bytes: AtomicU64,
    total_connections: AtomicU64,
    active_connections: AtomicU64,
}

impl DataProxyCounters {
    fn add_uplink(&self, bytes: u64) {
        self.uplink_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn add_downlink(&self, bytes: u64) {
        self.downlink_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn connection_opened(&self) {
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    fn connection_closed(&self) {
        // Saturating: a double-close must not wrap the gauge to u64::MAX.
        let _ =
            self.active_connections
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                    Some(current.saturating_sub(1))
                });
    }

    fn snapshot(&self) -> DataProxyTraffic {
        DataProxyTraffic {
            uplink_bytes: self.uplink_bytes.load(Ordering::Relaxed),
            downlink_bytes: self.downlink_bytes.load(Ordering::Relaxed),
            total_connections: self.total_connections.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
        }
    }

    fn reset(&self) {
        self.uplink_bytes.store(0, Ordering::Relaxed);
        self.downlink_bytes.store(0, Ordering::Relaxed);
        self.total_connections.store(0, Ordering::Relaxed);
        // `active_connections` is a live gauge, not a total — resetting it would
        // desync it from the connections that are still running.
    }

    fn clear_active_connections(&self) {
        self.active_connections.store(0, Ordering::Relaxed);
    }
}

/// Traffic this line's proxy has carried.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DataProxyTraffic {
    /// Bytes sent out through this SIM.
    pub uplink_bytes: u64,
    /// Bytes received on this SIM.
    pub downlink_bytes: u64,
    pub total_connections: u64,
    pub active_connections: u64,
}

impl DataProxyTraffic {
    pub fn total_bytes(&self) -> u64 {
        self.uplink_bytes.saturating_add(self.downlink_bytes)
    }

    /// Whether this SIM has carried any proxied traffic at all — the "是否用过
    /// 流量" question the status page asks.
    pub fn used_any(&self) -> bool {
        self.total_bytes() > 0
    }

    pub fn saturating_add(&self, other: &Self) -> Self {
        Self {
            uplink_bytes: self.uplink_bytes.saturating_add(other.uplink_bytes),
            downlink_bytes: self.downlink_bytes.saturating_add(other.downlink_bytes),
            total_connections: self
                .total_connections
                .saturating_add(other.total_connections),
            // Active connections belong to the live session only; summing a
            // persisted total into it would report phantom open connections.
            active_connections: self.active_connections,
        }
    }
}

/// Wraps one side of a proxied connection and records the bytes that pass
/// through it. Reads from the client are uplink; writes to the client are
/// downlink.
struct CountingStream<S> {
    inner: S,
    counters: Arc<DataProxyCounters>,
    /// `true` for the client-facing socket, `false` for the upstream socket.
    /// Only the client side counts, so each byte is recorded exactly once.
    count_reads_as_uplink: bool,
}

impl<S> CountingStream<S> {
    fn client_side(inner: S, counters: Arc<DataProxyCounters>) -> Self {
        Self {
            inner,
            counters,
            count_reads_as_uplink: true,
        }
    }

    fn upstream_side(inner: S, counters: Arc<DataProxyCounters>) -> Self {
        Self {
            inner,
            counters,
            count_reads_as_uplink: false,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountingStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            let read = buf.filled().len().saturating_sub(before) as u64;
            if read > 0 && self.count_reads_as_uplink {
                self.counters.add_uplink(read);
            }
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = &result {
            if *written > 0 && self.count_reads_as_uplink {
                self.counters.add_downlink(*written as u64);
            }
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Pump a proxied connection while counting both directions.
async fn relay_counted(
    inbound: TcpStream,
    outbound: TcpStream,
    counters: &Arc<DataProxyCounters>,
) -> io::Result<()> {
    let mut client = CountingStream::client_side(inbound, Arc::clone(counters));
    let mut upstream = CountingStream::upstream_side(outbound, Arc::clone(counters));
    tokio::io::copy_bidirectional(&mut client, &mut upstream)
        .await
        .map(|_| ())
}

#[derive(Debug, Clone, Serialize)]
pub struct DataProxyStatus {
    pub running: bool,
    pub phase: String,
    pub stage: String,
    pub listen_ip: Option<String>,
    pub port: Option<u16>,
    pub interface_name: Option<String>,
    pub protocols: Vec<String>,
    pub auth_required: bool,
    pub last_error: Option<String>,
    /// Traffic carried since the counters were last reset, including anything
    /// carried before the most recent restart.
    pub traffic: DataProxyTraffic,
    /// Whether this SIM has carried any proxied traffic at all.
    pub traffic_used: bool,
}

impl Default for DataProxyStatus {
    fn default() -> Self {
        Self {
            running: false,
            phase: "disabled".to_string(),
            stage: "流量未启用".to_string(),
            listen_ip: None,
            port: None,
            interface_name: None,
            protocols: Vec::new(),
            auth_required: false,
            last_error: None,
            traffic: DataProxyTraffic::default(),
            traffic_used: false,
        }
    }
}

#[derive(Default)]
struct ProxyState {
    status: DataProxyStatus,
    auth: Option<(String, String)>,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct DataProxyRuntime {
    state: Mutex<ProxyState>,
    /// Counters for the current process lifetime. Kept outside `state` so the
    /// serving tasks can bump them without contending on the status mutex.
    counters: Arc<DataProxyCounters>,
    /// Traffic carried before this process started, loaded from the database so
    /// the reported totals survive a restart.
    persisted: Mutex<DataProxyTraffic>,
}

impl DataProxyRuntime {
    pub async fn status(&self) -> DataProxyStatus {
        let mut status = self.state.lock().await.status.clone();
        status.traffic = self.traffic().await;
        status.traffic_used = status.traffic.used_any();
        status
    }

    /// Persisted total plus what this process has carried.
    pub async fn traffic(&self) -> DataProxyTraffic {
        let persisted = *self.persisted.lock().await;
        self.counters.snapshot().saturating_add(&persisted)
    }

    /// Traffic carried since this process started, i.e. excluding the persisted
    /// baseline. Used when flushing the delta to the database.
    pub fn session_traffic(&self) -> DataProxyTraffic {
        self.counters.snapshot()
    }

    /// Seed the persisted baseline at startup.
    pub async fn restore_persisted_traffic(&self, traffic: DataProxyTraffic) {
        *self.persisted.lock().await = traffic;
    }

    /// Zero both the live counters and the persisted baseline.
    pub async fn reset_traffic(&self) -> DataProxyTraffic {
        self.counters.reset();
        *self.persisted.lock().await = DataProxyTraffic::default();
        self.traffic().await
    }

    pub async fn start(
        &self,
        interface_name: &str,
        config: &LineDataProxyConfig,
    ) -> Result<DataProxyStatus, String> {
        let interface_name = interface_name.trim();
        if interface_name.is_empty() {
            return Err("cellular_data_interface_unavailable".to_string());
        }

        let mut state = self.state.lock().await;
        if state.status.running
            && state.status.interface_name.as_deref() == Some(interface_name)
            && state.status.listen_ip.as_deref() == Some(config.listen_ip.as_str())
            && (config.listen_port == 0 || state.status.port == Some(config.listen_port))
            && state.status.auth_required == !config.username.is_empty()
            && state.auth.as_ref() == Some(&(config.username.clone(), config.password.clone()))
        {
            return Ok(state.status.clone());
        }
        stop_locked(&mut state).await;

        let listen_ip = config
            .listen_ip
            .parse::<std::net::IpAddr>()
            .map_err(|_| "data_proxy_listen_ip_invalid".to_string())?;
        let listener = TcpListener::bind(SocketAddr::new(listen_ip, config.listen_port))
            .await
            .map_err(|error| format!("data_proxy_bind_failed: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("data_proxy_local_addr_failed: {error}"))?
            .port();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let outbound_interface = interface_name.to_string();
        let username = config.username.clone();
        let password = config.password.clone();
        let counters = Arc::clone(&self.counters);
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = clients.join_next(), if !clients.is_empty() => {},
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                let interface = outbound_interface.clone();
                                let auth = ProxyAuth::new(username.clone(), password.clone());
                                let counters = Arc::clone(&counters);
                                clients.spawn(async move {
                                    counters.connection_opened();
                                    let result = serve_client(stream, &interface, &auth, &counters).await;
                                    counters.connection_closed();
                                    if let Err(error) = result {
                                        tracing::debug!(interface = %interface, error = %error, "Cellular data proxy client closed");
                                    }
                                });
                            }
                            Err(error) => {
                                tracing::warn!(error = %error, "Cellular data proxy accept failed");
                                break;
                            }
                        }
                    }
                }
            }
            clients.abort_all();
            while clients.join_next().await.is_some() {}
            counters.clear_active_connections();
        });

        state.status = DataProxyStatus {
            running: true,
            phase: "ready".to_string(),
            stage: "代理出口已就绪".to_string(),
            listen_ip: Some(listen_ip.to_string()),
            port: Some(port),
            interface_name: Some(interface_name.to_string()),
            protocols: vec!["http".to_string(), "socks5".to_string()],
            auth_required: !config.username.is_empty(),
            last_error: None,
            // Filled in by `status()` from the live counters; restarting the
            // listener must not zero the traffic totals.
            traffic: DataProxyTraffic::default(),
            traffic_used: false,
        };
        state.shutdown = Some(shutdown_tx);
        state.task = Some(task);
        state.auth = Some((config.username.clone(), config.password.clone()));
        Ok(state.status.clone())
    }

    pub async fn stop(&self) -> DataProxyStatus {
        let mut state = self.state.lock().await;
        stop_locked(&mut state).await;
        state.status.clone()
    }

    pub async fn record_error(&self, error: impl Into<String>) -> DataProxyStatus {
        let mut state = self.state.lock().await;
        let error = error.into();
        state.status.phase = "failed".to_string();
        state.status.stage = error.clone();
        state.status.last_error = Some(error);
        state.status.clone()
    }
}

async fn stop_locked(state: &mut ProxyState) {
    if let Some(shutdown) = state.shutdown.take() {
        let _ = shutdown.send(());
    }
    if let Some(task) = state.task.take() {
        let _ = task.await;
    }
    state.status.running = false;
    state.status.phase = "disabled".to_string();
    state.status.stage = "流量未启用".to_string();
    state.status.listen_ip = None;
    state.status.port = None;
    state.status.interface_name = None;
    state.status.protocols.clear();
    state.status.auth_required = false;
    state.status.last_error = None;
    state.auth = None;
}

#[derive(Clone)]
struct ProxyAuth {
    username: String,
    password: String,
}

impl ProxyAuth {
    fn new(username: String, password: String) -> Self {
        Self { username, password }
    }

    fn required(&self) -> bool {
        !self.username.is_empty()
    }

    fn matches(&self, username: &[u8], password: &[u8]) -> bool {
        username == self.username.as_bytes() && password == self.password.as_bytes()
    }
}

async fn serve_client(
    inbound: TcpStream,
    interface_name: &str,
    auth: &ProxyAuth,
    counters: &Arc<DataProxyCounters>,
) -> io::Result<()> {
    let mut first = [0u8; 1];
    let count = inbound.peek(&mut first).await?;
    if count == 0 {
        return Ok(());
    }
    if first[0] == 0x05 {
        serve_socks5(inbound, interface_name, auth, counters).await
    } else {
        serve_http_proxy(inbound, interface_name, auth, counters).await
    }
}

async fn serve_socks5(
    mut inbound: TcpStream,
    interface_name: &str,
    auth: &ProxyAuth,
    counters: &Arc<DataProxyCounters>,
) -> io::Result<()> {
    let version = inbound.read_u8().await?;
    let method_count = inbound.read_u8().await? as usize;
    let mut methods = vec![0u8; method_count];
    inbound.read_exact(&mut methods).await?;
    let selected_method = if auth.required() { 2 } else { 0 };
    if version != 5 || !methods.contains(&selected_method) {
        inbound.write_all(&[5, 0xff]).await?;
        return Ok(());
    }
    inbound.write_all(&[5, selected_method]).await?;
    if selected_method == 2 && !authenticate_socks5(&mut inbound, auth).await? {
        return Ok(());
    }

    let version = inbound.read_u8().await?;
    let command = inbound.read_u8().await?;
    let _reserved = inbound.read_u8().await?;
    let address_type = inbound.read_u8().await?;
    if version != 5 || command != 1 {
        write_socks_reply(&mut inbound, 7).await?;
        return Ok(());
    }
    let host = match address_type {
        1 => {
            let mut address = [0u8; 4];
            inbound.read_exact(&mut address).await?;
            std::net::Ipv4Addr::from(address).to_string()
        }
        3 => {
            let length = inbound.read_u8().await? as usize;
            let mut address = vec![0u8; length];
            inbound.read_exact(&mut address).await?;
            String::from_utf8(address)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid SOCKS host"))?
        }
        4 => {
            let mut address = [0u8; 16];
            inbound.read_exact(&mut address).await?;
            std::net::Ipv6Addr::from(address).to_string()
        }
        _ => {
            write_socks_reply(&mut inbound, 8).await?;
            return Ok(());
        }
    };
    let port = inbound.read_u16().await?;
    match connect_bound(&host, port, interface_name).await {
        Ok(outbound) => {
            write_socks_reply(&mut inbound, 0).await?;
            relay_counted(inbound, outbound, counters).await?;
        }
        Err(_) => write_socks_reply(&mut inbound, 5).await?,
    }
    Ok(())
}

async fn authenticate_socks5(inbound: &mut TcpStream, auth: &ProxyAuth) -> io::Result<bool> {
    let version = inbound.read_u8().await?;
    let username_len = inbound.read_u8().await? as usize;
    let mut username = vec![0; username_len];
    inbound.read_exact(&mut username).await?;
    let password_len = inbound.read_u8().await? as usize;
    let mut password = vec![0; password_len];
    inbound.read_exact(&mut password).await?;
    let accepted = version == 1 && auth.matches(&username, &password);
    inbound.write_all(&[1, u8::from(!accepted)]).await?;
    Ok(accepted)
}

async fn write_socks_reply(stream: &mut TcpStream, code: u8) -> io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

async fn serve_http_proxy(
    mut inbound: TcpStream,
    interface_name: &str,
    auth: &ProxyAuth,
    counters: &Arc<DataProxyCounters>,
) -> io::Result<()> {
    let mut header = Vec::with_capacity(2048);
    let header_end = loop {
        if header.len() >= MAX_HTTP_HEADER_BYTES {
            write_http_error(&mut inbound, 431, "Request Header Fields Too Large").await?;
            return Ok(());
        }
        let mut chunk = [0u8; 2048];
        let read = inbound.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        header.extend_from_slice(&chunk[..read]);
        if let Some(end) = find_header_end(&header) {
            break end;
        }
    };
    let head = std::str::from_utf8(&header[..header_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid HTTP header"))?;
    let first_line = head.lines().next().unwrap_or_default();
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or("HTTP/1.1");

    if auth.required() && !http_proxy_authorized(head, auth) {
        write_http_proxy_auth_required(&mut inbound).await?;
        return Ok(());
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(target, 443)?;
        match connect_bound(&host, port, interface_name).await {
            Ok(outbound) => {
                inbound
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                relay_counted(inbound, outbound, counters).await?;
            }
            Err(_) => write_http_error(&mut inbound, 502, "Bad Gateway").await?,
        }
        return Ok(());
    }

    let host_header = head.lines().skip(1).find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("host").then(|| value.trim())
    });
    let (host, port, origin_target) = parse_http_target(target, host_header)?;
    let mut outbound = match connect_bound(&host, port, interface_name).await {
        Ok(stream) => stream,
        Err(_) => {
            write_http_error(&mut inbound, 502, "Bad Gateway").await?;
            return Ok(());
        }
    };
    let rewritten = rewrite_http_request_head(head, method, &origin_target, version);
    outbound.write_all(rewritten.as_bytes()).await?;
    // The request head was consumed before the counting relay took over, so
    // account for it explicitly; otherwise plain-HTTP requests would under-report.
    let mut head_bytes = rewritten.len();
    if header.len() > header_end {
        outbound.write_all(&header[header_end..]).await?;
        head_bytes += header.len() - header_end;
    }
    counters.add_uplink(head_bytes as u64);
    relay_counted(inbound, outbound, counters).await?;
    Ok(())
}

fn http_proxy_authorized(head: &str, auth: &ProxyAuth) -> bool {
    head.lines().skip(1).any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        if !name.eq_ignore_ascii_case("proxy-authorization") {
            return false;
        }
        let Some((scheme, encoded)) = value.trim().split_once(char::is_whitespace) else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("basic") {
            return false;
        }
        BASE64_STANDARD
            .decode(encoded.trim())
            .ok()
            .is_some_and(|decoded| auth.matches_basic_credentials(&decoded))
    })
}

impl ProxyAuth {
    fn matches_basic_credentials(&self, decoded: &[u8]) -> bool {
        let Some(separator) = decoded.iter().position(|byte| *byte == b':') else {
            return false;
        };
        self.matches(&decoded[..separator], &decoded[separator + 1..])
    }
}

fn rewrite_http_request_head(head: &str, method: &str, target: &str, version: &str) -> String {
    let mut rewritten = format!("{method} {target} {version}\r\n");
    for line in head.lines().skip(1) {
        let is_proxy_auth = line
            .split_once(':')
            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("proxy-authorization"));
        if !line.is_empty() && !is_proxy_auth {
            rewritten.push_str(line);
            rewritten.push_str("\r\n");
        }
    }
    rewritten.push_str("\r\n");
    rewritten
}

async fn write_http_proxy_auth_required(stream: &mut TcpStream) -> io::Result<()> {
    stream
        .write_all(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"SimAdmin cellular proxy\"\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        )
        .await
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

fn split_host_port(value: &str, default_port: u16) -> io::Result<(String, u16)> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (host, suffix) = rest
            .split_once(']')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid IPv6 host"))?;
        let port = suffix
            .strip_prefix(':')
            .map(str::parse)
            .transpose()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid port"))?
            .unwrap_or(default_port);
        return Ok((host.to_string(), port));
    }
    if value.matches(':').count() == 1 {
        if let Some((host, port)) = value.rsplit_once(':') {
            let port = port
                .parse()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid port"))?;
            return Ok((host.to_string(), port));
        }
    }
    Ok((value.to_string(), default_port))
}

fn parse_http_target(target: &str, host_header: Option<&str>) -> io::Result<(String, u16, String)> {
    if let Some(absolute) = target.strip_prefix("http://") {
        let (authority, path) = absolute.split_once('/').unwrap_or((absolute, ""));
        let (host, port) = split_host_port(authority, 80)?;
        return Ok((host, port, format!("/{path}")));
    }
    let authority = host_header
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing Host header"))?;
    let (host, port) = split_host_port(authority, 80)?;
    Ok((host, port, target.to_string()))
}

async fn write_http_error(stream: &mut TcpStream, code: u16, reason: &str) -> io::Result<()> {
    let response =
        format!("HTTP/1.1 {code} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    stream.write_all(response.as_bytes()).await
}

async fn connect_bound(host: &str, port: u16, interface_name: &str) -> io::Result<TcpStream> {
    let addresses = lookup_host((host, port))
        .await?
        .collect::<Vec<SocketAddr>>();
    let mut last_error = None;
    for address in addresses {
        let socket = if address.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        bind_socket_to_device(&socket, interface_name)?;
        match socket.connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host not resolved")))
}

#[cfg(target_os = "linux")]
fn bind_socket_to_device(socket: &TcpSocket, interface_name: &str) -> io::Result<()> {
    use std::{ffi::CString, os::fd::AsRawFd};

    let name = CString::new(interface_name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid interface name"))?;
    // SO_BINDTODEVICE is the enforcement boundary that prevents proxy traffic
    // from falling back to Wi-Fi or another modem's default route.
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
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn bind_socket_to_device(_socket: &TcpSocket, _interface_name: &str) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn binds_configured_listener_and_reports_stage() {
        let runtime = DataProxyRuntime::default();
        let status = runtime
            .start(
                "test-interface",
                &LineDataProxyConfig {
                    listen_ip: "127.0.0.1".to_string(),
                    listen_port: 0,
                    ..LineDataProxyConfig::default()
                },
            )
            .await
            .unwrap();
        assert!(status.running);
        assert_eq!(status.phase, "ready");
        assert_eq!(status.listen_ip.as_deref(), Some("127.0.0.1"));
        assert!(status.port.is_some_and(|port| port > 0));

        let stopped = runtime.stop().await;
        assert_eq!(stopped.phase, "disabled");
        assert_eq!(stopped.stage, "流量未启用");
    }

    #[tokio::test]
    async fn socks5_relay_counts_uplink_and_downlink_separately() {
        // An echo-ish upstream that reads a request and answers with a longer
        // body, so uplink and downlink cannot be confused for each other.
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            stream.write_all(b"0123456789").await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let runtime = DataProxyRuntime::default();
        let status = runtime
            .start(
                "lo",
                &LineDataProxyConfig {
                    listen_ip: "127.0.0.1".to_string(),
                    listen_port: 0,
                    ..LineDataProxyConfig::default()
                },
            )
            .await
            .unwrap();
        let proxy_port = status.port.unwrap();

        let mut client = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
        // SOCKS5 greeting, no auth.
        client.write_all(&[5, 1, 0]).await.unwrap();
        let mut greeting = [0u8; 2];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 0]);
        // CONNECT to the upstream over IPv4.
        let mut request = vec![5, 1, 0, 1];
        request.extend_from_slice(&[127, 0, 0, 1]);
        request.extend_from_slice(&upstream_addr.port().to_be_bytes());
        client.write_all(&request).await.unwrap();
        let mut reply = [0u8; 10];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[1], 0);

        client.write_all(b"ping").await.unwrap();
        let mut body = Vec::new();
        client.read_to_end(&mut body).await.unwrap();
        assert_eq!(body, b"0123456789");
        drop(client);

        // Give the relay task a moment to finish accounting.
        for _ in 0..50 {
            if runtime.session_traffic().downlink_bytes >= 10 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let traffic = runtime.traffic().await;
        assert_eq!(traffic.uplink_bytes, 4, "client sent exactly 4 bytes");
        assert_eq!(traffic.downlink_bytes, 10, "upstream sent exactly 10 bytes");
        assert_eq!(traffic.total_connections, 1);
        assert!(traffic.used_any());

        let status = runtime.status().await;
        assert!(status.traffic_used);
        assert_eq!(status.traffic.uplink_bytes, 4);

        runtime.stop().await;
        // Stopping the listener must not discard the totals.
        assert_eq!(runtime.traffic().await.uplink_bytes, 4);

        // Reset zeroes the cumulative totals. `active_connections` is a live
        // gauge and is deliberately left alone, so it is not asserted here.
        let cleared = runtime.reset_traffic().await;
        assert_eq!(cleared.uplink_bytes, 0);
        assert_eq!(cleared.downlink_bytes, 0);
        assert_eq!(cleared.total_connections, 0);
        assert!(!cleared.used_any());
    }

    #[tokio::test]
    async fn persisted_traffic_is_added_to_the_live_session() {
        let runtime = DataProxyRuntime::default();
        runtime
            .restore_persisted_traffic(DataProxyTraffic {
                uplink_bytes: 100,
                downlink_bytes: 200,
                total_connections: 3,
                active_connections: 0,
            })
            .await;
        runtime.counters.add_uplink(5);
        runtime.counters.add_downlink(7);
        runtime.counters.connection_opened();

        let traffic = runtime.traffic().await;
        assert_eq!(traffic.uplink_bytes, 105);
        assert_eq!(traffic.downlink_bytes, 207);
        assert_eq!(traffic.total_connections, 4);
        // The live gauge must come from the session, not the persisted total.
        assert_eq!(traffic.active_connections, 1);
    }

    #[tokio::test]
    async fn stopping_proxy_terminates_clients_and_clears_active_gauge() {
        let runtime = DataProxyRuntime::default();
        let status = runtime
            .start(
                "lo",
                &LineDataProxyConfig {
                    listen_ip: "127.0.0.1".to_string(),
                    listen_port: 0,
                    ..LineDataProxyConfig::default()
                },
            )
            .await
            .unwrap();
        let _client = TcpStream::connect(("127.0.0.1", status.port.unwrap()))
            .await
            .unwrap();
        for _ in 0..50 {
            if runtime.session_traffic().active_connections == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(runtime.session_traffic().active_connections, 1);

        runtime.stop().await;
        assert_eq!(runtime.session_traffic().active_connections, 0);
    }

    #[test]
    fn active_connection_gauge_never_wraps_below_zero() {
        let counters = DataProxyCounters::default();
        counters.connection_closed();
        assert_eq!(counters.snapshot().active_connections, 0);
    }

    #[test]
    fn parses_absolute_http_target() {
        assert_eq!(
            parse_http_target("http://example.com:8080/path?q=1", None).unwrap(),
            ("example.com".to_string(), 8080, "/path?q=1".to_string())
        );
    }

    #[test]
    fn parses_ipv6_connect_authority() {
        assert_eq!(
            split_host_port("[2001:db8::1]:443", 80).unwrap(),
            ("2001:db8::1".to_string(), 443)
        );
    }

    #[test]
    fn locates_complete_http_header() {
        assert_eq!(
            find_header_end(b"GET / HTTP/1.1\r\nHost: x\r\n\r\nbody"),
            Some(27)
        );
    }

    #[test]
    fn validates_http_basic_credentials_and_strips_proxy_header() {
        let auth = ProxyAuth::new("alice".to_string(), "secret".to_string());
        let encoded = BASE64_STANDARD.encode("alice:secret");
        let head = format!(
            "GET http://example.test/ HTTP/1.1\r\nHost: example.test\r\nProxy-Authorization: Basic {encoded}\r\n\r\n"
        );
        assert!(http_proxy_authorized(&head, &auth));
        let rewritten = rewrite_http_request_head(&head, "GET", "/", "HTTP/1.1");
        assert!(!rewritten
            .to_ascii_lowercase()
            .contains("proxy-authorization"));
    }

    #[tokio::test]
    async fn requires_socks5_username_password_handshake() {
        let runtime = DataProxyRuntime::default();
        let status = runtime
            .start(
                "test-interface",
                &LineDataProxyConfig {
                    listen_ip: "127.0.0.1".to_string(),
                    listen_port: 0,
                    username: "alice".to_string(),
                    password: "secret".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(status.auth_required);
        let mut client = TcpStream::connect(("127.0.0.1", status.port.unwrap()))
            .await
            .unwrap();
        client.write_all(&[5, 1, 2]).await.unwrap();
        let mut method = [0; 2];
        client.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 2]);
        client
            .write_all(&[
                1, 5, b'a', b'l', b'i', b'c', b'e', 6, b's', b'e', b'c', b'r', b'e', b't',
            ])
            .await
            .unwrap();
        let mut result = [0; 2];
        client.read_exact(&mut result).await.unwrap();
        assert_eq!(result, [1, 0]);
        runtime.stop().await;
    }
}
