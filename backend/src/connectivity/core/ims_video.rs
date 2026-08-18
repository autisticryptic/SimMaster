//! Shared IMS video (ViLTE) SDP/H.264 layer — access-agnostic.
//!
//! Clean-room from public specs: GSMA IR.94 (IMS video profile), 3GPP TS 26.114
//! (media handling), RFC 4566 (SDP), RFC 6184 (RTP payload for H.264), RFC 3550
//! (RTP). No third-party binary source is used.
//!
//! ## What this module does (and does not) do
//!
//! IMS video is *video added to an ongoing IMS voice call*: the SIP INVITE
//! carries one SDP body with **two** media sections — the existing `m=audio`
//! line plus a new `m=video` line (H.264). The SIP envelope is SDP-agnostic
//! (it appends the SDP body verbatim), so no SIP change is needed. The work is
//! entirely in the SDP media layer and the media relay.
//!
//! On the target hardware class (no audio/video capture) the device is a
//! **pure packet relay**: it never encodes or decodes video. It forwards H.264
//! RTP between the operator IMS media endpoint and an internal SIP UA
//! (Linphone/Asterisk behind the trunk). Therefore this module does not
//! transcode; `codec`/`fmtp` values are what we *advertise*, carried through the
//! offer/answer verbatim.
//!
//! ## Scope
//!
//! Implemented and unit-tested offline:
//!   - [`VideoMediaDescription`]: an H.264 `m=video` section model + serializer.
//!   - [`build_video_offer`] / [`parse_video_sdp`]: build/parse the video
//!     section from discrete codec parameters.
//!   - [`build_av_sdp`]: compose an audio [`SdpAudioDescription`] and a video
//!     section into a single multi-line SDP body (the video offer/answer).
//!   - [`negotiate_video`]: pure video codec/PT matching for the answer.
//!   - [`VideoRelay`]: a second [`RtpRelayCore`] instance dedicated to the video
//!     stream (audio and video are separate RTP flows on separate ports).
//!
//! [`TrunkVideoSeam`] is the small negotiated-endpoint adapter used by the live
//! VoLTE/Trunk and VoWiFi/Trunk media paths; the concrete SIP dialog and RTP
//! relay lifecycle live in the voice/Trunk services.

use crate::connectivity::core::media::{
    ForwardDecision, LegEndpoint, RelayError, RelayLeg, RtpRelayCore,
};
use crate::connectivity::core::voice::{
    MediaDirection, MediaTransportKind, SdpAddrType, SdpAudioDescription,
};

/// The H.264 RTP clock rate (fixed at 90 kHz for video per RFC 6184).
pub const H264_CLOCK_RATE: u32 = 90_000;

/// The SDP media type token for video.
const MEDIA_VIDEO: &str = "video";

/// A single H.264 `m=video` media section, enough to serialize an RFC 4566
/// video description and to drive the video RTP relay.
///
/// This mirrors [`SdpAudioDescription`] but for the video stream. The two are
/// composed by [`build_av_sdp`] into one SDP body. Video normally inherits the
/// session-level `c=` address, but a media-level override is retained because
/// SIP peers may place audio and video on different hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMediaDescription {
    /// Media port for the video RTP stream (distinct from the audio port).
    pub media_port: u16,
    /// Effective video connection address. A media-level `c=` overrides the
    /// session-level address when the two RTP streams use different hosts.
    pub connection_addr: Option<String>,
    pub addr_type: Option<SdpAddrType>,
    pub transport: MediaTransportKind,
    /// Dynamic RTP payload type advertised for H.264.
    pub payload_type: u8,
    /// The rtpmap encoding name (normally `H264`).
    pub encoding: String,
    /// `a=fmtp` parameters (profile-level-id / packetization-mode), verbatim.
    pub fmtp: Option<String>,
    pub direction: MediaDirection,
}

impl VideoMediaDescription {
    /// Whether the peer rejected this stream with the RFC 4566 port-0
    /// convention. This rejects video only; sibling audio media stays valid.
    pub fn is_rejected(&self) -> bool {
        self.media_port == 0
    }

