//! Clean-room VoWiFi voice-calling planning, state machine, SDP and RTP codecs.
//!
//! This module mirrors the split used by `sms.rs`: it holds the pure,
//! offline-testable pieces of the voice path (call state machine, SDP
//! offer/answer builder + parser, AMR/AMR-WB RTP payload framing) while the
//! real network I/O (INVITE over the ESP-protected IMS route, RTP media loop)
//! lives in `live.rs`.
//!
//! It intentionally contains only SimAdmin-owned names and data structures.
//! Sensitive values (phone numbers, SDP bodies, raw RTP payloads, media keys)
//! are never serialized; every public state type carries an explicit
//! `sensitive_values_policy` marker just like the SMS module.
//!
//! The audio plane is deliberately abstracted behind traits (`AudioSource`,
//! `AudioSink`, `CarrierVoiceLeg`, `SipEndpointBridge`) so that later work can
//! plug in real media (VoWiFi AMR-over-RTP, carrier AT + USB-Audio PCM) and
//! expose a standard SIP endpoint per SIM without touching the state machine.

#![allow(dead_code)]

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::profiles::CarrierProfile;

static VOICE_CALL_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Allocate a monotonic call sequence used to derive trace/message ids.
fn next_call_sequence() -> u64 {
    VOICE_CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Neutral voice parameters (transport-agnostic)
// ---------------------------------------------------------------------------

/// Transport-agnostic voice policy parameters.
///
/// This is the small slice of a carrier/service profile that the pure voice
/// signaling layer actually needs (codec preference, ptime, AMR framing, leg
/// availability, SIP transport token, and identity labels for snapshots).
///
/// Extracting it lets both the VoWiFi path (which builds it from a
/// `&'static CarrierProfile`) and the VoLTE path (which builds it from its own
/// `VolteConfig`/runtime) reuse the exact same SDP/state-machine logic without
/// depending on the VoWiFi-specific `CarrierProfile` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceParams {
    /// Preferred codec tokens in priority order ("amr", "amr-wb", "pcmu", "pcma").
    pub preferred_codecs: Vec<String>,
    /// Packetization time (ms) advertised in the SDP offer.
    pub ptime_ms: u16,
    /// Whether AMR payloads are offered octet-aligned (`octet-align=1`).
    pub amr_octet_align: bool,
    /// Whether the VoWiFi voice leg is enabled for this profile.
    pub vowifi_enabled: bool,
    /// Whether the operator (AT + USB-Audio) fallback leg may be attempted.
    pub carrier_fallback_enabled: bool,
    /// SIP transport token surfaced in call summaries ("tcp" / "udp").
    pub ims_transport: &'static str,
    /// Stable profile id label for snapshots.
    pub profile_id: &'static str,
    /// PLMN label for snapshots.
    pub plmn: &'static str,
}

impl VoiceParams {
    /// Build the neutral params from a VoWiFi carrier profile (adapter used by
    /// the existing VoWiFi entry points; keeps their behavior identical).
    pub fn from_carrier_profile(profile: &'static CarrierProfile) -> Self {
        Self {
            preferred_codecs: profile
                .voice
                .preferred_codecs
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ptime_ms: profile.voice.ptime_ms,
            amr_octet_align: profile.voice.amr_octet_align,
            vowifi_enabled: profile.voice.vowifi_enabled,
            carrier_fallback_enabled: profile.voice.carrier_fallback_enabled,
            ims_transport: profile.ims.transport,
            profile_id: profile.meta.profile_id,
            plmn: profile.meta.plmn,
        }
    }
}

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

/// Which side originated the call leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallDirection {
    /// SimAdmin/user side dialed out (INVITE sent).
    MobileOriginated,
    /// Network delivered an inbound INVITE.
    MobileTerminated,
}

impl CallDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MobileOriginated => "mobile_originated",
            Self::MobileTerminated => "mobile_terminated",
        }
    }
}

/// SIP INVITE transaction state (control-plane view of the dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SipInviteState {
    /// Nothing sent/received yet.
    Idle,
    /// INVITE queued locally, not yet on the wire.
    Queued,
    /// INVITE sent (MO) or received (MT); awaiting a response/answer.
    InviteSent,
    /// 1xx provisional received (100 Trying / 180 Ringing).
    Ringing,
    /// 183 Session Progress with early media SDP answer.
    EarlyMedia,
    /// 200 OK received and ACK sent 锟?dialog confirmed.
    Confirmed,
    /// BYE exchanged, dialog terminated normally.
    Terminated,
    /// 4xx/5xx/6xx final response, CANCEL, or timeout.
    Failed,
}

impl SipInviteState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::InviteSent => "invite_sent",
            Self::Ringing => "ringing",
            Self::EarlyMedia => "early_media",
            Self::Confirmed => "confirmed",
            Self::Terminated => "terminated",
            Self::Failed => "failed",
        }
    }
}

/// Aggregate, user/API-facing call lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    /// Dialing (INVITE queued/sent, no provisional yet).
    Dialing,
    /// Remote is ringing (180) or early media (183).
    Ringing,
    /// Call answered and media is (or should be) flowing.
    Active,
    /// Call finished cleanly (either side hung up).
    Ended,
    /// Call setup or media failed.
    Failed,
}

impl CallState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dialing => "dialing",
            Self::Ringing => "ringing",
            Self::Active => "active",
            Self::Ended => "ended",
            Self::Failed => "failed",
        }
    }

    /// Coarse status used by the public HTTP API.
    pub fn api_status(self) -> &'static str {
        match self {
            Self::Dialing => "dialing",
            Self::Ringing => "ringing",
            Self::Active => "active",
            Self::Ended => "ended",
            Self::Failed => "failed",
        }
    }
}

/// Which voice leg is carrying (or would carry) the media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceLegKind {
    /// Self-authored VoWiFi leg: INVITE/SDP/RTP over the ePDG ESP tunnel.
    Vowifi,
    /// Operator leg driven by AT commands (ATD/ATA/CLCC) + USB audio PCM.
    Carrier,
    /// No leg currently selected.
    None,
}

impl VoiceLegKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vowifi => "vowifi",
            Self::Carrier => "carrier",
            Self::None => "none",
        }
    }
}

/// Media transport for the RTP flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaTransportKind {
    /// RTP/AVP over UDP (standard, and what the VoWiFi ESP inner stack carries).
    RtpAvp,
    /// Placeholder for a future SRTP profile.
    RtpSavp,
}

impl MediaTransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RtpAvp => "rtp_avp",
            Self::RtpSavp => "rtp_savp",
        }
    }

    pub fn sdp_proto(self) -> &'static str {
        match self {
            Self::RtpAvp => "RTP/AVP",
            Self::RtpSavp => "RTP/SAVP",
        }
    }
}

/// Audio codec negotiated (or planned) for the media session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    /// AMR narrowband (3GPP TS 26.071), RTP payload per RFC 4867.
    Amr,
    /// AMR wideband (3GPP TS 26.171), RTP payload per RFC 4867.
    AmrWb,
    /// G.711 mu-law (PCMU), static payload type 0.
    Pcmu,
    /// G.711 a-law (PCMA), static payload type 8.
    Pcma,
}

