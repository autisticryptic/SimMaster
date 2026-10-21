//! Jegotrip hosting adapter contracts established from offline APK analysis.
//!
//! Account-login UI is optional when the user supplies an existing SDK session
//! credential and gateway credential. They are two separate authentication
//! layers, neither is IMS AKA. No Android library is loaded here and no guessed
//! HTTP/SIP transport is provided. A real backend must implement the SDK path
//! before this provider can be installed as available.
pub mod credentials;
pub mod session;

pub const PROVIDER_ID: &str = "app_voice";

#[cfg(test)]
mod tests;
