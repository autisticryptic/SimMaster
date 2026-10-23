//! Trusted, original-ARC-owned TLS over recovered opaque carrier packets.
//!
//! Uses rustls with ONLY the explicitly imported root and original authority.
//! TLS readiness is not SDK login, hosting, a message channel or paid-service
//! authorization. No socket is opened. Standard TLS records are accumulated
//! before verification; verified plaintext is parsed by the recovered Common
//! format rather than treating a TLS record/read as a call-packet boundary.
use super::{
    packet_transport::{OwnedPacketTransport, OwnedWireFrames, PacketRoute, RouterTiming},
    rpc_pending::RpcConnectionScope,
    session::AttemptOwner,
    tls_plaintext::PlaintextPacketDecoder,
    transport_packets::{TransportBytes, TransportError, TLS_FRAGMENT_THRESHOLD},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fmt, io::{Cursor, Read, Write}, sync::Arc, time::{Duration, Instant}};
use tokio_rustls::rustls::{self, pki_types::{CertificateDer, ServerName}};

const MAX_ROOT_BYTES: usize = 16_384;
const MAX_CIPHER_RECORD_BYTES: usize = 18_432 + 5;
const MAX_TLS_IO_BYTES: usize = 2 * (super::rpc_stream::MAX_STREAM_BYTES + 64);
const MAX_TLS_RECORDS_PER_STEP: usize = 128;
const TLS_FLUSH_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsError {
    Transport(TransportError), InvalidConfiguration, HandshakeFailed,
    NotReady, Timeout, ClockRollback, TooLarge, Closed,
}
impl From<TransportError> for TlsError { fn from(error: TransportError) -> Self { Self::Transport(error) } }
impl TlsError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Transport(error) => error.code(),
            Self::InvalidConfiguration => "jegotrip_tls_configuration_invalid",
            Self::HandshakeFailed => "jegotrip_tls_handshake_failed",
            Self::NotReady => "jegotrip_tls_not_verified",
            Self::Timeout => "jegotrip_tls_deadline_expired",
            Self::ClockRollback => "jegotrip_tls_clock_rollback",
            Self::TooLarge => "jegotrip_tls_too_large",
            Self::Closed => "jegotrip_tls_closed",
        }
    }
}

pub struct TlsParameters<'a> {
    pub authority: &'a str,
    pub root_der_base64: &'a str,
    pub tls_only: bool,
    /// Original deployment SNI choice, not a resolved TCP address or default.
    pub enable_sni: bool,
    /// Local bounds. Partial traffic never extends an existing deadline.
    pub handshake_timeout: Duration,
    pub partial_timeout: Duration,
}
impl fmt::Debug for TlsParameters<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("TlsParameters(<redacted>)") }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsPhase { Handshaking, Verified, Closed }

pub struct VerifiedPlaintextPackets { owner: AttemptOwner, scope: RpcConnectionScope, packets: Vec<TransportBytes> }
impl VerifiedPlaintextPackets {
    /// Checked snapshot, NOT a revocable IO lease. Only this module can create
    /// it after actual TLS verification; future live dispatch must serialize
    /// final consumption/close rather than keep this snapshot across await.
    pub fn packets_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<&[TransportBytes]> {
        if &self.owner == owner && self.scope.matches(scope) && self.scope.is_open() { Some(&self.packets) } else { None }
    }
}
impl fmt::Debug for VerifiedPlaintextPackets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("VerifiedPlaintextPackets(<redacted>)") }
}

pub struct TlsProgress {
    /// Newly verified TLS, never a callback that grants SDK/hosting readiness.
    pub became_verified: bool,
    pub wire: Vec<OwnedWireFrames>,
    pub plaintext: VerifiedPlaintextPackets,
}
impl fmt::Debug for TlsProgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("TlsProgress(<redacted>)") }
}

/// Bounds ciphertext without interpreting it as authenticated plaintext. A TLS
/// record can span carriers; one carrier can contain multiple records.
#[derive(Default)]
struct CipherRecords { buffer: Vec<u8> }
impl CipherRecords {
    fn feed(&mut self, mut bytes: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
        if bytes.len() > MAX_TLS_IO_BYTES { return Err(TlsError::TooLarge); }
        let mut records = Vec::new();
        loop {
            if self.buffer.len() >= 5 {
                if !(20..=23).contains(&self.buffer[0]) || self.buffer[1] != 3 || !(1..=3).contains(&self.buffer[2]) {
                    return Err(TlsError::HandshakeFailed);
                }
                let length = u16::from_be_bytes([self.buffer[3], self.buffer[4]]) as usize + 5;
                if length > MAX_CIPHER_RECORD_BYTES { return Err(TlsError::TooLarge); }
                if self.buffer.len() >= length {
                    if records.len() >= MAX_TLS_RECORDS_PER_STEP { return Err(TlsError::TooLarge); }
                    records.push(self.buffer.drain(..length).collect()); continue;
                }
            }
            if bytes.is_empty() { break; }
            let count = bytes.len().min(MAX_CIPHER_RECORD_BYTES - self.buffer.len());
            if count == 0 { return Err(TlsError::TooLarge); }
            self.buffer.extend_from_slice(&bytes[..count]); bytes = &bytes[count..];
        }
        Ok(records)
    }
}