impl AudioCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amr => "amr",
            Self::AmrWb => "amr_wb",
            Self::Pcmu => "pcmu",
            Self::Pcma => "pcma",
        }
    }

    /// The `a=rtpmap` encoding name for this codec.
    pub fn rtpmap_encoding(self) -> &'static str {
        match self {
            Self::Amr => "AMR",
            Self::AmrWb => "AMR-WB",
            Self::Pcmu => "PCMU",
            Self::Pcma => "PCMA",
        }
    }

    /// RTP clock rate in Hz.
    pub fn clock_rate(self) -> u32 {
        match self {
            Self::Amr | Self::Pcmu | Self::Pcma => 8000,
            Self::AmrWb => 16000,
        }
    }

    /// Static RTP payload type, when the codec has one (dynamic 锟?None).
    pub fn static_payload_type(self) -> Option<u8> {
        match self {
            Self::Pcmu => Some(0),
            Self::Pcma => Some(8),
            Self::Amr | Self::AmrWb => None,
        }
    }

    /// Whether this is one of the AMR family codecs (bandwidth-efficient
    /// framing, dynamic payload type).
    pub fn is_amr_family(self) -> bool {
        matches!(self, Self::Amr | Self::AmrWb)
    }

    fn from_token(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "amr" => Some(Self::Amr),
            "amr-wb" | "amr_wb" | "amrwb" => Some(Self::AmrWb),
            "pcmu" => Some(Self::Pcmu),
            "pcma" => Some(Self::Pcma),
            _ => None,
        }
    }
}

/// Reason a call ended or failed, kept coarse so no sensitive detail leaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallEndReason {
    LocalHangup,
    RemoteHangup,
    RemoteBusy,
    NoAnswer,
    Declined,
    NetworkFailure,
    MediaFailure,
    Canceled,
}

impl CallEndReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalHangup => "local_hangup",
            Self::RemoteHangup => "remote_hangup",
            Self::RemoteBusy => "remote_busy",
            Self::NoAnswer => "no_answer",
            Self::Declined => "declined",
            Self::NetworkFailure => "network_failure",
            Self::MediaFailure => "media_failure",
            Self::Canceled => "canceled",
        }
    }

    /// Map a SIP final status code onto a coarse end reason.
    pub fn from_sip_status(status: u16) -> Self {
        match status {
            486 | 600 => Self::RemoteBusy,
            408 | 480 => Self::NoAnswer,
            603 => Self::Declined,
            487 => Self::Canceled,
            _ => Self::NetworkFailure,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceRuntimeError {
    InvalidTransition {
        from: &'static str,
        event: &'static str,
    },
    SipRejected(u16),
    NoCommonCodec,
    InconsistentState(&'static str),
}

impl std::fmt::Display for VoiceRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTransition { from, event } => {
                write!(f, "invalid voice transition from={from} event={event}")
            }
            Self::SipRejected(code) => write!(f, "SIP INVITE rejected code={code}"),
            Self::NoCommonCodec => write!(f, "no common audio codec in SDP answer"),
            Self::InconsistentState(reason) => write!(f, "inconsistent voice state: {reason}"),
        }
    }
}

impl std::error::Error for VoiceRuntimeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEncodingError {
    EmptyCallee,
    InvalidAddress,
    EmptySdp,
    SdpMalformed,
    NoAudioMedia,
    UnsupportedCodec,
}

impl std::fmt::Display for VoiceEncodingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::EmptyCallee => "voice_callee_empty",
            Self::InvalidAddress => "voice_address_invalid",
            Self::EmptySdp => "voice_sdp_empty",
            Self::SdpMalformed => "voice_sdp_malformed",
            Self::NoAudioMedia => "voice_sdp_no_audio_media",
            Self::UnsupportedCodec => "voice_sdp_unsupported_codec",
        };
        write!(f, "{reason}")
    }
}

impl std::error::Error for VoiceEncodingError {}

// ---------------------------------------------------------------------------
// SDP model, builder and parser (pure)
// ---------------------------------------------------------------------------

/// A single negotiated/offered audio codec with its dynamic payload type and
/// codec-specific fmtp parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpCodec {
    pub codec: AudioCodec,
    pub payload_type: u8,
    /// Extra `a=fmtp` parameters (e.g. `mode-set`, `octet-align`), verbatim.
    pub fmtp: Option<String>,
}

impl SdpCodec {
    fn rtpmap_line(&self) -> String {
        format!(
            "a=rtpmap:{} {}/{}\r\n",
            self.payload_type,
            self.codec.rtpmap_encoding(),
            self.codec.clock_rate()
        )
    }

    fn fmtp_line(&self) -> String {
        match &self.fmtp {
            Some(params) if !params.is_empty() => {
                format!("a=fmtp:{} {}\r\n", self.payload_type, params)
            }
            _ => String::new(),
        }
    }
}

/// Direction attribute of the media stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaDirection {
    SendRecv,
    SendOnly,
    RecvOnly,
    Inactive,
}

impl MediaDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SendRecv => "sendrecv",
            Self::SendOnly => "sendonly",
            Self::RecvOnly => "recvonly",
            Self::Inactive => "inactive",
        }
    }

    fn sdp_attr(self) -> &'static str {
        self.as_str()
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "sendrecv" => Some(Self::SendRecv),
            "sendonly" => Some(Self::SendOnly),
            "recvonly" => Some(Self::RecvOnly),
            "inactive" => Some(Self::Inactive),
            _ => None,
        }
    }
}

/// Whether the connection address is IPv4 or IPv6 (drives the SDP `c=` line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdpAddrType {
    Ip4,
    Ip6,
}

impl SdpAddrType {
    fn network_type(self) -> &'static str {
        "IN"
    }
    fn addr_type(self) -> &'static str {
        match self {
            Self::Ip4 => "IP4",
            Self::Ip6 => "IP6",
        }
    }
}

/// A fully described audio offer/answer, enough to build an RFC 4566 SDP body
/// and to drive the RTP media loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpAudioDescription {
    pub session_id: u64,
    pub session_version: u64,
    pub origin_username: String,
    pub connection_addr: String,
    pub addr_type: SdpAddrType,
    pub media_port: u16,
    pub transport: MediaTransportKind,
    pub codecs: Vec<SdpCodec>,
    pub direction: MediaDirection,
    /// ptime (ms), if advertised.
    pub ptime: Option<u16>,
}

