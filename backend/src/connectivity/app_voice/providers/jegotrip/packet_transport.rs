//! Original ARC-owned PacketSender stream and optional exact Router subset.
//!
//! All output is opaque TLS carrier traffic for a FUTURE authenticated engine.
//! Decoding a length/route never produces RPC plaintext, channels, a provider or
//! a socket operation. Transmit/receive route bindings are supplied separately;
//! this adapter does not guess reverse addresses, endpoints or certificate trust.
use super::{
    packet_stream::{encode_packet_stream_frame, PacketStreamDecoder, MAX_PACKETS_PER_INPUT},
    router_packets::{decode_header, decode_routed_body, encode_routed_body, encode_router_packets,
        RouterAddress, RouterHeader, RouterPacketAssembler, RouterRouting},
    rpc_pending::RpcConnectionScope,
    session::AttemptOwner,
    transport_packets::{decode_tls_carrier, encode_tls_carrier, TransportBytes, TransportError},
};
use std::fmt;

pub struct RouterBinding<'a> { pub packet_kind: u8, pub routing: RouterRouting<'a> }

/// Select the already-confirmed original PacketSender route, never autodetect
/// from incoming bytes. A Router path needs independent captured TX/RX bindings.
pub enum PacketRoute<'a> {
    DirectTlsCarrier,
    Router { transmit: RouterBinding<'a>, receive: RouterBinding<'a>, mtu: usize, first_sequence: u16 },
}

impl fmt::Debug for PacketRoute<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PacketRoute(<redacted>)") }
}

#[derive(Clone, Copy)]
pub struct RouterTiming { pub word1: u16, pub word2: u16, pub byte6: u8 }

impl fmt::Debug for RouterTiming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RouterTiming(<redacted>)") }
}

struct OwnedAddress { mask: u8, label: u8, bytes: Vec<u8> }
struct OwnedRouting { packet_kind: u8, tag: u32, flags: u8, path_words: Vec<u16>, address1: OwnedAddress, address2: OwnedAddress }

impl OwnedRouting {
    fn capture(binding: RouterBinding<'_>) -> Result<Self, TransportError> {
        if binding.packet_kind > 1 { return Err(TransportError::UnsupportedKind); }
        encode_routed_body(&binding.routing, &[0])?;
        Ok(Self { packet_kind: binding.packet_kind, tag: binding.routing.tag, flags: binding.routing.flags,
            path_words: binding.routing.path_words.to_vec(),
            address1: OwnedAddress { mask: binding.routing.address1.mask, label: binding.routing.address1.label, bytes: binding.routing.address1.bytes.to_vec() },
            address2: OwnedAddress { mask: binding.routing.address2.mask, label: binding.routing.address2.label, bytes: binding.routing.address2.bytes.to_vec() } })
    }
    fn routing(&self) -> RouterRouting<'_> {
        RouterRouting { tag: self.tag, flags: self.flags, path_words: &self.path_words,
            address1: RouterAddress { mask: self.address1.mask, label: self.address1.label, bytes: &self.address1.bytes },
            address2: RouterAddress { mask: self.address2.mask, label: self.address2.label, bytes: &self.address2.bytes } }
    }
    fn matches(&self, body: &super::router_packets::RoutedBody<'_>) -> bool {
        body.tag == self.tag && body.flags == self.flags && body.path_words == self.path_words
            && body.address1.mask == self.address1.mask && body.address1.label == self.address1.label && body.address1.bytes == self.address1.bytes
            && body.address2.mask == self.address2.mask && body.address2.label == self.address2.label && body.address2.bytes == self.address2.bytes
    }
}

enum CapturedRoute {
    Direct,
    Router { transmit: OwnedRouting, receive: OwnedRouting, mtu: usize, next_sequence: Option<u16>, last_received: Option<u16> },
}

pub struct OwnedWireFrames { owner: AttemptOwner, connection: RpcConnectionScope, frames: Vec<TransportBytes> }

impl OwnedWireFrames {
    /// Checked snapshot for the original writer task, NOT a revocable IO lease.
    /// Close/drop blocks NEW access, but cannot revoke a previously borrowed or
    /// copied slice. A future writer must serialize close/cancellation with
    /// final submission and must not retain this snapshot across await/rebinding.
    /// That live writer/queue protocol is intentionally not implemented here.
    pub fn frames_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<&[TransportBytes]> {
        if &self.owner == owner && self.connection.matches(scope) && self.connection.is_open() { Some(&self.frames) } else { None }
    }
}

impl fmt::Debug for OwnedWireFrames {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OwnedWireFrames(<redacted>)") }
}

pub struct OwnedTlsInput { owner: AttemptOwner, connection: RpcConnectionScope, bytes: TransportBytes }

impl OwnedTlsInput {
    /// Checked snapshot, not proof of TLS authentication or a revocable lease.
    /// A future TLS task must serialize scope closure with consumption rather
    /// than retaining these bytes across an await after this openness check.
    pub fn bytes_for_tls(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Option<&[u8]> {
        if &self.owner == owner && self.connection.matches(scope) && self.connection.is_open() {
            Some(self.bytes.expose_to_transport())
        } else { None }
    }
}

impl fmt::Debug for OwnedTlsInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OwnedTlsInput(<redacted>)") }
}

pub struct OwnedPacketTransport {
    owner: AttemptOwner,
    connection: RpcConnectionScope,
    route: CapturedRoute,
    decoder: PacketStreamDecoder,
    router: RouterPacketAssembler,
}

impl fmt::Debug for OwnedPacketTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OwnedPacketTransport(<redacted>)") }
}

