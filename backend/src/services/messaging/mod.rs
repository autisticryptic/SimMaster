//! Messaging domain: SMS reception/forwarding and verification-code extraction.
//!
//!   - `sms_listener`: watches ModemManager for new SMS and forwards them
//!   - `verification_code`: extracts one-time codes from SMS bodies

pub mod sms_listener;
pub mod verification_code;
