//! Framed SIP over TCP/TLS. Caller owns bearer/worker and connection lifetime.
//! No DNS, socket creation, trust override, or transport downgrade occurs here.
use std::{collections::VecDeque, net::SocketAddr, sync::{Arc, atomic::{AtomicU64, Ordering}}, time::Duration};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt}, net::TcpStream};
use tokio_rustls::{TlsConnector, rustls::{self, pki_types::ServerName}};
use crate::connectivity::core::{sip_frame, ImsError};
use super::super::errors::code;
#[path = "sip_tls_identity.rs"]
mod tls_identity;

pub(super) const MAX_FRAME: usize = 1_048_576;
const MAX_HEADER: usize = 16_384;
const MAX_PENDING_RESPONSES: usize = 64;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

trait SipIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> SipIo for T {}

pub(super) struct SipStream {
    id: u64,
    io: Box<dyn SipIo>,
    local: SocketAddr,
    buffer: Vec<u8>,
    poisoned: bool,
    tls_algorithms: Option<crate::connectivity::core::registration_diagnostics::TlsAlgorithms>,
}

pub(super) enum StreamFrame { Sip(Vec<u8>), Ping, Pong }

impl SipStream {
    pub(super) fn plain(socket: TcpStream) -> Result<Self, ImsError> {
        let local = socket.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?;
        socket.set_nodelay(true).map_err(|_| ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED))?;
        Ok(Self::new(Box::new(socket), local))
    }

    pub(super) async fn tls(socket: TcpStream, authority: &str, timeout: Duration) -> Result<Self, ImsError> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::tls_with_roots(socket, authority, timeout, roots).await
    }

    async fn tls_with_roots(socket: TcpStream, authority: &str, timeout: Duration, roots: rustls::RootCertStore) -> Result<Self, ImsError> {
        // Original configured/service authority, NOT a DNS SRV target. Numeric
        // names use IP SAN validation; there is never an insecure verifier.
        if authority.is_empty() || authority.len() > 253 || authority.bytes().any(|b| b <= 32 || b >= 127) {
            return Err(ImsError::new(code::TLS_AUTHORITY_INVALID));
        }
        let name = ServerName::try_from(authority.to_owned()).map_err(|_| ImsError::new(code::TLS_AUTHORITY_INVALID))?;
        let local = socket.local_addr().map_err(|_| ImsError::new(code::CHANNEL_LOCAL_ADDR_FAILED))?;
        socket.set_nodelay(true).map_err(|_| ImsError::new(code::CHANNEL_STREAM_CONNECT_FAILED))?;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().map_err(|_| ImsError::new(code::TLS_HANDSHAKE_FAILED))?
            .with_root_certificates(roots).with_no_client_auth();
        let tls = tokio::time::timeout(timeout, TlsConnector::from(Arc::new(config)).connect(name, socket)).await
            .map_err(|_| ImsError::new(code::TLS_HANDSHAKE_TIMEOUT))?
            .map_err(|_| ImsError::new(code::TLS_HANDSHAKE_FAILED))?;
        let peer = tls.get_ref().1.peer_certificates().and_then(|certificates| certificates.first())
            .ok_or_else(|| ImsError::new(code::TLS_HANDSHAKE_FAILED))?;
        if !tls_identity::has_exact_identity(peer.as_ref(), authority) {
            return Err(ImsError::new(code::TLS_HANDSHAKE_FAILED));
        }
        let algorithms = crate::connectivity::core::registration_diagnostics::TlsAlgorithms {
            version: tls.get_ref().1.protocol_version().map(|version| format!("{version:?}")),
            cipher_suite: tls.get_ref().1.negotiated_cipher_suite().map(|suite| format!("{:?}", suite.suite())),
        };
        let mut stream = Self::new(Box::new(tls), local);
        stream.tls_algorithms = Some(algorithms);
        Ok(stream)
    }

    fn new(io: Box<dyn SipIo>, local: SocketAddr) -> Self {
        Self { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), io, local, buffer: Vec::new(), poisoned: false, tls_algorithms: None }
    }
    pub(super) fn tls_algorithms(&self) -> Option<crate::connectivity::core::registration_diagnostics::TlsAlgorithms> { self.tls_algorithms.clone() }
    pub(super) fn id(&self) -> u64 { self.id }
    pub(super) fn local_addr(&self) -> SocketAddr { self.local }
    pub(super) fn is_usable(&self) -> bool { !self.poisoned }

    pub(super) async fn send(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        if self.poisoned { return Err(ImsError::new(code::CHANNEL_STREAM_CLOSED)); }
        if frame.len() > MAX_FRAME { return Err(ImsError::new(code::CHANNEL_STREAM_FRAME_INVALID)); }
        // Cancellation after a partial write poisons this stream. Reusing its
        // prefix for another SIP request would corrupt transaction framing.
        self.poisoned = true;
        let sent = tokio::time::timeout(IO_TIMEOUT, async {
            self.io.write_all(frame).await?;
            self.io.flush().await
        }).await;
        if !matches!(sent, Ok(Ok(()))) { return Err(ImsError::new(code::CHANNEL_STREAM_WRITE_FAILED)); }
        self.poisoned = false;
        Ok(())
    }

    pub(super) async fn receive(&mut self, expect_pong: bool) -> Result<StreamFrame, ImsError> {
        loop {
            if self.poisoned { return Err(ImsError::new(code::CHANNEL_STREAM_CLOSED)); }
            match take_frame(&mut self.buffer, expect_pong) {
                Ok(Some(frame)) => return Ok(frame),
                Err(error) => { self.poisoned = true; return Err(error); }
                Ok(None) => {},
            }
            let remaining = MAX_FRAME.saturating_sub(self.buffer.len());
            if remaining == 0 { self.poisoned = true; return Err(ImsError::new(code::CHANNEL_STREAM_FRAME_INVALID)); }
            let mut bytes = [0u8; 8192];
            let count = match self.io.read(&mut bytes[..remaining.min(8192)]).await {
                Ok(count) => count,
                Err(_) => { self.poisoned = true; return Err(ImsError::new(code::CHANNEL_STREAM_CLOSED)); }
            };
            if count == 0 { self.poisoned = true; return Err(ImsError::new(code::CHANNEL_STREAM_CLOSED)); }
            // `read` is cancellation-safe; partial SIP bytes stay in this owned
            // buffer if select! changes to another protected connection.
            self.buffer.extend_from_slice(&bytes[..count]);
        }
    }
}

