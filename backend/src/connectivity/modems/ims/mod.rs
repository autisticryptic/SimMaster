//! User-space IMS soft-stack: the access legs where the host builds the
//! protected SIP channel itself, on top of the shared [`crate::connectivity::core`].
//!
//! The two legs differ only in *how they build a protected SIP channel*:
//!
//!   - [`vowifi`] — WiFi → ePDG, protected by a user-space IKEv2/ESP stack.
//!   - [`cellular_ims`] — 4G/5G → IMS bearer, protected by the kernel IPsec stack
//!     (`ip xfrm`); carries SMS, supplementary services and voice.
//!
//! They share the same IMS core and cross-reference each other (cellular IMS reuses
//! VoWiFi's USIM-AKA, SMS codec, and RTP helpers), so they form one soft-stack
//! unit rather than independent legs. CS remains a separate business fallback,
//! not a third IMS registration.

pub mod access_network;
pub mod cellular_ims;
pub mod effective_profile;
pub mod profile_override;
pub mod vowifi;
