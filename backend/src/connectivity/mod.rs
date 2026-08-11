//! Connectivity domain: how the device reaches the operator with the SIM's
//! identity, split into the transport-agnostic protocol core and the pluggable
//! access implementations that build on it.
//!
//!   - [`core`]   — the shared IMS core (SIP wire format, message framing,
//!     Digest-AKA), written and tested exactly once and reused by every leg.
//!   - [`modems`] — the access implementations. Today this is the user-space
//!     soft-stack ([`modems::ims`], VoLTE + VoWiFi); future turnkey chip
//!     drivers (Quectel, UNISOC) become sibling units under `modems`.
//!
//! Dependency direction is one-way: `modems` depends on `core`, never the
//! reverse. This is the seam a future workspace split follows — `core` becomes a
//! shared crate compiled into the binary, each `modems` unit an independent crate.

pub mod core;
pub mod modems;
