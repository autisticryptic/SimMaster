//! User-space IMS soft-stack: the access legs where the host builds the
//! protected SIP channel itself, on top of the shared [`crate::connectivity::core`].
//!
//! The two legs differ only in *how they build a protected SIP channel*:
//!
//!   - [`vowifi`] — WiFi → ePDG, protected by a user-space IKEv2/ESP stack.
//!   - [`volte`]  — LTE → IMS APN bearer, protected by the kernel IPsec stack
//!     (`ip xfrm`); also hosts the VoLTE voice (gateway/relay) path.
//!
//! They share the same IMS core and cross-reference each other (VoLTE reuses
//! VoWiFi's USIM-AKA, SMS codec, and RTP helpers), so they form one soft-stack
//! unit rather than independent legs. A future CS/baseband leg or ViLTE video
//! leg reusing VoLTE belongs here too.

pub mod effective_profile;
pub mod profile_override;
pub mod volte;
pub mod vowifi;
