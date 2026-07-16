//! Per-line SIP Trunk gateway.
//!
//! D3b provides the persisted profile and runtime/status boundary. D4 adds the
//! per-line UDP endpoint and outbound REGISTER client. The call/media bridge is
//! deliberately kept for D5-D6.

pub mod digest;
pub mod driver;
pub mod runtime;
pub mod sip;
pub mod transport;
