//! Common mode-3 plaintext boundaries independent of TLS read/record boundaries.
//!
//! There is NO added inner length prefix. Ordinary request/reply envelopes are
//! self-delimiting by their native counts/byte lengths. Larger packets use the
//! verified fixed mode-3 fragment threshold and canonical balanced chunk plan.
//! Alternate thresholds, compression and unrecognized kinds fail closed. This
//! parser runs only AFTER a trusted TLS engine, not on raw network traffic.
use super::{
    rpc_envelope::{decode_reply, decode_request},
    rpc_stream::{CodecError, MAX_FIELD_BYTES, MAX_MAP_ENTRIES, MAX_STREAM_BYTES},
    transport_packets::{CallAssembler, CallKind, TransportBytes, TransportError, TLS_FRAGMENT_THRESHOLD},
};
use std::fmt;

pub const MAX_PLAINTEXT_INPUT_BYTES: usize = 2 * (MAX_STREAM_BYTES + 64);
const MAX_PACKETS_PER_INPUT: usize = 128;
const MAX_PACKET_BYTES: usize = TLS_FRAGMENT_THRESHOLD + 1;

fn codec_error(error: CodecError) -> TransportError {
    if error == CodecError::TooLarge { TransportError::TooLarge } else { TransportError::InvalidPacket }
}

/// Incremental boundary scanner. It scans a bounded incomplete envelope without
/// allocating maps/fields or interpreting its contents as credentials or text.
struct Boundary<'a> { bytes: &'a [u8], position: usize }
impl<'a> Boundary<'a> {
    fn take(&mut self, count: usize) -> Result<Option<&'a [u8]>, TransportError> {
        let end = self.position.checked_add(count).ok_or(TransportError::TooLarge)?;
        if end > TLS_FRAGMENT_THRESHOLD { return Err(TransportError::InvalidPacket); }
        let Some(value) = self.bytes.get(self.position..end) else { return Ok(None); };
        self.position = end; Ok(Some(value))
    }
    fn word(&mut self) -> Result<Option<u32>, TransportError> {
        Ok(self.take(4)?.map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
    }
    fn field(&mut self) -> Result<Option<&'a [u8]>, TransportError> {
        let Some(length) = self.word()? else { return Ok(None); };
        if length as usize > MAX_FIELD_BYTES { return Err(TransportError::TooLarge); }
        self.take(length as usize)
    }
}

fn envelope_length(kind: u8, bytes: &[u8]) -> Result<Option<usize>, TransportError> {
    let mut scanner = Boundary { bytes, position: 0 };
    let Some(marker) = scanner.take(1)? else { return Ok(None); };
    if marker != [0] { return Err(TransportError::InvalidPacket); }
    let Some(id) = scanner.word()? else { return Ok(None); };
    if id == 0 || id > i32::MAX as u32 { return Err(TransportError::InvalidPacket); }
    if kind == CallKind::Reply as u8 {
        if scanner.word()?.is_none() || scanner.field()?.is_none() { return Ok(None); }
    } else {
        let Some(count) = scanner.word()? else { return Ok(None); };
        if count as usize > MAX_MAP_ENTRIES { return Err(TransportError::TooLarge); }
        for _ in 0..count {
            if scanner.field()?.is_none() || scanner.field()?.is_none() { return Ok(None); }
        }
        let Some(method) = scanner.field()? else { return Ok(None); };
        if method.is_empty() { return Err(TransportError::InvalidPacket); }
        if scanner.field()?.is_none() { return Ok(None); }
    }
    Ok(Some(scanner.position))
}

fn chunk_length(remaining: usize) -> usize {
    if remaining >= 2 * TLS_FRAGMENT_THRESHOLD { TLS_FRAGMENT_THRESHOLD }
    else if remaining > TLS_FRAGMENT_THRESHOLD { remaining / 2 }
    else { remaining }
}

#[derive(Default)]
pub struct PlaintextPacketDecoder {
    buffer: Vec<u8>,
    fragment_remaining: usize,
    assembler: CallAssembler,
    closed: bool,
}

