//! ViLTE (video telephony over LTE) — offline layer (design doc phase F).
//!
//! Clean-room from public specs: GSMA IR.94 (IMS video profile), 3GPP TS 26.114
//! (media handling), RFC 4566 (SDP), RFC 6184 (RTP payload for H.264), RFC 3550
//! (RTP). No third-party binary source is used.
//!
//! ## What this module does (and does not) do
//!
//! ViLTE is *video added to an ongoing VoLTE voice call*: the SIP INVITE carries
//! one SDP body with **two** media sections — the existing `m=audio` line plus a
//! new `m=video` line (H.264). The SIP envelope (`access::volte::sip::build_invite`)
//! is already SDP-agnostic (it appends the SDP body verbatim), so no SIP change
//! is needed. The work is entirely in the SDP media layer and the media relay.
//!
//! On the target hardware class (no audio/video capture, per design §1.4) the
//! device is a **pure packet relay**: it never encodes or decodes video. It
//! forwards H.264 RTP between the operator IMS media endpoint and an internal
//! SIP UA (Linphone/Asterisk behind the trunk). Therefore this module does not
//! transcode; `codec`/`fmtp` values are what we *advertise*, carried through the
//! offer/answer verbatim.
//!
//! ## Scope (phase F: offline layer + reserved trunk seam)
//!
//! Implemented and unit-tested offline:
//!   - [`VideoMediaDescription`]: an H.264 `m=video` section model + serializer.
//!   - [`build_video_offer`] / [`parse_video_sdp`]: build/parse the video
//!     section from [`VilteConfig`].
//!   - [`build_av_sdp`]: compose an audio [`SdpAudioDescription`] and a video
//!     section into a single multi-line SDP body (the ViLTE offer/answer).
//!   - [`negotiate_video`]: pure video codec/PT matching for the answer.
//!   - [`VideoRelay`]: a second [`RtpRelayCore`] instance dedicated to the video
//!     stream (audio and video are separate RTP flows on separate ports).
//!
//! Reserved for later (needs the Trunk/Asterisk decision — design phase D):
//!   - [`TrunkVideoSeam`]: the interface the trunk bridge will call to wire the
//!     internal-UA video endpoint to the relay. It is defined here so the shape
//!     is stable, but the concrete trunk implementation is deferred.

use crate::access::vowifi::voice::{
    MediaDirection, MediaTransportKind, SdpAddrType, SdpAudioDescription, VoiceEncodingError,
};
use crate::infra::config::VilteConfig;

/// The H.264 RTP clock rate (fixed at 90 kHz for video per RFC 6184).
pub const H264_CLOCK_RATE: u32 = 90_000;

/// The SDP media type token for video.
const MEDIA_VIDEO: &str = "video";

/// A single H.264 `m=video` media section, enough to serialize an RFC 4566
/// video description and to drive the video RTP relay.
///
/// This mirrors [`SdpAudioDescription`] but for the video stream. The two are
/// composed by [`build_av_sdp`] into one SDP body; they share the session-level
/// `o=`/`c=` lines (a call has one connection address) but each has its own
/// `m=` line and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMediaDescription {
    /// Media port for the video RTP stream (distinct from the audio port).
    pub media_port: u16,
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

/// Build a video offer section from the ViLTE config.
pub fn build_video_offer(config: &VilteConfig, media_port: u16) -> VideoMediaDescription {
    VideoMediaDescription {
        media_port,
        transport: MediaTransportKind::RtpAvp,
        payload_type: config.video_payload_type,
        encoding: encoding_for_codec(&config.codec),
        fmtp: if config.h264_fmtp.is_empty() {
            None
        } else {
            Some(config.h264_fmtp.clone())
        },
        direction: MediaDirection::SendRecv,
    }
}

/// Map a configured codec name onto the SDP rtpmap encoding token. Only H.264
/// is a first-class ViLTE codec; anything else is advertised verbatim (upper-
/// cased) since the relay is pass-through and does not care about the payload.
fn encoding_for_codec(codec: &str) -> String {
    match codec.to_ascii_lowercase().as_str() {
        "h264" | "h.264" => "H264".to_string(),
        "h265" | "h.265" | "hevc" => "H265".to_string(),
        other => other.to_ascii_uppercase(),
    }
}

/// Compose a full ViLTE SDP body: the session/audio lines from
/// [`SdpAudioDescription::to_sdp`] followed by the video media section. The
/// result is a valid multi-media SDP (`m=audio` then `m=video`).
///
/// This keeps the audio serialization as the single source of truth (so any
/// audio-side change is inherited) and simply appends the video `m=` block,
/// which is exactly how a ViLTE offer extends a VoLTE voice offer.
pub fn build_av_sdp(audio: &SdpAudioDescription, video: &VideoMediaDescription) -> String {
    let mut sdp = audio.to_sdp();
    sdp.push_str(&video.media_lines());
    sdp
}

