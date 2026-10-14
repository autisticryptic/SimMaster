//! Cellular adapter names and legacy regression entry point for the shared,
//! I/O-free REGISTER endpoint/lease retry policy. No behavior is reimplemented.
pub(super) use crate::connectivity::core::register_retry::{
    EndpointRetryState, RefreshRetryDecision, RetryAdmission, RetryStopReason,
    refresh_wait_decision,
};

#[cfg(test)]
use std::{net::SocketAddr, time::{Duration, Instant}};
#[cfg(test)]
use crate::connectivity::core::{
    register::{RegisterFailureKind, RetryAfter},
    register_retry::{MAX_ENDPOINT_RETRY_ENTRIES, MAX_ENDPOINT_RETRY_DELAY, TRANSPORT_RETRY_DELAY},
};
#[cfg(test)]
#[path = "register_retry_tests.rs"]
mod tests;
