//! Recovered PacketSender framing for the explicitly configured stream route.
//!
//! The four-byte BIG-ENDIAN prefix is the original opaque packet byte length,
//! not length+4. This decoder accepts arbitrary split/coalesced stream input;
//! it does not perform TLS, Router routing, authentication or RPC dispatch.
//! Bounds below are local policies, smaller than the native 64 MiB maximum.
use super::transport_packets::{TransportBytes, TransportError, MAX_CARRIER_BYTES};
use std::fmt;
use zeroize::Zeroize;

/// Allows the bounded TLS carrier plus the supported Router header/routing.
pub const MAX_NETWORK_PACKET_BYTES: usize = MAX_CARRIER_BYTES + 64;
/// A caller may supply two maximum-size frames in one chunk, not unlimited IO.
pub const MAX_STREAM_INPUT_BYTES: usize = 2 * (MAX_NETWORK_PACKET_BYTES + 4);
/// Prevents tiny coalesced frames from producing an unbounded output list.
pub const MAX_PACKETS_PER_INPUT: usize = 128;

pub fn encode_packet_stream_frame(packet: &[u8]) -> Result<TransportBytes, TransportError> {
    if packet.is_empty() { return Err(TransportError::InvalidPacket); }
    if packet.len() > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
    let mut bytes = Vec::with_capacity(packet.len() + 4);
    bytes.extend_from_slice(&(packet.len() as u32).to_be_bytes());
    bytes.extend_from_slice(packet);
    Ok(TransportBytes::from_bytes(bytes))
}

/// Keeps at most one incomplete packet. Errors and EOF are terminal; callers
/// cannot resynchronize by scanning bytes or resetting a live connection.
#[derive(Default)]
pub struct PacketStreamDecoder {
    prefix: [u8; 4],
    prefix_length: usize,
    expected: Option<usize>,
    partial: Vec<u8>,
    closed: bool,
}

impl fmt::Debug for PacketStreamDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PacketStreamDecoder").field("closed", &self.closed)
            .field("prefix_length", &self.prefix_length)
            .field("partial_length", &self.partial.len()).field("data", &"<redacted>").finish()
    }
}

impl PacketStreamDecoder {
    pub fn close(&mut self) {
        self.prefix = [0; 4]; self.prefix_length = 0;
        self.expected = None; self.partial.zeroize(); self.closed = true;
    }

    pub fn is_closed(&self) -> bool { self.closed }
    pub fn has_partial(&self) -> bool { self.prefix_length != 0 || self.expected.is_some() }

    /// A malformed later frame discards all output from THIS call before any
    /// consumer sees it. Earlier calls remain earlier observations, not ACKs.
    pub fn on_bytes(&mut self, bytes: &[u8]) -> Result<Vec<TransportBytes>, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(bytes);
        if result.is_err() { self.close(); }
        result
    }

    fn consume(&mut self, mut bytes: &[u8]) -> Result<Vec<TransportBytes>, TransportError> {
        if bytes.len() > MAX_STREAM_INPUT_BYTES { return Err(TransportError::TooLarge); }
        let mut packets = Vec::new();
        while !bytes.is_empty() {
            if self.expected.is_none() {
                let count = bytes.len().min(4 - self.prefix_length);
                self.prefix[self.prefix_length..self.prefix_length + count].copy_from_slice(&bytes[..count]);
                self.prefix_length += count; bytes = &bytes[count..];
                if self.prefix_length != 4 { break; }
                let expected = u32::from_be_bytes(self.prefix) as usize;
                if expected == 0 { return Err(TransportError::InvalidPacket); }
                if expected > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
                // This decoder is also used INSIDE authenticated endpoint TLS.
                // Allocate the validated bounded size once; incremental packet
                // copies must not reallocate and free unwiped plaintext buffers.
                self.partial.reserve_exact(expected);
                self.expected = Some(expected);
            }
            let expected = self.expected.ok_or(TransportError::InvalidPacket)?;
            let count = bytes.len().min(expected - self.partial.len());
            self.partial.extend_from_slice(&bytes[..count]); bytes = &bytes[count..];
            if self.partial.len() != expected { break; }
            if packets.len() >= MAX_PACKETS_PER_INPUT { return Err(TransportError::TooLarge); }
            packets.push(TransportBytes::from_bytes(std::mem::take(&mut self.partial)));
            self.expected = None; self.prefix_length = 0; self.prefix = [0; 4];
        }
        Ok(packets)
    }

    /// Empty on_bytes is not EOF. The original receive task must explicitly
    /// report EOF; an unfinished prefix/body is a truncation, never a packet.
    pub fn finish_eof(&mut self) -> Result<(), TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let incomplete = self.has_partial(); self.close();
        if incomplete { Err(TransportError::Truncated) } else { Ok(()) }
    }
}

impl Drop for PacketStreamDecoder { fn drop(&mut self) { self.close(); } }

#[cfg(test)]
#[path = "packet_stream_tests.rs"]
mod tests;
