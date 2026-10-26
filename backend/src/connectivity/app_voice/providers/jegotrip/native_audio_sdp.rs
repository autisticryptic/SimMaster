//! Strict basic incoming-audio SDP preflight over the native RseEx.SDP field.
//! Native 5FA47C/60ED60 have an explicit raw-SDP branch. This module supports
//! that branch only; compressed SDP, crypto, ICE/BUNDLE/video are not guessed.
//! Native SDP network addresses never authorize a Jegotrip UDP destination.
use super::{call_binding::OwnedCallNotice, call_control::CallControlTicket, media_endpoint::MediaError};
use crate::connectivity::core::voice::{parse_audio_sdp, MediaDirection, MediaTransportKind, SdpAddrType, SdpAudioDescription};
use std::{collections::BTreeSet, fmt, net::{IpAddr, SocketAddr}};
use zeroize::Zeroizing;
const MAX_SDP: usize = 16_384;

#[derive(Clone)]
struct EventCodec { payload: u8, clock: u32, fmtp: Option<String> }
pub(super) struct PlainAudioOffer { description: SdpAudioDescription, events: Vec<EventCodec> }
impl fmt::Debug for PlainAudioOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PlainAudioOffer(<redacted>)") }
}
impl PlainAudioOffer {
    pub(super) fn native(notice: &OwnedCallNotice, ticket: &CallControlTicket, plain_media_enabled: bool) -> Result<Self, MediaError> {
        if !ticket.is_live() || notice.kind() != super::rpc_calls::CallNoticeKind::Invited || notice.native_id() != ticket.native_id() { return Err(MediaError::Scope); }
        let data = notice.data_for(ticket.owner(), &ticket.connection_scope()).ok_or(MediaError::Scope)?;
        let raw = data.properties.iter().find(|property| property.key == b"RseEx.SDP").ok_or(MediaError::MissingProperty)?.value;
        Self::parse(raw, plain_media_enabled, false)
    }
    pub(super) fn trunk(body: &[u8], plain_media_enabled: bool) -> Result<Self, MediaError> { Self::parse(body, plain_media_enabled, true) }
    fn parse(body: &[u8], enabled: bool, trunk: bool) -> Result<Self, MediaError> {
        if !enabled || body.is_empty() || body.len() > MAX_SDP { return Err(MediaError::Configuration); }
        let text = std::str::from_utf8(body).map_err(|_| MediaError::Protocol)?;
        if !text.starts_with("v=0\r\n") || text.bytes().any(|byte| byte == 0 || byte < 32 && byte != b'\r' && byte != b'\n') {
            return Err(MediaError::Protocol);
        }
        let lines = text.split("\r\n").filter(|line| !line.is_empty()).collect::<Vec<_>>();
        if lines.len() > 128 || lines.iter().any(|line| line.len() > 1024 || line.contains('\r') || line.contains('\n')) { return Err(MediaError::Protocol); }
        for line in &lines {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("a=crypto:") || lower.starts_with("a=fingerprint:") || lower.starts_with("a=setup:")
                || lower.starts_with("a=ice-") || lower.starts_with("a=candidate:") || lower.starts_with("a=group:")
                || lower.starts_with("a=ssrc-group:") { return Err(MediaError::Configuration); }
        }
        let media = lines.iter().filter(|line| line.starts_with("m=")).collect::<Vec<_>>();
        if media.len() != 1 { return Err(MediaError::Configuration); }
        let words = media[0].split_ascii_whitespace().collect::<Vec<_>>();
        if words.len() < 4 || words[0] != "m=audio" || words[2] != "RTP/AVP" || words[1].contains('/') { return Err(MediaError::Configuration); }
        let mut payloads = BTreeSet::new();
        for word in &words[3..] {
            let payload: u8 = word.parse().map_err(|_| MediaError::Protocol)?;
            if payload > 127 || (64..=95).contains(&payload) || !payloads.insert(payload) { return Err(MediaError::Configuration); }
        }
        let description = parse_audio_sdp(body).map_err(|_| MediaError::Protocol)?;
        if description.transport != MediaTransportKind::RtpAvp || description.codecs.is_empty() || description.codecs.len() > 16
            || description.direction != MediaDirection::SendRecv || description.codecs.iter().any(|codec| !payloads.contains(&codec.payload_type)) {
            return Err(MediaError::Configuration);
        }
        let mut events = Vec::new(); let mut declared = BTreeSet::new();
        for line in &lines {
            let Some(mapping) = line.strip_prefix("a=rtpmap:") else { continue; };
            let words = mapping.split_ascii_whitespace().collect::<Vec<_>>();
            if words.len() != 2 { return Err(MediaError::Protocol); }
            let payload: u8 = words[0].parse().map_err(|_| MediaError::Protocol)?;
            if !payloads.contains(&payload) || !declared.insert(payload) { return Err(MediaError::Configuration); }
            let encoding = words[1].split('/').collect::<Vec<_>>();
            if !(2..=3).contains(&encoding.len()) { return Err(MediaError::Protocol); }
            let clock: u32 = encoding[1].parse().map_err(|_| MediaError::Protocol)?;
            let channels: u8 = if encoding.len() == 3 { encoding[2].parse().map_err(|_| MediaError::Protocol)? } else { 1 };
            if encoding[0].eq_ignore_ascii_case("telephone-event") {
                if !matches!(clock, 8000 | 16000 | 48000) || channels != 1 || events.len() >= 4 { return Err(MediaError::Configuration); }
                let prefix = format!("a=fmtp:{payload} ");
                let fmtps = lines.iter().filter_map(|line| line.strip_prefix(&prefix)).collect::<Vec<_>>();
                if fmtps.len() > 1 { return Err(MediaError::Configuration); }
                events.push(EventCodec { payload, clock, fmtp: fmtps.first().map(|value| value.to_string()) });
            } else {
                let codec = description.codecs.iter().find(|codec| codec.payload_type == payload).ok_or(MediaError::Configuration)?;
                // The shared permissive parser must not replace a malformed
                // static-codec rtpmap with an implicit PCMU/PCMA declaration.
                if !codec.codec.rtpmap_encoding().eq_ignore_ascii_case(encoding[0]) || codec.clock_rate != clock
                    || codec.channels.unwrap_or(1) != channels { return Err(MediaError::Configuration); }
            }
        }
        if payloads.iter().any(|payload| !description.codecs.iter().any(|codec| codec.payload_type == *payload)
            && !events.iter().any(|codec| codec.payload == *payload)) { return Err(MediaError::Configuration); }
        // The current relay supports RTP/RTCP-mux only. Preserve this explicit
        // restriction instead of silently forwarding separate RTCP/crypto.
        if trunk && !lines.contains(&"a=rtcp-mux") { return Err(MediaError::Configuration); }
        Ok(Self { description, events })
    }
    pub(super) fn endpoint(&self) -> Result<SocketAddr, MediaError> {
        let address: IpAddr = self.description.connection_addr.parse().map_err(|_| MediaError::Configuration)?;
        if address.is_unspecified() || address.is_multicast() || self.description.media_port == 0 { return Err(MediaError::Configuration); }
        Ok(SocketAddr::new(address, self.description.media_port))
    }
    pub(super) fn with_local_endpoint(&self, ticket: &CallControlTicket, local: SocketAddr) -> Result<Zeroizing<Vec<u8>>, MediaError> {
        if !ticket.is_live() { return Err(MediaError::Scope); }
        if local.port() == 0 || local.ip().is_unspecified() || local.ip().is_multicast() { return Err(MediaError::Configuration); }
        let mut description = self.description.clone();
        description.session_id = ticket.native_id().expose_to_backend(); description.session_version = 1; description.origin_username = "app-account".into();
        description.connection_addr = local.ip().to_string(); description.media_port = local.port();
        description.addr_type = if local.is_ipv4() { SdpAddrType::Ip4 } else { SdpAddrType::Ip6 };
        let mut rendered = description.to_sdp().replace("s=SimAdmin VoWiFi Call", "s=SimAdmin application call");
        if !self.events.is_empty() {
            let media = rendered.lines().find(|line| line.starts_with("m=audio ")).ok_or(MediaError::Protocol)?.to_owned();
            let suffix = self.events.iter().map(|codec| format!(" {}", codec.payload)).collect::<String>();
            rendered = rendered.replacen(&media, &format!("{media}{suffix}"), 1);
            for codec in &self.events {
                rendered.push_str(&format!("a=rtpmap:{} telephone-event/{}\r\n", codec.payload, codec.clock));
                if let Some(fmtp) = &codec.fmtp { rendered.push_str(&format!("a=fmtp:{} {}\r\n", codec.payload, fmtp)); }
            }
        }
        let mut text = Zeroizing::new(rendered.into_bytes());
        text.extend_from_slice(b"a=rtcp-mux\r\n"); Ok(text)
    }
    pub(super) fn matching_payloads(&self, answer: &Self) -> Result<Vec<u8>, MediaError> {
        let mut payloads = Vec::new();
        for codec in &answer.description.codecs {
            let Some(original) = self.description.codecs.iter().find(|candidate| candidate.payload_type == codec.payload_type) else {
                return Err(MediaError::Configuration);
            };
            if original.codec != codec.codec || original.clock_rate != codec.clock_rate || original.channels.unwrap_or(1) != codec.channels.unwrap_or(1)
                || original.fmtp != codec.fmtp { return Err(MediaError::Configuration); }
            payloads.push(codec.payload_type);
        }
        for codec in &answer.events {
            if !self.events.iter().any(|original| original.payload == codec.payload && original.clock == codec.clock && original.fmtp == codec.fmtp) {
                return Err(MediaError::Configuration);
            }
            payloads.push(codec.payload);
        }
        if payloads.is_empty() { return Err(MediaError::Configuration); } Ok(payloads)
    }
}
#[cfg(test)]
#[path = "native_audio_sdp_tests.rs"]
mod tests;
