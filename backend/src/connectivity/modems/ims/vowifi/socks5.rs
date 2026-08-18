//! SOCKS5 UDP ASSOCIATE transport for IKEv2/NAT-T.
//!
//! # Why SOCKS5 and not HTTP CONNECT
//!
//! VoWiFi carries IKEv2 on UDP 500 and NAT-T-encapsulated ESP on UDP 4500. HTTP
//! CONNECT tunnels TCP only, so it cannot carry this traffic at all. SOCKS5, by
//! contrast, has a first-class UDP mode (`UDP ASSOCIATE`, RFC 1928 §4/§7) that
//! every common local proxy supports — mihomo, sing-box, Xray, Shadowsocks.
//!
//! # How UDP ASSOCIATE works
//!
//! It is a two-channel protocol:
//!
//! 1. A **TCP control connection** performs the greeting, optional
//!    username/password authentication (RFC 1929), then a `UDP ASSOCIATE`
//!    request. The proxy replies with the address/port of a UDP relay socket.
//! 2. Application datagrams are then sent to that **UDP relay socket**, each one
//!    prefixed with a small header naming the final destination
//!    (`RSV(2) | FRAG(1) | ATYP(1) | DST.ADDR | DST.PORT(2) | DATA`).
//!
//! The TCP control connection must stay open: per RFC 1928 §7 the association is
//! torn down when it closes, so this type holds it for its whole lifetime and
//! `is_control_alive()` lets callers notice a dead relay.
//!
//! # Address rewriting
//!
//! Proxies commonly answer `UDP ASSOCIATE` with `0.0.0.0`/`::` meaning "same host
//! as the control connection". [`Socks5UdpClient`] rewrites such a reply to the
//! proxy's own IP, which is what makes local proxies like mihomo work.

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UdpSocket},
    sync::Mutex,
};

/// SOCKS protocol version.
const SOCKS5_VERSION: u8 = 0x05;
/// No authentication required.
const AUTH_NONE: u8 = 0x00;
/// Username/password authentication (RFC 1929).
const AUTH_USERPASS: u8 = 0x02;
/// No acceptable authentication method.
const AUTH_UNACCEPTABLE: u8 = 0xff;
/// RFC 1929 sub-negotiation version.
const USERPASS_VERSION: u8 = 0x01;
/// `UDP ASSOCIATE` command.
const CMD_UDP_ASSOCIATE: u8 = 0x03;

const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

const REPLY_SUCCEEDED: u8 = 0x00;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Socks5Error {
    /// The endpoint string was not a usable `socks5://` URL.
    InvalidEndpoint(String),
    /// TCP connect / IO against the proxy failed.
    Io(String),
    /// The proxy spoke something that is not SOCKS5.
    Protocol(String),
    /// The proxy rejected our authentication methods or credentials.
    AuthFailed(String),
    /// The proxy refused `UDP ASSOCIATE`.
    AssociateRejected(u8),
    /// An operation exceeded its deadline.
    Timeout(String),
}

impl fmt::Display for Socks5Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(d) => write!(f, "socks5_invalid_endpoint:{d}"),
            Self::Io(d) => write!(f, "socks5_io:{d}"),
            Self::Protocol(d) => write!(f, "socks5_protocol:{d}"),
            Self::AuthFailed(d) => write!(f, "socks5_auth_failed:{d}"),
            Self::AssociateRejected(code) => {
                write!(f, "socks5_associate_rejected:{}", reply_reason(*code))
            }
            Self::Timeout(d) => write!(f, "socks5_timeout:{d}"),
        }
    }
}

impl std::error::Error for Socks5Error {}

/// Human-readable RFC 1928 §6 reply code.
pub fn reply_reason(code: u8) -> &'static str {
    match code {
        0x00 => "succeeded",
        0x01 => "general-failure",
        0x02 => "not-allowed-by-ruleset",
        0x03 => "network-unreachable",
        0x04 => "host-unreachable",
        0x05 => "connection-refused",
        0x06 => "ttl-expired",
        0x07 => "command-not-supported",
        0x08 => "address-type-not-supported",
        _ => "unknown",
    }
}

