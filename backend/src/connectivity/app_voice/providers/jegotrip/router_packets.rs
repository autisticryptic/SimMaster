//! Bounded subset of the recovered Router packet/header and routing format.
//!
//! Router sequence/timing fields and addresses are opaque transport data, not
//! ARC call IDs, credentials, endpoints, or proof of hosting. Only packet kinds
//! 0/1, normal part1 and one ordered part2+3 pair are supported. Native controls,
//! out-of-order windows, legacy address decorations and extensions are rejected.
use super::{packet_stream::MAX_NETWORK_PACKET_BYTES, transport_packets::{TransportBytes, TransportError}};
use std::fmt;

const HEADER_BYTES: usize = 8;
const ADDRESS_BYTES: [usize; 8] = [2, 6, 8, 12, 8, 12, 14, 18];
pub const MAX_ROUTED_BODY_BYTES: usize = MAX_NETWORK_PACKET_BYTES - HEADER_BYTES;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RouterHeader {
    pub sequence: u16,
    pub word1: u16,
    pub word2: u16,
    pub byte6: u8,
    pub packet_kind: u8,
    pub fragment_part: u8,
}

impl fmt::Debug for RouterHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterHeader(<redacted>)") }
}

fn validate_header(header: &RouterHeader) -> Result<(), TransportError> {
    if header.packet_kind > 1 || !(1..=3).contains(&header.fragment_part) {
        return Err(TransportError::UnsupportedKind);
    }
    Ok(())
}

pub fn encode_header(header: RouterHeader) -> Result<[u8; HEADER_BYTES], TransportError> {
    validate_header(&header)?;
    let mut bytes = [0; HEADER_BYTES];
    bytes[..2].copy_from_slice(&header.sequence.to_be_bytes());
    bytes[2..4].copy_from_slice(&header.word1.to_be_bytes());
    bytes[4..6].copy_from_slice(&header.word2.to_be_bytes());
    bytes[6] = header.byte6;
    bytes[7] = header.packet_kind | (header.fragment_part << 4);
    Ok(bytes)
}

pub fn decode_header(packet: &[u8]) -> Result<(RouterHeader, &[u8]), TransportError> {
    if packet.len() > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
    let bytes = packet.get(..HEADER_BYTES).ok_or(TransportError::Truncated)?;
    if bytes[7] & 0x88 != 0 { return Err(TransportError::UnsupportedKind); }
    let header = RouterHeader { sequence: u16::from_be_bytes([bytes[0], bytes[1]]),
        word1: u16::from_be_bytes([bytes[2], bytes[3]]), word2: u16::from_be_bytes([bytes[4], bytes[5]]),
        byte6: bytes[6], packet_kind: bytes[7] & 7, fragment_part: (bytes[7] >> 4) & 7 };
    validate_header(&header)?;
    Ok((header, &packet[HEADER_BYTES..]))
}

#[derive(Clone, Copy)]
pub struct RouterAddress<'a> { pub mask: u8, pub label: u8, pub bytes: &'a [u8] }

impl fmt::Debug for RouterAddress<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterAddress(<redacted>)") }
}

pub struct RouterRouting<'a> {
    pub tag: u32,
    pub flags: u8,
    pub path_words: &'a [u16],
    pub address1: RouterAddress<'a>,
    pub address2: RouterAddress<'a>,
}

impl fmt::Debug for RouterRouting<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterRouting(<redacted>)") }
}

pub struct RoutedBody<'a> {
    pub tag: u32,
    pub flags: u8,
    pub path_words: Vec<u16>,
    pub address1: RouterAddress<'a>,
    pub address2: RouterAddress<'a>,
    pub payload: &'a [u8],
}

impl fmt::Debug for RoutedBody<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RoutedBody(<redacted>)") }
}

fn address_length(mask: u8) -> Result<usize, TransportError> {
    ADDRESS_BYTES.get(mask as usize).copied().ok_or(TransportError::UnsupportedRoute)
}

fn validate_route(flags: u8, address1_mask: u8) -> Result<(), TransportError> {
    // Bit1 of address1 requires a legacy four-byte IPv4 decoration or the
    // native extension format. Neither is silently removed by this subset.
    if flags & !3 != 0 || address1_mask & 2 != 0 { return Err(TransportError::UnsupportedRoute); }
    Ok(())
}

pub fn encode_routed_body(routing: &RouterRouting<'_>, payload: &[u8]) -> Result<TransportBytes, TransportError> {
    validate_route(routing.flags, routing.address1.mask)?;
    if routing.path_words.len() > 3 || payload.is_empty() { return Err(TransportError::InvalidPacket); }
    let first = address_length(routing.address1.mask)?; let second = address_length(routing.address2.mask)?;
    if routing.address1.bytes.len() != first || routing.address2.bytes.len() != second {
        return Err(TransportError::InvalidPacket);
    }
    let prefix = 8 + 2 * routing.path_words.len() + first + second;
    if payload.len() > MAX_ROUTED_BODY_BYTES - prefix { return Err(TransportError::TooLarge); }
    let mut bytes = Vec::with_capacity(prefix + payload.len());
    bytes.extend_from_slice(&routing.tag.to_be_bytes()); bytes.push(routing.flags);
    bytes.push(routing.path_words.len() as u8 | (routing.address1.mask << 2) | (routing.address2.mask << 5));
    bytes.push(routing.address1.label); bytes.push(routing.address2.label);
    for word in routing.path_words { bytes.extend_from_slice(&word.to_be_bytes()); }
    bytes.extend_from_slice(routing.address1.bytes); bytes.extend_from_slice(routing.address2.bytes);
    bytes.extend_from_slice(payload);
    Ok(TransportBytes::from_bytes(bytes))
}

