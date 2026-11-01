//! Default non-resumable NetStream record security and Channel escape.
//! E0A75C/E09D14, DEA640/DEAA7C and D775D0/D96640. Legacy seeded XOR is
//! interoperability framing, NOT authentication or a replacement for endpoint
//! TLS. Only complete, ordered Channel records may enter the receive codec.
use super::{packet_stream::MAX_NETWORK_PACKET_BYTES, transport_packets::{TransportBytes, TransportError}};
use std::fmt;
use zeroize::Zeroize;

pub(super) const MAX_PLAIN_RECORD: usize = 128 * 1024; // project bound, below native DATA_MAX_SIZE
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecordSecurity { Plain, SeededXor }

#[derive(Clone)]
struct LegacyState { counter: u32, key: u32, state: u32, block: [u8; 16] }
impl LegacyState {
    fn new(seed: u32) -> Self { Self { counter: 0, key: seed, state: seed, block: [0; 16] } }
    fn apply(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            if self.counter & 15 == 0 {
                let a = self.state.wrapping_add(843686452);
                let b = a.wrapping_mul(self.key);
                self.key = self.key.wrapping_add(278017159);
                let c = b ^ b.wrapping_shl(16);
                self.state = c ^ self.key.wrapping_add(self.counter >> 4);
                self.block[..4].copy_from_slice(&a.to_be_bytes());
                self.block[4..8].copy_from_slice(&b.to_be_bytes());
                self.block[8..12].copy_from_slice(&c.to_be_bytes());
                self.block[12..].copy_from_slice(&self.state.to_be_bytes());
            }
            *byte ^= self.block[(self.counter & 15) as usize];
            self.counter = self.counter.wrapping_add(1);
        }
    }
}
impl Drop for LegacyState {
    fn drop(&mut self) { self.counter.zeroize(); self.key.zeroize(); self.state.zeroize(); self.block.zeroize(); }
}

/// A sender is one direction of ONE original NetStream. No reset/resume API is
/// offered; retransmission reuses already encoded Channel bytes, never this
/// method on an old plaintext record. A caller must reserve queue capacity
/// before advancing the codec, or use a private clone transactionally.
#[derive(Clone)]
pub(super) struct RecordSender { mode: RecordSecurity, seed: Option<u32>, cipher: Option<LegacyState>, closed: bool }
impl fmt::Debug for RecordSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RecordSender(<redacted>)") }
}
impl RecordSender {
    pub fn new(mode: RecordSecurity, seed: Option<u32>) -> Result<Self, TransportError> {
        if (mode == RecordSecurity::SeededXor) != seed.is_some() { return Err(TransportError::InvalidPacket); }
        Ok(Self { mode, seed, cipher: None, closed: false })
    }
    pub fn close(&mut self) {
        if let Some(seed) = &mut self.seed { seed.zeroize(); } self.seed = None; self.cipher = None; self.closed = true;
    }
    pub fn encode(&mut self, plain: &[u8]) -> Result<TransportBytes, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        if plain.is_empty() { return Err(TransportError::InvalidPacket); }
        if plain.len() > MAX_PLAIN_RECORD { return Err(TransportError::TooLarge); }
        if self.mode == RecordSecurity::Plain { return Ok(TransportBytes::from_bytes(plain.to_vec())); }
        let first = self.cipher.is_none();
        let mut bytes = Vec::with_capacity(plain.len() + if first {4} else {0});
        bytes.push(plain[0]);
        if first {
            let seed = self.seed.ok_or(TransportError::Closed)?;
            bytes.extend_from_slice(&seed.to_be_bytes()); self.cipher = Some(LegacyState::new(seed));
        }
        let prefix = bytes.len(); bytes.extend_from_slice(&plain[1..]);
        self.cipher.as_mut().ok_or(TransportError::Closed)?.apply(&mut bytes[prefix..]);
        Ok(TransportBytes::from_bytes(bytes))
    }
}
impl Drop for RecordSender { fn drop(&mut self) { self.close(); } }

pub(super) struct RecordReceiver { mode: RecordSecurity, cipher: Option<LegacyState>, closed: bool }
impl fmt::Debug for RecordReceiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RecordReceiver(<redacted>)") }
}
impl RecordReceiver {
    pub fn new(mode: RecordSecurity) -> Self { Self { mode, cipher: None, closed: false } }
    pub fn close(&mut self) { self.cipher = None; self.closed = true; }
    pub fn decode(&mut self, record: &[u8]) -> Result<TransportBytes, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.decode_inner(record); if result.is_err() { self.close(); } result
    }
    fn decode_inner(&mut self, record: &[u8]) -> Result<TransportBytes, TransportError> {
        if record.is_empty() { return Err(TransportError::Truncated); }
        if self.mode == RecordSecurity::Plain {
            if record.len() > MAX_PLAIN_RECORD { return Err(TransportError::TooLarge); }
            return Ok(TransportBytes::from_bytes(record.to_vec()));
        }
        let first = self.cipher.is_none(); let overhead = if first {4} else {0};
        if record.len() <= overhead { return Err(TransportError::Truncated); }
        if record.len() - overhead > MAX_PLAIN_RECORD { return Err(TransportError::TooLarge); }
        if first {
            let seed = u32::from_be_bytes(record[1..5].try_into().map_err(|_| TransportError::Truncated)?);
            self.cipher = Some(LegacyState::new(seed));
        }
        let mut plain = Vec::with_capacity(record.len() - overhead); plain.push(record[0]);
        plain.extend_from_slice(&record[1 + overhead..]);
        self.cipher.as_mut().ok_or(TransportError::Closed)?.apply(&mut plain[1..]);
        Ok(TransportBytes::from_bytes(plain))
    }
}
impl Drop for RecordReceiver { fn drop(&mut self) { self.close(); } }

/// Channel data uses FFFF; an initial FE/FF gets an extra FE. This is NOT the
/// different Common mode3 FFFF06 inner-TLS carrier selected by old direct IO.
pub(super) fn escape_channel(bytes: &[u8]) -> Result<TransportBytes, TransportError> {
    let first = *bytes.first().ok_or(TransportError::Truncated)?;
    let extra = usize::from(first >= 0xfe);
    if bytes.len() > MAX_NETWORK_PACKET_BYTES - 2 - extra { return Err(TransportError::TooLarge); }
    let mut wire = Vec::with_capacity(bytes.len() + 2 + extra); wire.extend_from_slice(&[0xff, 0xff]);
    if extra != 0 { wire.push(0xfe); } wire.extend_from_slice(bytes);
    Ok(TransportBytes::from_bytes(wire))
}
pub(super) fn unescape_channel(wire: &[u8]) -> Result<&[u8], TransportError> {
    if wire.len() > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
    if wire.len() < 3 { return Err(TransportError::Truncated); }
    if wire[..2] != [0xff, 0xff] { return Err(TransportError::UnsupportedKind); }
    match wire[2] {
        0xfe => {
            let body = &wire[3..];
            if body.is_empty() { return Err(TransportError::Truncated); }
            if body[0] < 0xfe { return Err(TransportError::InvalidPacket); }
            Ok(body)
        }
        // Native FFFFFF is a separate raw-Common escape. Do not accidentally
        // admit it as a new Channel or bypass configured transport mode.
        0xff => Err(TransportError::UnsupportedKind),
        _ => Ok(&wire[2..]),
    }
}

#[cfg(test)]
#[path = "netstream_records_tests.rs"]
mod tests;
