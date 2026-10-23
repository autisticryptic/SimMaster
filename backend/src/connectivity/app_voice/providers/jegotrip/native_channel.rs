//! Recovered Channel packet/record framing used below native NetStream. This
//! data codec does not install a transport, authenticate a peer or mark hosted.
//! Timestamp mode is explicitly chosen by the original channel, never sniffed.
use super::{packet_stream::MAX_NETWORK_PACKET_BYTES, transport_packets::{extend_wiping, TransportBytes, TransportError}};
use std::fmt;
use zeroize::Zeroize;

const BASE_HEADER: usize = 8;
const MAX_RECORD: usize = 1_048_484; // native ChannelFrag DATA_MAX_SIZE
const MAX_RECORDS_PER_PACKET: usize = 64; // local resource policy

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderMode { Basic, Timestamped }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketKind { Start, Data, Skip, Probe, Fec }
impl PacketKind {
    fn from_bits(bits: u8) -> Result<Self, TransportError> { match bits {
        0 => Ok(Self::Start), 1 => Ok(Self::Data), 2 => Ok(Self::Skip), 3 => Ok(Self::Probe), 4 => Ok(Self::Fec),
        _ => Err(TransportError::UnsupportedKind),
    } }
    fn bits(self) -> u8 { match self { Self::Start => 0, Self::Data => 1, Self::Skip => 2, Self::Probe => 3, Self::Fec => 4 } }
}
pub struct ChannelPacket<'a> {
    pub timestamp: Option<u16>, pub sequence: u16, pub kind: PacketKind,
    pub data_gap: u8, pub ack_sequence: u16, pub delivered_sequence: u16,
    pub inclusive_ack: bool, pub ack_mask: &'a [u8], pub body: &'a [u8],
}
impl fmt::Debug for ChannelPacket<'_> { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ChannelPacket(<redacted>)") } }

pub fn decode_packet(mode: HeaderMode, bytes: &[u8]) -> Result<ChannelPacket<'_>, TransportError> {
    if bytes.len() > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
    let prefix = if mode == HeaderMode::Timestamped { 2 } else { 0 };
    let header = bytes.get(prefix..prefix + BASE_HEADER).ok_or(TransportError::Truncated)?;
    let mask_len = (header[3] & 0x7f) as usize; let offset = prefix + BASE_HEADER;
    let mask = bytes.get(offset..offset + mask_len).ok_or(TransportError::Truncated)?;
    let timestamp = if prefix == 2 { Some(u16::from_be_bytes([bytes[0],bytes[1]])) } else { None };
    Ok(ChannelPacket { timestamp, sequence: u16::from_be_bytes([header[0],header[1]]),
        kind: PacketKind::from_bits(header[2] >> 5)?, data_gap: header[2] & 31,
        ack_sequence: u16::from_be_bytes([header[4],header[5]]), delivered_sequence: u16::from_be_bytes([header[6],header[7]]),
        inclusive_ack: header[3] & 0x80 != 0, ack_mask: mask, body: &bytes[offset + mask_len..] })
}
pub fn encode_packet(mode: HeaderMode, packet: &ChannelPacket<'_>) -> Result<TransportBytes, TransportError> {
    if packet.data_gap > 31 || packet.ack_mask.len() > 127
        || (mode == HeaderMode::Timestamped) != packet.timestamp.is_some() { return Err(TransportError::InvalidPacket); }
    let prefix = if mode == HeaderMode::Timestamped { 2 } else { 0 };
    if packet.body.len() > MAX_NETWORK_PACKET_BYTES - prefix - BASE_HEADER - packet.ack_mask.len() { return Err(TransportError::TooLarge); }
    let mut bytes = Vec::with_capacity(prefix + BASE_HEADER + packet.ack_mask.len() + packet.body.len());
    if let Some(timestamp) = packet.timestamp { bytes.extend_from_slice(&timestamp.to_be_bytes()); }
    bytes.extend_from_slice(&packet.sequence.to_be_bytes()); bytes.push(packet.kind.bits() << 5 | packet.data_gap);
    bytes.push(packet.ack_mask.len() as u8 | if packet.inclusive_ack { 0x80 } else { 0 });
    bytes.extend_from_slice(&packet.ack_sequence.to_be_bytes()); bytes.extend_from_slice(&packet.delivered_sequence.to_be_bytes());
    bytes.extend_from_slice(packet.ack_mask); bytes.extend_from_slice(packet.body);
    Ok(TransportBytes::from_bytes(bytes))
}

