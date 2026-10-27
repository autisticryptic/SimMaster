//! Jegotrip hosting adapter contracts established from offline APK analysis.
//!
//! Account-login UI is optional when the user supplies an existing SDK session
//! credential and gateway credential. They are two separate authentication
//! layers, neither is IMS AKA. No Android library is loaded here and no guessed
//! HTTP/SIP transport is provided. A real backend must implement the SDK path
//! before this provider can be installed as available.
pub mod batch_scheduler;
pub mod arc_protocol;
mod direct_history;
#[cfg(test)]
pub(crate) mod long_reception_test_support;
pub mod arc_session;
pub mod account_config;
pub mod account_store;
pub mod account_inbox;
pub mod account_sms;
pub mod smgw_text;
pub mod account_runtime;
pub mod account_runs;
pub mod account_notifications;
pub(crate) mod call_settings;
pub(crate) mod account_calls;
pub mod account_call_history;
pub(crate) mod message_provider;
#[cfg(test)]
pub(crate) mod account_test_support;
#[cfg(test)]
pub(crate) mod account_call_test_support;
pub mod credentials;
pub mod controller;
pub mod offline_backend;
pub mod packet_stream;
pub mod packet_transport;
pub mod router_packets;
pub mod native_events;
pub mod rpc_arguments;
pub mod rpc_calls;
pub mod call_binding;
pub mod call_delivery;
pub mod call_control;
pub mod rpc_endpoint;
pub mod raw_arc_audio;
pub mod tcp_mpath;
pub(crate) mod native_audio_sdp;
pub(crate) mod tcp_mpath_audio;
pub mod media_endpoint;
pub(crate) mod media_endpoint_runtime;
#[cfg(test)]
pub(crate) mod media_test_support;
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
pub mod runtime_provision;
pub mod runtime_backend;
pub mod legacy_config;
pub mod object_reference;
pub mod deployment;
pub mod native_router;
pub mod native_channel;

// Keep the existing API/provider id stable while moving the implementation.
pub const PROVIDER_ID: &str = "app_voice";
pub const LABEL: &str = "无忧行 / 应用语音";

/// Implemented operations, not current account readiness or a billing grant.
/// Outbound interfaces deliberately do not appear in this list.
pub(crate) const BASIC_INBOUND_OPERATIONS: &[crate::connectivity::app_voice::registry::ProviderOperation] = &[
    crate::connectivity::app_voice::registry::ProviderOperation::IncomingCall,
    crate::connectivity::app_voice::registry::ProviderOperation::AnswerCall,
    crate::connectivity::app_voice::registry::ProviderOperation::HangupCall,
    crate::connectivity::app_voice::registry::ProviderOperation::BidirectionalAudio,
    crate::connectivity::app_voice::registry::ProviderOperation::IncomingMessage,
    crate::connectivity::app_voice::registry::ProviderOperation::MessageHistory,
    crate::connectivity::app_voice::registry::ProviderOperation::MessageNotification,
    crate::connectivity::app_voice::registry::ProviderOperation::IncomingTrunk,
];

#[cfg(test)]
mod tests;
