//! Per-line SIP Trunk gateway.
//!
//! D3b provides the persisted profile and runtime/status boundary, D4 the UDP
//! endpoint and outbound REGISTER client, and D5-D6 the dialog/event/RTP bridge
//! to each line's VoLTE live session.

pub mod access_router;
pub mod bridge;
pub mod dialog;
pub mod digest;
pub mod driver;
pub mod operator;
pub mod runtime;
pub mod sip;
pub mod transport;