impl SdpAudioDescription {
    /// Serialize this description into an RFC 4566 SDP body.
    pub fn to_sdp(&self) -> String {
        let mut sdp = String::new();
        sdp.push_str("v=0\r\n");
        sdp.push_str(&format!(
            "o={} {} {} {} {} {}\r\n",
            self.origin_username,
            self.session_id,
            self.session_version,
            self.addr_type.network_type(),
            self.addr_type.addr_type(),
            self.connection_addr,
        ));
        sdp.push_str("s=SimAdmin VoWiFi Call\r\n");
        sdp.push_str(&format!(
            "c={} {} {}\r\n",
            self.addr_type.network_type(),
            self.addr_type.addr_type(),
            self.connection_addr,
        ));
        sdp.push_str("t=0 0\r\n");

        let payload_types = self
            .codecs
            .iter()
            .map(|codec| codec.payload_type.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        sdp.push_str(&format!(
            "m=audio {} {} {}\r\n",
            self.media_port,
            self.transport.sdp_proto(),
            payload_types,
        ));
        for codec in &self.codecs {
            sdp.push_str(&codec.rtpmap_line());
            sdp.push_str(&codec.fmtp_line());
        }
        if let Some(ptime) = self.ptime {
            sdp.push_str(&format!("a=ptime:{ptime}\r\n"));
        }
        sdp.push_str(&format!("a={}\r\n", self.direction.sdp_attr()));
        sdp
    }

    /// The list of codecs both sides support, in this description's preference
    /// order, intersected with `remote`.
    pub fn common_codecs(&self, remote: &SdpAudioDescription) -> Vec<AudioCodec> {
        self.codecs
            .iter()
            .filter(|local| {
                remote
                    .codecs
                    .iter()
                    .any(|other| other.codec == local.codec)
            })
            .map(|local| local.codec)
            .collect()
    }
}

/// Parse the audio media of an SDP body into an [`SdpAudioDescription`].
///
/// This is deliberately permissive: it extracts the `c=`, `m=audio`,
/// `a=rtpmap`, `a=fmtp`, `a=ptime` and direction attributes needed to drive an
/// AMR/G.711 RTP flow, and ignores everything else.
pub fn parse_audio_sdp(body: &[u8]) -> Result<SdpAudioDescription, VoiceEncodingError> {
    if body.is_empty() {
        return Err(VoiceEncodingError::EmptySdp);
    }
    let text = std::str::from_utf8(body).map_err(|_| VoiceEncodingError::SdpMalformed)?;

    let mut origin_username = String::from("-");
    let mut session_id = 0u64;
    let mut session_version = 0u64;
    let mut connection_addr = String::new();
    let mut addr_type = SdpAddrType::Ip4;
    let mut media_port = 0u16;
    let mut transport = MediaTransportKind::RtpAvp;
    let mut payload_order: Vec<u8> = Vec::new();
    let mut direction = MediaDirection::SendRecv;
    let mut ptime: Option<u16> = None;
    let mut rtpmaps: Vec<(u8, AudioCodec)> = Vec::new();
    let mut fmtps: Vec<(u8, String)> = Vec::new();
    let mut in_audio_media = false;
    let mut saw_audio_media = false;

    for raw_line in text.split(|ch| ch == '\n') {
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
                    origin_username = parts[0].to_string();
                    session_id = parts[1].parse().unwrap_or(0);
                    session_version = parts[2].parse().unwrap_or(0);
                    addr_type = if parts[5].contains("IP6") {
                        SdpAddrType::Ip6
                    } else {
                        SdpAddrType::Ip4
                    };
                    connection_addr = parts[5].to_string();
                }
            }
            "c" => {
                let parts = value.split_whitespace().collect::<Vec<_>>();
                if parts.len() >= 3 {
                    addr_type = if parts[1].eq_ignore_ascii_case("IP6") {
                        SdpAddrType::Ip6
                    } else {
                        SdpAddrType::Ip4
                    };
                    connection_addr = parts[2].to_string();
                }
            }
            "m" => {
                // m=<media> <port> <proto> <fmt list>
                let parts = value.split_whitespace().collect::<Vec<_>>();
                in_audio_media = parts.first().copied() == Some("audio");
                if in_audio_media {
                    saw_audio_media = true;
                    if parts.len() >= 2 {
                        media_port = parts[1].parse().unwrap_or(0);
                    }
                    if parts.len() >= 3 {
                        transport = if parts[2].to_ascii_uppercase().contains("SAVP") {
                            MediaTransportKind::RtpSavp
                        } else {
                            MediaTransportKind::RtpAvp
                        };
                    }
                    for token in parts.iter().skip(3) {
                        if let Ok(pt) = token.parse::<u8>() {
                            payload_order.push(pt);
                            // seed static codecs so PCMU/PCMA resolve without rtpmap
                            if pt == 0 {
                                rtpmaps.push((0, AudioCodec::Pcmu));
                            } else if pt == 8 {
                                rtpmaps.push((8, AudioCodec::Pcma));
                            }
                        }
                    }
                }
            }
            "a" if in_audio_media => {
                if let Some(rtpmap) = value.strip_prefix("rtpmap:") {
                    // <pt> <encoding>/<clock>[/<channels>]
                    let mut iter = rtpmap.split_whitespace();
                    if let (Some(pt_str), Some(enc)) = (iter.next(), iter.next()) {
                        if let Ok(pt) = pt_str.parse::<u8>() {
                            let encoding = enc.split('/').next().unwrap_or("");
                            if let Some(codec) = AudioCodec::from_token(encoding) {
                                rtpmaps.retain(|(existing, _)| *existing != pt);
                                rtpmaps.push((pt, codec));
                            }
                        }
                    }
                } else if let Some(fmtp) = value.strip_prefix("fmtp:") {
                    let mut iter = fmtp.splitn(2, char::is_whitespace);
                    if let (Some(pt_str), Some(params)) = (iter.next(), iter.next()) {
                        if let Ok(pt) = pt_str.parse::<u8>() {
                            fmtps.push((pt, params.trim().to_string()));
                        }
                    }
                } else if let Some(pt) = value.strip_prefix("ptime:") {
                    ptime = pt.trim().parse::<u16>().ok();
                } else if let Some(found) = MediaDirection::from_token(value.trim()) {
                    direction = found;
                }
            }
            _ => {}
        }
    }

    if !saw_audio_media {
        return Err(VoiceEncodingError::NoAudioMedia);
    }

    // Assemble codecs following the m= payload order, using rtpmap when present.
    let mut codecs = Vec::new();
    for pt in &payload_order {
        if let Some((_, codec)) = rtpmaps.iter().find(|(mapped, _)| mapped == pt) {
            let fmtp = fmtps
                .iter()
                .find(|(mapped, _)| mapped == pt)
                .map(|(_, params)| params.clone());
            codecs.push(SdpCodec {
                codec: *codec,
                payload_type: *pt,
                fmtp,
            });
        }
    }

    if codecs.is_empty() {
        return Err(VoiceEncodingError::UnsupportedCodec);
    }

    Ok(SdpAudioDescription {
        session_id,
        session_version,
        origin_username,
        connection_addr,
        addr_type,
        media_port,
        transport,
        codecs,
        direction,
        ptime,
    })
}

/// Build a standard MO audio offer from a carrier profile's voice policy plus
/// the locally chosen media address/port. The offer advertises the profile's
/// preferred codecs in order.
pub fn build_mo_audio_offer(
    profile: &'static CarrierProfile,
    connection_addr: &str,
    addr_type: SdpAddrType,
    media_port: u16,
) -> SdpAudioDescription {
    build_mo_audio_offer_with_params(
        &VoiceParams::from_carrier_profile(profile),
        connection_addr,
        addr_type,
        media_port,
    )
}

/// Params-driven variant of [`build_mo_audio_offer`] (transport-agnostic).
pub fn build_mo_audio_offer_with_params(
    params: &VoiceParams,
    connection_addr: &str,
    addr_type: SdpAddrType,
    media_port: u16,
) -> SdpAudioDescription {
    let sequence = next_call_sequence();
    let base = unix_millis() as u64;
    let codecs = build_codec_offer_with_params(params);
    SdpAudioDescription {
        session_id: base.wrapping_add(sequence),
        session_version: base.wrapping_add(sequence),
        origin_username: "SimAdmin".to_string(),
        connection_addr: connection_addr.to_string(),
        addr_type,
        media_port,
        transport: MediaTransportKind::RtpAvp,
        codecs,
        direction: MediaDirection::SendRecv,
        ptime: Some(params.ptime_ms),
    }
}

