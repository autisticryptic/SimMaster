//! Software USIM (five-code) support for host-side IMS.
//!
//! Scope: VoWiFi only. Cellular attach/PDN stay with the modem and its physical
//! card, so a virtual USIM never replaces a modem line's UICC.
//!
//! Layering:
//! - `milenage`: TS 35.206 card-side AKA math (validated with TS 35.208 vectors).
//! - `sqn`: per-profile 48-bit SQN store used for freshness and AUTS.
//! - `store`: at-rest encryption of the five codes.
//!
//! Non-goals: no ISO 7816 file system, no eUICC/EID handling, no modem APDU
//! interception (the baseband's card traffic is not visible to the host).
pub mod milenage;

pub use milenage::{Milenage, AUTN_LEN, AUTS_LEN, KEY_LEN};