/// ACK bits are LSB-first. A set bit acknowledges its individual original
/// channel sequence. The final ack_sequence is always acknowledged itself.
/// This does not consume a queued RPC, prove hosting or authorize data delivery.
pub fn acknowledged_sequences(packet: &ChannelPacket<'_>, previous_ack: u16)
    -> Result<Vec<u16>, TransportError> {
    if packet.ack_mask.len() > 127 { return Err(TransportError::TooLarge); }
    let ack = packet.ack_sequence;
    let delta = ack.wrapping_sub(previous_ack) as i16;
    if delta < 0 { return Ok(Vec::new()); }
    let start = if packet.ack_mask.is_empty() { ack }
        else { (ack.wrapping_sub(1) & !7).wrapping_sub((8 * (packet.ack_mask.len() - 1)) as u16) };
    let mut result = Vec::new();
    if packet.inclusive_ack {
        let mut sequence = previous_ack.wrapping_add(1);
        // A peer may claim an arbitrarily large stale span; the native send
        // window is4096 and a local caller never iterates half of u16 space.
        let count = start.wrapping_sub(sequence) as i16;
        if count > 4096 { return Err(TransportError::TooLarge); }
        while start.wrapping_sub(sequence) as i16 > 0 { result.push(sequence); sequence = sequence.wrapping_add(1); }
    }
    let bits = ack.wrapping_sub(start) as usize;
    if bits > packet.ack_mask.len() * 8 { return Err(TransportError::InvalidPacket); }
    for bit in 0..bits {
        if packet.ack_mask[bit / 8] & (1 << (bit & 7)) != 0 { result.push(start.wrapping_add(bit as u16)); }
    }
    result.push(ack); Ok(result)
}

/// A DATA packet's five low bits count intervening NON-data sequences. Escape31
/// instead carries the original preceding data sequence as a BE16 body prefix.
pub fn previous_data<'a>(packet: &ChannelPacket<'a>) -> Result<(u16, &'a [u8]), TransportError> {
    if packet.kind != PacketKind::Data { return Err(TransportError::UnsupportedKind); }
    if packet.data_gap > 31 { return Err(TransportError::InvalidPacket); }
    if packet.data_gap < 31 { return Ok((packet.sequence.wrapping_sub(1 + packet.data_gap as u16), packet.body)); }
    let previous = packet.body.get(..2).ok_or(TransportError::Truncated)?;
    Ok((u16::from_be_bytes([previous[0],previous[1]]), &packet.body[2..]))
}

/// Expand a native retransmission: each BE16 descriptor names its previous
/// packet kind (high2) and u16 backwards distance(low14). Re-sent Skip packets
/// add another descriptor; Start/Data terminate and retain the original body.
/// This reconstructs DATA ONLY; the caller must retain original live ownership
/// and must never use it to replay cancelled work on a replacement connection.
pub fn original_packet<'a>(packet: &ChannelPacket<'a>) -> Result<ChannelPacket<'a>, TransportError> {
    let mut kind = packet.kind; let mut sequence = packet.sequence; let mut body = packet.body;
    let mut descriptors = 0;
    while kind == PacketKind::Skip {
        descriptors += 1;
        if descriptors > 100 { return Err(TransportError::TooLarge); } // local retention/cpu bound
        let word = body.get(..2).ok_or(TransportError::Truncated)?;
        let value = u16::from_be_bytes([word[0],word[1]]); let delta = value & 0x3fff;
        if delta == 0 { return Err(TransportError::InvalidPacket); }
        sequence = sequence.wrapping_sub(delta); body = &body[2..];
        kind = match value >> 14 { 0 => PacketKind::Start, 1 => PacketKind::Data, 2 => PacketKind::Skip,
            _ => return Err(TransportError::UnsupportedKind) };
    }
    Ok(ChannelPacket { timestamp: packet.timestamp, sequence, kind, data_gap: packet.data_gap,
        ack_sequence: packet.ack_sequence, delivered_sequence: packet.delivered_sequence,
        inclusive_ack: packet.inclusive_ack, ack_mask: packet.ack_mask, body })
}

/// Add precisely one native retransmission descriptor to original Start/Data/
/// Skip bytes. A later live frame uses a NEW channel sequence but still names
/// the SAME pending original data. No callback/ticket or replacement IO bound.
pub fn retransmission_body(original: &ChannelPacket<'_>, new_sequence: u16) -> Result<TransportBytes, TransportError> {
    if !matches!(original.kind, PacketKind::Start | PacketKind::Data | PacketKind::Skip) { return Err(TransportError::UnsupportedKind); }
    let delta = new_sequence.wrapping_sub(original.sequence);
    if delta == 0 || delta >= 0x4000 || original.body.len() > MAX_NETWORK_PACKET_BYTES - 2 { return Err(TransportError::InvalidPacket); }
    let mut body = Vec::with_capacity(original.body.len() + 2);
    body.extend_from_slice(&(delta | (u16::from(original.kind.bits()) << 14)).to_be_bytes()); body.extend_from_slice(original.body);
    Ok(TransportBytes::from_bytes(body))
}

