//! Jegotrip hosting adapter contracts established from offline APK analysis.
//!
//! Account-login UI is optional when the user supplies an existing SDK session
//! credential and gateway credential. They are two separate authentication
//! layers, neither is IMS AKA. No Android library is loaded here and no guessed
//! HTTP/SIP transport is provided. A real backend must implement the SDK path
//! before this provider can be installed as available.
pub mod credentials;
pub mod controller;
pub mod native_events;
pub mod rpc_arguments;
pub mod session;

// Keep the existing API/provider id stable while moving the implementation.
pub const PROVIDER_ID: &str = "app_voice";
pub const LABEL: &str = "无忧行 / 应用语音";

#[cfg(test)]
mod tests;