    /// Build the port-0 answer used when a peer declines an offered video
    /// stream. Keeping the media line preserves SDP offer/answer ordering.
    pub fn rejected_answer(&self) -> Self {
        let mut rejected = self.clone();
        rejected.media_port = 0;
        rejected.direction = MediaDirection::Inactive;
        rejected
    }

    /// Serialize just the media-level lines for this video section:
    /// `m=video ...`, `a=rtpmap`, optional `a=fmtp`, and the direction attr.
    /// (Session-level `v=`/`o=`/`s=`/`c=`/`t=` are emitted once by
    /// [`build_av_sdp`].)
    pub fn media_lines(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "m={} {} {} {}\r\n",
            MEDIA_VIDEO,
            self.media_port,
            self.transport.sdp_proto(),
            self.payload_type,
        ));
        if let (Some(connection_addr), Some(addr_type)) =
            (self.connection_addr.as_deref(), self.addr_type)
        {
            let addr_type = match addr_type {
                SdpAddrType::Ip4 => "IP4",
                SdpAddrType::Ip6 => "IP6",
            };
            out.push_str(&format!("c=IN {addr_type} {connection_addr}\r\n"));
        }
        out.push_str(&format!(
            "a=rtpmap:{} {}/{}\r\n",
            self.payload_type, self.encoding, H264_CLOCK_RATE
        ));
        if let Some(fmtp) = &self.fmtp {
            if !fmtp.is_empty() {
                out.push_str(&format!("a=fmtp:{} {}\r\n", self.payload_type, fmtp));
            }
        }
        out.push_str(&format!("a={}\r\n", self.direction.as_str()));
        out
    }
}

/// Build a video offer section from discrete codec parameters.
///
/// The caller (the per-line IMS video config) supplies the advertised codec,
/// dynamic payload type and `a=fmtp`; `media_port` is the bound local video RTP
/// port. This keeps `core` free of any platform config type.
pub fn build_video_offer(
    codec: &str,
    payload_type: u8,
    h264_fmtp: &str,
    media_port: u16,
) -> VideoMediaDescription {
    VideoMediaDescription {
        media_port,
        connection_addr: None,
        addr_type: None,
        transport: MediaTransportKind::RtpAvp,
        payload_type,
        encoding: encoding_for_codec(codec),
        fmtp: if h264_fmtp.is_empty() {
            None
        } else {
            Some(h264_fmtp.to_string())
        },
        direction: MediaDirection::SendRecv,
    }
}

