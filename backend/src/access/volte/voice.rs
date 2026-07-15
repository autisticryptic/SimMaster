//! VoLTE voice orchestration.
//!
//! Clean-room extension (not reverse-engineered: the reference VoLTE binary
//! only implements SMS-over-IMS, so voice on the LTE path is a forward-looking
//! addition built from public 3GPP/RFC specs — TS 24.229 (IMS SIP), TS 26.114
//! (IMS media / AMR), RFC 3261 (SIP), RFC 4566 (SDP), RFC 3550 (RTP),
//! RFC 4867 (AMR RTP payload)).
//!
//! Hardware reality: the target device (Qualcomm 410 pocket-WiFi) has no
//! mic/speaker/PCM, so voice is **gateway/relay-only** — the device carries the
//! IMS SIP dialog and relays RTP between the operator IMS leg and an internal
//! SIP UA (Linphone/Asterisk). It never plays audio locally.
//!
//! Reuse policy: the pure signaling layer (`VoiceCallStateMachine`, SDP
//! offer/answer, RTP/AMR framing) is reused from `vowifi::voice` via its neutral
//! `VoiceParams` entry points. This module only supplies VoLTE-specific voice
//! parameters (from `VolteConfig`) and wires the call state machine to the VoLTE
//! IMS session established in `runtime`/`register`.

use crate::access::volte::vilte::{build_av_sdp, build_video_offer, VideoMediaDescription};
use crate::ims::voice::{
    build_mo_audio_offer_with_params, build_sdp_answer_with_params, parse_audio_sdp, AudioCodec,
    CallEndReason, CallState, SdpAddrType, SdpAudioDescription, VoiceCallStateMachine,
    VoiceLegKind, VoiceParams, VoiceRuntimeError,
};
use crate::infra::config::{VilteConfig, VolteConfig};

/// Default codec preference for a VoLTE voice offer. AMR-WB first (HD voice),
/// then narrowband AMR, then G.711 fallbacks. Matches typical operator IMS
/// media policy (TS 26.114). Kept here (not in VolteConfig) until the user
/// exposes per-codec tuning.
pub const DEFAULT_VOICE_CODECS: &[&str] = &["amr-wb", "amr", "pcmu", "pcma"];

/// Default packetization time (ms) for the audio offer.
pub const DEFAULT_PTIME_MS: u16 = 20;

/// Build the neutral voice params for the VoLTE leg from persisted config.
///
/// VoLTE is an IMS-over-LTE leg, so `vowifi_enabled` maps to "is the VoLTE voice
/// leg enabled" from the leg's own perspective (the shared state machine only
/// needs to know an IMS leg is available); the carrier/USB-audio fallback leg is
/// off (the target device has no audio hardware).
pub fn volte_voice_params(config: &VolteConfig) -> VoiceParams {
    VoiceParams {
        preferred_codecs: DEFAULT_VOICE_CODECS.iter().map(|s| s.to_string()).collect(),
        ptime_ms: DEFAULT_PTIME_MS,
        amr_octet_align: false,
        // From the shared state machine's viewpoint the VoLTE leg *is* the IMS
        // voice leg; enable it when the VoLTE voice feature is on.
        vowifi_enabled: config.voice_enabled,
        carrier_fallback_enabled: false,
        ims_transport: "udp",
        profile_id: "volte_ims",
        plmn: "",
    }
}

/// Relay endpoint addressing for one RTP stream (operator side or internal
/// SIP-UA side). The device relays between two of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaEndpoint {
    pub addr: String,
    pub addr_type: SdpAddrType,
    pub port: u16,
}

/// Outcome of processing an inbound SDP offer for a gateway-relayed call: the
/// answer to send back plus the negotiated codec (for RTP payload mapping).
#[derive(Debug, Clone)]
pub struct SdpNegotiation {
    pub answer: SdpAudioDescription,
    pub negotiated_codec: Option<AudioCodec>,
}