/// A native Start, including one wrapped in retransmission descriptors, names
/// the receive incarnation. A retransmitted DATA does NOT start/reset a stream.
pub fn starting_sequence(packet: &ChannelPacket<'_>) -> Result<Option<u16>, TransportError> {
    let original = original_packet(packet)?;
    if original.kind == PacketKind::Start { Ok(Some(original.sequence)) } else { Ok(None) }
}

pub struct Records { partial: Vec<u8>, closed: bool }
impl Default for Records { fn default() -> Self { Self { partial: Vec::new(), closed: false } } }
impl Drop for Records { fn drop(&mut self) { self.partial.zeroize(); } }
impl fmt::Debug for Records { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.debug_struct("Records")
    .field("closed", &self.closed).field("partial", &!self.partial.is_empty()).field("data", &"<redacted>").finish() } }
impl Records {
    pub fn has_partial(&self) -> bool { !self.partial.is_empty() }
    pub fn close(&mut self) { self.partial.zeroize(); self.closed = true; }
    /// Input is one complete ordered channel DATA body AFTER channel ACK/gap
    /// fields, not an arbitrary TCP/TLS read. Caller establishes original scope.
    /// Native0xfe is continuation and0xff completes the accumulated record;
    /// ordinary BE16 lengths may batch several records in one channel packet.
    pub fn on_payload(&mut self, mut bytes: &[u8]) -> Result<Vec<TransportBytes>, TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let result = self.consume(&mut bytes); if result.is_err() { self.close(); } result
    }
    fn consume(&mut self, bytes: &mut &[u8]) -> Result<Vec<TransportBytes>, TransportError> {
        if bytes.len() > MAX_NETWORK_PACKET_BYTES { return Err(TransportError::TooLarge); }
        let mut completed = Vec::new();
        while !bytes.is_empty() {
            if completed.len() >= MAX_RECORDS_PER_PACKET { return Err(TransportError::TooLarge); }
            match bytes[0] {
                0xfe | 0xff => {
                    let final_piece = bytes[0] == 0xff; let chunk = &bytes[1..];
                    if chunk.is_empty() || chunk.len() > MAX_RECORD - self.partial.len() { return Err(TransportError::InvalidFragment); }
                    extend_wiping(&mut self.partial, chunk, MAX_RECORD)?; *bytes = &[];
                    if final_piece { completed.push(TransportBytes::from_bytes(std::mem::take(&mut self.partial))); }
                }
                _ => {
                    let prefix = bytes.get(..2).ok_or(TransportError::Truncated)?;
                    let length = u16::from_be_bytes([prefix[0],prefix[1]]) as usize;
                    if length == 0 { return Err(TransportError::InvalidFragment); }
                    let body = bytes.get(2..2 + length).ok_or(TransportError::Truncated)?;
                    if length > MAX_RECORD - self.partial.len() { return Err(TransportError::TooLarge); }
                    extend_wiping(&mut self.partial, body, MAX_RECORD)?; completed.push(TransportBytes::from_bytes(std::mem::take(&mut self.partial)));
                    *bytes = &bytes[2 + length..];
                }
            }
        }
        Ok(completed)
    }
    pub fn finish_eof(&mut self) -> Result<(), TransportError> {
        if self.closed { return Err(TransportError::Closed); }
        let partial = self.has_partial(); self.close(); if partial { Err(TransportError::Truncated) } else { Ok(()) }
    }
}

/// Unbatched native record segmentation with an explicit original-channel body
/// budget. Full pieces use0xfe, last0xff. No invented inner BE32 prefix.
pub fn encode_record(bytes: &[u8], body_budget: usize) -> Result<Vec<TransportBytes>, TransportError> {
    if bytes.is_empty() || bytes.len() > MAX_RECORD || !(2..=MAX_NETWORK_PACKET_BYTES).contains(&body_budget) { return Err(TransportError::InvalidPacket); }
    let mut input = bytes; let mut packets = Vec::new();
    while !input.is_empty() {
        let count = input.len().min(body_budget - 1); let mut packet = Vec::with_capacity(count + 1);
        packet.push(if input.len() > count { 0xfe } else { 0xff }); packet.extend_from_slice(&input[..count]);
        packets.push(TransportBytes::from_bytes(packet)); input = &input[count..];
        if packets.len() > 4096 { return Err(TransportError::TooLarge); }
    }
    Ok(packets)
}

#[cfg(test)]
#[path = "native_channel_tests.rs"]
mod tests;