/// Parse the **video** media section of an SDP body into a
/// [`VideoMediaDescription`].
///
/// Like [`crate::access::vowifi::voice::parse_audio_sdp`], this is permissive:
/// it walks lines, tracks whether it is inside the `m=video` section, and pulls
/// the port, payload type, `a=rtpmap`, `a=fmtp`, and direction for that section.
/// Returns [`VoiceEncodingError::NoAudioMedia`] repurposed as "no video media"
/// when there is no `m=video` line (kept on the shared error enum to avoid a
/// parallel error type; see [`NoVideoMedia`]).
pub fn parse_video_sdp(body: &[u8]) -> Result<VideoMediaDescription, VideoSdpError> {
    if body.is_empty() {
        return Err(VideoSdpError::Empty);
    }
    let text = std::str::from_utf8(body).map_err(|_| VideoSdpError::Malformed)?;

    let mut media_port = 0u16;
    let mut transport = MediaTransportKind::RtpAvp;
    let mut payload_order: Vec<u8> = Vec::new();
    let mut direction = MediaDirection::SendRecv;
    let mut encoding: Option<(u8, String)> = None;
    let mut fmtp: Option<(u8, String)> = None;
    let mut in_video = false;
    let mut saw_video = false;

    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim_end();
        if line.is_empty() {
            continue;
        }
        let Some((kind, value)) = line.split_once('=') else {
            continue;
        };
        match kind {
            "m" => {
                let parts = value.split_whitespace().collect::<Vec<_>>();
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

    Ok(VideoMediaDescription {
        media_port,
        transport,
        payload_type,
        encoding: enc_name,
        fmtp,
        direction,
    })
}

/// Errors from ViLTE video SDP handling.
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
            Self::Empty => "vilte_sdp_empty",
            Self::Malformed => "vilte_sdp_malformed",
            Self::NoVideoMedia => "vilte_sdp_no_video_media",
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

pub use crate::access::volte::rtp_relay::{
    ForwardDecision, LegEndpoint, RelayError, RelayLeg, RtpRelayCore,
};

/// A dedicated relay for the ViLTE **video** stream.
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
// Reserved trunk seam (design phase D is undecided; keep the shape stable)
// ---------------------------------------------------------------------------

/// The video-side attachment point the Trunk bridge will implement once the
/// SIP-endpoint/Asterisk strategy is chosen (design phase D). It intentionally
/// carries no behavior yet — it just fixes the contract so ViLTE wiring can be
/// added later without reshaping this module.
///
/// The intended flow: when a ViLTE call is set up, the trunk bridge supplies the
/// internal UA's negotiated video endpoint via [`internal_video_endpoint`], and
/// the orchestrator builds a [`VideoRelay`] pairing it with the operator side.
pub trait TrunkVideoSeam {
    /// The internal SIP UA's negotiated video media endpoint (addr:port), or
    /// `None` if the UA declined video for this call.
    fn internal_video_endpoint(&self) -> Option<std::net::SocketAddr>;
}

/// A placeholder trunk seam used until the real trunk bridge lands. It always
/// reports "no internal video endpoint", so ViLTE stays inert (relay not built)
/// when no trunk is wired — matching the requirement that the host device never
/// terminates calls on its own.
#[derive(Debug, Clone, Default)]
pub struct UnwiredTrunkVideoSeam;

impl TrunkVideoSeam for UnwiredTrunkVideoSeam {
    fn internal_video_endpoint(&self) -> Option<std::net::SocketAddr> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::vowifi::profiles::GB_EE_23433;
    use crate::access::vowifi::voice::{build_mo_audio_offer, SdpAddrType};

    fn config() -> VilteConfig {
        VilteConfig::default()
    }

    fn sample_video() -> VideoMediaDescription {
        build_video_offer(&config(), 50000)
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
        assert_eq!(
            parsed.fmtp.as_deref(),
            Some("profile-level-id=42e01f;packetization-mode=1")
        );
    }

    #[test]
    fn av_sdp_carries_both_audio_and_video_media() {
        let audio = build_mo_audio_offer(&GB_EE_23433, "192.0.2.10", SdpAddrType::Ip4, 40000);
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
        assert!(crate::access::vowifi::voice::parse_audio_sdp(sdp.as_bytes()).is_ok());
    }

    #[test]
    fn parse_video_sdp_without_video_errors() {
        let audio = build_mo_audio_offer(&GB_EE_23433, "192.0.2.10", SdpAddrType::Ip4, 40000);
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
    fn unwired_trunk_seam_reports_no_video_endpoint() {
        let seam = UnwiredTrunkVideoSeam;
        assert!(seam.internal_video_endpoint().is_none());
    }
}
