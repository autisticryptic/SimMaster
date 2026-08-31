//! Native VoLTE (IMS over LTE) SMS module.
//!
//! Clean-room implementation of SMS-over-IMS on the LTE cellular path, written
//! from public 3GPP/IETF specifications:
//!   - 3GPP TS 24.229 (IMS SIP), TS 24.341 (SMS over IP), TS 24.011 (RP/CP),
//!     TS 23.040 (TPDU), TS 24.301 (EPS bearer), TS 33.203 (IMS access security)
//!   - RFC 3261 (SIP), RFC 3310 (HTTP Digest AKAv1), RFC 4169 (AKAv2),
//!     RFC 2617/2104 (Digest/HMAC)
//!
//! Architecture: SIM AKA runs on the USIM hardware (reused via
//! `vowifi::qmi_uim`); the IMS APN uses a SimAdmin-owned native QMI bearer whose
//! netdev is moved into the line's mandatory UE namespace; SIP signaling
//! integrity is protected by the Linux kernel IPsec stack (`ip xfrm`) inside
//! that namespace.
//!
//! Reuse policy (confirmed by design): SIM AKA (`vowifi::qmi_uim`) and 3GPP SMS
//! codec (`vowifi::sms`) are reused directly; they are transport-agnostic.

#![allow(dead_code)]
#![allow(unused_imports)]

pub mod bearer;
pub mod channel;
// `data_path` (the direct-QMI IPv6 WDS preflight) was removed: it was disabled
// by default behind an env var, hardcoded to IPv6-only, and probed
// `a2-mux-rmnet*` MUX ports that do not exist on bam-dmux targets. It never ran,
// and running it would have contended with ModemManager for the primary QMI
// port.
//
// The direct-QMI implementation now lives behind the device transport and
// resolves a real secondary netdev before the runtime migrates it into the UE
// namespace. There is no primary ModemManager/host bearer alternative.
pub mod data_slot;
pub mod digest_aka;
pub mod errors;
pub mod identity;
pub mod ipsec;
pub mod live;
pub mod native_bearer;
pub mod pcscf;
pub mod plan;
pub mod readiness;
pub mod rtp_relay;
pub mod runtime;
pub mod sip;
pub mod sms;
pub mod vilte;
pub mod voice;

pub use errors::VolteError;
pub use runtime::{
    RegistrationMode, VoltePhase, VolteRuntime, VolteRuntimeStatus, VolteSnapshot, VolteStage,
};