pub fn decode_routed_body(bytes: &[u8]) -> Result<RoutedBody<'_>, TransportError> {
    if bytes.len() > MAX_ROUTED_BODY_BYTES { return Err(TransportError::TooLarge); }
    let header = bytes.get(..8).ok_or(TransportError::Truncated)?;
    let first_mask = (header[5] >> 2) & 7; let second_mask = header[5] >> 5;
    validate_route(header[4], first_mask)?;
    let count = (header[5] & 3) as usize;
    let first_length = address_length(first_mask)?; let second_length = address_length(second_mask)?;
    let end_paths = 8 + 2 * count; let end_first = end_paths + first_length; let end_second = end_first + second_length;
    if bytes.len() <= end_second { return Err(TransportError::Truncated); }
    let path_words = bytes[8..end_paths].chunks_exact(2).map(|word| u16::from_be_bytes([word[0], word[1]])).collect();
    Ok(RoutedBody { tag: u32::from_be_bytes([header[0], header[1], header[2], header[3]]), flags: header[4], path_words,
        address1: RouterAddress { mask: first_mask, label: header[6], bytes: &bytes[end_paths..end_first] },
        address2: RouterAddress { mask: second_mask, label: header[7], bytes: &bytes[end_first..end_second] },
        payload: &bytes[end_second..] })
}

fn packet(header: RouterHeader, body: &[u8]) -> Result<TransportBytes, TransportError> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES + body.len());
    bytes.extend_from_slice(&encode_header(header)?); bytes.extend_from_slice(body);
    Ok(TransportBytes::from_bytes(bytes))
}

/// Native uses a strict body+8 < MTU check; otherwise it sends balanced halves.
/// This local subset refuses bodies needing more than two MTU-sized packets and
/// sequence wrap. The caller supplies an actual route MTU, never a guessed one.
pub fn encode_router_packets(header: RouterHeader, body: &[u8], mtu: usize) -> Result<Vec<TransportBytes>, TransportError> {
    validate_header(&header)?;
    if header.fragment_part != 1 || body.is_empty() || !(16..=MAX_NETWORK_PACKET_BYTES).contains(&mtu) {
        return Err(TransportError::InvalidPacket);
    }
    if body.len() > MAX_ROUTED_BODY_BYTES { return Err(TransportError::TooLarge); }
    if body.len() + HEADER_BYTES < mtu { return Ok(vec![packet(header, body)?]); }
    let half = body.len() / 2;
    if half == 0 || body.len() - half + HEADER_BYTES > mtu { return Err(TransportError::TooLarge); }
    let next = header.sequence.checked_add(1).ok_or(TransportError::InvalidFragment)?;
    Ok(vec![packet(RouterHeader { fragment_part: 2, ..header }, &body[..half])?,
        packet(RouterHeader { sequence: next, fragment_part: 3, ..header }, &body[half..])?])
}

pub struct RouterMessage {
    pub first_header: RouterHeader,
    pub continuation_header: Option<RouterHeader>,
    pub routing_and_payload: TransportBytes,
}

impl fmt::Debug for RouterMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterMessage(<redacted>)") }
}

struct PartialRouter { header: RouterHeader, body: Vec<u8> }

/// Ordered two-half safety subset, not the native out-of-order replay window.
#[derive(Default)]
pub struct RouterPacketAssembler { partial: Option<PartialRouter>, closed: bool }

impl fmt::Debug for RouterPacketAssembler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouterPacketAssembler").field("closed", &self.closed)
            .field("partial", &self.partial.is_some()).field("data", &"<redacted>").finish()
    }
}

impl RouterPacketAssembler {
    pub fn close(&mut self) { self.partial = None; self.closed = true; }
    pub fn on_packet(&mut self, bytes: &[u8]) -> Result<Option<RouterMessage>, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(bytes);
        if result.is_err() { self.close(); }
        result
    }
    fn consume(&mut self, bytes: &[u8]) -> Result<Option<RouterMessage>, TransportError> {
        let (header, body) = decode_header(bytes)?;
        if body.is_empty() { return Err(TransportError::Truncated); }
        match header.fragment_part {
            1 if self.partial.is_none() => Ok(Some(RouterMessage { first_header: header, continuation_header: None,
                routing_and_payload: TransportBytes::from_bytes(body.to_vec()) })),
            2 if self.partial.is_none() && header.sequence != u16::MAX => {
                self.partial = Some(PartialRouter { header, body: body.to_vec() }); Ok(None)
            }
            3 => {
                let original = self.partial.as_ref().ok_or(TransportError::InvalidFragment)?;
                if original.header.sequence.checked_add(1) != Some(header.sequence)
                    || original.header.packet_kind != header.packet_kind { return Err(TransportError::InvalidFragment); }
                if body.len() > MAX_ROUTED_BODY_BYTES - original.body.len() { return Err(TransportError::TooLarge); }
                let mut original = self.partial.take().ok_or(TransportError::InvalidFragment)?;
                original.body.extend_from_slice(body);
                Ok(Some(RouterMessage { first_header: original.header, continuation_header: Some(header),
                    routing_and_payload: TransportBytes::from_bytes(original.body) }))
            }
            _ => Err(TransportError::InvalidFragment),
        }
    }
    pub fn finish_eof(&mut self) -> Result<(), TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let incomplete = self.partial.is_some(); self.close();
        if incomplete { Err(TransportError::Truncated) } else { Ok(()) }
    }
}

#[cfg(test)]
#[path = "router_packets_tests.rs"]
mod tests;
