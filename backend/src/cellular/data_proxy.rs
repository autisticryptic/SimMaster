//! Per-line HTTP/SOCKS5 proxy bound to a cellular data interface.

use std::{io, net::SocketAddr};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{lookup_host, TcpListener, TcpSocket, TcpStream},
    sync::{oneshot, Mutex},
    task::JoinHandle,
};

use crate::infra::config::LineDataProxyConfig;

const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;

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
}

impl DataProxyRuntime {
    pub async fn status(&self) -> DataProxyStatus {
        self.state.lock().await.status.clone()
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
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                let interface = outbound_interface.clone();
                                let auth = ProxyAuth::new(username.clone(), password.clone());
                                tokio::spawn(async move {
                                    if let Err(error) = serve_client(stream, &interface, &auth).await {
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
) -> io::Result<()> {
    let mut first = [0u8; 1];
    let count = inbound.peek(&mut first).await?;
    if count == 0 {
        return Ok(());
    }
    if first[0] == 0x05 {
        serve_socks5(inbound, interface_name, auth).await
    } else {
        serve_http_proxy(inbound, interface_name, auth).await
    }
}

async fn serve_socks5(
    mut inbound: TcpStream,
    interface_name: &str,
    auth: &ProxyAuth,
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
        Ok(mut outbound) => {
            write_socks_reply(&mut inbound, 0).await?;
            let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
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
            Ok(mut outbound) => {
                inbound
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await?;
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
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
    if header.len() > header_end {
        outbound.write_all(&header[header_end..]).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
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