fn take_frame(buffer: &mut Vec<u8>, expect_pong: bool) -> Result<Option<StreamFrame>, ImsError> {
    let invalid = || ImsError::new(code::CHANNEL_STREAM_FRAME_INVALID);
    if buffer.starts_with(b"\r\n\r\n") { buffer.drain(..4); return Ok(Some(StreamFrame::Ping)); }
    if buffer.starts_with(b"\r\n") {
        // Do not consume the first half of an unsolicited, fragmented ping.
        // A lone CRLF is an immediate pong only for our outstanding probe.
        if (buffer.len() == 2 && !expect_pong) || (buffer.len() == 3 && buffer[2] == b'\r') { return Ok(None); }
        buffer.drain(..2); return Ok(Some(StreamFrame::Pong));
    }
    if buffer == b"\r" { return Ok(None); }
    let Some(header_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") else {
        if buffer.len() > MAX_HEADER { return Err(invalid()); }
        return Ok(None);
    };
    if header_end > MAX_HEADER { return Err(invalid()); }
    let header = std::str::from_utf8(&buffer[..header_end]).map_err(|_| invalid())?;
    let mut lines = header.split("\r\n");
    let start = lines.next().ok_or_else(invalid)?;
    if start.bytes().any(|b| b < 32 || b == 127) { return Err(invalid()); }
    if let Some(response) = start.strip_prefix("SIP/2.0 ") {
        let (status, _) = response.split_once(' ').ok_or_else(invalid)?;
        if status.len() != 3 || !status.bytes().all(|b| b.is_ascii_digit())
            || !status.parse::<u16>().is_ok_and(|status| (100..=699).contains(&status))
        { return Err(invalid()); }
    } else {
        let mut parts = start.split(' ');
        if !parts.next().is_some_and(token) || !parts.next().is_some_and(|uri| !uri.is_empty())
            || parts.next() != Some("SIP/2.0") || parts.next().is_some()
        { return Err(invalid()); }
    }
    let mut had_header = false;
    for (count, line) in lines.enumerate() {
        if count >= 128 || line.bytes().any(|b| (b < 32 && b != b'\t') || b == 127) { return Err(invalid()); }
        if line.starts_with([' ', '\t']) {
            if !had_header { return Err(invalid()); }
        } else {
            let (name, _) = line.split_once(':').ok_or_else(invalid)?;
            if !token(name.trim_end_matches([' ', '\t'])) { return Err(invalid()); }
            had_header = true;
        }
    }
    let mut values = sip_frame::header_values(buffer, "Content-Length");
    values.extend(sip_frame::header_values(buffer, "l"));
    let [length] = values.as_slice() else { return Err(invalid()); };
    let length = length.trim();
    if length.is_empty() || !length.bytes().all(|byte| byte.is_ascii_digit()) { return Err(invalid()); }
    let size = length.parse::<usize>().ok().and_then(|body| (header_end + 4).checked_add(body))
        .filter(|size| *size <= MAX_FRAME).ok_or_else(invalid)?;
    if buffer.len() < size { return Ok(None); }
    Ok(Some(StreamFrame::Sip(buffer.drain(..size).collect())))
}