/// Map a configured codec name onto the SDP rtpmap encoding token. Only H.264
/// is a first-class video codec; anything else is advertised verbatim (upper-
/// cased) since the relay is pass-through and does not care about the payload.
fn encoding_for_codec(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "h264" | "h.264" => "H264".to_string(),
        "h265" | "h.265" | "hevc" => "H265".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// Compose a full IMS video SDP body: the session/audio lines from
/// [`SdpAudioDescription::to_sdp`] followed by the video media section. The
/// result is a valid multi-media SDP (`m=audio` then `m=video`).
///
/// This keeps the audio serialization as the single source of truth (so any
/// audio-side change is inherited) and simply appends the video `m=` block,
/// which is exactly how a video offer extends a voice offer.
pub fn build_av_sdp(audio: &SdpAudioDescription, video: &VideoMediaDescription) -> String {
    let mut sdp = audio.to_sdp();
    sdp.push_str(&video.media_lines());
    sdp
}

/// Parse the **video** media section of an SDP body into a
/// [`VideoMediaDescription`].
///
/// Like [`crate::connectivity::core::voice::parse_audio_sdp`], this is permissive:
/// it walks lines, tracks whether it is inside the `m=video` section, and pulls
/// the port, payload type, `a=rtpmap`, `a=fmtp`, and direction for that section.
/// Returns [`VideoSdpError::NoVideoMedia`] when there is no `m=video` line.
pub fn parse_video_sdp(body: &[u8]) -> Result<VideoMediaDescription, VideoSdpError> {
    if body.is_empty() {
        return Err(VideoSdpError::Empty);
    }
    let text = std::str::from_utf8(body).map_err(|_| VideoSdpError::Malformed)?;

    let mut media_port = 0u16;
    let mut origin_connection: Option<(SdpAddrType, String)> = None;
    let mut session_connection: Option<(SdpAddrType, String)> = None;
    let mut video_connection: Option<(SdpAddrType, String)> = None;
    let mut transport = MediaTransportKind::RtpAvp;
    let mut payload_order: Vec<u8> = Vec::new();
    let mut direction = MediaDirection::SendRecv;
    let mut encoding: Option<(u8, String)> = None;
    let mut fmtp: Option<(u8, String)> = None;
    let mut in_video = false;
    let mut saw_video = false;
    let mut saw_any_media = false;

    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim_end();
        if line.is_empty() {
            continue;
        }
        let Some((kind, value)) = line.split_once('=') else {
            continue;
        };
        match kind {
            "o" => {
                let parts = value.split_whitespace().collect::<Vec<_>>();
                if parts.len() >= 6 {
                    let addr_type = if parts[4].eq_ignore_ascii_case("IP6") {
                        SdpAddrType::Ip6
                    } else {
                        SdpAddrType::Ip4
                    };
                    origin_connection = Some((addr_type, parts[5].to_string()));
                }
            }
            "c" => {
                let parts = value.split_whitespace().collect::<Vec<_>>();
                if parts.len() >= 3 {
                    let addr_type = if parts[1].eq_ignore_ascii_case("IP6") {
                        SdpAddrType::Ip6
                    } else {
                        SdpAddrType::Ip4
                    };
                    let connection = (addr_type, parts[2].to_string());
                    if !saw_any_media {
                        session_connection = Some(connection);
                    } else if in_video {
                        video_connection = Some(connection);
                    }
                }
            }
            "m" => {
                let parts = value.split_whitespace().collect::<Vec<_>>();
                saw_any_media = true;
                in_video = parts.first().copied() == Some(MEDIA_VIDEO);
                if in_video {
                    saw_video = true;
                    if parts.len() >= 2 {
                        media_port = parts[1].parse().unwrap_or(0);
                    }
                    if parts.len() >= 3 {
                        transport = match parts[2] {
                            "RTP/SAVP" => MediaTransportKind::RtpSavp,
                            _ => MediaTransportKind::RtpAvp,
                        };
                    }
                    for pt in parts.iter().skip(3) {
                        if let Ok(pt) = pt.parse::<u8>() {
                            payload_order.push(pt);
                        }
                    }
                }
            }
            "a" if in_video => {
                if let Some(rest) = value.strip_prefix("rtpmap:") {
                    // rtpmap:<pt> <encoding>/<clock>
                    let mut it = rest.split_whitespace();
                    if let (Some(pt), Some(enc)) = (it.next(), it.next()) {
                        if let Ok(pt) = pt.parse::<u8>() {
                            let name = enc.split('/').next().unwrap_or(enc).to_string();
                            encoding = Some((pt, name));
                        }
                    }
                } else if let Some(rest) = value.strip_prefix("fmtp:") {
                    let mut it = rest.splitn(2, ' ');
                    if let (Some(pt), Some(params)) = (it.next(), it.next()) {
                        if let Ok(pt) = pt.parse::<u8>() {
                            fmtp = Some((pt, params.to_string()));
                        }
                    }
                } else if let Some(dir) = MediaDirection::from_token_pub(value) {
                    direction = dir;
                }
            }
            _ => {}
        }
    }

    if !saw_video {
        return Err(VideoSdpError::NoVideoMedia);
    }

    // Prefer the rtpmap payload type; fall back to the first m-line PT.
    let (payload_type, enc_name) = match encoding {
        Some((pt, name)) => (pt, name),
        None => (
            payload_order.first().copied().unwrap_or(0),
            "H264".to_string(),
        ),
    };
    let fmtp = fmtp
        .filter(|(pt, _)| *pt == payload_type)
        .map(|(_, params)| params);
    let connection = video_connection
        .or(session_connection)
        .or(origin_connection);

    Ok(VideoMediaDescription {
        media_port,
        connection_addr: connection.as_ref().map(|(_, address)| address.clone()),
        addr_type: connection.map(|(addr_type, _)| addr_type),
        transport,
        payload_type,
        encoding: enc_name,
        fmtp,
        direction,
    })
}