/// Reject oversized output BEFORE allocation, including handshake traffic which
/// rustls' application-data buffer limit alone does not cap.
struct BoundedTlsOutput(Vec<u8>);
impl Write for BoundedTlsOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_TLS_IO_BYTES - self.0.len() {
            return Err(std::io::Error::other("bounded TLS output"));
        }
        self.0.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

pub struct TlsEngine {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    transport: OwnedPacketTransport,
    connection: Option<rustls::ClientConnection>,
    authority: String,
    phase: TlsPhase,
    records: CipherRecords,
    plaintext: PlaintextPacketDecoder,
    handshake_deadline: Instant,
    partial_deadline: Option<Instant>,
    partial_timeout: Duration,
    last_now: Instant,
}
impl fmt::Debug for TlsEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsEngine").field("phase", &self.phase()).field("materials", &"<redacted>").finish()
    }
}

impl TlsEngine {
    pub(super) fn for_arc_session(owner: AttemptOwner, scope: RpcConnectionScope, route: PacketRoute<'_>,
        parameters: TlsParameters<'_>, now: Instant) -> Result<Self, TlsError> {
        if !scope.is_open() { return Err(TlsError::Closed); }
        if !parameters.tls_only || !valid_authority(parameters.authority)
            || parameters.root_der_base64.is_empty() || parameters.root_der_base64.len() > (MAX_ROOT_BYTES + 2) / 3 * 4
            || parameters.handshake_timeout.is_zero() || parameters.handshake_timeout > Duration::from_secs(60)
            || parameters.partial_timeout.is_zero() || parameters.partial_timeout > Duration::from_secs(30) {
            return Err(TlsError::InvalidConfiguration);
        }
        let der = STANDARD.decode(parameters.root_der_base64).map_err(|_| TlsError::InvalidConfiguration)?;
        if der.is_empty() || der.len() > MAX_ROOT_BYTES { return Err(TlsError::InvalidConfiguration); }
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(der)).map_err(|_| TlsError::InvalidConfiguration)?;
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().map_err(|_| TlsError::InvalidConfiguration)?
            .with_root_certificates(roots).with_no_client_auth();
        config.enable_sni = parameters.enable_sni;
        config.enable_early_data = false;
        config.resumption = rustls::client::Resumption::disabled();
        // rustls0.23 subtracts the5-byte record header here. A native Common
        // packet <=16383 fits one plaintext record; no extra inner BE32 prefix.
        config.max_fragment_size = Some(TLS_FRAGMENT_THRESHOLD + 1 + 5);
        let name = ServerName::try_from(parameters.authority.to_owned()).map_err(|_| TlsError::InvalidConfiguration)?;
        let mut connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|_| TlsError::InvalidConfiguration)?;
        connection.set_buffer_limit(Some(MAX_TLS_IO_BYTES));
        let handshake_deadline = now.checked_add(parameters.handshake_timeout).ok_or(TlsError::InvalidConfiguration)?;
        let transport = OwnedPacketTransport::for_arc_session(owner.clone(), scope.clone(), route)?;
        Ok(Self { owner, scope, transport, connection: Some(connection), authority: parameters.authority.to_owned(),
            phase: TlsPhase::Handshaking, records: CipherRecords::default(), plaintext: PlaintextPacketDecoder::default(),
            handshake_deadline, partial_deadline: None, partial_timeout: parameters.partial_timeout, last_now: now })
    }
    pub fn phase(&self) -> TlsPhase { if self.scope.is_open() { self.phase } else { TlsPhase::Closed } }
    fn accepts(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && self.scope.is_open()
    }
    pub fn close(&mut self) {
        self.phase = TlsPhase::Closed; self.scope.close(); self.transport.close(); self.connection = None;
        self.records.buffer.clear(); self.plaintext.close(); self.partial_deadline = None;
    }
    fn check_time(&mut self, now: Instant) -> Result<(), TlsError> {
        if now < self.last_now { return Err(TlsError::ClockRollback); }
        self.last_now = now;
        if (self.phase == TlsPhase::Handshaking && now >= self.handshake_deadline)
            || self.partial_deadline.is_some_and(|deadline| now >= deadline) { return Err(TlsError::Timeout); }
        Ok(())
    }
    fn update_partial(&mut self, now: Instant) -> Result<(), TlsError> {
        if self.transport.has_partial() || !self.records.buffer.is_empty() || self.plaintext.has_partial() {
            if self.partial_deadline.is_none() { self.partial_deadline = Some(now.checked_add(self.partial_timeout).ok_or(TlsError::Timeout)?); }
        } else { self.partial_deadline = None; }
        Ok(())
    }

    pub fn tick(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, now: Instant) -> Result<bool, TlsError> {
        if !self.scope.is_open() { self.close(); return Ok(false); }
        if !self.accepts(owner, scope) { return Ok(false); }
        if let Err(error) = self.check_time(now) { self.close(); return Err(error); }
        Ok(true)
    }

    /// Returns only client TLS handshake/control output. No login data can be
    /// written until complete chain/time/signature/name AND exact SAN checks.
    pub fn take_wire_output(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, timing: Option<RouterTiming>, now: Instant)
        -> Result<Vec<OwnedWireFrames>, TlsError> {
        if !self.tick(owner, scope, now)? { return Ok(vec![]); }
        let result = self.drain_wire(timing);
        if result.is_err() { self.close(); }
        result
    }
    fn drain_wire(&mut self, timing: Option<RouterTiming>) -> Result<Vec<OwnedWireFrames>, TlsError> {
        let connection = self.connection.as_mut().ok_or(TlsError::Closed)?;
        let mut output = BoundedTlsOutput(Vec::new());
        while connection.wants_write() {
            let written = connection.write_tls(&mut output).map_err(|_| TlsError::HandshakeFailed)?;
            if written == 0 { return Err(TlsError::HandshakeFailed); }
        }
        let mut wire = Vec::new();
        for chunk in output.0.chunks(TLS_FLUSH_BYTES) {
            wire.push(self.transport.encode_tls_bytes(&self.owner, &self.scope, chunk, timing)?.ok_or(TlsError::Closed)?);
        }
        Ok(wire)
    }

    pub fn on_wire_bytes(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, bytes: &[u8],
        timing: Option<RouterTiming>, now: Instant) -> Result<Option<TlsProgress>, TlsError> {
        if !self.scope.is_open() { self.close(); return Ok(None); }
        if !self.accepts(owner, scope) { return Ok(None); }
        let result = self.receive(bytes, timing, now);
        if result.is_err() { self.close(); }
        result.map(Some)
    }
    fn receive(&mut self, bytes: &[u8], timing: Option<RouterTiming>, now: Instant) -> Result<TlsProgress, TlsError> {
        self.check_time(now)?;
        let was_verified = self.phase == TlsPhase::Verified;
        let input = self.transport.on_bytes(&self.owner, &self.scope, bytes)?;
        let mut packets = Vec::new(); let mut record_count = 0usize;
        for input in input {
            let raw = input.bytes_for_tls(&self.owner, &self.scope).ok_or(TlsError::Closed)?;
            // Allowance is shared across all carrier packets in this step.
            // Handshake ciphertext counts as well as application records.
            for record in self.records.feed(raw)? {
                record_count += 1;
                if record_count > MAX_TLS_RECORDS_PER_STEP { return Err(TlsError::TooLarge); }
                let mut cursor = Cursor::new(record);
                while cursor.position() < cursor.get_ref().len() as u64 {
                    let connection = self.connection.as_mut().ok_or(TlsError::Closed)?;
                    let count = connection.read_tls(&mut cursor).map_err(|_| TlsError::HandshakeFailed)?;
                    if count == 0 { return Err(TlsError::HandshakeFailed); }
                    let state = connection.process_new_packets().map_err(|_| TlsError::HandshakeFailed)?;
                    if !connection.is_handshaking() && self.phase != TlsPhase::Verified {
                        let peer = connection.peer_certificates().and_then(|certificates| certificates.first()).ok_or(TlsError::HandshakeFailed)?;
                        if !crate::connectivity::core::tls_identity::has_exact_identity(peer.as_ref(), &self.authority) {
                            return Err(TlsError::HandshakeFailed);
                        }
                        self.phase = TlsPhase::Verified;
                    }
                    if state.peer_has_closed() { return Err(TlsError::Closed); }
                    let mut plaintext = [0; TLS_FLUSH_BYTES];
                    loop {
                        match connection.reader().read(&mut plaintext) {
                            Ok(0) => break,
                            Ok(count) if self.phase == TlsPhase::Verified => {
                                let decoded = self.plaintext.on_bytes(&plaintext[..count])?;
                                if packets.len() + decoded.len() > super::packet_stream::MAX_PACKETS_PER_INPUT { return Err(TlsError::TooLarge); }
                                packets.extend(decoded);
                            }
                            Ok(_) => return Err(TlsError::NotReady),
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return Err(TlsError::HandshakeFailed),
                        }
                    }
                }
            }
        }
        self.update_partial(now)?;
        let wire = self.drain_wire(timing)?;
        if !self.scope.is_open() { return Err(TlsError::Closed); }
        Ok(TlsProgress { became_verified: !was_verified && self.phase == TlsPhase::Verified, wire,
            plaintext: VerifiedPlaintextPackets { owner: self.owner.clone(), scope: self.scope.clone(), packets } })
    }

    /// Validate the entire canonical Common group BEFORE encrypting. Each
    /// native packet is one writer call <=16383; no lower frame/carrier or
    /// plaintext write result is interpreted as SDK/hosting success.
    pub fn encrypt_packets(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, packets: &[TransportBytes],
        timing: Option<RouterTiming>, now: Instant) -> Result<Option<Vec<OwnedWireFrames>>, TlsError> {
        if !self.scope.is_open() { self.close(); return Ok(None); }
        if !self.accepts(owner, scope) { return Ok(None); }
        if let Err(error) = self.check_time(now) { self.close(); return Err(error); }
        if self.phase != TlsPhase::Verified { return Err(TlsError::NotReady); }
        let mut validator = PlaintextPacketDecoder::default();
        if packets.is_empty() || packets.len() > super::packet_stream::MAX_PACKETS_PER_INPUT { return Err(TlsError::TooLarge); }
        let mut total = 0usize;
        for packet in packets {
            let raw = packet.expose_to_transport();
            if raw.len() > TLS_FRAGMENT_THRESHOLD + 1 { return Err(TlsError::TooLarge); }
            total = total.checked_add(raw.len()).ok_or(TlsError::TooLarge)?;
            if total > MAX_TLS_IO_BYTES { return Err(TlsError::TooLarge); }
            let decoded = validator.on_bytes(raw)?;
            // Preserve one native call packet per TLS writer call. Arbitrary
            // caller-built slices or coalesced packets are not wire-equivalent.
            if decoded.len() != 1 || decoded[0].expose_to_transport() != raw {
                return Err(TransportError::InvalidPacket.into());
            }
        }
        validator.finish_eof()?;
        let result = self.encrypt(packets, timing);
        if result.is_err() { self.close(); }
        result.map(Some)
    }
    fn encrypt(&mut self, packets: &[TransportBytes], timing: Option<RouterTiming>) -> Result<Vec<OwnedWireFrames>, TlsError> {
        let mut output = self.drain_wire(timing)?;
        for packet in packets {
            let raw = packet.expose_to_transport();
            let count = self.connection.as_mut().ok_or(TlsError::Closed)?.writer().write(raw).map_err(|_| TlsError::HandshakeFailed)?;
            if count != raw.len() { return Err(TlsError::HandshakeFailed); }
            output.extend(self.drain_wire(timing)?);
            if output.len() > super::packet_stream::MAX_PACKETS_PER_INPUT { return Err(TlsError::TooLarge); }
        }
        Ok(output)
    }
    pub fn finish_eof(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Result<bool, TlsError> {
        if !self.scope.is_open() { self.close(); return Ok(false); }
        if !self.accepts(owner, scope) { return Ok(false); }
        let partial = self.transport.has_partial() || !self.records.buffer.is_empty() || self.plaintext.has_partial();
        self.close();
        // Even a complete TCP EOF without verified close_notify is an unclean
        // TLS closure; a partial stream is additionally reported as truncation.
        if partial { Err(TransportError::Truncated.into()) } else { Err(TlsError::Closed) }
    }
}
impl Drop for TlsEngine { fn drop(&mut self) { self.close(); } }

fn valid_authority(authority: &str) -> bool {
    if authority.is_empty() || authority.len() > 253 || authority.bytes().any(|b| b <= 32 || b >= 127) { return false; }
    if authority.parse::<std::net::IpAddr>().is_ok() { return true; }
    if authority.ends_with('.') { return false; }
    authority.split('.').all(|label| !label.is_empty() && label.len() <= 63
        && label.as_bytes()[0].is_ascii_alphanumeric() && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
        && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

#[cfg(test)]
#[path = "tls_engine_tests.rs"]
mod tests;
