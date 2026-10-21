//! Each provider owns its own authentication, signaling, media and tariffs.
//! Reserved declarations are not installed implementations.
pub mod google_voice;
pub mod imessage;
pub mod jegotrip;
pub mod telegram;

pub const RESERVED: &[(&str, &str)] = &[
    (jegotrip::PROVIDER_ID, jegotrip::LABEL),
    (google_voice::PROVIDER_ID, google_voice::LABEL),
    (telegram::PROVIDER_ID, telegram::LABEL),
    (imessage::PROVIDER_ID, imessage::LABEL),
];

/// Declaration only; these capabilities do not grant operational availability.
pub fn reserved_capabilities(id: &str) -> Vec<crate::connectivity::app_voice::registry::VoiceCapability> {
    use crate::connectivity::app_voice::registry::VoiceCapability;
    match id {
        jegotrip::PROVIDER_ID => vec![VoiceCapability::Voice, VoiceCapability::Messaging],
        google_voice::PROVIDER_ID => vec![VoiceCapability::Voice, VoiceCapability::Messaging],
        telegram::PROVIDER_ID => vec![VoiceCapability::Voice, VoiceCapability::Messaging],
        imessage::PROVIDER_ID => vec![VoiceCapability::Messaging],
        _ => vec![],
    }
}
