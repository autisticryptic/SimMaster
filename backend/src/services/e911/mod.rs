//! E911 address and entitlement orchestration.
//!
//! Per `docs/E911_IMPLEMENTATION_RESEARCH.md`, this layer owns per-line/SIM
//! orchestration, permissions, state and audit. The protocol (TS.43 query,
//! HTTP EAP-AKA) lives in `connectivity::core::entitlement`; VoWiFi runtime
//! only consumes the final capability and never executes address forms itself.
//!
//! Security invariants:
//!   - entitlement URLs come only from the sealed catalog and pass the SSRF
//!     guard (HTTPS + host allow-list + DNS/IP re-check + redirect re-check);
//!   - entitlement state and secrets are stored separately from the user
//!     override file; background work never rewrites `SimOverride`;
//!   - addresses / ICCID / IMSI / EID / IMEI and AKA material never reach logs;
//!   - a successful websheet completion always triggers a fresh entitlement
//!     re-query before anything is reported as confirmed.

pub mod orchestrator;
pub mod registry;
pub mod ssrf;
pub mod state_store;
pub mod ts43;

pub use orchestrator::{EntitlementRequestContext, SimAkaProvider};
