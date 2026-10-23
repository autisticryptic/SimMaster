//! Recovered Common call fragments and FF-FF/DataKeyTls carrier packets.
//!
//! APIs take ONE complete lower-layer packet, not arbitrary TCP reads. The
//! opaque TLS bytes go to an authenticated TLS engine; only that engine's
//! verified plaintext call-packet boundary goes to CallAssembler. No socket,
//! key exchange, certificate policy, legacy XOR or provider is enabled here.
use super::rpc_stream::MAX_STREAM_BYTES;
use std::fmt;
use zeroize::{Zeroize, Zeroizing};

/// Native mode-3 fragment threshold, not a deployment endpoint or secret.
pub const TLS_FRAGMENT_THRESHOLD: usize = 16_382;
/// Local bound for bytes delivered by the lower-layer packet decoder.
pub const MAX_CARRIER_BYTES: usize = MAX_STREAM_BYTES + 3;
const FRAGMENTED: u8 = 0x20;
const COMPRESSED: u8 = 0x10;
const CONTINUATION: u8 = 3;
const DATA_KEY_TLS: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    TooLarge,
    Truncated,
    InvalidPacket,
    InvalidFragment,
    UnsupportedKind,
    UnsupportedCompression,
    UnsupportedRoute,
    Closed,
}

impl TransportError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "jegotrip_transport_too_large",
            Self::Truncated => "jegotrip_transport_truncated",
            Self::InvalidPacket => "jegotrip_transport_packet_invalid",
            Self::InvalidFragment => "jegotrip_transport_fragment_invalid",
            Self::UnsupportedKind => "jegotrip_transport_kind_unsupported",
            Self::UnsupportedCompression => "jegotrip_transport_compression_unsupported",
            Self::UnsupportedRoute => "jegotrip_transport_route_unsupported",
            Self::Closed => "jegotrip_transport_closed",
        }
    }
}

pub struct TransportBytes(Vec<u8>);

impl TransportBytes {
    pub(super) fn from_bytes(bytes: Vec<u8>) -> Self { Self(bytes) }
    pub fn expose_to_transport(&self) -> &[u8] { &self.0 }
}

impl Drop for TransportBytes { fn drop(&mut self) { self.0.zeroize(); } }

/// Grow a retained byte buffer without freeing an unwiped old allocation.
/// The new capacity is geometric and bounded, avoiding tiny-fragment O(n²)
/// growth. This cannot revoke caller-owned/previously borrowed byte copies.
pub(super) fn extend_wiping(buffer: &mut Vec<u8>, input: &[u8], limit: usize) -> Result<(), TransportError> {
    if input.len() > limit.saturating_sub(buffer.len()) || buffer.len() > limit { return Err(TransportError::TooLarge); }
    let needed = buffer.len() + input.len();
    if needed > buffer.capacity() {
        let capacity = needed.checked_next_power_of_two().unwrap_or(limit).min(limit);
        let mut replacement = Vec::with_capacity(capacity);
        replacement.extend_from_slice(buffer);
        buffer.zeroize(); *buffer = replacement;
    }
    buffer.extend_from_slice(input); Ok(())
}

impl fmt::Debug for TransportBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportBytes").field("byte_length", &self.0.len())
            .field("data", &"<redacted>").finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CallKind { Request = 0, Reply = 1, Ping = 2, Release = 4, Pong = 14 }

impl CallKind {
    fn from_byte(value: u8) -> Result<Self, TransportError> {
        match value {
            0 => Ok(Self::Request), 1 => Ok(Self::Reply), 2 => Ok(Self::Ping),
            4 => Ok(Self::Release), 14 => Ok(Self::Pong),
            _ => Err(TransportError::UnsupportedKind),
        }
    }

    fn validate_payload(self, payload: &[u8]) -> Result<(), TransportError> {
        match self {
            Self::Request | Self::Reply if payload.is_empty() => Err(TransportError::InvalidPacket),
            Self::Ping | Self::Pong | Self::Release if !payload.is_empty() => Err(TransportError::InvalidPacket),
            _ => Ok(()),
        }
    }
}

pub struct CallMessage { pub kind: CallKind, pub payload: TransportBytes }

impl fmt::Debug for CallMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallMessage").field("kind", &self.kind).field("payload", &self.payload).finish()
    }
}

/// Native writes the ORIGINAL payload length, not payload+4, into the first
/// fragment's BE32 prefix. It balances the last two chunks when remainder<2T.
/// T>=8 and the total 128KiB bound are local safety policies, not negotiation.
pub fn encode_call_packets(kind: CallKind, payload: &[u8], threshold: usize)
    -> Result<Vec<TransportBytes>, TransportError> {
    if payload.len() > MAX_STREAM_BYTES { return Err(TransportError::TooLarge); }
    if !(8..=TLS_FRAGMENT_THRESHOLD).contains(&threshold) { return Err(TransportError::InvalidPacket); }
    kind.validate_payload(payload)?;
    if payload.len() <= threshold {
        let mut bytes = Vec::with_capacity(payload.len() + 1);
        bytes.push(kind as u8);
        bytes.extend_from_slice(payload);
        return Ok(vec![TransportBytes(bytes)]);
    }
    let mut full = Zeroizing::new(Vec::with_capacity(payload.len() + 4));
    full.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    full.extend_from_slice(payload);
    let mut remaining = full.as_slice();
    let mut first = true;
    let mut packets = Vec::new();
    while !remaining.is_empty() {
        let length = if remaining.len() >= 2 * threshold { threshold }
            else if remaining.len() > threshold { remaining.len() / 2 }
            else { remaining.len() };
        let (chunk, rest) = remaining.split_at(length);
        let mut bytes = Vec::with_capacity(length + 1);
        bytes.push(if first { kind as u8 | FRAGMENTED } else { CONTINUATION });
        bytes.extend_from_slice(chunk);
        packets.push(TransportBytes(bytes));
        remaining = rest;
        first = false;
    }
    Ok(packets)
}

