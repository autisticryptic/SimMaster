//! Shared IMS core: the transport-agnostic SIP + Digest-AKA logic reused by
//! every IMS access leg (VoWiFi, VoLTE, and future ViLTE).
//!
//! Clean-room from public specifications (RFC 3261 SIP, RFC 2617 Digest,
//! RFC 2104 HMAC, RFC 3310 AKAv1, RFC 4169 AKAv2, 3GPP TS 24.229/33.203).
//!
//! Design: the "how do we build a protected SIP channel" differs per leg
//! (VoWiFi = user-space IKEv2/ESP over ePDG; VoLTE = kernel `ip xfrm` over the
//! IMS APN bearer), but the SIP wire format, message framing, and Digest-AKA
//! proof are identical. Those identical parts live here so they are written and
//! tested exactly once.
//!
//! Neutrality: functions here never depend on a leg-specific error type. The
//! fallible ones return [`ImsError`] (a stable `&'static str` code); each leg
//! maps that into its own error enum at the call site.

#![allow(dead_code)]

pub mod access;
pub mod context;
pub mod digest_aka;
pub mod register;
pub mod sip_frame;
pub mod sip_message;
pub mod sms_codec;
pub mod voice;

use std::fmt;

/// A neutral IMS-core error carrying a stable, greppable code (no leg-specific
/// type). Callers map this into their own error (`VolteError`, `LiveStageError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImsError {
    code: &'static str,
}

impl ImsError {
    pub const fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for ImsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

impl std::error::Error for ImsError {}