/// Produce the profile's preferred codec offer list, assigning dynamic payload
/// types to the AMR family (96/97) while keeping static types for G.711.
pub fn build_profile_codec_offer(profile: &'static CarrierProfile) -> Vec<SdpCodec> {
    build_codec_offer_with_params(&VoiceParams::from_carrier_profile(profile))
}

/// Params-driven variant of [`build_profile_codec_offer`] (transport-agnostic).
pub fn build_codec_offer_with_params(params: &VoiceParams) -> Vec<SdpCodec> {
    let mut codecs = Vec::new();
    let mut next_dynamic_pt = 96u8;
    for token in &params.preferred_codecs {
        let Some(codec) = AudioCodec::from_token(token) else {
            continue;
        };
        let payload_type = match codec.static_payload_type() {
            Some(pt) => pt,
            None => {
                let pt = next_dynamic_pt;
                next_dynamic_pt = next_dynamic_pt.saturating_add(1);
                pt
            }
        };
        let fmtp = amr_default_fmtp(codec, params.amr_octet_align);
        codecs.push(SdpCodec {
            codec,
            payload_type,
            fmtp,
        });
    }
    codecs
}

/// Default AMR fmtp parameters for a codec, driven by the octet-align
/// preference. Non-AMR codecs get no fmtp.
fn amr_default_fmtp(codec: AudioCodec, amr_octet_align: bool) -> Option<String> {
    if !codec.is_amr_family() {
        return None;
    }
    let octet_align = if amr_octet_align { 1 } else { 0 };
    // mode-set left open (all modes) by default; align + mode-change-capability
    // reflect a typical VoLTE/VoWiFi offer.
    Some(format!(
        "octet-align={octet_align}; mode-change-capability=2; max-red=0"
    ))
}

/// Build an SDP answer by intersecting a received offer with the profile's
/// preferred codecs and binding it to our local media address/port.
pub fn build_sdp_answer(
    profile: &'static CarrierProfile,
    offer: &SdpAudioDescription,
    connection_addr: &str,
    addr_type: SdpAddrType,
    media_port: u16,
) -> Result<SdpAudioDescription, VoiceRuntimeError> {
    build_sdp_answer_with_params(
        &VoiceParams::from_carrier_profile(profile),
        offer,
        connection_addr,
        addr_type,
        media_port,
    )
}

/// Params-driven variant of [`build_sdp_answer`] (transport-agnostic).
pub fn build_sdp_answer_with_params(
    params: &VoiceParams,
    offer: &SdpAudioDescription,
    connection_addr: &str,
    addr_type: SdpAddrType,
    media_port: u16,
) -> Result<SdpAudioDescription, VoiceRuntimeError> {
    let local_offer = build_codec_offer_with_params(params);
    // Keep the offer's payload types (the offerer owns the numbering), but only
    // for codecs we also support, honoring the offerer's preference order.
    let mut answer_codecs = Vec::new();
    for offered in &offer.codecs {
        if local_offer
            .iter()
            .any(|local| local.codec == offered.codec)
        {
            answer_codecs.push(offered.clone());
        }
    }
    if answer_codecs.is_empty() {
        return Err(VoiceRuntimeError::NoCommonCodec);
    }
    let sequence = next_call_sequence();
    let base = unix_millis() as u64;
    Ok(SdpAudioDescription {
        session_id: base.wrapping_add(sequence),
        session_version: base.wrapping_add(sequence),
        origin_username: "SimAdmin".to_string(),
        connection_addr: connection_addr.to_string(),
        addr_type,
        media_port,
        transport: offer.transport,
        codecs: answer_codecs,
        direction: MediaDirection::SendRecv,
        ptime: offer.ptime.or(Some(params.ptime_ms)),
    })
}

// ---------------------------------------------------------------------------
// RTP packet framing (pure)
// ---------------------------------------------------------------------------

/// A parsed/parseable RTP header + payload (RFC 3550, no extensions/CSRC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RtpPacket {
    pub payload_type: u8,
    pub marker: bool,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Vec<u8>,
}

impl RtpPacket {
    /// Encode this packet into wire bytes (12-byte header + payload).
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + self.payload.len());
        // V=2, P=0, X=0, CC=0
        buf.push(0x80);
        // M + PT
        let mt = if self.marker { 0x80 } else { 0x00 };
        buf.push(mt | (self.payload_type & 0x7f));
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf.extend_from_slice(&self.ssrc.to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Parse wire bytes into an [`RtpPacket`], skipping CSRC and extension
    /// headers if present.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 12 {
            return None;
        }
        let version = bytes[0] >> 6;
        if version != 2 {
            return None;
        }
        let has_padding = bytes[0] & 0x20 != 0;
        let has_extension = bytes[0] & 0x10 != 0;
        let csrc_count = (bytes[0] & 0x0f) as usize;
        let marker = bytes[1] & 0x80 != 0;
        let payload_type = bytes[1] & 0x7f;
        let sequence = u16::from_be_bytes([bytes[2], bytes[3]]);
        let timestamp = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let ssrc = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);

        let mut offset = 12 + csrc_count * 4;
        if offset > bytes.len() {
            return None;
        }
        if has_extension {
            if offset + 4 > bytes.len() {
                return None;
            }
            let ext_len = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]) as usize;
            offset += 4 + ext_len * 4;
            if offset > bytes.len() {
                return None;
            }
        }
        let mut payload_end = bytes.len();
        if has_padding && payload_end > offset {
            let pad = bytes[payload_end - 1] as usize;
            if pad <= payload_end - offset {
                payload_end -= pad;
            }
        }
        Some(Self {
            payload_type,
            marker,
            sequence,
            timestamp,
            ssrc,
            payload: bytes[offset..payload_end].to_vec(),
        })
    }
}

/// Wrap an AMR/AMR-WB speech frame into an RTP payload using the
/// bandwidth-efficient single-frame layout (RFC 4867 搂4.3), given the frame
/// type index (FT) and the raw speech bits.
///
/// This is the minimal framing needed to carry one speech frame per packet;
/// multi-frame aggregation and CRC are intentionally out of scope for the
/// reserved media path.
pub fn build_amr_rtp_payload(frame_type: u8, speech_bits: &[u8], octet_aligned: bool) -> Vec<u8> {
    if octet_aligned {
        // Octet-aligned: CMR byte, then TOC byte (F=0, FT, Q=1), then payload.
        let mut payload = Vec::with_capacity(2 + speech_bits.len());
        payload.push(0xf0); // CMR = 15 (no mode request), padding bits zero
        let toc = ((frame_type & 0x0f) << 3) | 0x04; // F=0, FT, Q=1
        payload.push(toc);
        payload.extend_from_slice(speech_bits);
        payload
    } else {
        // Bandwidth-efficient: 4-bit CMR + 6-bit ToC (F,FT,Q) packed, then bits.
        // For a single frame this is a common simplification used for planning;
        // full bit-packing is deferred to the live media implementation.
        let mut payload = Vec::with_capacity(2 + speech_bits.len());
        // CMR(4)=15, then F(1)=0 FT(4) Q(1)=1 spread across the next byte.
        payload.push(0xf0 | ((frame_type >> 1) & 0x0f));
        payload.push(((frame_type & 0x01) << 7) | 0x40);
        payload.extend_from_slice(speech_bits);
        payload
    }
}

