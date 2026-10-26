//! Native TCP-Mpath packet DATA, separate from Common RPC/TLS and raw ARC.
//! 659D60/659D94 pack/unpack a BE32 kind/path/seq14/tick16>>2 header.
//! 664188 prepends the channel byte only to the first/whole payload, and
//! 664620 deduplicates and joins adjacent seq14 halves in either order.
//! These bytes alone grant neither a socket, call acceptance nor audio-ready.
use super::transport_packets::{TransportBytes, TransportError};
use std::{collections::BTreeMap, fmt, time::{Duration, Instant}};
use zeroize::{Zeroize, Zeroizing};

pub const MAX_MPATH_PAYLOAD: usize = 2918;
pub const MPATH_SINGLE_THRESHOLD: usize = 1459;
const MAX_HALVES: usize = 64;
const HALF_RETENTION: Duration = Duration::from_secs(2); // local resource policy
const MASK: u16 = 0x3fff;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part { Whole, First, Last, Control }
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Header { pub part: Part, pub path: u8, pub sequence: u16, pub tick: u16 }
impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("MpathHeader(<redacted>)") }
}
fn encode_header(header: Header) -> Result<[u8; 4], TransportError> {
    if header.path > 3 || header.sequence > MASK { return Err(TransportError::InvalidPacket); }
    let kind = match header.part { Part::Whole => 0u32, Part::First => 1, Part::Last => 2, Part::Control => 3 };
    Ok(((kind << 30) | ((header.path as u32) << 28) | ((header.sequence as u32) << 14)
        | ((header.tick as u32) >> 2)).to_be_bytes())
}
pub fn decode_header(bytes: &[u8]) -> Result<(Header, &[u8]), TransportError> {
    let prefix: [u8; 4] = bytes.get(..4).ok_or(TransportError::Truncated)?.try_into().map_err(|_| TransportError::Truncated)?;
    let word = u32::from_be_bytes(prefix);
    let part = match word >> 30 { 0 => Part::Whole, 1 => Part::First, 2 => Part::Last, _ => Part::Control };
    Ok((Header { part, path: ((word >> 28) & 3) as u8, sequence: ((word >> 14) & 0x3fff) as u16,
        tick: ((word & 0x3fff) << 2) as u16 }, &bytes[4..]))
}
fn packet(header: Header, body: &[u8]) -> Result<TransportBytes, TransportError> {
    let mut bytes = Vec::with_capacity(body.len() + 4);
    bytes.extend_from_slice(&encode_header(header)?); bytes.extend_from_slice(body);
    Ok(TransportBytes::from_bytes(bytes))
}
/// Channel zero is audio, but the byte is a channel selector, not an RTP/SRTP
/// decoder. The native 1..2918 byte policy is distinct from a UDP MTU.
/// Caller retains the original per-path sequence state; wrap is modulo 16384.
pub fn encode_payload(path: u8, sequence: u16, tick: u16, channel: u8, payload: &[u8])
    -> Result<(Vec<TransportBytes>, u16), TransportError> {
    if payload.is_empty() || payload.len() > MAX_MPATH_PAYLOAD { return Err(TransportError::TooLarge); }
    let mut first = Zeroizing::new(Vec::with_capacity(payload.len().min(MPATH_SINGLE_THRESHOLD) + 1));
    first.push(channel);
    if payload.len() <= MPATH_SINGLE_THRESHOLD {
        first.extend_from_slice(payload);
        Ok((vec![packet(Header { part: Part::Whole, path, sequence, tick }, &first)?], sequence.wrapping_add(1) & MASK))
    } else {
        let half = payload.len() / 2;
        first.extend_from_slice(&payload[..half]);
        let second = sequence.wrapping_add(1) & MASK;
        let packets = vec![packet(Header { part: Part::First, path, sequence, tick }, &first)?,
            packet(Header { part: Part::Last, path, sequence: second, tick }, &payload[half..])?];
        first.zeroize();
        Ok((packets, second.wrapping_add(1) & MASK))
    }
}
fn signed_delta(sequence: u16, base: u16) -> i16 { ((sequence.wrapping_sub(base) & MASK) << 2) as i16 >> 2 }
struct Seen { words: [u32; 64], head: usize, base: Option<u16> }
impl Default for Seen { fn default() -> Self { Self { words: [0; 64], head: 0, base: None } } }
impl Seen {
    fn observe(&mut self, sequence: u16) -> (bool, bool) {
        let mut reset = false;
        let mut distance = self.base.map(|base| signed_delta(sequence, base));
        if distance.is_none_or(|delta| !(0..=2047).contains(&delta)) {
            self.words.fill(0); self.head = 0; self.base = Some(sequence.wrapping_sub(512) & MASK);
            distance = Some(512); reset = true;
        }
        let mut distance = distance.unwrap_or(512) as usize;
        while distance > 1023 {
            self.words[self.head] = 0; self.head = (self.head + 1) & 63;
            self.base = self.base.map(|base| base.wrapping_add(32) & MASK); distance -= 32;
        }
        let word = (self.head + distance / 32) & 63; let bit = 1u32 << (distance & 31);
        let duplicate = self.words[word] & bit != 0; self.words[word] |= bit;
        (duplicate, reset)
    }
}
struct Half { header: Header, body: Vec<u8>, received: Instant }
impl Drop for Half { fn drop(&mut self) { self.body.zeroize(); } }
pub enum Input {
    Payload { header: Header, channel: u8, payload: TransportBytes },
    /// Native sync/metrics data is not an RPC result or an audio packet.
    Control { header: Header, body: TransportBytes },
}
impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("MpathInput(<redacted>)") }
}
pub struct Receiver { seen: [Seen; 4], halves: BTreeMap<(u8, u16), Half>, clock: Option<Instant>, closed: bool }
impl Default for Receiver {
    fn default() -> Self { Self { seen: std::array::from_fn(|_| Seen::default()), halves: BTreeMap::new(), clock: None, closed: false } }
}
impl fmt::Debug for Receiver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("MpathReceiver(<redacted>)") }
}
impl Receiver {
    pub fn close(&mut self) { self.halves.clear(); self.closed = true; }
    pub fn expire(&mut self, now: Instant) -> Result<(), TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        if self.clock.is_some_and(|old| now < old) { self.close(); return Err(TransportError::InvalidPacket); }
        self.clock = Some(now); self.halves.retain(|_, half| now.duration_since(half.received) < HALF_RETENTION);
        Ok(())
    }
    pub fn on_packet(&mut self, bytes: &[u8], now: Instant) -> Result<Option<Input>, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(bytes, now); if result.is_err() { self.close(); } result
    }
    fn consume(&mut self, bytes: &[u8], now: Instant) -> Result<Option<Input>, TransportError> {
        self.expire(now)?;
        if bytes.len() > MAX_MPATH_PAYLOAD + 5 { return Err(TransportError::TooLarge); }
        let (header, body) = decode_header(bytes)?;
        if body.is_empty() { return Err(TransportError::Truncated); }
        let (duplicate, reset) = self.seen[header.path as usize].observe(header.sequence);
        if reset { self.halves.retain(|(path, _), _| *path != header.path); }
        if duplicate { return Ok(None); }
        if header.part == Part::Control {
            return Ok(Some(Input::Control { header, body: TransportBytes::from_bytes(body.to_vec()) }));
        }
        if header.part == Part::Whole { return payload(header, body).map(Some); }
        let peer = match header.part { Part::First => header.sequence.wrapping_add(1) & MASK, _ => header.sequence.wrapping_sub(1) & MASK };
        if let Some(other) = self.halves.remove(&(header.path, peer)) {
            if !matches!((header.part, other.header.part), (Part::First, Part::Last) | (Part::Last, Part::First)) {
                return Err(TransportError::InvalidFragment);
            }
            if body.len() + other.body.len() > MAX_MPATH_PAYLOAD + 1 { return Err(TransportError::TooLarge); }
            let mut joined = Vec::with_capacity(body.len() + other.body.len());
            let first = if header.part == Part::First {
                joined.extend_from_slice(body); joined.extend_from_slice(&other.body); header
            } else { joined.extend_from_slice(&other.body); joined.extend_from_slice(body); other.header };
            let result = payload(first, &joined); joined.zeroize(); return result.map(Some);
        }
        if self.halves.len() >= MAX_HALVES { return Err(TransportError::TooLarge); }
        self.halves.insert((header.path, header.sequence), Half { header, body: body.to_vec(), received: now });
        Ok(None)
    }
}
fn payload(header: Header, body: &[u8]) -> Result<Input, TransportError> {
    let (&channel, payload) = body.split_first().ok_or(TransportError::Truncated)?;
    if payload.is_empty() { return Err(TransportError::Truncated); }
    Ok(Input::Payload { header, channel, payload: TransportBytes::from_bytes(payload.to_vec()) })
}
impl Drop for Receiver { fn drop(&mut self) { self.close(); } }
#[cfg(test)]
#[path = "tcp_mpath_tests.rs"]
mod tests;
