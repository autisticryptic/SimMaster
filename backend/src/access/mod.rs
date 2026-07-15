//! IMS/CS access legs.
//!
//! Each submodule is one way to reach the operator with the SIM's identity. They
//! all sit on top of the shared, transport-agnostic IMS core (`crate::ims`) and
//! differ only in *how they build a protected SIP channel*:
//!
//!   - [`vowifi`] — WiFi → ePDG, protected by a user-space IKEv2/ESP stack.
//!   - [`volte`]  — LTE → IMS APN bearer, protected by the kernel IPsec stack
//!     (`ip xfrm`); also hosts the VoLTE voice (gateway/relay) path.
//!
//! Future legs (e.g. a CS/baseband leg, or ViLTE video reusing the VoLTE leg)
//! belong here too. Keeping every leg under one roof makes the "shared core +
//! pluggable access legs" architecture self-evident from the directory tree.

pub mod line_registry;
pub mod volte;
pub mod vowifi;
