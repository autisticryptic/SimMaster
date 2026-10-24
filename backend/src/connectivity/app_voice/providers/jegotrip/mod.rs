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
pub mod account_config;
pub mod account_store;
pub mod account_inbox;
pub mod account_sms;
pub mod smgw_text;
pub mod account_runtime;
pub mod account_runs;
pub mod account_notifications;
#[cfg(test)]
pub(crate) mod account_test_support;
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
pub mod rpc_logout;
pub mod rpc_pending;
pub mod rpc_stream;
pub mod session;
pub mod transport_packets;
pub mod tls_engine;
pub mod tls_plaintext;
pub mod transport_actor;
pub mod runtime_connector;
pub mod runtime_backend;
pub mod legacy_config;
pub mod object_reference;
pub mod deployment;
pub mod native_router;
pub mod native_channel;

// Keep the existing API/provider id stable while moving the implementation.
pub const PROVIDER_ID: &str = "app_voice";
pub const LABEL: &str = "无忧行 / 应用语音";

#[cfg(test)]
mod tests;