/// A parsed `socks5://[user[:pass]@]host:port` endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Socks5Endpoint {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Socks5Endpoint {
    /// Parse the configured `proxy_endpoint`.
    ///
    /// Accepts `socks5://` and `socks5h://` (the `h` variant only means "resolve
    /// names at the proxy", which is already how UDP ASSOCIATE targets work here).
    /// Credentials are optional and percent-decoded so passwords may contain `@`
    /// or `:` when encoded.
    pub fn parse(endpoint: &str) -> Result<Self, Socks5Error> {
        let trimmed = endpoint.trim();
        let rest = trimmed
            .strip_prefix("socks5://")
            .or_else(|| trimmed.strip_prefix("socks5h://"))
            .ok_or_else(|| {
                Socks5Error::InvalidEndpoint("expected socks5:// or socks5h://".to_string())
            })?;
        if rest.is_empty() {
            return Err(Socks5Error::InvalidEndpoint("empty authority".to_string()));
        }

        // Split credentials from the host part at the LAST '@' so passwords may
        // themselves contain '@'.
        let (credentials, authority) = match rest.rsplit_once('@') {
            Some((credentials, authority)) => (Some(credentials), authority),
            None => (None, rest),
        };
        if authority.is_empty() {
            return Err(Socks5Error::InvalidEndpoint("missing host".to_string()));
        }

        let (username, password) = match credentials {
            Some(credentials) => {
                let (user, pass) = match credentials.split_once(':') {
                    Some((user, pass)) => (user, Some(pass)),
                    None => (credentials, None),
                };
                if user.is_empty() {
                    return Err(Socks5Error::InvalidEndpoint(
                        "empty username in credentials".to_string(),
                    ));
                }
                (Some(percent_decode(user)), pass.map(percent_decode))
            }
            None => (None, None),
        };

        // Bracketed IPv6 literal, else host:port.
        let (host, port) = if let Some(without_open) = authority.strip_prefix('[') {
            let (host, tail) = without_open.split_once(']').ok_or_else(|| {
                Socks5Error::InvalidEndpoint("unterminated IPv6 literal".to_string())
            })?;
            let port = tail.strip_prefix(':').ok_or_else(|| {
                Socks5Error::InvalidEndpoint("missing port after IPv6 literal".to_string())
            })?;
            (host.to_string(), port)
        } else {
            let (host, port) = authority
                .rsplit_once(':')
                .ok_or_else(|| Socks5Error::InvalidEndpoint("missing port".to_string()))?;
            (host.to_string(), port)
        };

        if host.is_empty() {
            return Err(Socks5Error::InvalidEndpoint("missing host".to_string()));
        }
        let port: u16 = port
            .parse()
            .map_err(|_| Socks5Error::InvalidEndpoint(format!("bad port {port:?}")))?;
        if port == 0 {
            return Err(Socks5Error::InvalidEndpoint(
                "port must not be 0".to_string(),
            ));
        }

        Ok(Self {
            host,
            port,
            username,
            password,
        })
    }

    fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Minimal percent-decoding so credentials can carry reserved characters.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Encode a SOCKS5 UDP request header followed by the payload (RFC 1928 §7).
///
/// `FRAG` is always 0: fragmentation is optional in the RFC and no common proxy
/// implements reassembly, so datagrams are always sent whole.
pub fn encode_udp_datagram(destination: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 22);
    frame.extend_from_slice(&[0x00, 0x00]); // RSV
    frame.push(0x00); // FRAG
    match destination.ip() {
        IpAddr::V4(addr) => {
            frame.push(ATYP_IPV4);
            frame.extend_from_slice(&addr.octets());
        }
        IpAddr::V6(addr) => {
            frame.push(ATYP_IPV6);
            frame.extend_from_slice(&addr.octets());
        }
    }
    frame.extend_from_slice(&destination.port().to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Decode a SOCKS5 UDP reply header, returning the origin and the payload range.
///
/// Domain-typed origins are reported as `None` for the address: the caller only
/// needs the payload, and a proxy answering with a name (rather than the IP it
/// received from) carries no routable address for us to use.
pub fn decode_udp_datagram(frame: &[u8]) -> Result<(Option<SocketAddr>, &[u8]), Socks5Error> {
    if frame.len() < 5 {
        return Err(Socks5Error::Protocol("short UDP reply header".to_string()));
    }
    if frame[2] != 0x00 {
        // A fragmented reply cannot be reassembled here; treat as protocol error
        // rather than silently handing a partial IKE message to the state machine.
        return Err(Socks5Error::Protocol(
            "fragmented SOCKS5 UDP reply is unsupported".to_string(),
        ));
    }
    let atyp = frame[3];
    let (address, cursor) = match atyp {
        ATYP_IPV4 => {
            if frame.len() < 10 {
                return Err(Socks5Error::Protocol("short IPv4 UDP reply".to_string()));
            }
            let octets = [frame[4], frame[5], frame[6], frame[7]];
            let port = u16::from_be_bytes([frame[8], frame[9]]);
            (
                Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port)),
                10,
            )
        }
        ATYP_IPV6 => {
            if frame.len() < 22 {
                return Err(Socks5Error::Protocol("short IPv6 UDP reply".to_string()));
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&frame[4..20]);
            let port = u16::from_be_bytes([frame[20], frame[21]]);
            (
                Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port)),
                22,
            )
        }
        ATYP_DOMAIN => {
            let length = frame[4] as usize;
            let end = 5 + length + 2;
            if frame.len() < end {
                return Err(Socks5Error::Protocol("short domain UDP reply".to_string()));
            }
            (None, end)
        }
        other => {
            return Err(Socks5Error::Protocol(format!(
                "unsupported reply ATYP {other:#04x}"
            )))
        }
    };
    Ok((address, &frame[cursor..]))
}