/// Current media composition of the call. VoLTE = audio only; ViLTE = audio +
/// video. The device switches between them mid-call with a re-INVITE (adding or
/// removing the `m=video` section), exactly like a handset's "turn video
/// on/off" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallMediaMode {
    /// VoLTE: a single `m=audio` stream.
    AudioOnly,
    /// ViLTE: `m=audio` + `m=video` (H.264).
    AudioVideo,
}

impl CallMediaMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CallMediaMode::AudioOnly => "audio",
            CallMediaMode::AudioVideo => "audio_video",
        }
    }

    /// Whether video is present in this mode.
    pub fn has_video(self) -> bool {
        matches!(self, CallMediaMode::AudioVideo)
    }
}

/// Error from a mid-call media switch (VoLTE <-> ViLTE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSwitchError {
    /// The call is not in the `Active` state; you can only re-negotiate media on
    /// an established dialog.
    CallNotActive,
    /// Already in the requested media mode (no-op guarded to avoid a needless
    /// re-INVITE).
    AlreadyInMode(&'static str),
    /// ViLTE is not enabled in configuration, so video cannot be added.
    VideoNotEnabled,
    /// The far end rejected the media change (re-INVITE non-2xx); the prior
    /// media mode remains in effect.
    Rejected(u16),
}

impl std::fmt::Display for MediaSwitchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CallNotActive => write!(f, "media_switch_call_not_active"),
            Self::AlreadyInMode(mode) => write!(f, "media_switch_already_{mode}"),
            Self::VideoNotEnabled => write!(f, "media_switch_video_not_enabled"),
            Self::Rejected(code) => write!(f, "media_switch_rejected_{code}"),
        }
    }
}

impl std::error::Error for MediaSwitchError {}

/// The SDP body + target media mode produced by a mid-call switch, to be carried
/// in a re-INVITE. `cseq` is the dialog CSeq the caller should use for the
/// re-INVITE (the caller owns the `DialogIds` and bumps it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaReoffer {
    /// The full SDP offer body for the re-INVITE (audio-only or audio+video).
    pub sdp: String,
    /// The media mode this re-offer moves the call toward (applied once the far
    /// end answers 2xx).
    pub target_mode: CallMediaMode,
}

/// A VoLTE voice call orchestrator wrapping the shared call state machine with
/// VoLTE-specific SDP construction. Gateway mode: no local audio, only signaling
/// + (elsewhere) RTP relay.
///
/// Supports mid-call VoLTE <-> ViLTE switching: on an `Active` call, the user
/// can add video ([`upgrade_to_video`](Self::upgrade_to_video)) or drop it
/// ([`downgrade_to_audio`](Self::downgrade_to_audio)); each produces a
/// [`MediaReoffer`] the caller sends as an in-dialog re-INVITE
/// (`sip::build_reinvite`). The new mode becomes effective when the far end
/// answers 2xx ([`confirm_media_switch`](Self::confirm_media_switch)).
pub struct VolteVoiceCall {
    machine: VoiceCallStateMachine,
    params: VoiceParams,
    local_media: MediaEndpoint,
    /// Local RTP media endpoint for the video stream (distinct port from audio).
    local_video_media: Option<MediaEndpoint>,
    /// ViLTE video configuration (codec / payload type / fmtp).
    vilte: VilteConfig,
    /// Whether ViLTE video is permitted for this call (feature-gated).
    vilte_enabled: bool,
    /// Current media composition of the established call.
    media_mode: CallMediaMode,
    /// A media switch is in flight (re-INVITE sent, awaiting the far end's
    /// answer); holds the mode we are switching to.
    pending_switch: Option<CallMediaMode>,
}

impl VolteVoiceCall {
    /// Create a new orchestrator bound to the local media relay endpoint.
    /// Audio-only (VoLTE); video is disabled unless [`with_vilte`](Self::with_vilte)
    /// supplies a ViLTE config + a local video media endpoint.
    pub fn new(config: &VolteConfig, local_media: MediaEndpoint) -> Self {
        let params = volte_voice_params(config);
        Self {
            machine: VoiceCallStateMachine::with_params(params.clone()),
            params,
            local_media,
            local_video_media: None,
            vilte: VilteConfig::default(),
            vilte_enabled: false,
            media_mode: CallMediaMode::AudioOnly,
            pending_switch: None,
        }
    }