impl OwnedPacketTransport {
    pub(super) fn for_arc_session(owner: AttemptOwner, connection: RpcConnectionScope, route: PacketRoute<'_>)
        -> Result<Self, TransportError> {
        if !connection.is_open() { return Err(TransportError::Closed); }
        let route = match route {
            PacketRoute::DirectTlsCarrier => CapturedRoute::Direct,
            PacketRoute::Router { transmit, receive, mtu, first_sequence } => {
                if !(16..=super::packet_stream::MAX_NETWORK_PACKET_BYTES).contains(&mtu) { return Err(TransportError::InvalidPacket); }
                CapturedRoute::Router { transmit: OwnedRouting::capture(transmit)?, receive: OwnedRouting::capture(receive)?,
                    mtu, next_sequence: Some(first_sequence), last_received: None }
            }
        };
        Ok(Self { owner, connection, route, decoder: PacketStreamDecoder::default(), router: RouterPacketAssembler::default() })
    }
    fn accepts(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.connection.matches(scope) && self.connection.is_open()
    }
    pub fn has_partial(&self) -> bool { self.decoder.has_partial() || self.router.has_partial() }
    pub fn close(&mut self) { self.connection.close(); self.decoder.close(); self.router.close(); }

    /// Failed local encoding emits nothing and consumes no Router sequence.
    /// Input must already be TLS engine output; no fake plaintext downgrade.
    pub fn encode_tls_bytes(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope,
        tls_bytes: &[u8], timing: Option<RouterTiming>) -> Result<Option<OwnedWireFrames>, TransportError> {
        if !self.connection.is_open() { self.close(); return Ok(None); }
        if !self.accepts(owner, scope) { return Ok(None); }
        let carrier = encode_tls_carrier(tls_bytes)?;
        let (packets, next) = match &self.route {
            CapturedRoute::Direct => {
                if timing.is_some() { return Err(TransportError::InvalidPacket); }
                (vec![carrier], None)
            }
            CapturedRoute::Router { transmit, mtu, next_sequence, .. } => {
                let timing = timing.ok_or(TransportError::InvalidPacket)?;
                let sequence = next_sequence.ok_or(TransportError::InvalidFragment)?;
                let body = encode_routed_body(&transmit.routing(), carrier.expose_to_transport())?;
                let packets = encode_router_packets(RouterHeader { sequence, word1: timing.word1, word2: timing.word2,
                    byte6: timing.byte6, packet_kind: transmit.packet_kind, fragment_part: 1 }, body.expose_to_transport(), *mtu)?;
                let last = sequence.checked_add((packets.len() - 1) as u16).ok_or(TransportError::InvalidFragment)?;
                (packets, Some(last.checked_add(1)))
            }
        };
        let frames = packets.iter().map(|packet| encode_packet_stream_frame(packet.expose_to_transport())).collect::<Result<Vec<_>, _>>()?;
        if let (CapturedRoute::Router { next_sequence, .. }, Some(next)) = (&mut self.route, next) { *next_sequence = next; }
        Ok(Some(OwnedWireFrames { owner: self.owner.clone(), connection: self.connection.clone(), frames }))
    }

    /// Checks original scope BEFORE any parse/copy; malformed input on another
    /// receive task cannot poison the current connection. Successful parsing is
    /// merely opaque input for TLS, never an authenticated call-packet boundary.
    pub fn on_bytes(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope, bytes: &[u8])
        -> Result<Vec<OwnedTlsInput>, TransportError> {
        if !self.connection.is_open() { self.close(); return Ok(vec![]); }
        if !self.accepts(owner, scope) { return Ok(vec![]); }
        let result = self.consume(bytes);
        if result.is_err() { self.close(); }
        result
    }
    fn consume(&mut self, bytes: &[u8]) -> Result<Vec<OwnedTlsInput>, TransportError> {
        let packets = self.decoder.on_bytes(bytes)?; let mut input = Vec::new();
        for packet in packets {
            let tls = match &mut self.route {
                CapturedRoute::Direct => TransportBytes::from_bytes(decode_tls_carrier(packet.expose_to_transport())?.to_vec()),
                CapturedRoute::Router { receive, last_received, .. } => {
                    let (header, _) = decode_header(packet.expose_to_transport())?;
                    if header.packet_kind != receive.packet_kind { return Err(TransportError::UnsupportedRoute); }
                    if last_received.is_some_and(|last| header.sequence <= last) { return Err(TransportError::InvalidFragment); }
                    let message = self.router.on_packet(packet.expose_to_transport())?; *last_received = Some(header.sequence);
                    let Some(message) = message else { continue; };
                    let body = decode_routed_body(message.routing_and_payload.expose_to_transport())?;
                    if !receive.matches(&body) { return Err(TransportError::UnsupportedRoute); }
                    TransportBytes::from_bytes(decode_tls_carrier(body.payload)?.to_vec())
                }
            };
            if input.len() >= MAX_PACKETS_PER_INPUT { return Err(TransportError::TooLarge); }
            input.push(OwnedTlsInput { owner: self.owner.clone(), connection: self.connection.clone(), bytes: tls });
        }
        Ok(input)
    }
    pub fn finish_eof(&mut self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> Result<bool, TransportError> {
        if !self.connection.is_open() { self.close(); return Ok(false); }
        if !self.accepts(owner, scope) { return Ok(false); }
        let result = self.decoder.finish_eof().and_then(|()| self.router.finish_eof()); self.close();
        result.map(|()| true)
    }
}

impl Drop for OwnedPacketTransport {
    fn drop(&mut self) { self.close(); }
}

#[cfg(test)]
#[path = "packet_transport_tests.rs"]
mod tests;