/// Wrap already-produced TLS bytes without interpreting them as RPC or success.
/// This has no TCP length prefix: lower-layer network framing is still separate.
pub fn encode_tls_carrier(tls_bytes: &[u8]) -> Result<TransportBytes, TransportError> {
    if tls_bytes.is_empty() { return Err(TransportError::InvalidPacket); }
    if tls_bytes.len() > MAX_STREAM_BYTES { return Err(TransportError::TooLarge); }
    let mut bytes = Vec::with_capacity(tls_bytes.len() + 3);
    bytes.extend_from_slice(&[0xff, 0xff, DATA_KEY_TLS]);
    bytes.extend_from_slice(tls_bytes);
    Ok(TransportBytes(bytes))
}

/// Mode3 accepts exact FF FF 06. FF FF FF is the escaped NetStream route, not
/// TLS, and neither that route nor plaintext/Kx may downgrade this codec.
pub fn decode_tls_carrier(packet: &[u8]) -> Result<&[u8], TransportError> {
    if packet.len() > MAX_CARRIER_BYTES { return Err(TransportError::TooLarge); }
    if packet.len() < 3 { return Err(TransportError::Truncated); }
    if packet[..3] != [0xff, 0xff, DATA_KEY_TLS] { return Err(TransportError::InvalidPacket); }
    if packet.len() == 3 { return Err(TransportError::Truncated); }
    Ok(&packet[3..])
}

struct PartialMessage { kind: CallKind, expected: usize, bytes: Vec<u8> }
impl Drop for PartialMessage { fn drop(&mut self) { self.bytes.zeroize(); } }

/// Connection-local fragment state. Malformed input poisons the instance;
/// callers must create a new connection, never reset/replay on the old scope.
#[derive(Default)]
pub struct CallAssembler { partial: Option<PartialMessage>, closed: bool }

impl fmt::Debug for CallAssembler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallAssembler").field("closed", &self.closed)
            .field("partial", &self.partial.is_some()).finish()
    }
}

impl CallAssembler {
    pub fn close(&mut self) { self.partial = None; self.closed = true; }

    pub fn on_packet(&mut self, packet: &[u8]) -> Result<Option<CallMessage>, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(packet);
        if result.is_err() { self.close(); }
        result
    }

    fn consume(&mut self, packet: &[u8]) -> Result<Option<CallMessage>, TransportError> {
        if packet.len() > MAX_STREAM_BYTES + 1 { return Err(TransportError::TooLarge); }
        let (&header, body) = packet.split_first().ok_or(TransportError::Truncated)?;
        if header & 0xc0 != 0 { return Err(TransportError::UnsupportedKind); }
        if header & COMPRESSED != 0 { return Err(TransportError::UnsupportedCompression); }
        if header == CONTINUATION {
            let partial = self.partial.as_mut().ok_or(TransportError::InvalidFragment)?;
            if body.is_empty() || body.len() > partial.expected - partial.bytes.len() {
                return Err(TransportError::InvalidFragment);
            }
            partial.bytes.extend_from_slice(body);
            if partial.bytes.len() != partial.expected { return Ok(None); }
            let mut partial = self.partial.take().ok_or(TransportError::InvalidFragment)?;
            return Ok(Some(CallMessage { kind: partial.kind, payload: TransportBytes(std::mem::take(&mut partial.bytes)) }));
        }
        if self.partial.is_some() { return Err(TransportError::InvalidFragment); }
        let kind = CallKind::from_byte(header & 0x0f)?;
        if header & FRAGMENTED == 0 {
            kind.validate_payload(body)?;
            return Ok(Some(CallMessage { kind, payload: TransportBytes(body.to_vec()) }));
        }
        if !matches!(kind, CallKind::Request | CallKind::Reply) { return Err(TransportError::InvalidFragment); }
        let prefix = body.get(..4).ok_or(TransportError::Truncated)?;
        let expected = u32::from_be_bytes(prefix.try_into().map_err(|_| TransportError::Truncated)?) as usize;
        if expected > MAX_STREAM_BYTES { return Err(TransportError::TooLarge); }
        let initial = &body[4..];
        if expected == 0 || initial.is_empty() || initial.len() >= expected {
            return Err(TransportError::InvalidFragment);
        }
        let mut bytes = Vec::with_capacity(expected); bytes.extend_from_slice(initial);
        self.partial = Some(PartialMessage { kind, expected, bytes });
        Ok(None)
    }
}

#[cfg(test)]
#[path = "transport_packets_tests.rs"]
mod tests;
