//! I/O-free REGISTER retry policy shared by cellular and WiFi access owners.
//!
//! Keep this state outside live sessions/profile loops. The caller separately
//! validates its physical subscription and worker before checking/observing.
//! Key the logical P-CSCF address/port, not the profile, local socket, security
//! port, or mutable modem owner. This policy never authenticates or sleeps.
use std::{net::SocketAddr, time::{Duration, Instant}};
use super::register::{RegisterFailureKind, RegisterFailureMetadata, RetryAfter};

pub(crate) const MAX_ENDPOINT_RETRY_ENTRIES: usize = 32;
pub(crate) const MAX_ENDPOINT_RETRY_DELAY: Duration = Duration::from_secs(86_400);
const TEMPORARY_ENDPOINT_DELAY: Duration = Duration::from_secs(30);
pub(crate) const TRANSPORT_RETRY_DELAY: Duration = Duration::from_secs(5);
const MIN_ENDPOINT_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryStopReason {
    InvalidRetryAfter, RetryAfterTooLong, CapacityExceeded, ClockOverflow, OwnershipUncertain,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryAdmission {
    Allowed,
    Deferred { not_before: Instant },
    /// Only a positively revalidated new subscription may discard this latch.
    Stopped(RetryStopReason),
}
impl RetryAdmission {
    pub(crate) fn is_allowed(self) -> bool { self == Self::Allowed }
    /// Scheduling only: None includes Stopped and must never become zero wait.
    pub(crate) fn remaining(self, now: Instant) -> Option<Duration> {
        match self {
            Self::Deferred { not_before } => Some(not_before.saturating_duration_since(now)),
            Self::Allowed | Self::Stopped(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct EndpointRetryEntry { endpoint: SocketAddr, admission: RetryAdmission }
#[derive(Debug, Default)]
pub(crate) struct EndpointRetryState {
    entries: Vec<EndpointRetryEntry>,
    stopped: Option<RetryStopReason>,
    restart: Option<RetryAdmission>,
    registration_event_wait: Option<RetryAdmission>,
    unregister_wait: Option<RetryAdmission>,
}
impl EndpointRetryState {
    /// After exhausting a discovery pass, prevent a new bearer/profile batch
    /// from jumping over pending endpoint waits. An eligible alternate within
    /// the current pass is not forbidden by this restart-only gate.
    pub(crate) fn pause_restart(&mut self, now: Instant) -> RetryAdmission {
        self.prune(now);
        let admission = if let Some(reason) = self.stopped {
            RetryAdmission::Stopped(reason)
        } else if let Some(not_before) = self.entries.iter().filter_map(|entry| match entry.admission {
            RetryAdmission::Deferred { not_before } => Some(not_before), _ => None,
        }).min() {
            RetryAdmission::Deferred { not_before }
        } else {
            self.entries.first().map(|entry| entry.admission).unwrap_or(RetryAdmission::Allowed)
        };
        self.restart = Some(admission);
        self.restart_admission(now)
    }
    pub(crate) fn restart_admission(&mut self, now: Instant) -> RetryAdmission {
        for slot in [&mut self.restart, &mut self.registration_event_wait, &mut self.unregister_wait] {
            if slot.is_some_and(|admission| matches!(admission,
                RetryAdmission::Deferred { not_before } if not_before <= now)) { *slot = None; }
        }
        merge_admission(merge_admission(self.restart.unwrap_or(RetryAdmission::Allowed),
            self.registration_event_wait.unwrap_or(RetryAdmission::Allowed)),
            self.unregister_wait.unwrap_or(RetryAdmission::Allowed))
    }
    /// A new success cannot erase other endpoints or subscription reg-events.
    pub(crate) fn registered(&mut self) { self.restart = None; }
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|entry| match entry.admission {
            RetryAdmission::Deferred { not_before } => not_before > now,
            RetryAdmission::Stopped(_) => true,
            RetryAdmission::Allowed => false,
        });
    }
    pub(crate) fn check(&mut self, endpoint: SocketAddr, now: Instant) -> RetryAdmission {
        let endpoint = canonical_endpoint(endpoint);
        self.prune(now);
        if let Some(reason) = self.stopped { return RetryAdmission::Stopped(reason); }
        self.entries.iter().find(|entry| entry.endpoint == endpoint)
            .map(|entry| entry.admission).unwrap_or(RetryAdmission::Allowed)
    }
    /// Observe actual final failures. Finish any already permitted bounded
    /// silent-transport probe before recording its transport backoff; a final
    /// temporary response or Retry-After is never a header-format hint.
    pub(crate) fn observe_failure(&mut self, endpoint: SocketAddr,
        failure: &RegisterFailureMetadata, now: Instant) -> RetryAdmission {
        use RegisterFailureKind as Kind;
        let endpoint = canonical_endpoint(endpoint);
        let current = self.check(endpoint, now);
        if matches!(current, RetryAdmission::Stopped(_)) { return current; }
        // A saved 401/423 is not a final response to a failed later send.
        let retry_after = if failure.kind == Kind::Transport { RetryAfter::Absent } else { failure.retry_after };
        let delay = match retry_after {
            RetryAfter::Invalid(_) => return self.record(endpoint, RetryAdmission::Stopped(RetryStopReason::InvalidRetryAfter), now),
            RetryAfter::DelaySeconds(seconds) => Duration::from_secs(u64::from(seconds)),
            RetryAfter::Absent => match failure.kind {
                Kind::TemporaryEndpoint => TEMPORARY_ENDPOINT_DELAY,
                Kind::Transport => TRANSPORT_RETRY_DELAY,
                _ => return current,
            },
        };
        self.defer_delay(endpoint, delay, now)
    }
    /// Deregistration tears down the current pass: a newly learned wait must
    /// gate its successor even after cleanup, an unrelated endpoint's shorter
    /// wait, or a late success. The caller supplies the scope captured BEFORE
    /// the exchange; this never restores a lease or retries Expires: 0.
    pub(crate) fn observe_unregister_failure(&mut self, endpoint: SocketAddr,
        failure: &RegisterFailureMetadata, now: Instant) -> RetryAdmission {
        let admission = self.observe_failure(endpoint, failure, now);
        let previous = self.restart_admission(now);
        self.unregister_wait = Some(merge_admission(previous, admission));
        self.restart_admission(now)
    }
    pub(crate) fn observe_registration_event(&mut self, endpoint: SocketAddr,
        retry_after_seconds: Option<u32>, now: Instant) -> RetryAdmission {
        let endpoint = canonical_endpoint(endpoint);
        let admission = self.defer_delay(endpoint,
            retry_after_seconds.map(|seconds| Duration::from_secs(u64::from(seconds)))
                .unwrap_or(TEMPORARY_ENDPOINT_DELAY), now);
        self.restart_admission(now);
        self.registration_event_wait = Some(merge_admission(
            self.registration_event_wait.unwrap_or(RetryAdmission::Allowed), admission));
        self.restart_admission(now)
    }
    fn defer_delay(&mut self, endpoint: SocketAddr, delay: Duration, now: Instant) -> RetryAdmission {
        let current = self.check(endpoint, now);
        if matches!(current, RetryAdmission::Stopped(_)) { return current; }
        if delay > MAX_ENDPOINT_RETRY_DELAY {
            return self.record(endpoint, RetryAdmission::Stopped(RetryStopReason::RetryAfterTooLong), now);
        }
        let delay = delay.max(MIN_ENDPOINT_RETRY_DELAY);
        let Some(not_before) = now.checked_add(delay) else {
            self.stopped = Some(RetryStopReason::ClockOverflow);
            return RetryAdmission::Stopped(RetryStopReason::ClockOverflow);
        };
        self.record(endpoint, RetryAdmission::Deferred { not_before }, now)
    }
    fn record(&mut self, endpoint: SocketAddr, admission: RetryAdmission, now: Instant) -> RetryAdmission {
        self.prune(now);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.endpoint == endpoint) {
            entry.admission = match (entry.admission, admission) {
                (stopped @ RetryAdmission::Stopped(_), _) => stopped,
                (_, stopped @ RetryAdmission::Stopped(_)) => stopped,
                (RetryAdmission::Deferred { not_before: old }, RetryAdmission::Deferred { not_before: new }) =>
                    RetryAdmission::Deferred { not_before: old.max(new) },
                (_, other) => other,
            };
            return entry.admission;
        }
        if self.entries.len() >= MAX_ENDPOINT_RETRY_ENTRIES {
            self.stopped = Some(RetryStopReason::CapacityExceeded);
            return RetryAdmission::Stopped(RetryStopReason::CapacityExceeded);
        }
        self.entries.push(EndpointRetryEntry { endpoint, admission });
        admission
    }
    /// Caller must separately reject stale ownership and positively dead flow.
    pub(crate) fn observe_refresh_failure(&mut self, endpoint: SocketAddr,
        failure: &RegisterFailureMetadata, now: Instant, remaining_old_lifetime: Duration) -> RefreshRetryDecision {
        let admission = self.observe_failure(endpoint, failure, now);
        if !failure.is_temporary() || remaining_old_lifetime.is_zero() { return RefreshRetryDecision::InvalidateBinding; }
        refresh_wait_decision(admission, now, remaining_old_lifetime)
            .unwrap_or(RefreshRetryDecision::RetainUntilExpiry(remaining_old_lifetime))
    }
    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize { self.entries.len() }
}

#[cfg(test)]
#[path = "unregister_retry_tests.rs"]
mod unregister_tests;

fn merge_admission(first: RetryAdmission, second: RetryAdmission) -> RetryAdmission {
    match (first, second) {
        (stopped @ RetryAdmission::Stopped(_), _) | (_, stopped @ RetryAdmission::Stopped(_)) => stopped,
        (RetryAdmission::Deferred { not_before: old }, RetryAdmission::Deferred { not_before: new }) =>
            RetryAdmission::Deferred { not_before: old.max(new) },
        (deferred @ RetryAdmission::Deferred { .. }, _) | (_, deferred @ RetryAdmission::Deferred { .. }) => deferred,
        _ => RetryAdmission::Allowed,
    }
}
fn canonical_endpoint(endpoint: SocketAddr) -> SocketAddr {
    match endpoint {
        SocketAddr::V6(mut address) => {
            if let Some(ipv4) = address.ip().to_ipv4_mapped() { SocketAddr::new(ipv4.into(), address.port()) }
            else { address.set_flowinfo(0); SocketAddr::V6(address) }
        }
        address => address,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshRetryDecision {
    RetryAfter(Duration),
    /// Wake to invalidate at ORIGINAL expiry, never to send early or renew.
    RetainUntilExpiry(Duration),
    InvalidateBinding,
}
pub(crate) fn refresh_wait_decision(admission: RetryAdmission, now: Instant,
    remaining_old_lifetime: Duration) -> Option<RefreshRetryDecision> {
    if remaining_old_lifetime.is_zero() { return Some(RefreshRetryDecision::InvalidateBinding); }
    match admission {
        RetryAdmission::Allowed => None,
        RetryAdmission::Deferred { not_before } => {
            let wait = not_before.saturating_duration_since(now);
            if wait.is_zero() { None }
            else if wait < remaining_old_lifetime { Some(RefreshRetryDecision::RetryAfter(wait)) }
            else { Some(RefreshRetryDecision::RetainUntilExpiry(remaining_old_lifetime)) }
        }
        RetryAdmission::Stopped(_) => Some(RefreshRetryDecision::RetainUntilExpiry(remaining_old_lifetime)),
    }
}
