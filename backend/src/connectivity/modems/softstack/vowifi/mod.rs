//! Clean-room VoWiFi planning and diagnostics scaffolding.
//!
//! This module intentionally contains only SimAdmin-owned names and data
//! structures. Carrier profile identifiers are based on public brand + PLMN
//! metadata, not on any third-party binary or private preset naming.

pub mod aka;
pub mod channel;
pub mod dataplane;
pub mod diagnostics;
pub mod eap_aka;
pub mod epdg;
pub mod executor;
pub mod flow;
pub mod identity;
pub mod ike;
pub mod ike_codec;
pub mod ike_dh;
pub mod ike_eap;
pub mod ike_encrypted;
pub mod ike_events;
pub mod ike_identity;
pub mod ike_keys;
pub mod ike_payloads;
pub mod ike_retransmit;
pub mod ike_state;
pub mod ims;
pub mod live;
pub mod profile_import;
pub mod profile_record;
pub mod profile_store;
pub mod profiles;
pub mod qmi_uim;
pub mod restore;
pub mod runtime;
pub mod socks5;
pub mod sms {
    pub use crate::connectivity::core::sms_codec::*;
}
pub mod soak;
pub mod stability;
pub mod transport;
pub mod tun_gateway;
pub mod voice;
