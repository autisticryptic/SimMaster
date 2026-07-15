//! Call-screening and voice-inbox business layer.
//!
//! This module intentionally contains no SIP socket, RTP, Trunk, Asterisk or
//! browser WebRTC implementation. A future media adapter supplies caller
//! metadata and speech transcripts; the stable business layer decides whether
//! to forward, screen, keep as voicemail or reject, then persists the result.

pub mod screening;

use serde::{Deserialize, Serialize};

/// Contract advertised to future media ingress implementations. Keeping this
/// small prevents the call-screening policy from depending on one PBX choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaIngressCapabilities {
    pub adapter: String,
    pub signaling_ready: bool,
    pub audio_capture_ready: bool,
    pub browser_webrtc_ready: bool,
    pub reason: String,
}

impl MediaIngressCapabilities {
    pub fn unwired() -> Self {
        Self {
            adapter: "unwired".to_string(),
            signaling_ready: false,
            audio_capture_ready: false,
            browser_webrtc_ready: false,
            reason: "media_ingress_not_selected".to_string(),
        }
    }
}

impl Default for MediaIngressCapabilities {
    fn default() -> Self {
        Self::unwired()
    }
}
