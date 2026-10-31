//! Recovered Router routing-body codec (D614F8/D61524/D61634/D61AD8/D61FB8).
//!
//! This is separate from the intentionally strict router_packets subset. It
//! preserves mixed-width addresses, native IP decorations and known extensions.
//! Decoding never authenticates a Channel, grants a local port or authorizes an
//! OS endpoint. Implicit IDs may only be restored by the ORIGINAL Router path.
use super::{router_packets::MAX_ROUTED_BODY_BYTES, transport_packets::{TransportBytes, TransportError}};
use std::{fmt, net::{IpAddr, Ipv4Addr, Ipv6Addr}};

const ADDRESS_BYTES: [usize; 8] = [2, 6, 8, 12, 8, 12, 14, 18];
const MAX_EXTENSION_BYTES: usize = 64; // local policy; three known TLVs need at most 37
pub(super) const NO_ADDRESS_TAG: u16 = u16::MAX;

/// The two auxiliary u16 words are NOT ports or allocation generations. A
/// missing segment retains native defaults: ID zero, auxiliary word 0xffff.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct RouteAddress {
    pub mask: u8,
    pub label: u8,
    pub network: u16,
    pub network_tag: u16,
    pub node: u32,
    pub node_tag: u16,
    pub forward_network: u16,
    pub forward_node: u32,
    pub port: u16,
}
impl Default for RouteAddress {
    fn default() -> Self {
        Self { mask: 0, label: 0, network: 0, network_tag: NO_ADDRESS_TAG,
            node: 0, node_tag: NO_ADDRESS_TAG, forward_network: 0, forward_node: 0, port: 0 }
    }
}
impl fmt::Debug for RouteAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouteAddress(<redacted>)") }
}
impl RouteAddress {
    pub fn wire_len(&self) -> Result<usize, TransportError> {
        ADDRESS_BYTES.get(self.mask as usize).copied().ok_or(TransportError::UnsupportedRoute)
    }
    fn write_to(&self, out: &mut Vec<u8>) -> Result<(), TransportError> {
        self.wire_len()?;
        if self.mask & 1 != 0 {
            out.extend_from_slice(&self.network.to_be_bytes());
            out.extend_from_slice(&self.network_tag.to_be_bytes());
        }
        if self.mask & 2 != 0 {
            out.extend_from_slice(&self.node.to_be_bytes());
            out.extend_from_slice(&self.node_tag.to_be_bytes());
        }
        if self.mask & 4 != 0 {
            out.extend_from_slice(&self.forward_network.to_be_bytes());
            out.extend_from_slice(&self.forward_node.to_be_bytes());
        }
        out.extend_from_slice(&self.port.to_be_bytes());
        Ok(())
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Routing {
    pub tag: u32,
    pub priority: u8,
    pub path_words: Vec<u16>,
    pub source: RouteAddress,
    pub destination: RouteAddress,
    /// Wire metadata only. Never use it as a replacement TCP/TLS authority.
    pub source_ip: Option<IpAddr>,
    pub source_port: u16,
    /// None = unspecified; Some(empty) = native bit0x20 explicit clearing.
    pub relay_paths: Option<Vec<u16>>,
}
impl fmt::Debug for Routing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("Routing(<redacted>)") }
}
pub(super) struct Routed<'a> { pub routing: Routing, pub payload: &'a [u8] }
impl fmt::Debug for Routed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("Routed(<redacted>)") }
}

struct Cursor<'a> { rest: &'a [u8] }
impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], TransportError> {
        let result = self.rest.get(..count).ok_or(TransportError::Truncated)?;
        self.rest = &self.rest[count..];
        Ok(result)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], TransportError> {
        self.take(N)?.try_into().map_err(|_| TransportError::Truncated)
    }
    fn u16(&mut self) -> Result<u16, TransportError> { Ok(u16::from_be_bytes(self.array()?)) }
    fn u32(&mut self) -> Result<u32, TransportError> { Ok(u32::from_be_bytes(self.array()?)) }
}
fn read_address(mask: u8, label: u8, input: &mut Cursor<'_>) -> Result<RouteAddress, TransportError> {
    let mut address = RouteAddress { mask, label, ..RouteAddress::default() };
    address.wire_len()?;
    if mask & 1 != 0 { address.network = input.u16()?; address.network_tag = input.u16()?; }
    if mask & 2 != 0 { address.node = input.u32()?; address.node_tag = input.u16()?; }
    if mask & 4 != 0 { address.forward_network = input.u16()?; address.forward_node = input.u32()?; }
    address.port = input.u16()?;
    Ok(address)
}