impl fmt::Debug for PlaintextPacketDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaintextPacketDecoder").field("closed", &self.closed)
            .field("buffer_length", &self.buffer.len()).field("fragmented", &(self.fragment_remaining != 0))
            .field("data", &"<redacted>").finish()
    }
}

impl PlaintextPacketDecoder {
    pub fn has_partial(&self) -> bool { !self.buffer.is_empty() || self.fragment_remaining != 0 }
    pub fn close(&mut self) { self.buffer.clear(); self.fragment_remaining = 0; self.assembler.close(); self.closed = true; }

    /// Bad later data discards THIS input call's output; the caller closes the
    /// same ARC scope/fragment assembler. Empty input is not EOF or a boundary.
    pub fn on_bytes(&mut self, bytes: &[u8]) -> Result<Vec<TransportBytes>, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(bytes);
        if result.is_err() { self.close(); }
        result
    }

    fn consume(&mut self, mut bytes: &[u8]) -> Result<Vec<TransportBytes>, TransportError> {
        if bytes.len() > MAX_PLAINTEXT_INPUT_BYTES { return Err(TransportError::TooLarge); }
        let mut packets = Vec::new();
        loop {
            while let Some(length) = self.next_length()? {
                if self.buffer.len() < length { break; }
                if packets.len() >= MAX_PACKETS_PER_INPUT { return Err(TransportError::TooLarge); }
                let packet = self.buffer.drain(..length).collect::<Vec<_>>();
                let header = packet[0];
                if header == 0x20 || header == 0x21 {
                    let original = u32::from_be_bytes([packet[1], packet[2], packet[3], packet[4]]) as usize;
                    self.fragment_remaining = original + 4 - (length - 1);
                } else if header == 3 { self.fragment_remaining -= length - 1; }
                if let Some(message) = self.assembler.on_packet(&packet)? {
                    match message.kind {
                        CallKind::Request => { decode_request(message.payload.expose_to_transport()).map_err(codec_error)?; }
                        CallKind::Reply => { decode_reply(message.payload.expose_to_transport()).map_err(codec_error)?; }
                        _ => {},
                    }
                }
                packets.push(TransportBytes::from_bytes(packet));
            }
            if bytes.is_empty() { break; }
            // Buffer at most one maximum native packet plus a small look-ahead,
            // never append an arbitrary coalesced input before checking counts.
            let count = bytes.len().min(MAX_PACKET_BYTES - self.buffer.len());
            if count == 0 { return Err(TransportError::TooLarge); }
            self.buffer.extend_from_slice(&bytes[..count]); bytes = &bytes[count..];
        }
        Ok(packets)
    }

    fn next_length(&self) -> Result<Option<usize>, TransportError> {
        let Some(&header) = self.buffer.first() else { return Ok(None); };
        if header & 0xc0 != 0 { return Err(TransportError::UnsupportedKind); }
        if header & 0x10 != 0 { return Err(TransportError::UnsupportedCompression); }
        if self.fragment_remaining != 0 {
            if header != 3 { return Err(TransportError::InvalidFragment); }
            return Ok(Some(1 + chunk_length(self.fragment_remaining)));
        }
        match header {
            0 | 1 => Ok(envelope_length(header, &self.buffer[1..])?.map(|length| length + 1)),
            2 | 4 | 14 => Ok(Some(1)),
            0x20 | 0x21 => {
                let Some(prefix) = self.buffer.get(1..5) else { return Ok(None); };
                let length = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
                if length > MAX_STREAM_BYTES { return Err(TransportError::TooLarge); }
                if length <= TLS_FRAGMENT_THRESHOLD { return Err(TransportError::InvalidFragment); }
                Ok(Some(1 + chunk_length(length + 4)))
            }
            3 => Err(TransportError::InvalidFragment),
            _ => Err(TransportError::UnsupportedKind),
        }
    }

    pub fn finish_eof(&mut self) -> Result<(), TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let incomplete = self.has_partial(); self.close();
        if incomplete { Err(TransportError::Truncated) } else { Ok(()) }
    }
}

#[cfg(test)]
#[path = "tls_plaintext_tests.rs"]
mod tests;
