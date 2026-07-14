//! Native VoLTE (IMS over LTE) SMS module.
//!
//! Clean-room implementation of SMS-over-IMS on the LTE cellular path, written
//! from public 3GPP/IETF specifications:
//!   - 3GPP TS 24.229 (IMS SIP), TS 24.341 (SMS over IP), TS 24.011 (RP/CP),
//!     TS 23.040 (TPDU), TS 24.301 (EPS bearer), TS 33.203 (IMS access security)
//!   - RFC 3261 (SIP), RFC 3310 (HTTP Digest AKAv1), RFC 4169 (AKAv2),
//!     RFC 2617/2104 (Digest/HMAC)
//!
//! Architecture ("borrow" the heavy lifting from the system, like the reference
//! behavior): SIM AKA runs on the USIM hardware (reused via `vowifi::qmi_uim`);
//! the IMS APN bearer is established via ModemManager; SIP signaling integrity
//! is protected by the Linux kernel IPsec stack (`ip xfrm`). This module writes
//! only the top SIP + 3GPP SMS business logic.
//!
//! Reuse policy (confirmed by design): SIM AKA (`vowifi::qmi_uim`) and 3GPP SMS
//! codec (`vowifi::sms`) are reused directly; they are transport-agnostic.

#![allow(dead_code)]
#![allow(unused_imports)]

pub mod bearer;
pub mod digest_aka;
pub mod errors;
pub mod identity;
pub mod ipsec;
pub mod pcscf;
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