/// Canonical native writer order is header, path words, both addresses, legacy
/// IPv4 (when required), BE16 extension length, TLVs, then payload. The native
/// writer PREPENDS TLVs: IPv6 precedes port, which precedes relay-path words.
/// An empty body is syntactically valid; port/control/Channel admission is later.
pub(super) fn encode_routing_body(route: &Routing, payload: &[u8]) -> Result<TransportBytes, TransportError> {
    if route.priority > 3 || route.path_words.len() > 3 { return Err(TransportError::InvalidPacket); }
    let first = route.source.wire_len()?;
    let second = route.destination.wire_len()?;
    if route.source.mask & 2 == 0 && (route.source_ip.is_some() || route.source_port != 0) {
        return Err(TransportError::UnsupportedRoute);
    }
    if route.relay_paths.as_ref().is_some_and(|paths| paths.len() > 8) { return Err(TransportError::TooLarge); }
    if route.relay_paths.is_some() && route.source.network_tag == NO_ADDRESS_TAG {
        // Native emits these only with a known auxiliary network word. Do not
        // silently discard an explicitly supplied update on an implicit route.
        return Err(TransportError::UnsupportedRoute);
    }
    let mut flags = route.priority;
    let mut extension = Vec::with_capacity(37);
    if let Some(IpAddr::V6(ip)) = route.source_ip {
        flags |= 0x40;
        extension.push(0x1f);
        extension.extend_from_slice(&ip.octets());
    }
    if route.source_port != 0 {
        extension.push(0x21);
        extension.extend_from_slice(&route.source_port.to_be_bytes());
    }
    if let Some(paths) = &route.relay_paths {
        if paths.is_empty() { flags |= 0x20; }
        else {
            extension.push(0x30 | (paths.len() as u8 * 2 - 1));
            for word in paths { extension.extend_from_slice(&word.to_be_bytes()); }
        }
    }
    let ipv4 = route.source.mask & 2 != 0 && flags & 0x40 == 0;
    let prefix = 8 + 2 * route.path_words.len() + first + second + usize::from(ipv4) * 4
        + if extension.is_empty() { 0 } else { 2 + extension.len() };
    if prefix > MAX_ROUTED_BODY_BYTES || payload.len() > MAX_ROUTED_BODY_BYTES - prefix {
        return Err(TransportError::TooLarge);
    }
    if !extension.is_empty() { flags |= 0x80; }
    let mut out = Vec::with_capacity(prefix + payload.len());
    out.extend_from_slice(&route.tag.to_be_bytes());
    out.push(flags);
    out.push(route.path_words.len() as u8 | route.source.mask << 2 | route.destination.mask << 5);
    out.push(route.source.label); out.push(route.destination.label);
    for word in &route.path_words { out.extend_from_slice(&word.to_be_bytes()); }
    route.source.write_to(&mut out)?; route.destination.write_to(&mut out)?;
    if ipv4 {
        let ip = match route.source_ip { Some(IpAddr::V4(ip)) => ip.octets(), _ => [0; 4] };
        out.extend_from_slice(&ip);
    }
    if !extension.is_empty() {
        out.extend_from_slice(&(extension.len() as u16).to_be_bytes());
        out.extend_from_slice(&extension);
    }
    out.extend_from_slice(payload);
    Ok(TransportBytes::from_bytes(out))
}

pub(super) fn decode_routing_body(bytes: &[u8]) -> Result<Routed<'_>, TransportError> {
    if bytes.len() > MAX_ROUTED_BODY_BYTES { return Err(TransportError::TooLarge); }
    let mut input = Cursor { rest: bytes };
    let h = input.array::<8>()?;
    // Known native bits only. Reserved/ambiguous encodings are rejected instead
    // of being stripped and accidentally reinterpreted as Channel bytes.
    if h[4] & 0x1c != 0 { return Err(TransportError::UnsupportedRoute); }
    let count = (h[5] & 3) as usize;
    let mut path_words = Vec::with_capacity(count);
    for _ in 0..count { path_words.push(input.u16()?); }
    let source = read_address((h[5] >> 2) & 7, h[6], &mut input)?;
    let destination = read_address(h[5] >> 5, h[7], &mut input)?;
    let mut route = Routing { tag: u32::from_be_bytes(h[..4].try_into().map_err(|_| TransportError::Truncated)?),
        priority: h[4] & 3, path_words, source, destination, source_ip: None, source_port: 0,
        relay_paths: (h[4] & 0x20 != 0).then(Vec::new) };
    if h[4] & 0x40 == 0 && source.mask & 2 != 0 {
        route.source_ip = Some(IpAddr::V4(Ipv4Addr::from(input.array::<4>()?)));
    }
    let mut seen = 0u8;
    if h[4] & 0x80 != 0 {
        let len = input.u16()? as usize;
        if len > MAX_EXTENSION_BYTES { return Err(TransportError::TooLarge); }
        let mut extension = Cursor { rest: input.take(len)? };
        while !extension.rest.is_empty() {
            let tag = extension.array::<1>()?[0];
            let kind = tag >> 4;
            let value = extension.take(usize::from(tag & 15) + 1)?;
            if !(1..=3).contains(&kind) { return Err(TransportError::UnsupportedRoute); }
            let bit = 1u8 << kind;
            if seen & bit != 0 { return Err(TransportError::InvalidPacket); }
            seen |= bit;
            match kind {
                1 => {
                    if value.len() != 16 || h[4] & 0x40 == 0 || source.mask & 2 == 0 {
                        return Err(TransportError::InvalidPacket);
                    }
                    route.source_ip = Some(IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(value)
                        .map_err(|_| TransportError::InvalidPacket)?)));
                }
                2 => {
                    if value.len() != 2 || source.mask & 2 == 0 { return Err(TransportError::InvalidPacket); }
                    route.source_port = u16::from_be_bytes([value[0], value[1]]);
                }
                3 => {
                    if value.len() % 2 != 0 || route.relay_paths.is_some() { return Err(TransportError::InvalidPacket); }
                    route.relay_paths = Some(value.chunks_exact(2).map(|word| u16::from_be_bytes([word[0], word[1]])).collect());
                }
                _ => unreachable!(),
            }
        }
    }
    if h[4] & 0x40 != 0 && seen & (1 << 1) == 0 { return Err(TransportError::InvalidPacket); }
    Ok(Routed { routing: route, payload: input.rest })
}

#[cfg(test)]
#[path = "native_routing_tests.rs"]
mod tests;
