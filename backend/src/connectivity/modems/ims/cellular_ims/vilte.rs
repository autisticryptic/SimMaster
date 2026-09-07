//! Compatibility shim: the shared IMS video (ViLTE) SDP/H.264 types now live in
//! `crate::connectivity::core::ims_video`. This re-export preserves the
//! historical VoLTE module path while VoLTE/VoWiFi live adapters migrate.

pub use crate::connectivity::core::ims_video::*;
