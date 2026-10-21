//! Application-specific voice/messaging, separate from SIM/IMS transports.
//! Providers share lifecycle/capability contracts, not login or wire protocols.
pub mod providers;
pub mod registry;