/// A live SOCKS5 UDP association.
///
/// Holds the TCP control connection open for its whole lifetime — dropping it
/// tears down the association at the proxy (RFC 1928 §7).
pub struct Socks5UdpClient {
    /// Kept alive purely so the association stays valid.
    control: Mutex<TcpStream>,
    socket: Arc<UdpSocket>,
    relay: SocketAddr,
    endpoint: Socks5Endpoint,
    recv_timeout: Duration,
    max_datagram_bytes: usize,
}

impl fmt::Debug for Socks5UdpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Socks5UdpClient")
            .field("proxy", &self.endpoint.authority())
            .field("relay", &self.relay)
            .field("authenticated", &self.endpoint.username.is_some())
            .finish()
    }
}

impl Socks5UdpClient {
    /// Establish a UDP association through the proxy.
    ///
    /// `local_family_hint` decides whether the local UDP socket is bound v4 or v6;
    /// it should match the family of the ePDG addresses that will be targeted.
    pub async fn connect(
        endpoint: &Socks5Endpoint,
        local_family_hint: IpAddr,
        connect_timeout: Duration,
    ) -> Result<Self, Socks5Error> {
        let authority = endpoint.authority();
        let mut control = tokio::time::timeout(connect_timeout, TcpStream::connect(&authority))
            .await
            .map_err(|_| Socks5Error::Timeout(format!("connect {authority}")))?
            .map_err(|err| Socks5Error::Io(format!("connect {authority}: {}", err.kind())))?;
        // IKE retransmits on its own; Nagle would only add latency here.
        let _ = control.set_nodelay(true);

        negotiate_auth(&mut control, endpoint, connect_timeout).await?;

        // Bind the local UDP socket first so we can tell the proxy where we will
        // send from. Port 0 lets the OS pick.
        let bind_addr = match local_family_hint {
            IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        };
        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|err| Socks5Error::Io(format!("bind local udp: {}", err.kind())))?;
        let local = socket
            .local_addr()
            .map_err(|err| Socks5Error::Io(format!("local_addr: {}", err.kind())))?;

        let relay = request_udp_associate(&mut control, local, connect_timeout).await?;
        // Proxies routinely answer with a wildcard address meaning "reach me at the
        // same host you used for TCP". Resolve that to the proxy's real IP.
        let relay = normalize_relay_addr(relay, control.peer_addr().ok());

        // Connecting the socket to the relay keeps stray datagrams out.
        socket
            .connect(relay)
            .await
            .map_err(|err| Socks5Error::Io(format!("connect relay {relay}: {}", err.kind())))?;