/// Errors from IMS video SDP handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoSdpError {
    Empty,
    Malformed,
    /// The SDP body contained no `m=video` section.
    NoVideoMedia,
}

impl std::fmt::Display for VideoSdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::Empty => "ims_video_sdp_empty",
            Self::Malformed => "ims_video_sdp_malformed",
            Self::NoVideoMedia => "ims_video_sdp_no_video_media",
        };
        write!(f, "{reason}")
    }
}

impl std::error::Error for VideoSdpError {}

/// The result of matching our video offer against the far end's video answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoNegotiation {
    /// The negotiated payload type to use for the video stream.
    pub payload_type: u8,
    /// The negotiated encoding name (e.g. `H264`).
    pub encoding: String,
    /// The far end's video media port (where the relay forwards operator→UA).
    pub remote_port: u16,
}

/// Negotiate the video stream: confirm the answer advertises the same encoding
/// family we offered, and adopt the answer's payload type + port. Because the
/// device is a relay (no transcoding), the only hard requirement is that both
/// sides speak the same codec; the payload type is taken from the answer (the
/// answerer picks the PT it will send with).
pub fn negotiate_video(
    offer: &VideoMediaDescription,
    answer: &VideoMediaDescription,
) -> Result<VideoNegotiation, VideoSdpError> {
    if !offer.encoding.eq_ignore_ascii_case(&answer.encoding) {
        return Err(VideoSdpError::NoVideoMedia);
    }
    Ok(VideoNegotiation {
        payload_type: answer.payload_type,
        encoding: answer.encoding.clone(),
        remote_port: answer.media_port,
    })
}

// ---------------------------------------------------------------------------
// Video RTP relay (reuses the media-agnostic RtpRelayCore)
// ---------------------------------------------------------------------------

/// A dedicated relay for the IMS **video** stream.
///
/// Audio and video are independent RTP flows (separate `m=` lines, separate
/// ports), so a video call runs *two* relays: the existing audio
/// [`RtpRelayCore`] and this one. The core is media-type-neutral — it validates
/// only the RTP v2 header, not payload semantics — so H.264 packets relay
/// through it unchanged. This wrapper exists to make the "one relay per media
/// stream" intent explicit and to give the trunk seam a concrete video handle.
#[derive(Debug, Clone)]
pub struct VideoRelay {
    core: RtpRelayCore,
}

impl VideoRelay {
    /// Build a video relay between the operator IMS video endpoint and the
    /// internal SIP UA video endpoint.
    pub fn new(operator: LegEndpoint, internal: LegEndpoint) -> Self {
        Self {
            core: RtpRelayCore::new(operator, internal),
        }
    }

    /// Access the underlying core (for counters / diagnostics).
    pub fn core(&self) -> &RtpRelayCore {
        &self.core
    }

    /// Forward one inbound video datagram. See [`RtpRelayCore::ingest`].
    pub fn ingest(
        &mut self,
        leg: RelayLeg,
        src: std::net::SocketAddr,
        datagram: &[u8],
    ) -> Result<ForwardDecision, RelayError> {
        self.core.ingest(leg, src, datagram)
    }
}

// ---------------------------------------------------------------------------
// Trunk video seam
// ---------------------------------------------------------------------------

