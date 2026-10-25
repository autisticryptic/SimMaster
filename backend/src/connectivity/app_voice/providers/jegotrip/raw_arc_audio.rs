//! Recovered raw-ARC MEDIA CALLBACK framing, not the complete UDP carrier.
//! MediaClient send4A1D3C prepends one channel byte; receive4AA94C consumes it.
//! Channel zero is audio (60851C/589930). Socket/worker framing is a separate
//! layer. No socket, SDP, crypto key or call-ready capability is created here.
use std::fmt;
use zeroize::Zeroizing;
pub const AUDIO_CHANNEL: u8 = 0;
pub const MAX_RAW_MEDIA_BYTES: usize = 65_507;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawMediaError { Truncated, ResourceLimit, InvalidChannel, UnsupportedPayload }
pub struct RawMediaDatagram(Zeroizing<Vec<u8>>);
impl fmt::Debug for RawMediaDatagram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("RawMediaDatagram(<redacted>)") }
}
impl RawMediaDatagram { pub fn expose_to_transport(&self) -> &[u8] { &self.0 } }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioPacketKind { Rtp, Rtcp }

fn validate_sdes(packet: &[u8], count: usize) -> bool {
    let mut offset = 4;
    for _ in 0..count {
        if offset + 4 > packet.len() { return false; }
        offset += 4; // source identifier
        loop {
            let Some(&kind) = packet.get(offset) else { return false; };
            offset += 1;
            if kind == 0 { break; }
            let Some(&length) = packet.get(offset) else { return false; };
            offset += 1 + length as usize;
            if offset > packet.len() { return false; }
        }
        while offset % 4 != 0 {
            if packet.get(offset) != Some(&0) { return false; }
            offset += 1;
        }
    }
    offset == packet.len()
}
fn validate_rtcp_body(packet: &[u8]) -> bool {
    let count = (packet[0] & 0x1f) as usize;
    match packet[1] {
        200 => packet.len() == 28 + count * 24, // Sender Report
        201 => packet.len() == 8 + count * 24, // Receiver Report
        202 => validate_sdes(packet, count),
        203 => {
            let sources = 4 + count * 4;
            if count == 0 || sources > packet.len() { return false; }
            if sources == packet.len() { return true; }
            let end = sources + 1 + packet[sources] as usize;
            end <= packet.len() && (end + 3) / 4 * 4 == packet.len() && packet[end..].iter().all(|byte| *byte == 0)
        }
        204 => packet.len() >= 12, // APP: source, name, word-aligned private data
        205 => packet.len() >= 12 && match count {
            1 => packet.len() >= 16, // Generic NACK FCI: one or more 4-byte blocks
            3 | 4 => (packet.len() - 12) % 8 == 0,
            _ => false,
        },
        206 => packet.len() >= 12 && match count {
            1 => packet.len() == 12, // PLI has no FCI
            2 => packet.len() >= 16, // SLI blocks
            4 => packet.len() >= 20 && (packet.len() - 12) % 8 == 0, // FIR blocks
            _ => false,
        },
        // Unrecovered/private RTCP formats are not silently passed as supported.
        _ => false,
    }
}
fn validate_rtcp(mut remaining: &[u8]) -> Result<(), RawMediaError> {
    while !remaining.is_empty() {
        if remaining.len() < 4 || remaining[0] >> 6 != 2 { return Err(RawMediaError::UnsupportedPayload); }
        let length = (u16::from_be_bytes([remaining[2], remaining[3]]) as usize + 1) * 4;
        if length > remaining.len() { return Err(RawMediaError::Truncated); }
        let mut packet = &remaining[..length];
        if packet[0] & 0x20 != 0 {
            let padding = packet[length - 1] as usize;
            // Only the final compound packet may be padded. Padding cannot
            // replace its header/body or leave a non-word-aligned RTCP body.
            if length != remaining.len() || padding == 0 || padding % 4 != 0 || padding > length - 4 {
                return Err(RawMediaError::UnsupportedPayload);
            }
            packet = &packet[..length - padding];
        }
        if !validate_rtcp_body(packet) { return Err(RawMediaError::UnsupportedPayload); }
        remaining = &remaining[length..];
    }
    Ok(())
}
/// RFC5761 RTP/RTCP-mux admission: the negotiated profile must exclude RTP
/// payload types 64..95, including marker-clear packets. Common complete RTCP
/// bodies are checked; unknown feedback/XR formats require a separate profile.
/// Framing does not authenticate the peer or infer unrecognized crypto/ZRTP.
pub fn audio_kind(bytes: &[u8]) -> Result<AudioPacketKind, RawMediaError> {
    if bytes.len() < 4 || bytes[0] >> 6 != 2 { return Err(RawMediaError::UnsupportedPayload); }
    if (192..=223).contains(&bytes[1]) {
        validate_rtcp(bytes)?;
        return Ok(AudioPacketKind::Rtcp);
    }
    if bytes.len() < 12 || (64..=95).contains(&(bytes[1] & 0x7f)) { return Err(RawMediaError::UnsupportedPayload); }
    let mut header = 12 + (bytes[0] & 0x0f) as usize * 4;
    if header > bytes.len() { return Err(RawMediaError::Truncated); }
    if bytes[0] & 0x10 != 0 {
        if header + 4 > bytes.len() { return Err(RawMediaError::Truncated); }
        header += 4 + u16::from_be_bytes([bytes[header + 2], bytes[header + 3]]) as usize * 4;
        if header > bytes.len() { return Err(RawMediaError::Truncated); }
    }
    if bytes[0] & 0x20 != 0 {
        let padding = bytes[bytes.len() - 1] as usize;
        if padding == 0 || padding > bytes.len() - header { return Err(RawMediaError::UnsupportedPayload); }
    }
    Ok(AudioPacketKind::Rtp)
}
pub fn encode_audio(payload: &[u8]) -> Result<RawMediaDatagram, RawMediaError> {
    if payload.len() >= MAX_RAW_MEDIA_BYTES { return Err(RawMediaError::ResourceLimit); }
    audio_kind(payload)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(payload.len() + 1));
    bytes.push(AUDIO_CHANNEL); bytes.extend_from_slice(payload);
    Ok(RawMediaDatagram(bytes))
}
pub fn decode_audio(datagram: &[u8]) -> Result<Option<&[u8]>, RawMediaError> {
    if datagram.len() > MAX_RAW_MEDIA_BYTES { return Err(RawMediaError::ResourceLimit); }
    let (&channel, payload) = datagram.split_first().ok_or(RawMediaError::Truncated)?;
    if channel != AUDIO_CHANNEL { return Ok(None); }
    if payload.is_empty() { return Err(RawMediaError::Truncated); }
    audio_kind(payload)?;
    Ok(Some(payload))
}
#[cfg(test)]
#[path = "raw_arc_audio_tests.rs"]
mod tests;