        Ok(Self {
            control: Mutex::new(control),
            socket: Arc::new(socket),
            relay,
            endpoint: endpoint.clone(),
            recv_timeout: Duration::from_secs(8),
            max_datagram_bytes: 4096,
        })
    }

    pub fn with_recv_timeout(mut self, recv_timeout: Duration) -> Self {
        self.recv_timeout = recv_timeout;
        self
    }

    #[cfg(test)]
    pub fn relay_addr(&self) -> SocketAddr {
        self.relay
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Socks5Error> {
        self.socket
            .local_addr()
            .map_err(|err| Socks5Error::Io(err.kind().to_string()))
    }

    /// Whether the association is still valid.
    ///
    /// The proxy drops the UDP relay when the TCP control connection closes, so a
    /// dead control socket means datagrams are silently going nowhere.
    pub async fn is_control_alive(&self) -> bool {
        let control = self.control.lock().await;
        // A readable control socket that yields 0 bytes means the peer closed it.
        match control.try_read(&mut [0u8; 1]) {
            Ok(0) => false,
            Ok(_) => true,
            Err(err) => err.kind() == std::io::ErrorKind::WouldBlock,
        }
    }

    /// Send one datagram to `destination` through the proxy.
    pub async fn send_to(
        &self,
        destination: SocketAddr,
        payload: &[u8],
    ) -> Result<(), Socks5Error> {
        let frame = encode_udp_datagram(destination, payload);
        self.socket
            .send(&frame)
            .await
            .map_err(|err| Socks5Error::Io(format!("send: {}", err.kind())))?;
        Ok(())
    }

    /// Receive one datagram, returning the origin (when the proxy reports one)
    /// and the payload.
    pub async fn recv_from(&self) -> Result<(Option<SocketAddr>, Vec<u8>), Socks5Error> {
        let mut buffer = vec![0u8; self.max_datagram_bytes];
        let read = tokio::time::timeout(self.recv_timeout, self.socket.recv(&mut buffer))
            .await
            .map_err(|_| Socks5Error::Timeout("recv".to_string()))?
            .map_err(|err| Socks5Error::Io(format!("recv: {}", err.kind())))?;
        let (origin, payload) = decode_udp_datagram(&buffer[..read])?;
        Ok((origin, payload.to_vec()))
    }
}

/// A wildcard relay address means "same host as the control connection".
pub fn normalize_relay_addr(relay: SocketAddr, control_peer: Option<SocketAddr>) -> SocketAddr {
    let is_wildcard = match relay.ip() {
        IpAddr::V4(addr) => addr.is_unspecified(),
        IpAddr::V6(addr) => addr.is_unspecified(),
    };
    match (is_wildcard, control_peer) {
        (true, Some(peer)) => SocketAddr::new(peer.ip(), relay.port()),
        _ => relay,
    }
}

async fn negotiate_auth(
    control: &mut TcpStream,
    endpoint: &Socks5Endpoint,
    timeout: Duration,
) -> Result<(), Socks5Error> {
    let offer_userpass = endpoint.username.is_some();
    let greeting: Vec<u8> = if offer_userpass {
        vec![SOCKS5_VERSION, 2, AUTH_NONE, AUTH_USERPASS]
    } else {
        vec![SOCKS5_VERSION, 1, AUTH_NONE]
    };
    write_all(control, &greeting, timeout).await?;

    let mut reply = [0u8; 2];
    read_exact(control, &mut reply, timeout).await?;
    if reply[0] != SOCKS5_VERSION {
        return Err(Socks5Error::Protocol(format!(
            "greeting version {:#04x}",
            reply[0]
        )));
    }
    match reply[1] {
        AUTH_NONE => Ok(()),
        AUTH_USERPASS => {
            let (Some(user), pass) = (endpoint.username.as_deref(), endpoint.password.as_deref())
            else {
                return Err(Socks5Error::AuthFailed(
                    "proxy requires credentials but none were configured".to_string(),
                ));
            };
            let pass = pass.unwrap_or("");
            if user.len() > 255 || pass.len() > 255 {
                return Err(Socks5Error::AuthFailed(
                    "username or password exceeds 255 bytes".to_string(),
                ));
            }
            let mut request = Vec::with_capacity(3 + user.len() + pass.len());
            request.push(USERPASS_VERSION);
            request.push(user.len() as u8);
            request.extend_from_slice(user.as_bytes());
            request.push(pass.len() as u8);
            request.extend_from_slice(pass.as_bytes());
            write_all(control, &request, timeout).await?;

            let mut auth_reply = [0u8; 2];
            read_exact(control, &mut auth_reply, timeout).await?;
            if auth_reply[1] != 0x00 {
                return Err(Socks5Error::AuthFailed(format!(
                    "credentials rejected (status {:#04x})",
                    auth_reply[1]
                )));
            }
            Ok(())
        }
        AUTH_UNACCEPTABLE => Err(Socks5Error::AuthFailed(
            "proxy accepted none of the offered methods".to_string(),
        )),
        other => Err(Socks5Error::AuthFailed(format!(
            "unsupported auth method {other:#04x}"
        ))),
    }
}

