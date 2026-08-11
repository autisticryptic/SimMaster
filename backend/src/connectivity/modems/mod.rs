//! Access implementations ("modems"): the pluggable units that build a
//! protected SIP channel on top of the shared [`super::core`].
//!
//!   - [`ims`] — the user-space IMS soft-stack: the host builds the IMS stack
//!     itself (VoWiFi over a user-space IKEv2/ESP tunnel, VoLTE over the kernel
//!     `ip xfrm` IMS bearer). VoLTE and VoWiFi share the same core and cross-call
//!     each other, so they live together as one unit.
//!
//! Future turnkey chip drivers (e.g. Quectel EC25, UNISOC Air724) belong here as
//! sibling units: a single chip driver covers multiple access types (VoLTE +
//! VoWiFi) by delegating to modem firmware over AT, so it is organized by chip,
//! not split by access type.

pub mod ims;
