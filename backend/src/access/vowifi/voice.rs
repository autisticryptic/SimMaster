//! VoWiFi compatibility adapter for the shared IMS voice core.
//!
//! The transport-neutral call state machine, SDP and RTP codecs now live in
//! `crate::ims::voice`. This re-export preserves the established VoWiFi module
//! API while live VoWiFi transport code is migrated incrementally.

#![allow(dead_code)]

pub use crate::ims::voice::*;

use super::profiles::CarrierProfile;

pub fn voice_params(profile: &'static CarrierProfile) -> VoiceParams {
    VoiceParams {
        preferred_codecs: profile
            .voice
            .preferred_codecs
            .iter()
            .map(|value| value.to_string())
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

impl VoiceCallStateMachine {
    pub fn new(profile: &'static CarrierProfile) -> Self {
        Self::with_params(voice_params(profile))
    }
}

pub fn build_mo_audio_offer(
    profile: &'static CarrierProfile,
    connection_addr: &str,
    addr_type: SdpAddrType,
    media_port: u16,
) -> SdpAudioDescription {
    build_mo_audio_offer_with_params(
        &voice_params(profile),
        connection_addr,
        addr_type,
        media_port,
    )
}

pub fn build_profile_codec_offer(profile: &'static CarrierProfile) -> Vec<SdpCodec> {
    build_codec_offer_with_params(&voice_params(profile))
}

pub fn build_sdp_answer(
    profile: &'static CarrierProfile,
    offer: &SdpAudioDescription,
    connection_addr: &str,
    addr_type: SdpAddrType,
    media_port: u16,
) -> Result<SdpAudioDescription, VoiceRuntimeError> {
    build_sdp_answer_with_params(
        &voice_params(profile),
        offer,
        connection_addr,
        addr_type,
        media_port,
    )
}

pub fn build_dry_run_voice_snapshot(profile: &'static CarrierProfile) -> VoiceRuntimePublicState {
    build_dry_run_voice_snapshot_with_params(voice_params(profile))
}

pub fn select_voice_leg(
    profile: &'static CarrierProfile,
    vowifi_ready: bool,
    carrier_usb_audio_available: bool,
) -> VoiceLegKind {
    select_voice_leg_with_params(
        &voice_params(profile),
        vowifi_ready,
        carrier_usb_audio_available,
    )
}
