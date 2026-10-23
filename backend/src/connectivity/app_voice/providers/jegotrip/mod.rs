//! Jegotrip hosting adapter contracts established from offline APK analysis.
//!
//! Account-login UI is optional when the user supplies an existing SDK session
//! credential and gateway credential. They are two separate authentication
//! layers, neither is IMS AKA. No Android library is loaded here and no guessed
//! HTTP/SIP transport is provided. A real backend must implement the SDK path
//! before this provider can be installed as available.
pub mod batch_scheduler;
pub mod arc_protocol;
pub mod arc_session;
pub mod credentials;
pub mod controller;
pub mod offline_backend;
pub mod packet_stream;
pub mod packet_transport;
pub mod router_packets;
pub mod native_events;
pub mod rpc_arguments;
pub mod rpc_batch;
pub mod rpc_bootstrap;
pub mod rpc_envelope;
pub mod rpc_incoming;
pub mod rpc_login;
pub mod rpc_pending;
pub mod rpc_stream;
pub mod session;
pub mod transport_packets;
pub mod tls_engine;
pub mod tls_plaintext;
pub mod transport_actor;

// Keep the existing API/provider id stable while moving the implementation.
pub const PROVIDER_ID: &str = "app_voice";
pub const LABEL: &str = "无忧行 / 应用语音";

#[cfg(test)]
mod tests;
