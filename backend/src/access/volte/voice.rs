//! VoLTE voice orchestration.
//!
//! Clean-room extension (not reverse-engineered: the reference VoLTE binary
//! only implements SMS-over-IMS, so voice on the LTE path is a forward-looking
//! addition built from public 3GPP/RFC specs 鈥?TS 24.229 (IMS SIP), TS 26.114
//! (IMS media / AMR), RFC 3261 (SIP), RFC 4566 (SDP), RFC 3550 (RTP),
//! RFC 4867 (AMR RTP payload)).
//!
//! Hardware reality: the target device (Qualcomm 410 pocket-WiFi) has no
//! mic/speaker/PCM, so voice is **gateway/relay-only** 鈥?the device carries the
//! IMS SIP dialog and relays RTP between the operator IMS leg and an internal
//! SIP UA (Linphone/Asterisk). It never plays audio locally.
//!
//! Reuse policy: the pure signaling layer (`VoiceCallStateMachine`, SDP
//! offer/answer, RTP/AMR framing) is reused from `vowifi::voice` via its neutral
//! `VoiceParams` entry points. This module only supplies VoLTE-specific voice
//! parameters (from `VolteConfig`) and wires the call state machine to the VoLTE
//! IMS session established in `runtime`/`register`.

use crate::infra::config::VolteConfig;
use crate::access::vowifi::voice::{
    build_mo_audio_offer_with_params, build_sdp_answer_with_params, parse_audio_sdp, AudioCodec,
    CallEndReason, SdpAddrType, SdpAudioDescription, VoiceCallStateMachine, VoiceLegKind,
    VoiceParams, VoiceRuntimeError,
};

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

/// A VoLTE voice call orchestrator wrapping the shared call state machine with
/// VoLTE-specific SDP construction. Gateway mode: no local audio, only signaling
/// + (elsewhere) RTP relay.
pub struct VolteVoiceCall {
    machine: VoiceCallStateMachine,
    params: VoiceParams,
    local_media: MediaEndpoint,
}

impl VolteVoiceCall {
    /// Create a new orchestrator bound to the local media relay endpoint.
    pub fn new(config: &VolteConfig, local_media: MediaEndpoint) -> Self {
        let params = volte_voice_params(config);
        Self {
            machine: VoiceCallStateMachine::with_params(params.clone()),
            params,
            local_media,
        }
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
    pub fn negotiate_answer(
        &self,
        offer_body: &[u8],
    ) -> Result<SdpNegotiation, VoiceRuntimeError> {
        let offer =
            parse_audio_sdp(offer_body).map_err(|_| VoiceRuntimeError::NoCommonCodec)?;
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
        assert!(!on.carrier_fallback_enabled, "no audio hardware -> no carrier leg");
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
        assert_eq!(offer.codecs.first().map(|c| c.codec), Some(AudioCodec::AmrWb));
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
}