    /// Enable ViLTE video for this call. `vilte.feature_enabled` gates whether
    /// video can be added mid-call; `local_video_media` is the (distinct) RTP
    /// endpoint the device relays the video stream on.
    pub fn with_vilte(mut self, vilte: &VilteConfig, local_video_media: MediaEndpoint) -> Self {
        self.vilte_enabled = vilte.feature_enabled;
        self.vilte = vilte.clone();
        self.local_video_media = Some(local_video_media);
        self
    }

    /// The current media composition of the call (VoLTE audio vs ViLTE video).
    pub fn media_mode(&self) -> CallMediaMode {
        self.media_mode
    }

    /// Whether ViLTE video may be added to this call (feature-enabled and a
    /// local video media endpoint is configured).
    pub fn video_available(&self) -> bool {
        self.vilte_enabled && self.local_video_media.is_some()
    }

    /// Whether VoLTE voice is enabled for this configuration.
    pub fn enabled(&self) -> bool {
        self.params.vowifi_enabled
    }

    /// Mark the underlying IMS registration ready (voice can start).
    pub fn mark_registration_ready(&mut self) {
        self.machine.mark_registration_ready();
    }

    /// Build the SDP offer for a mobile-originated call, bound to the local
    /// relay media address/port.
    pub fn build_mo_offer(&mut self) -> SdpAudioDescription {
        self.machine.queue_mo_call(VoiceLegKind::Vowifi);
        build_mo_audio_offer_with_params(
            &self.params,
            &self.local_media.addr,
            self.local_media.addr_type,
            self.local_media.port,
        )
    }

    /// Process an inbound SDP offer (mobile-terminated call or the internal UA's
    /// offer) and produce an answer intersecting supported codecs.
    pub fn negotiate_answer(&self, offer_body: &[u8]) -> Result<SdpNegotiation, VoiceRuntimeError> {
        let offer = parse_audio_sdp(offer_body).map_err(|_| VoiceRuntimeError::NoCommonCodec)?;
        let answer = build_sdp_answer_with_params(
            &self.params,
            &offer,
            &self.local_media.addr,
            self.local_media.addr_type,
            self.local_media.port,
        )?;
        let negotiated_codec = answer.codecs.first().map(|c| c.codec);
        Ok(SdpNegotiation {
            answer,
            negotiated_codec,
        })
    }

    /// Record that the INVITE (with SDP offer) was sent.
    pub fn on_invite_sent(&mut self, offered_codecs: usize) {
        self.machine.submit_invite(offered_codecs);
    }

    /// Record a provisional response (180 ringing / 183 early media).
    pub fn on_provisional(&mut self, sip_status: u16) {
        self.machine.accept_provisional(sip_status);
    }

    /// Record the final answer; on 2xx transitions to active with the codec.
    pub fn on_final_answer(
        &mut self,
        sip_status: u16,
        negotiated_codec: Option<AudioCodec>,
    ) -> Result<(), VoiceRuntimeError> {
        self.machine
            .accept_final_answer(sip_status, negotiated_codec)
            .map(|_| ())
    }

    /// Record RTP relay progress (packet counts, for status/diagnostics).
    pub fn on_media_progress(&mut self, packets_sent: u64, packets_received: u64) {
        self.machine
            .record_media_progress(packets_sent, packets_received);
    }

    /// Terminate the call.
    pub fn terminate(&mut self, reason: CallEndReason) {
        self.machine.terminate(reason);
    }

    /// Verify state invariants (active call must have a confirmed dialog + codec).
    pub fn assert_consistent(&self) -> Result<(), VoiceRuntimeError> {
        self.machine.assert_state_consistency()
    }

    // ---------------------------------------------------------------------
    // Mid-call VoLTE <-> ViLTE switching (re-INVITE media re-negotiation)
    // ---------------------------------------------------------------------