/// Extract the AMR frame-type index (FT) from an RTP payload built as above.
pub fn parse_amr_frame_type(payload: &[u8], octet_aligned: bool) -> Option<u8> {
    if octet_aligned {
        payload.get(1).map(|toc| (toc >> 3) & 0x0f)
    } else {
        if payload.len() < 2 {
            return None;
        }
        let high = payload[0] & 0x0f; // top 4 bits of FT (after CMR nibble)
        let low = (payload[1] >> 7) & 0x01;
        Some((high << 1) | low)
    }
}

// ---------------------------------------------------------------------------
// Wire I/O DTOs (used by live.rs)
// ---------------------------------------------------------------------------

/// An outbound INVITE ready to be framed into a SIP request by `live.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoCallInvite {
    pub trace_id: String,
    pub call_id: String,
    pub callee: String,
    pub sdp_offer: Vec<u8>,
    pub sdp_bytes: usize,
    pub offered_codecs: Vec<AudioCodec>,
}

/// The synchronous outcome of an INVITE send, before the dialog completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoCallSipOutcome {
    pub trace_id: String,
    pub call_id: String,
    pub sip_status: u16,
    pub invite_state: SipInviteState,
    pub call_state: CallState,
    pub negotiated_codec: Option<AudioCodec>,
    pub failure_cause: Option<String>,
}

impl MoCallSipOutcome {
    pub fn api_status(&self) -> &'static str {
        self.call_state.api_status()
    }
}

/// An inbound INVITE observed by the media/dialog loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtCallInvite {
    pub call_id: String,
    pub caller: String,
    pub sdp_offer_bytes: usize,
    pub offered_codecs: Vec<AudioCodec>,
}