async fn request_udp_associate(
    control: &mut TcpStream,
    local: SocketAddr,
    timeout: Duration,
) -> Result<SocketAddr, Socks5Error> {
    let mut request = Vec::with_capacity(22);
    request.push(SOCKS5_VERSION);
    request.push(CMD_UDP_ASSOCIATE);
    request.push(0x00); // RSV
    match local.ip() {
        IpAddr::V4(addr) => {
            request.push(ATYP_IPV4);
            request.extend_from_slice(&addr.octets());
        }
        IpAddr::V6(addr) => {
            request.push(ATYP_IPV6);
            request.extend_from_slice(&addr.octets());
        }
    }
    request.extend_from_slice(&local.port().to_be_bytes());
    write_all(control, &request, timeout).await?;

    let mut head = [0u8; 4];
    read_exact(control, &mut head, timeout).await?;
    if head[0] != SOCKS5_VERSION {
        return Err(Socks5Error::Protocol(format!(
            "reply version {:#04x}",
            head[0]
        )));
    }
    if head[1] != REPLY_SUCCEEDED {
        return Err(Socks5Error::AssociateRejected(head[1]));
    }
    match head[3] {
        ATYP_IPV4 => {
            let mut rest = [0u8; 6];
            read_exact(control, &mut rest, timeout).await?;
            let ip = Ipv4Addr::new(rest[0], rest[1], rest[2], rest[3]);
            let port = u16::from_be_bytes([rest[4], rest[5]]);
            Ok(SocketAddr::new(IpAddr::V4(ip), port))
        }
        ATYP_IPV6 => {
            let mut rest = [0u8; 18];
            read_exact(control, &mut rest, timeout).await?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&rest[..16]);
            let port = u16::from_be_bytes([rest[16], rest[17]]);
            Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        ATYP_DOMAIN => {
            let mut length = [0u8; 1];
            read_exact(control, &mut length, timeout).await?;
            let mut name = vec![0u8; length[0] as usize + 2];
            read_exact(control, &mut name, timeout).await?;
            // A name here cannot be used as a datagram destination; the caller
            // needs a routable address for the relay socket.
            Err(Socks5Error::Protocol(
                "proxy returned a domain name for the UDP relay".to_string(),
            ))
        }
        other => Err(Socks5Error::Protocol(format!(
            "unsupported reply ATYP {other:#04x}"
        ))),
    }
}

async fn write_all(
    control: &mut TcpStream,
    bytes: &[u8],
    timeout: Duration,
) -> Result<(), Socks5Error> {
    tokio::time::timeout(timeout, control.write_all(bytes))
        .await
        .map_err(|_| Socks5Error::Timeout("write".to_string()))?
        .map_err(|err| Socks5Error::Io(format!("write: {}", err.kind())))
}

