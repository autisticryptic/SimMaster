//! Services domain: application-level business logic and operations that sit
//! above the access legs and hardware.
//!
//!   - [`orchestrator`] — multi-path SMS/voice routing decisions across legs
//!     (send routing, MT listener election, cross-transport dedup); pure logic.
//!   - [`trunk`] — per-line SIP Trunk gateway exposing an established leg.
//!   - [`messaging`] — SMS reception/forwarding + verification-code extraction.
//!   - [`automation`] — the scheduler and its tasks.
//!   - [`notify`] — multi-channel push delivery + its send queue.
//!   - [`system`] — OS-level status, events, OTA updates, device health.
//!   - [`network`] — dynamic DNS management + firewall (iptables) control.
//!   - [`line_registry`] — per-modem/SIM runtime registry that binds each
//!     physical line to its own VoLTE/VoWiFi/Trunk/data-proxy runtimes.

pub mod automation;
pub mod line_registry;
pub mod messaging;
pub mod network;
pub mod notify;
pub mod orchestrator;
pub mod system;
pub mod trunk;