// ---------------------------------------------------------------------------
// Public serializable state (never carries secrets)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallSdpSummary {
    pub role: &'static str,
    pub media_transport: &'static str,
    pub codec_count: usize,
    pub negotiated_codec: Option<&'static str>,
    pub direction: &'static str,
    pub ptime_ms: Option<u16>,
    pub sdp_bytes: usize,
    pub values_redacted: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallSipSummary {
    pub direction: &'static str,
    pub method: &'static str,
    pub transport: &'static str,
    pub invite_state: &'static str,
    pub sip_status: Option<u16>,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallMediaSummary {
    pub leg: &'static str,
    pub media_transport: &'static str,
    pub negotiated_codec: Option<&'static str>,
    pub clock_rate: Option<u32>,
    pub rtp_packets_sent: u64,
    pub rtp_packets_received: u64,
    pub audio_source_bound: bool,
    pub audio_sink_bound: bool,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallRecord {
    pub trace_id: String,
    pub call_id: String,
    pub direction: CallDirection,
    pub invite_state: SipInviteState,
    pub call_state: CallState,
    pub leg: VoiceLegKind,
    pub negotiated_codec: Option<AudioCodec>,
    pub end_reason: Option<CallEndReason>,
    pub failure_cause: Option<String>,
    pub retry_count: u8,
    pub rtp_packets_sent: u64,
    pub rtp_packets_received: u64,
}

impl CallRecord {
    pub fn api_status(&self) -> &'static str {
        self.call_state.api_status()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallPublicRecord {
    pub trace_id: String,
    pub call_id: String,
    pub direction: &'static str,
    pub call_state: &'static str,
    pub api_status: &'static str,
    pub invite_state: &'static str,
    pub leg: &'static str,
    pub negotiated_codec: Option<&'static str>,
    pub end_reason: Option<&'static str>,
    pub failure_cause: Option<String>,
    pub retry_count: u8,
    pub rtp_packets_sent: u64,
    pub rtp_packets_received: u64,
    pub db_fact_source: &'static str,
    pub sensitive_values_policy: &'static str,
}

impl CallPublicRecord {
    pub fn from_record(record: &CallRecord) -> Self {
        Self {
            trace_id: record.trace_id.clone(),
            call_id: record.call_id.clone(),
            direction: record.direction.as_str(),
            call_state: record.call_state.as_str(),
            api_status: record.api_status(),
            invite_state: record.invite_state.as_str(),
            leg: record.leg.as_str(),
            negotiated_codec: record.negotiated_codec.map(AudioCodec::as_str),
            end_reason: record.end_reason.map(CallEndReason::as_str),
            failure_cause: record.failure_cause.clone(),
            retry_count: record.retry_count,
            rtp_packets_sent: record.rtp_packets_sent,
            rtp_packets_received: record.rtp_packets_received,
            db_fact_source: "vowifi_voice_call",
            sensitive_values_policy: "phone_numbers_sdp_body_and_rtp_media_not_serialized",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VoiceRuntimePublicState {
    pub profile_id: &'static str,
    pub plmn: &'static str,
    pub voice_ready: bool,
    pub vowifi_voice_enabled: bool,
    pub carrier_leg_available: bool,
    pub preferred_leg: &'static str,
    pub media_transport: &'static str,
    pub preferred_codec: Option<&'static str>,
    pub active_call: CallPublicRecord,
    pub last_sip: Option<CallSipSummary>,
    pub last_sdp: Option<CallSdpSummary>,
    pub last_media: Option<CallMediaSummary>,
    pub sip_endpoint_exposed: bool,
    pub state_consistency_policy: &'static str,
    pub sensitive_values_policy: &'static str,
}

// ---------------------------------------------------------------------------
// Call state machine (pure, offline testable)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceCallStateMachine {
    params: VoiceParams,
    media_transport: MediaTransportKind,
    vowifi_voice_enabled: bool,
    carrier_leg_available: bool,
    preferred_leg: VoiceLegKind,
    reg_ready: bool,
    call: CallRecord,
    last_sip: Option<CallSipSummary>,
    last_sdp: Option<CallSdpSummary>,
    last_media: Option<CallMediaSummary>,
}

impl VoiceCallStateMachine {
    /// Construct from a VoWiFi carrier profile (adapter; behavior unchanged).
    pub fn new(profile: &'static CarrierProfile) -> Self {
        Self::with_params(VoiceParams::from_carrier_profile(profile))
    }

    /// Construct from transport-agnostic voice params (used by the VoLTE path).
    pub fn with_params(params: VoiceParams) -> Self {
        let preferred_leg = if params.vowifi_enabled {
            VoiceLegKind::Vowifi
        } else if params.carrier_fallback_enabled {
            VoiceLegKind::Carrier
        } else {
            VoiceLegKind::None
        };
        Self {
            media_transport: MediaTransportKind::RtpAvp,
            vowifi_voice_enabled: params.vowifi_enabled,
            carrier_leg_available: params.carrier_fallback_enabled,
            preferred_leg,
            reg_ready: false,
            call: CallRecord {
                trace_id: "voice-dry-run".to_string(),
                call_id: "call-dry-run-0001".to_string(),
                direction: CallDirection::MobileOriginated,
                invite_state: SipInviteState::Idle,
                call_state: CallState::Dialing,
                leg: VoiceLegKind::None,
                negotiated_codec: None,
                end_reason: None,
                failure_cause: None,
                retry_count: 0,
                rtp_packets_sent: 0,
                rtp_packets_received: 0,
            },
            last_sip: None,
            last_sdp: None,
            last_media: None,
            params,
        }
    }

    /// Mark that the IMS registration used for voice signaling is ready.
    pub fn mark_registration_ready(&mut self) {
        self.reg_ready = true;
    }

    /// Queue an MO call, recording the leg that will carry it.
    pub fn queue_mo_call(&mut self, leg: VoiceLegKind) {
        self.call.direction = CallDirection::MobileOriginated;
        self.call.invite_state = SipInviteState::Queued;
        self.call.call_state = CallState::Dialing;
        self.call.leg = leg;
        self.call.end_reason = None;
        self.call.failure_cause = None;
    }

    /// Record that the INVITE with an SDP offer left the device.
    pub fn submit_invite(&mut self, offered_codecs: usize) -> CallSipSummary {
        self.call.invite_state = SipInviteState::InviteSent;
        self.call.call_state = CallState::Dialing;
        let sip = CallSipSummary {
            direction: "outbound",
            method: "INVITE",
            transport: self.params.ims_transport,
            invite_state: self.call.invite_state.as_str(),
            sip_status: None,
            sensitive_values_policy: "sip_headers_and_sdp_body_not_serialized",
        };
        self.last_sip = Some(sip.clone());
        self.last_sdp = Some(CallSdpSummary {
            role: "offer",
            media_transport: self.media_transport.as_str(),
            codec_count: offered_codecs,
            negotiated_codec: None,
            direction: MediaDirection::SendRecv.as_str(),
            ptime_ms: Some(self.params.ptime_ms),
            sdp_bytes: 0,
            values_redacted: true,
            sensitive_values_policy: "sdp_connection_and_media_addresses_not_serialized",
        });
        sip
    }

    /// Accept a provisional 1xx response (100/180/183).
    pub fn accept_provisional(&mut self, sip_status: u16) -> CallSipSummary {
        self.call.invite_state = match sip_status {
            183 => SipInviteState::EarlyMedia,
            180 => SipInviteState::Ringing,
            _ => SipInviteState::Ringing,
        };
        self.call.call_state = CallState::Ringing;
        let sip = CallSipSummary {
            direction: "inbound",
            method: "INVITE",
            transport: self.params.ims_transport,
            invite_state: self.call.invite_state.as_str(),
            sip_status: Some(sip_status),
            sensitive_values_policy: "sip_headers_and_sdp_body_not_serialized",
        };
        self.last_sip = Some(sip.clone());
        sip
    }

    /// Accept the final 200 OK with a negotiated codec; the caller should send
    /// ACK. Transitions to a confirmed/active call.
    pub fn accept_final_answer(
        &mut self,
        sip_status: u16,
        negotiated_codec: Option<AudioCodec>,
    ) -> Result<CallSipSummary, VoiceRuntimeError> {
        if !(200..300).contains(&sip_status) {
            self.call.invite_state = SipInviteState::Failed;
            self.call.call_state = CallState::Failed;
            self.call.end_reason = Some(CallEndReason::from_sip_status(sip_status));
            self.call.failure_cause = Some(format!("sip_{sip_status}"));
            return Err(VoiceRuntimeError::SipRejected(sip_status));
        }
        self.call.invite_state = SipInviteState::Confirmed;
        self.call.call_state = CallState::Active;
        self.call.negotiated_codec = negotiated_codec;
        let sip = CallSipSummary {
            direction: "inbound",
            method: "INVITE",
            transport: self.params.ims_transport,
            invite_state: self.call.invite_state.as_str(),
            sip_status: Some(sip_status),
            sensitive_values_policy: "sip_headers_and_sdp_body_not_serialized",
        };
        self.last_sip = Some(sip.clone());
        self.last_sdp = Some(CallSdpSummary {
            role: "answer",
            media_transport: self.media_transport.as_str(),
            codec_count: negotiated_codec.map(|_| 1).unwrap_or(0),
            negotiated_codec: negotiated_codec.map(AudioCodec::as_str),
            direction: MediaDirection::SendRecv.as_str(),
            ptime_ms: Some(self.params.ptime_ms),
            sdp_bytes: 0,
            values_redacted: true,
            sensitive_values_policy: "sdp_connection_and_media_addresses_not_serialized",
        });
        self.last_media = Some(self.media_summary());
        Ok(sip)
    }

    /// Update RTP counters as media flows. Kept coarse (counts only).
    pub fn record_media_progress(&mut self, packets_sent: u64, packets_received: u64) {
        self.call.rtp_packets_sent = self.call.rtp_packets_sent.saturating_add(packets_sent);
        self.call.rtp_packets_received =
            self.call.rtp_packets_received.saturating_add(packets_received);
        self.last_media = Some(self.media_summary());
    }

    /// Terminate the call for the given reason.
    pub fn terminate(&mut self, reason: CallEndReason) {
        let failed = matches!(
            reason,
            CallEndReason::NetworkFailure | CallEndReason::MediaFailure
        );
        self.call.invite_state = if failed {
            SipInviteState::Failed
        } else {
            SipInviteState::Terminated
        };
        self.call.call_state = if failed {
            CallState::Failed
        } else {
            CallState::Ended
        };
        self.call.end_reason = Some(reason);
    }

    fn media_summary(&self) -> CallMediaSummary {
        CallMediaSummary {
            leg: self.call.leg.as_str(),
            media_transport: self.media_transport.as_str(),
            negotiated_codec: self.call.negotiated_codec.map(AudioCodec::as_str),
            clock_rate: self.call.negotiated_codec.map(AudioCodec::clock_rate),
            rtp_packets_sent: self.call.rtp_packets_sent,
            rtp_packets_received: self.call.rtp_packets_received,
            audio_source_bound: false,
            audio_sink_bound: false,
            sensitive_values_policy: "rtp_payloads_and_media_keys_not_serialized",
        }
    }

    /// Verify internal invariants; used to keep API/log/UI consistent.
    pub fn assert_state_consistency(&self) -> Result<(), VoiceRuntimeError> {
        if self.call.call_state == CallState::Active
            && self.call.invite_state != SipInviteState::Confirmed
        {
            return Err(VoiceRuntimeError::InconsistentState(
                "active_call_requires_confirmed_dialog",
            ));
        }
        if self.call.call_state == CallState::Active && self.call.negotiated_codec.is_none() {
            return Err(VoiceRuntimeError::InconsistentState(
                "active_call_requires_negotiated_codec",
            ));
        }
        Ok(())
    }

    pub fn snapshot(&self) -> VoiceRuntimePublicState {
        let active_call = CallPublicRecord::from_record(&self.call);
        let preferred_codec = self
            .params
            .preferred_codecs
            .first()
            .and_then(|token| AudioCodec::from_token(token))
            .map(AudioCodec::as_str);
        VoiceRuntimePublicState {
            profile_id: self.params.profile_id,
            plmn: self.params.plmn,
            voice_ready: self.reg_ready
                && (self.vowifi_voice_enabled || self.carrier_leg_available),
            vowifi_voice_enabled: self.vowifi_voice_enabled,
            carrier_leg_available: self.carrier_leg_available,
            preferred_leg: self.preferred_leg.as_str(),
            media_transport: self.media_transport.as_str(),
            preferred_codec,
            active_call,
            last_sip: self.last_sip.clone(),
            last_sdp: self.last_sdp.clone(),
            last_media: self.last_media.clone(),
            sip_endpoint_exposed: false,
            state_consistency_policy:
                "vowifi_voice_call_is_single_fact_source_for_logs_api_and_ui",
            sensitive_values_policy:
                "phone_numbers_sdp_body_rtp_media_and_media_keys_not_serialized",
        }
    }
}

/// Build the offline demo snapshot exercised by the dry-run executor. It walks
/// a full MO call: register ready 锟?INVITE 锟?180 锟?183 锟?200 OK (AMR) 锟?media 锟?/// hangup, asserting state consistency at the end.
pub fn build_dry_run_voice_snapshot(
    profile: &'static CarrierProfile,
) -> VoiceRuntimePublicState {
    let mut machine = VoiceCallStateMachine::new(profile);
    machine.mark_registration_ready();
    machine.queue_mo_call(VoiceLegKind::Vowifi);
    machine.submit_invite(2);
    machine.accept_provisional(180);
    machine.accept_provisional(183);
    machine
        .accept_final_answer(200, Some(AudioCodec::Amr))
        .expect("synthetic 200 OK is accepted");
    machine.record_media_progress(50, 50);
    machine
        .assert_state_consistency()
        .expect("dry-run voice states remain API/log consistent");
    machine.terminate(CallEndReason::LocalHangup);
    machine.snapshot()
}

// ---------------------------------------------------------------------------
// Reserved audio / leg / endpoint interfaces
// ---------------------------------------------------------------------------
//
// These traits are the reserved seams the architecture calls for. They let the
// live media loop later plug in:
//   * VoWiFi AMR-over-RTP audio (encode/decode + jitter buffer),
//   * a carrier voice leg driven by AT commands + USB-Audio PCM, and
//   * an outward-facing standard SIP endpoint (one per SIM) that Asterisk or a
//     softphone like Linphone can register/route through.
//
// Nothing here performs I/O yet; the default implementations are inert so the
// control-plane state machine is fully usable and testable on its own.

/// Result of pulling one encoded audio frame from a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    /// Codec-specific frame-type index (for AMR, the FT value).
    pub frame_type: u8,
    /// Encoded speech bits for a single frame.
    pub payload: Vec<u8>,
    /// RTP timestamp increment this frame represents (samples).
    pub timestamp_increment: u32,
}

/// A source of outbound audio frames (mic / PCM capture / test tone). The
/// media loop pulls frames from here, wraps them into RTP, and sends them.
pub trait AudioSource: Send + Sync {
    /// Pull the next encoded audio frame, or `None` when the source is idle.
    fn next_frame(&mut self) -> Option<AudioFrame>;

    /// Which codec this source produces frames for.
    fn codec(&self) -> AudioCodec;
}

/// A sink for inbound audio frames (speaker / PCM playback / recording). The
/// media loop parses received RTP and hands decoded frames here.
pub trait AudioSink: Send + Sync {
    /// Deliver a received audio frame for playback/recording.
    fn deliver_frame(&mut self, frame: &AudioFrame);

    /// Which codec this sink expects.
    fn codec(&self) -> AudioCodec;
}

/// A silent audio source that never produces frames 锟?the default before a
/// real media backend is wired in.
#[derive(Debug, Clone, Copy)]
pub struct SilentAudioSource {
    pub codec: AudioCodec,
}

impl AudioSource for SilentAudioSource {
    fn next_frame(&mut self) -> Option<AudioFrame> {
        None
    }
    fn codec(&self) -> AudioCodec {
        self.codec
    }
}

/// An audio sink that discards everything 锟?the default before playback is
/// wired in.
#[derive(Debug, Clone, Copy)]
pub struct NullAudioSink {
    pub codec: AudioCodec,
}

impl AudioSink for NullAudioSink {
    fn deliver_frame(&mut self, _frame: &AudioFrame) {}
    fn codec(&self) -> AudioCodec {
        self.codec
    }
}

/// Control surface for the operator (carrier) voice leg: dial, answer, hang up
/// via AT commands (ATD/ATA/AT+CHUP), poll call state via AT+CLCC, and stream
/// PCM through a USB-Audio device. Implemented later; the interface is reserved
/// so orchestration can target it today.
pub trait CarrierVoiceLeg: Send + Sync {
    /// Whether this device actually exposes a USB-Audio interface. When false,
    /// the carrier leg must be disabled (audio cannot be carried).
    fn usb_audio_available(&self) -> bool;

    /// Place an outbound call to `number` (maps to `ATD<number>;`).
    fn dial(&mut self, number: &str) -> Result<(), VoiceRuntimeError>;

    /// Answer the current inbound call (maps to `ATA`).
    fn answer(&mut self) -> Result<(), VoiceRuntimeError>;

    /// Hang up the current call (maps to `AT+CHUP`).
    fn hangup(&mut self) -> Result<(), VoiceRuntimeError>;

    /// Poll the current call state (derived from `AT+CLCC`).
    fn poll_state(&mut self) -> CallState;
}

/// A carrier leg that reports itself unavailable 锟?the safe default when no USB
/// Audio device is present, disabling the operator fallback leg.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisabledCarrierVoiceLeg;

impl CarrierVoiceLeg for DisabledCarrierVoiceLeg {
    fn usb_audio_available(&self) -> bool {
        false
    }
    fn dial(&mut self, _number: &str) -> Result<(), VoiceRuntimeError> {
        Err(VoiceRuntimeError::InconsistentState(
            "carrier_leg_disabled_no_usb_audio",
        ))
    }
    fn answer(&mut self) -> Result<(), VoiceRuntimeError> {
        Err(VoiceRuntimeError::InconsistentState(
            "carrier_leg_disabled_no_usb_audio",
        ))
    }
    fn hangup(&mut self) -> Result<(), VoiceRuntimeError> {
        Ok(())
    }
    fn poll_state(&mut self) -> CallState {
        CallState::Ended
    }
}

/// Reserved outward-facing SIP endpoint bridge: exposes one standard SIP
/// endpoint per SIM so external UAs (Asterisk PBX, Linphone, etc.) can place
/// and receive calls that SimAdmin then bridges onto the VoWiFi or carrier leg.
///
/// This is the seam described by the target architecture ("瀵瑰锛氫竴鏉℃爣锟?SIP
/// endpoint (per SIM)"). It is intentionally trait-only for now.
pub trait SipEndpointBridge: Send + Sync {
    /// Whether the outward SIP endpoint is currently exposed/enabled.
    fn is_exposed(&self) -> bool;

    /// The SIP AOR advertised for this SIM (e.g. `sip:<msisdn>@<host>`), if any.
    fn local_aor(&self) -> Option<String>;

    /// Called when an external UA sends an INVITE we should bridge onto an
    /// internal leg. Returns the leg chosen to carry the call.
    fn on_external_invite(&mut self, callee: &str) -> Result<VoiceLegKind, VoiceRuntimeError>;
}

/// A SIP endpoint bridge that is not yet exposed 锟?the default until the
/// outward endpoint feature is turned on.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnexposedSipEndpointBridge;

impl SipEndpointBridge for UnexposedSipEndpointBridge {
    fn is_exposed(&self) -> bool {
        false
    }
    fn local_aor(&self) -> Option<String> {
        None
    }
    fn on_external_invite(&mut self, _callee: &str) -> Result<VoiceLegKind, VoiceRuntimeError> {
        Err(VoiceRuntimeError::InconsistentState(
            "sip_endpoint_bridge_not_exposed",
        ))
    }
}

// ---------------------------------------------------------------------------
// Leg selection helper
// ---------------------------------------------------------------------------

/// Choose the voice leg to use, honoring the profile preference (VoWiFi first)
/// and falling back to the carrier leg only when it is actually usable
/// (USB-Audio present). Returns `VoiceLegKind::None` when neither leg can run.
pub fn select_voice_leg(
    profile: &'static CarrierProfile,
    vowifi_ready: bool,
    carrier_usb_audio_available: bool,
) -> VoiceLegKind {
    if profile.voice.vowifi_enabled && vowifi_ready {
        return VoiceLegKind::Vowifi;
    }
    if profile.voice.carrier_fallback_enabled && carrier_usb_audio_available {
        return VoiceLegKind::Carrier;
    }
    VoiceLegKind::None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::vowifi::profiles::GB_EE_23433;

    #[test]
    fn mo_call_reaches_active_with_negotiated_codec() {
        let mut machine = VoiceCallStateMachine::new(&GB_EE_23433);
        machine.mark_registration_ready();
        machine.queue_mo_call(VoiceLegKind::Vowifi);
        machine.submit_invite(2);
        machine.accept_provisional(180);
        machine
            .accept_final_answer(200, Some(AudioCodec::Amr))
            .expect("200 OK accepted");
        machine.record_media_progress(10, 10);

        let snap = machine.snapshot();
        assert_eq!(snap.active_call.call_state, "active");
        assert_eq!(snap.active_call.api_status, "active");
        assert_eq!(snap.active_call.negotiated_codec, Some("amr"));
        machine
            .assert_state_consistency()
            .expect("active call is consistent");
    }

    #[test]
    fn rejected_invite_marks_call_failed() {
        let mut machine = VoiceCallStateMachine::new(&GB_EE_23433);
        machine.mark_registration_ready();
        machine.queue_mo_call(VoiceLegKind::Vowifi);
        machine.submit_invite(2);
        let err = machine
            .accept_final_answer(486, None)
            .expect_err("486 is a rejection");
        assert_eq!(err, VoiceRuntimeError::SipRejected(486));

        let snap = machine.snapshot();
        assert_eq!(snap.active_call.call_state, "failed");
        assert_eq!(snap.active_call.end_reason, Some("remote_busy"));
    }

    #[test]
    fn active_call_without_codec_is_inconsistent() {
        let mut machine = VoiceCallStateMachine::new(&GB_EE_23433);
        machine.mark_registration_ready();
        machine.queue_mo_call(VoiceLegKind::Vowifi);
        machine.submit_invite(1);
        machine
            .accept_final_answer(200, None)
            .expect("200 OK accepted");
        assert!(machine.assert_state_consistency().is_err());
    }

    #[test]
    fn dry_run_snapshot_walks_full_call() {
        let snap = build_dry_run_voice_snapshot(&GB_EE_23433);
        assert_eq!(snap.profile_id, "gb_ee_23433");
        assert_eq!(snap.active_call.call_state, "ended");
        assert_eq!(snap.active_call.negotiated_codec, Some("amr"));
        assert!(snap.voice_ready);
    }

    #[test]
    fn sdp_offer_round_trips_through_parser() {
        let offer =
            build_mo_audio_offer(&GB_EE_23433, "192.0.2.10", SdpAddrType::Ip4, 40000);
        let body = offer.to_sdp();
        let parsed = parse_audio_sdp(body.as_bytes()).expect("parse own offer");
        assert_eq!(parsed.media_port, 40000);
        assert_eq!(parsed.connection_addr, "192.0.2.10");
        assert!(!parsed.codecs.is_empty());
        // The first offered codec should survive the round trip.
        assert_eq!(parsed.codecs[0].codec, offer.codecs[0].codec);
    }

    #[test]
    fn sdp_answer_intersects_codecs() {
        let offer = SdpAudioDescription {
            session_id: 1,
            session_version: 1,
            origin_username: "peer".to_string(),
            connection_addr: "192.0.2.20".to_string(),
            addr_type: SdpAddrType::Ip4,
            media_port: 5004,
            transport: MediaTransportKind::RtpAvp,
            codecs: vec![
                SdpCodec {
                    codec: AudioCodec::Amr,
                    payload_type: 96,
                    fmtp: Some("octet-align=1".to_string()),
                },
                SdpCodec {
                    codec: AudioCodec::Pcmu,
                    payload_type: 0,
                    fmtp: None,
                },
            ],
            direction: MediaDirection::SendRecv,
            ptime: Some(20),
        };
        let answer = build_sdp_answer(&GB_EE_23433, &offer, "192.0.2.10", SdpAddrType::Ip4, 40000)
            .expect("build answer");
        assert!(!answer.codecs.is_empty());
        // Answer keeps the offerer's payload numbering.
        assert!(answer
            .codecs
            .iter()
            .any(|codec| codec.codec == AudioCodec::Amr && codec.payload_type == 96));
    }

    #[test]
    fn rtp_packet_round_trips() {
        let packet = RtpPacket {
            payload_type: 96,
            marker: true,
            sequence: 1234,
            timestamp: 160,
            ssrc: 0xdead_beef,
            payload: vec![0x01, 0x02, 0x03, 0x04],
        };
        let bytes = packet.encode();
        let parsed = RtpPacket::parse(&bytes).expect("parse rtp");
        assert_eq!(parsed, packet);
    }

    #[test]
    fn amr_payload_frame_type_round_trips_octet_aligned() {
        let payload = build_amr_rtp_payload(7, &[0xaa, 0xbb, 0xcc], true);
        assert_eq!(parse_amr_frame_type(&payload, true), Some(7));
    }

    #[test]
    fn amr_payload_frame_type_round_trips_bandwidth_efficient() {
        let payload = build_amr_rtp_payload(5, &[0x11, 0x22], false);
        assert_eq!(parse_amr_frame_type(&payload, false), Some(5));
    }

    #[test]
    fn leg_selection_prefers_vowifi_then_carrier() {
        assert_eq!(
            select_voice_leg(&GB_EE_23433, true, true),
            VoiceLegKind::Vowifi
        );
        // When VoWiFi not ready, fall back to carrier only if USB audio present.
        assert_eq!(
            select_voice_leg(&GB_EE_23433, false, true),
            if GB_EE_23433.voice.carrier_fallback_enabled {
                VoiceLegKind::Carrier
            } else {
                VoiceLegKind::None
            }
        );
        assert_eq!(
            select_voice_leg(&GB_EE_23433, false, false),
            VoiceLegKind::None
        );
    }
}
