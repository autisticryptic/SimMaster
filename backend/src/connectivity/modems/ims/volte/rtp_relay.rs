//! Compatibility shim: the shared media-plane RTP relay now lives in
//! `crate::connectivity::core::media`. This re-export preserves the historical
//! VoLTE module path while VoLTE/VoWiFi live adapters migrate incrementally.

pub use crate::connectivity::core::media::*;