async fn read_exact(
    control: &mut TcpStream,
    buffer: &mut [u8],
    timeout: Duration,
) -> Result<(), Socks5Error> {
    tokio::time::timeout(timeout, control.read_exact(buffer))
        .await
        .map_err(|_| Socks5Error::Timeout("read".to_string()))?
        .map_err(|err| Socks5Error::Io(format!("read: {}", err.kind())))
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_endpoint() {
        let endpoint = Socks5Endpoint::parse("socks5://127.0.0.1:1080").unwrap();
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 1080);
        assert!(endpoint.username.is_none());
        assert!(endpoint.password.is_none());
    }

    #[test]
    fn parses_socks5h_and_credentials() {
        let endpoint = Socks5Endpoint::parse("socks5h://alice:s3cret@proxy.example:1080").unwrap();
        assert_eq!(endpoint.host, "proxy.example");
        assert_eq!(endpoint.username.as_deref(), Some("alice"));
        assert_eq!(endpoint.password.as_deref(), Some("s3cret"));
    }

    #[test]
    fn password_may_contain_at_and_percent_encoding() {
        // The authority is split at the LAST '@', so a literal '@' inside the
        // password still leaves host:port intact.
        let endpoint = Socks5Endpoint::parse("socks5://bob:p%40ss@word@10.0.0.5:7890").unwrap();
        assert_eq!(endpoint.host, "10.0.0.5");
        assert_eq!(endpoint.port, 7890);
        assert_eq!(endpoint.username.as_deref(), Some("bob"));
        // %40 decodes to '@', and the literal '@word' before the final '@' is
        // part of the password too.
        assert_eq!(endpoint.password.as_deref(), Some("p@ss@word"));

        // Percent-encoding a ':' keeps it out of the user/pass split.
        let colon = Socks5Endpoint::parse("socks5://user:a%3Ab@127.0.0.1:1080").unwrap();
        assert_eq!(colon.password.as_deref(), Some("a:b"));
    }

    #[test]
    fn parses_bracketed_ipv6_endpoint() {
        let endpoint = Socks5Endpoint::parse("socks5://[2001:db8::1]:1080").unwrap();
        assert_eq!(endpoint.host, "2001:db8::1");
        assert_eq!(endpoint.port, 1080);
        // Re-forming the authority must re-bracket the literal.
        assert_eq!(endpoint.authority(), "[2001:db8::1]:1080");
    }

    #[test]
    fn rejects_non_socks5_and_malformed_endpoints() {
        for bad in [
            "http://proxy:8080",
            "socks5://",
            "socks5://noport",
            "socks5://host:0",
            "socks5://host:notaport",
            "socks5://[2001:db8::1",
        ] {
            assert!(Socks5Endpoint::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn encodes_ipv4_udp_request_header() {
        let destination: SocketAddr = "203.0.113.9:500".parse().unwrap();
        let frame = encode_udp_datagram(destination, &[0xaa, 0xbb]);
        assert_eq!(&frame[..3], &[0x00, 0x00, 0x00]); // RSV + FRAG
        assert_eq!(frame[3], ATYP_IPV4);
        assert_eq!(&frame[4..8], &[203, 0, 113, 9]);
        assert_eq!(u16::from_be_bytes([frame[8], frame[9]]), 500);
        assert_eq!(&frame[10..], &[0xaa, 0xbb]);
    }

    #[test]
    fn encodes_ipv6_udp_request_header() {
        let destination: SocketAddr = "[2001:db8::2]:4500".parse().unwrap();
        let frame = encode_udp_datagram(destination, &[0x01]);
        assert_eq!(frame[3], ATYP_IPV6);
        assert_eq!(u16::from_be_bytes([frame[20], frame[21]]), 4500);
        assert_eq!(&frame[22..], &[0x01]);
    }

    #[test]
    fn round_trips_ipv4_datagram() {
        let destination: SocketAddr = "198.51.100.7:4500".parse().unwrap();
        let payload = [0xde, 0xad, 0xbe, 0xef];
        let frame = encode_udp_datagram(destination, &payload);
        let (origin, decoded) = decode_udp_datagram(&frame).unwrap();
        assert_eq!(origin, Some(destination));
        assert_eq!(decoded, payload);
    }

    #[test]
    fn round_trips_ipv6_datagram() {
        let destination: SocketAddr = "[2001:db8::abcd]:500".parse().unwrap();
        let payload = [1u8, 2, 3];
        let frame = encode_udp_datagram(destination, &payload);
        let (origin, decoded) = decode_udp_datagram(&frame).unwrap();
        assert_eq!(origin, Some(destination));
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decodes_domain_typed_reply_payload_without_address() {
        // ATYP=domain: no routable origin, but the payload must still be recovered.
        let mut frame = vec![0x00, 0x00, 0x00, ATYP_DOMAIN, 3];
        frame.extend_from_slice(b"abc");
        frame.extend_from_slice(&500u16.to_be_bytes());
        frame.extend_from_slice(&[0x42, 0x43]);
        let (origin, payload) = decode_udp_datagram(&frame).unwrap();
        assert!(origin.is_none());
        assert_eq!(payload, &[0x42, 0x43]);
    }

    #[test]
    fn rejects_fragmented_reply() {
        // FRAG != 0 would hand a partial IKE message to the state machine.
        let frame = vec![0x00, 0x00, 0x01, ATYP_IPV4, 1, 2, 3, 4, 0x01, 0xf4];
        assert!(matches!(
            decode_udp_datagram(&frame),
            Err(Socks5Error::Protocol(_))
        ));
    }

    #[test]
    fn rejects_truncated_and_unknown_atyp_replies() {
        assert!(decode_udp_datagram(&[0x00, 0x00, 0x00]).is_err());
        // ATYP=IPv4 but only 2 address bytes present.
        assert!(decode_udp_datagram(&[0x00, 0x00, 0x00, ATYP_IPV4, 1, 2]).is_err());
        assert!(decode_udp_datagram(&[0x00, 0x00, 0x00, 0x09, 1, 2, 3, 4, 0, 0]).is_err());
    }

    #[test]
    fn wildcard_relay_is_rewritten_to_the_proxy_host() {
        // This is what makes local proxies (mihomo, sing-box) usable: they answer
        // UDP ASSOCIATE with 0.0.0.0 meaning "same host as the TCP connection".
        let relay: SocketAddr = "0.0.0.0:34567".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:1080".parse().unwrap();
        assert_eq!(
            normalize_relay_addr(relay, Some(peer)),
            "127.0.0.1:34567".parse::<SocketAddr>().unwrap()
        );

        let relay6: SocketAddr = "[::]:34567".parse().unwrap();
        let peer6: SocketAddr = "[2001:db8::1]:1080".parse().unwrap();
        assert_eq!(
            normalize_relay_addr(relay6, Some(peer6)),
            "[2001:db8::1]:34567".parse::<SocketAddr>().unwrap()
        );

        // A concrete relay address is left alone.
        let concrete: SocketAddr = "203.0.113.5:34567".parse().unwrap();
        assert_eq!(normalize_relay_addr(concrete, Some(peer)), concrete);
    }

    #[test]
    fn reply_codes_are_named() {
        assert_eq!(reply_reason(0x00), "succeeded");
        assert_eq!(reply_reason(0x07), "command-not-supported");
        assert_eq!(reply_reason(0xee), "unknown");
    }

    #[test]
    fn error_codes_are_stable_and_prefixed() {
        assert_eq!(
            Socks5Error::InvalidEndpoint("x".into()).to_string(),
            "socks5_invalid_endpoint:x"
        );
        assert_eq!(
            Socks5Error::AssociateRejected(0x07).to_string(),
            "socks5_associate_rejected:command-not-supported"
        );
    }

    /// A throwaway SOCKS5 server that speaks just enough of RFC 1928 to prove the
    /// client's handshake and relay framing are correct end to end.
    ///
    /// It echoes each relayed datagram back with the original destination as the
    /// reported origin, so the test can assert the full encode → relay → decode
    /// round trip rather than only the codecs.
    async fn spawn_test_socks5_server(
        require_auth: Option<(String, String)>,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test proxy");
        let addr = listener.local_addr().expect("proxy addr");
        let handle = tokio::spawn(async move {
            let (mut control, _) = listener.accept().await.expect("accept");

            // Greeting.
            let mut head = [0u8; 2];
            control.read_exact(&mut head).await.expect("greeting head");
            let mut methods = vec![0u8; head[1] as usize];
            control.read_exact(&mut methods).await.expect("methods");

            match &require_auth {
                Some((want_user, want_pass)) => {
                    assert!(
                        methods.contains(&AUTH_USERPASS),
                        "client must offer userpass"
                    );
                    control
                        .write_all(&[SOCKS5_VERSION, AUTH_USERPASS])
                        .await
                        .expect("select userpass");
                    let mut version = [0u8; 1];
                    control
                        .read_exact(&mut version)
                        .await
                        .expect("auth version");
                    assert_eq!(version[0], USERPASS_VERSION);
                    let mut ulen = [0u8; 1];
                    control.read_exact(&mut ulen).await.expect("ulen");
                    let mut user = vec![0u8; ulen[0] as usize];
                    control.read_exact(&mut user).await.expect("user");
                    let mut plen = [0u8; 1];
                    control.read_exact(&mut plen).await.expect("plen");
                    let mut pass = vec![0u8; plen[0] as usize];
                    control.read_exact(&mut pass).await.expect("pass");
                    let ok = user == want_user.as_bytes() && pass == want_pass.as_bytes();
                    control
                        .write_all(&[USERPASS_VERSION, if ok { 0x00 } else { 0x01 }])
                        .await
                        .expect("auth reply");
                    assert!(ok, "credentials must match");
                }
                None => {
                    control
                        .write_all(&[SOCKS5_VERSION, AUTH_NONE])
                        .await
                        .expect("select none");
                }
            }

            // UDP ASSOCIATE request.
            let mut request = [0u8; 4];
            control
                .read_exact(&mut request)
                .await
                .expect("associate head");
            assert_eq!(request[1], CMD_UDP_ASSOCIATE);
            let mut discard = vec![0u8; if request[3] == ATYP_IPV6 { 18 } else { 6 }];
            control
                .read_exact(&mut discard)
                .await
                .expect("associate addr");

            // Stand up the relay socket and answer with a wildcard address, which
            // is what real proxies do — exercising the client's rewrite path.
            let relay = UdpSocket::bind("127.0.0.1:0").await.expect("bind relay");
            let relay_port = relay.local_addr().expect("relay addr").port();
            let mut reply = vec![SOCKS5_VERSION, REPLY_SUCCEEDED, 0x00, ATYP_IPV4];
            reply.extend_from_slice(&[0, 0, 0, 0]);
            reply.extend_from_slice(&relay_port.to_be_bytes());
            control.write_all(&reply).await.expect("associate reply");

            // Relay loop: decode the client's frame, echo the payload back tagged
            // with the destination it asked for.
            let mut buffer = vec![0u8; 2048];
            while let Ok((read, from)) = relay.recv_from(&mut buffer).await {
                let Ok((destination, payload)) = decode_udp_datagram(&buffer[..read]) else {
                    continue;
                };
                let origin = destination.expect("test frames use IP destinations");
                let echo = encode_udp_datagram(origin, payload);
                if relay.send_to(&echo, from).await.is_err() {
                    break;
                }
            }
            // Hold the control connection until the task is dropped, mirroring the
            // RFC 1928 §7 lifetime rule.
            drop(control);
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn associates_and_round_trips_a_datagram_without_auth() {
        let (proxy, server) = spawn_test_socks5_server(None).await;
        let endpoint = Socks5Endpoint::parse(&format!("socks5://{proxy}")).unwrap();
        let client = Socks5UdpClient::connect(
            &endpoint,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Duration::from_secs(5),
        )
        .await
        .expect("associate");

        // The proxy answered 0.0.0.0; the client must have rewritten it to the
        // proxy host, otherwise sending would fail.
        assert_eq!(client.relay_addr().ip(), proxy.ip());

        let epdg: SocketAddr = "203.0.113.10:500".parse().unwrap();
        let ike_payload = [0x21, 0x20, 0x22, 0x08];
        client.send_to(epdg, &ike_payload).await.expect("send");
        let (origin, echoed) = client.recv_from().await.expect("recv");
        assert_eq!(origin, Some(epdg));
        assert_eq!(echoed, ike_payload);

        server.abort();
    }

    #[tokio::test]
    async fn associates_with_username_password_auth() {
        let (proxy, server) =
            spawn_test_socks5_server(Some(("alice".to_string(), "s3cret".to_string()))).await;
        let endpoint = Socks5Endpoint::parse(&format!("socks5://alice:s3cret@{proxy}")).unwrap();
        let client = Socks5UdpClient::connect(
            &endpoint,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Duration::from_secs(5),
        )
        .await
        .expect("associate with auth");

        let epdg: SocketAddr = "198.51.100.20:4500".parse().unwrap();
        client.send_to(epdg, &[0xff]).await.expect("send");
        let (origin, echoed) = client.recv_from().await.expect("recv");
        assert_eq!(origin, Some(epdg));
        assert_eq!(echoed, vec![0xff]);

        server.abort();
    }

    #[tokio::test]
    async fn connect_fails_cleanly_when_no_proxy_is_listening() {
        // Port 1 on loopback is not going to be a SOCKS5 proxy.
        let endpoint = Socks5Endpoint::parse("socks5://127.0.0.1:1").unwrap();
        let result = Socks5UdpClient::connect(
            &endpoint,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            Duration::from_millis(500),
        )
        .await;
        assert!(matches!(
            result,
            Err(Socks5Error::Io(_)) | Err(Socks5Error::Timeout(_))
        ));
    }
}