    /// Upgrade an active VoLTE (audio-only) call to ViLTE (audio + video).
    ///
    /// Produces a [`MediaReoffer`] whose SDP carries the existing `m=audio`
    /// section plus a new H.264 `m=video` section. The caller sends this as an
    /// in-dialog re-INVITE (`sip::build_reinvite`) and, on a 2xx answer, calls
    /// [`confirm_media_switch`](Self::confirm_media_switch). This mirrors a
    /// handset's "turn on video" during a voice call.
    ///
    /// Guards: the call must be `Active`, ViLTE must be enabled with a local
    /// video media endpoint, and the call must not already carry video.
    pub fn upgrade_to_video(&mut self) -> Result<MediaReoffer, MediaSwitchError> {
        if self.machine.call_state() != CallState::Active {
            return Err(MediaSwitchError::CallNotActive);
        }
        if self.media_mode == CallMediaMode::AudioVideo {
            return Err(MediaSwitchError::AlreadyInMode("audio_video"));
        }
        if !self.video_available() {
            return Err(MediaSwitchError::VideoNotEnabled);
        }
        let video_media = self
            .local_video_media
            .as_ref()
            .ok_or(MediaSwitchError::VideoNotEnabled)?;

        let audio = build_mo_audio_offer_with_params(
            &self.params,
            &self.local_media.addr,
            self.local_media.addr_type,
            self.local_media.port,
        );
        let video: VideoMediaDescription = build_video_offer(&self.vilte, video_media.port);
        let sdp = build_av_sdp(&audio, &video);

        self.pending_switch = Some(CallMediaMode::AudioVideo);
        Ok(MediaReoffer {
            sdp,
            target_mode: CallMediaMode::AudioVideo,
        })
    }

    /// Downgrade an active ViLTE (audio + video) call back to VoLTE (audio
    /// only). Produces an audio-only re-offer; the removed video stream's
    /// `m=video` port would be set to 0 in a strict SDP, but since we relay and
    /// the far end re-negotiates from the audio-only body, we send the plain
    /// audio offer. On 2xx, call [`confirm_media_switch`](Self::confirm_media_switch).
    ///
    /// Guards: the call must be `Active` and must currently carry video.
    pub fn downgrade_to_audio(&mut self) -> Result<MediaReoffer, MediaSwitchError> {
        if self.machine.call_state() != CallState::Active {
            return Err(MediaSwitchError::CallNotActive);
        }
        if self.media_mode == CallMediaMode::AudioOnly {
            return Err(MediaSwitchError::AlreadyInMode("audio"));
        }
        let audio = build_mo_audio_offer_with_params(
            &self.params,
            &self.local_media.addr,
            self.local_media.addr_type,
            self.local_media.port,
        );
        self.pending_switch = Some(CallMediaMode::AudioOnly);
        Ok(MediaReoffer {
            sdp: audio.to_sdp(),
            target_mode: CallMediaMode::AudioOnly,
        })
    }

    /// Whether a media switch (re-INVITE) is awaiting the far end's answer.
    pub fn switch_pending(&self) -> bool {
        self.pending_switch.is_some()
    }

    /// Apply the far end's answer to a pending media switch. On a 2xx the new
    /// media mode takes effect; on a non-2xx the prior mode is retained and a
    /// [`MediaSwitchError::Rejected`] is returned (the call stays up in its old
    /// mode — a rejected video upgrade does not drop the voice call).
    pub fn confirm_media_switch(
        &mut self,
        sip_status: u16,
    ) -> Result<CallMediaMode, MediaSwitchError> {
        let target = match self.pending_switch.take() {
            Some(mode) => mode,
            // No switch in flight: nothing to confirm, report current mode.
            None => return Ok(self.media_mode),
        };
        if !(200..300).contains(&sip_status) {
            // Keep the existing media mode; the re-INVITE was refused.
            return Err(MediaSwitchError::Rejected(sip_status));
        }
        self.media_mode = target;
        Ok(self.media_mode)
    }