/// The video-side attachment point supplied by the negotiated Asterisk SDP.
pub trait TrunkVideoSeam {
    /// The internal SIP UA's negotiated video media endpoint (addr:port), or
    /// `None` if the UA declined video for this call.
    fn internal_video_endpoint(&self) -> Option<std::net::SocketAddr>;
}

/// Concrete attachment produced by Trunk offer/answer negotiation. `None`
/// explicitly represents an audio-only answer; a populated endpoint is paired
/// with the operator endpoint by the dedicated asynchronous video RTP relay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NegotiatedTrunkVideoSeam {
    endpoint: Option<std::net::SocketAddr>,
}

impl NegotiatedTrunkVideoSeam {
    pub fn new(endpoint: Option<std::net::SocketAddr>) -> Self {
        Self { endpoint }
    }
}

impl TrunkVideoSeam for NegotiatedTrunkVideoSeam {
    fn internal_video_endpoint(&self) -> Option<std::net::SocketAddr> {
        self.endpoint
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::voice::{
        build_mo_audio_offer_with_params, SdpAddrType, VoiceParams,
    };

    fn test_audio_offer() -> SdpAudioDescription {
        let params = VoiceParams {
            preferred_codecs: vec!["pcmu".to_string()],
            codec_policies: Vec::new(),
            ptime_ms: 20,
            amr_octet_align: true,
            vowifi_enabled: false,
            carrier_fallback_enabled: false,
            ims_transport: "tcp",
            profile_id: "test",
            plmn: "23433",
        };
        build_mo_audio_offer_with_params(&params, "192.0.2.10", SdpAddrType::Ip4, 40000)
    }

    const TEST_CODEC: &str = "h264";
    const TEST_PAYLOAD_TYPE: u8 = 99;
    const TEST_FMTP: &str = "profile-level-id=42e01f;packetization-mode=1";

    fn sample_video() -> VideoMediaDescription {
        build_video_offer(TEST_CODEC, TEST_PAYLOAD_TYPE, TEST_FMTP, 50000)
    }

    #[test]
    fn video_offer_uses_h264_defaults() {
        let v = sample_video();
        assert_eq!(v.encoding, "H264");
        assert_eq!(v.payload_type, 99);
        assert_eq!(v.media_port, 50000);
        assert!(v.fmtp.as_deref().unwrap().contains("packetization-mode=1"));
    }

    #[test]
    fn video_media_lines_are_well_formed() {
        let v = sample_video();
        let lines = v.media_lines();
        assert!(lines.contains("m=video 50000 RTP/AVP 99\r\n"));
        assert!(lines.contains("a=rtpmap:99 H264/90000\r\n"));
        assert!(lines.contains("a=fmtp:99 profile-level-id=42e01f;packetization-mode=1\r\n"));
        assert!(lines.contains("a=sendrecv\r\n"));
    }

    #[test]
    fn video_sdp_round_trips_through_parser() {
        let v = sample_video();
        // Wrap the video lines in a minimal SDP so the parser has session lines.
        let body = format!(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=x\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n{}",
            v.media_lines()
        );
        let parsed = parse_video_sdp(body.as_bytes()).expect("parse video");
        assert_eq!(parsed.media_port, 50000);
        assert_eq!(parsed.payload_type, 99);
        assert_eq!(parsed.encoding, "H264");
        assert_eq!(parsed.direction, MediaDirection::SendRecv);
        assert_eq!(parsed.connection_addr.as_deref(), Some("192.0.2.1"));
        assert_eq!(parsed.addr_type, Some(SdpAddrType::Ip4));
        assert_eq!(
            parsed.fmtp.as_deref(),
            Some("profile-level-id=42e01f;packetization-mode=1")
        );
    }

    #[test]
    fn rejected_video_answer_keeps_media_line_and_marks_it_inactive() {
        let rejected = sample_video().rejected_answer();
        assert!(rejected.is_rejected());
        let lines = rejected.media_lines();
        assert!(lines.contains("m=video 0 RTP/AVP 99\r\n"));
        assert!(lines.contains("a=rtpmap:99 H264/90000\r\n"));
        assert!(lines.contains("a=inactive\r\n"));
    }

    #[test]
    fn media_level_video_connection_overrides_session_address() {
        let body = b"v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=x\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\nm=video 50000 RTP/AVP 99\r\nc=IN IP4 198.51.100.20\r\na=rtpmap:99 H264/90000\r\na=sendrecv\r\n";
        let parsed = parse_video_sdp(body).unwrap();
        assert_eq!(parsed.connection_addr.as_deref(), Some("198.51.100.20"));
        assert_eq!(parsed.addr_type, Some(SdpAddrType::Ip4));
        assert!(parsed.media_lines().contains("c=IN IP4 198.51.100.20\r\n"));
    }

    #[test]
    fn av_sdp_carries_both_audio_and_video_media() {
        let audio = test_audio_offer();
        let video = sample_video();
        let sdp = build_av_sdp(&audio, &video);
        assert!(sdp.contains("m=audio 40000 "));
        assert!(sdp.contains("m=video 50000 "));
        // Audio comes before video (offer extends the voice call).
        let a = sdp.find("m=audio").unwrap();
        let v = sdp.find("m=video").unwrap();
        assert!(a < v);
        // Both media sections parse back out.
        assert!(parse_video_sdp(sdp.as_bytes()).is_ok());
        assert!(crate::connectivity::core::voice::parse_audio_sdp(sdp.as_bytes()).is_ok());
    }

    #[test]
    fn parse_video_sdp_without_video_errors() {
        let audio = test_audio_offer();
        let body = audio.to_sdp();
        assert_eq!(
            parse_video_sdp(body.as_bytes()),
            Err(VideoSdpError::NoVideoMedia)
        );
    }

    #[test]
    fn parse_empty_video_sdp_errors() {
        assert_eq!(parse_video_sdp(b""), Err(VideoSdpError::Empty));
    }

    #[test]
    fn negotiate_video_matches_same_codec_and_adopts_answer_pt() {
        let offer = sample_video();
        let mut answer = sample_video();
        answer.payload_type = 100; // answerer picks its own PT
        answer.media_port = 60000;
        let n = negotiate_video(&offer, &answer).expect("negotiate");
        assert_eq!(n.payload_type, 100);
        assert_eq!(n.encoding, "H264");
        assert_eq!(n.remote_port, 60000);
    }

    #[test]
    fn negotiate_video_rejects_codec_mismatch() {
        let offer = sample_video();
        let mut answer = sample_video();
        answer.encoding = "H265".to_string();
        assert_eq!(
            negotiate_video(&offer, &answer),
            Err(VideoSdpError::NoVideoMedia)
        );
    }

    #[test]
    fn video_relay_forwards_between_legs() {
        use std::net::SocketAddr;
        let operator: SocketAddr = "203.0.113.1:50000".parse().unwrap();
        let internal: SocketAddr = "192.168.1.50:60000".parse().unwrap();
        let mut relay = VideoRelay::new(
            LegEndpoint::new(Some(operator), true),
            LegEndpoint::new(Some(internal), true),
        );
        // A minimal valid RTP v2 header (12 bytes): version=2 in top 2 bits.
        let rtp = [
            0x80, 0x63, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0xAA, 0xBB,
        ];
        let decision = relay
            .ingest(RelayLeg::Operator, operator, &rtp)
            .expect("forward");
        assert_eq!(decision.to, RelayLeg::Internal);
        assert_eq!(decision.dest, internal);
    }

    #[test]
    fn negotiated_trunk_seam_exposes_video_endpoint_or_audio_only() {
        let endpoint = "192.0.2.50:60000".parse().unwrap();
        let video = NegotiatedTrunkVideoSeam::new(Some(endpoint));
        assert_eq!(video.internal_video_endpoint(), Some(endpoint));
        assert!(NegotiatedTrunkVideoSeam::default()
            .internal_video_endpoint()
            .is_none());
    }
}