/// An association has a UE client stream and optional peer-initiated server
/// stream. The listener is shared by the channel during an SA transition.
pub(super) struct StreamAssociation {
    pub client: SipStream,
    pub server: Option<SipStream>,
    pub expected_server: Option<SocketAddr>,
}
impl StreamAssociation {
    pub(super) async fn receive(&mut self, expect_pong: bool) -> Result<(u64, StreamFrame), ImsError> {
        let client_id = self.client.id();
        let Some(server) = self.server.as_mut() else {
            return self.client.receive(expect_pong).await.map(|frame| (client_id, frame));
        };
        let server_id = server.id();
        let (result, is_server) = tokio::select! {
            result = self.client.receive(expect_pong) => (result, false),
            result = server.receive(false) => (result, true),
        };
        if is_server && result.as_ref().is_err_and(|error| error.code() == code::CHANNEL_STREAM_CLOSED) {
            self.server = None;
            return self.client.receive(expect_pong).await.map(|frame| (client_id, frame));
        }
        result.map(|frame| (if is_server { server_id } else { client_id }, frame))
    }
    pub(super) fn connection(&mut self, id: u64) -> Option<&mut SipStream> {
        if self.client.id() == id { Some(&mut self.client) }
        else { self.server.as_mut().filter(|stream| stream.id() == id) }
    }
}

fn token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.!%*_+`'~".contains(&b))
}

#[derive(PartialEq, Eq)]
struct ResponseKey { call_id: String, cseq: u32, method: String, branch: String }
impl ResponseKey {
    fn parse(frame: &[u8]) -> Option<Self> {
        let mut ids = sip_frame::header_values(frame, "Call-ID");
        ids.extend(sip_frame::header_values(frame, "i"));
        let [call_id] = ids.as_slice() else { return None; };
        if call_id.is_empty() || call_id.len() > 1024 { return None; }
        let sequences = sip_frame::header_values(frame, "CSeq");
        let [cseq] = sequences.as_slice() else { return None; };
        let mut parts = cseq.split_ascii_whitespace();
        let number = parts.next()?;
        if !number.bytes().all(|b| b.is_ascii_digit()) { return None; }
        let cseq = number.parse::<u32>().ok().filter(|value| *value < 0x8000_0000)?;
        let method = parts.next()?;
        if parts.next().is_some() || !token(method) { return None; }
        let via = sip_frame::header_values(frame, "Via").into_iter().next()
            .or_else(|| sip_frame::header_values(frame, "v").into_iter().next())?;
        let branches = via.split(',').next()?.split(';').skip(1).filter_map(|parameter| {
            let (name, value) = parameter.trim().split_once('=')?;
            name.eq_ignore_ascii_case("branch").then_some(value.trim())
        }).collect::<Vec<_>>();
        let [branch] = branches.as_slice() else { return None; };
        if branch.is_empty() || branch.len() > 256 || !branch.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.!%*_+`'~".contains(&b)) {
            return None;
        }
        // received/rport may be added while building the response. Only the
        // transaction identity, not mutable Via routing parameters, binds it.
        Some(Self { call_id: call_id.clone(), cseq, method: method.to_ascii_uppercase(), branch: (*branch).to_owned() })
    }
}

#[derive(Default)]
pub(super) struct ResponseRoutes(VecDeque<(ResponseKey, u64)>);
impl ResponseRoutes {
    pub(super) fn received(&mut self, frame: &[u8], id: u64) -> Result<(), ImsError> {
        if frame.starts_with(b"SIP/2.0 ") || sip_frame::is_request(frame, "ACK") { return Ok(()); }
        let key = ResponseKey::parse(frame).ok_or_else(|| ImsError::new(code::CHANNEL_STREAM_FRAME_INVALID))?;
        if let Some((_, old)) = self.0.iter().find(|(other, _)| *other == key) {
            return if *old == id { Ok(()) } else { Err(ImsError::new(code::CHANNEL_STREAM_RESPONSE_OWNER_MISSING)) };
        }
        if self.0.len() >= MAX_PENDING_RESPONSES { return Err(ImsError::new(code::CHANNEL_STREAM_RESPONSE_OWNER_MISSING)); }
        self.0.push_back((key, id)); Ok(())
    }
    pub(super) fn response_connection(&mut self, frame: &[u8]) -> Result<Option<u64>, ImsError> {
        if !frame.starts_with(b"SIP/2.0 ") { return Ok(None); }
        let key = ResponseKey::parse(frame).ok_or_else(|| ImsError::new(code::CHANNEL_STREAM_RESPONSE_OWNER_MISSING))?;
        let index = self.0.iter().position(|(other, _)| *other == key)
            .ok_or_else(|| ImsError::new(code::CHANNEL_STREAM_RESPONSE_OWNER_MISSING))?;
        let id = self.0[index].1;
        if sip_frame::parse_status(frame).is_ok_and(|status| status >= 200) { self.0.remove(index); }
        Ok(Some(id))
    }
}

#[cfg(test)]
#[path = "sip_stream_tests.rs"]
mod tests;