    /// Borrow the underlying state machine (for snapshotting).
    pub fn machine(&self) -> &VoiceCallStateMachine {
        &self.machine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_voice_on() -> VolteConfig {
        VolteConfig {
            feature_enabled: true,
            sms_enabled: true,
            voice_enabled: true,
            connection_enabled: true,
            ..VolteConfig::default()
        }
    }

    fn local_media() -> MediaEndpoint {
        MediaEndpoint {
            addr: "10.0.0.2".to_string(),
            addr_type: SdpAddrType::Ip4,
            port: 40000,
        }
    }

    #[test]
    fn params_reflect_voice_enabled() {
        let on = volte_voice_params(&config_voice_on());
        assert!(on.vowifi_enabled);
        assert!(
            !on.carrier_fallback_enabled,
            "no audio hardware -> no carrier leg"
        );
        let off = volte_voice_params(&VolteConfig::default());
        assert!(!off.vowifi_enabled);
    }

    #[test]
    fn mo_offer_advertises_preferred_codecs_at_local_media() {
        let mut call = VolteVoiceCall::new(&config_voice_on(), local_media());
        let offer = call.build_mo_offer();
        assert_eq!(offer.connection_addr, "10.0.0.2");
        assert_eq!(offer.media_port, 40000);
        // AMR-WB first per DEFAULT_VOICE_CODECS.
        assert_eq!(
            offer.codecs.first().map(|c| c.codec),
            Some(AudioCodec::AmrWb)
        );
        // Round-trips through the SDP serializer + parser.
        let body = offer.to_sdp().into_bytes();
        let parsed = parse_audio_sdp(&body).expect("offer parses");
        assert!(!parsed.codecs.is_empty());
    }

    #[test]
    fn negotiate_answer_intersects_codecs() {
        let call = VolteVoiceCall::new(&config_voice_on(), local_media());
        // Remote offers PCMU (PT 0) + AMR (PT 96). We support both; answer keeps
        // the offerer's PT numbering.
        let remote = concat!(
            "v=0\r\n",
            "o=- 1 1 IN IP4 203.0.113.9\r\n",
            "s=call\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=audio 5004 RTP/AVP 96 0\r\n",
            "a=rtpmap:96 AMR/8000\r\n",
            "a=rtpmap:0 PCMU/8000\r\n",
        );
        let neg = call.negotiate_answer(remote.as_bytes()).expect("answer");
        assert!(neg.negotiated_codec.is_some());
        assert!(!neg.answer.codecs.is_empty());
    }

    #[test]
    fn full_mo_call_reaches_active_then_ends() {
        let mut call = VolteVoiceCall::new(&config_voice_on(), local_media());
        call.mark_registration_ready();
        let offer = call.build_mo_offer();
        call.on_invite_sent(offer.codecs.len());
        call.on_provisional(180);
        call.on_provisional(183);
        call.on_final_answer(200, Some(AudioCodec::AmrWb))
            .expect("200 OK accepted");
        call.on_media_progress(100, 98);
        call.assert_consistent().expect("active call is consistent");
        call.terminate(CallEndReason::LocalHangup);
    }

    #[test]
    fn rejected_call_is_error() {
        let mut call = VolteVoiceCall::new(&config_voice_on(), local_media());
        call.mark_registration_ready();
        let _ = call.build_mo_offer();
        call.on_invite_sent(2);
        let res = call.on_final_answer(486, None);
        assert!(res.is_err(), "486 Busy Here must be an error");
    }

    // -------------------- mid-call VoLTE <-> ViLTE switching --------------------

    fn vilte_on() -> VilteConfig {
        VilteConfig {
            feature_enabled: true,
            ..VilteConfig::default()
        }
    }

    fn video_media() -> MediaEndpoint {
        MediaEndpoint {
            addr: "10.0.0.2".to_string(),
            addr_type: SdpAddrType::Ip4,
            port: 40002,
        }
    }

    /// Bring a call to the Active state so media re-negotiation is allowed.
    fn active_call(vilte: bool) -> VolteVoiceCall {
        let mut call = VolteVoiceCall::new(&config_voice_on(), local_media());
        if vilte {
            call = call.with_vilte(&vilte_on(), video_media());
        }
        call.mark_registration_ready();
        let offer = call.build_mo_offer();
        call.on_invite_sent(offer.codecs.len());
        call.on_provisional(180);
        call.on_final_answer(200, Some(AudioCodec::AmrWb))
            .expect("call active");
        call
    }

    #[test]
    fn upgrade_to_video_produces_av_reoffer_and_confirms() {
        let mut call = active_call(true);
        assert_eq!(call.media_mode(), CallMediaMode::AudioOnly);
        let reoffer = call.upgrade_to_video().expect("upgrade");
        assert_eq!(reoffer.target_mode, CallMediaMode::AudioVideo);
        // The re-offer SDP carries BOTH audio and video media sections.
        assert!(reoffer.sdp.contains("m=audio "));
        assert!(reoffer.sdp.contains("m=video "));
        assert!(call.switch_pending());
        // Far end accepts: mode becomes audio+video.
        let mode = call.confirm_media_switch(200).expect("confirmed");
        assert_eq!(mode, CallMediaMode::AudioVideo);
        assert!(call.media_mode().has_video());
        assert!(!call.switch_pending());
    }

    #[test]
    fn downgrade_to_audio_removes_video() {
        let mut call = active_call(true);
        call.upgrade_to_video().expect("upgrade");
        call.confirm_media_switch(200).expect("confirm upgrade");
        assert_eq!(call.media_mode(), CallMediaMode::AudioVideo);

        let reoffer = call.downgrade_to_audio().expect("downgrade");
        assert_eq!(reoffer.target_mode, CallMediaMode::AudioOnly);
        assert!(reoffer.sdp.contains("m=audio "));
        assert!(!reoffer.sdp.contains("m=video "));
        let mode = call.confirm_media_switch(200).expect("confirm downgrade");
        assert_eq!(mode, CallMediaMode::AudioOnly);
    }

    #[test]
    fn rejected_upgrade_keeps_audio_and_call_stays_up() {
        let mut call = active_call(true);
        call.upgrade_to_video().expect("upgrade");
        // Far end rejects video (e.g. 488 Not Acceptable Here).
        let res = call.confirm_media_switch(488);
        assert_eq!(res, Err(MediaSwitchError::Rejected(488)));
        // Call remains active in audio-only; a rejected upgrade must not drop it.
        assert_eq!(call.media_mode(), CallMediaMode::AudioOnly);
        assert!(!call.switch_pending());
        call.assert_consistent().expect("call still consistent");
    }

    #[test]
    fn cannot_upgrade_before_call_is_active() {
        let mut call = VolteVoiceCall::new(&config_voice_on(), local_media())
            .with_vilte(&vilte_on(), video_media());
        call.mark_registration_ready();
        let _ = call.build_mo_offer();
        call.on_invite_sent(2);
        // Still dialing/ringing, not Active.
        assert_eq!(
            call.upgrade_to_video(),
            Err(MediaSwitchError::CallNotActive)
        );
    }

    #[test]
    fn cannot_upgrade_when_vilte_disabled() {
        // ViLTE not configured on this call.
        let mut call = active_call(false);
        assert_eq!(
            call.upgrade_to_video(),
            Err(MediaSwitchError::VideoNotEnabled)
        );
    }

    #[test]
    fn double_upgrade_is_rejected_as_already_in_mode() {
        let mut call = active_call(true);
        call.upgrade_to_video().expect("upgrade");
        call.confirm_media_switch(200).expect("confirm");
        assert_eq!(
            call.upgrade_to_video(),
            Err(MediaSwitchError::AlreadyInMode("audio_video"))
        );
    }

    #[test]
    fn downgrade_without_video_is_rejected() {
        let mut call = active_call(true);
        // Never upgraded; still audio-only.
        assert_eq!(
            call.downgrade_to_audio(),
            Err(MediaSwitchError::AlreadyInMode("audio"))
        );
    }
}
