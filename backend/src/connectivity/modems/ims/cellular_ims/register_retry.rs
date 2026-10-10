//! REGISTER endpoint not-before state owned by one SIM/access worker scope.
//!
//! Keep this object outside the profile/REGISTER candidate loop and outside the
//! live session: ordinary reconnect/cleanup must not erase it. The owner must
//! validate its SIM/access generation before both checking and observing it.
//! Key the logical registrar SocketAddr, NOT a profile ID, local socket, header
//! variant, or the security-negotiated server port. An alternate P-CSCF can be
//! tried, but changing the request must not bypass a deferred endpoint.
//!
//! No I/O or sleeps occur here. Call `check` immediately before every new
//! REGISTER exchange, and `observe_failure` before considering a next candidate.
//! UDP retransmits and the core's bounded 401/407/423 exchange are not new
//! compatibility candidates. This does not alter their budgets or authorize
//! authenticated-to-bare fallback.

use std::{net::SocketAddr, time::{Duration, Instant}};

use crate::connectivity::core::register::{
    RegisterFailureKind, RegisterFailureMetadata, RetryAfter,
};

pub(super) const MAX_ENDPOINT_RETRY_ENTRIES: usize = 32;
/// Finite deadlines are bounded. A valid request beyond this supported horizon
/// is NOT clamped to an earlier retry: the endpoint is stopped for this owner.
pub(super) const MAX_ENDPOINT_RETRY_DELAY: Duration = Duration::from_secs(86_400);
const TEMPORARY_ENDPOINT_DELAY: Duration = Duration::from_secs(30);
const TRANSPORT_RETRY_DELAY: Duration = Duration::from_secs(5);
const MIN_ENDPOINT_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryStopReason {
    InvalidRetryAfter,
    RetryAfterTooLong,
    CapacityExceeded,
    ClockOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RetryAdmission {
    Allowed,
    Deferred { not_before: Instant },
    /// No safe finite deadline exists. Only an explicit, revalidated owner
    /// replacement may discard a stopped state; reconnect/manual retry may not.
    Stopped(RetryStopReason),
}

impl RetryAdmission {
    pub(super) fn is_allowed(self) -> bool {
        self == Self::Allowed
    }

    /// A finite wait for diagnostics/scheduling only. `None` includes Stopped;
    /// never use `unwrap_or_default()` as admission. Match the enum or call
    /// `is_allowed()` so invalid/overflow state cannot become a zero delay.
    pub(super) fn remaining(self, now: Instant) -> Option<Duration> {
        match self {
            Self::Deferred { not_before } => Some(not_before.saturating_duration_since(now)),
            Self::Allowed | Self::Stopped(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct EndpointRetryEntry {
    endpoint: SocketAddr,
    admission: RetryAdmission,
}

#[derive(Debug, Default)]
pub(super) struct EndpointRetryState {
    entries: Vec<EndpointRetryEntry>,
    /// Capacity/clock overflow cannot evict an unexpired entry or silently
    /// forget the newest refusal. One bounded latch stops the entire scope.
    stopped: Option<RetryStopReason>,
}

impl EndpointRetryState {
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|entry| match entry.admission {
            RetryAdmission::Deferred { not_before } => not_before > now,
            RetryAdmission::Stopped(_) => true,
            RetryAdmission::Allowed => false,
        });
    }

    pub(super) fn check(&mut self, endpoint: SocketAddr, now: Instant) -> RetryAdmission {
        let endpoint = canonical_endpoint(endpoint);
        self.prune(now);
        if let Some(reason) = self.stopped {
            return RetryAdmission::Stopped(reason);
        }
        self.entries.iter().find(|entry| entry.endpoint == endpoint)
            .map(|entry| entry.admission)
            .unwrap_or(RetryAdmission::Allowed)
    }

    /// Record a failure from an actual attempted endpoint. A remote temporary
    /// refusal always defers the endpoint (even absent/zero Retry-After). Initial
    /// silent-transport compatibility probes, if retained by the caller, must
    /// finish their existing bounded ladder BEFORE this transport observation;
    /// once recorded, neither a profile nor a request-shape change may bypass it.
    pub(super) fn observe_failure(
        &mut self,
        endpoint: SocketAddr,
        failure: &RegisterFailureMetadata,
        now: Instant,
    ) -> RetryAdmission {
        use RegisterFailureKind as Kind;
        let endpoint = canonical_endpoint(endpoint);
        let current = self.check(endpoint, now);
        if matches!(current, RetryAdmission::Stopped(_)) {
            return current;
        }
        // A transport error may retain the previous 401/423. Its Retry-After
        // is not a final response to the send that just failed.
        let retry_after = if failure.kind == Kind::Transport {
            RetryAfter::Absent
        } else {
            failure.retry_after
        };
        let delay = match retry_after {
            RetryAfter::Invalid(_) => {
                return self.record(endpoint, RetryAdmission::Stopped(RetryStopReason::InvalidRetryAfter), now);
            }
            RetryAfter::DelaySeconds(seconds) => Duration::from_secs(u64::from(seconds)),
            RetryAfter::Absent => match failure.kind {
                Kind::TemporaryEndpoint => TEMPORARY_ENDPOINT_DELAY,
                Kind::Transport => TRANSPORT_RETRY_DELAY,
                _ => return current,
            },
        };
        if delay > MAX_ENDPOINT_RETRY_DELAY {
            return self.record(endpoint, RetryAdmission::Stopped(RetryStopReason::RetryAfterTooLong), now);
        }
        // Zero is legal SIP syntax, but is not permission to spin through the
        // static candidate/profile budget in a tight loop after a refusal.
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
                (RetryAdmission::Deferred { not_before: old }, RetryAdmission::Deferred { not_before: new }) => {
                    RetryAdmission::Deferred { not_before: old.max(new) }
                }
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

    /// After the adapter has rolled back staged sockets/SA/auth state, decide
    /// whether an old binding can be conservatively retained. The caller must
    /// separately verify worker/transport ownership; a healthy lease cannot
    /// override a proven-dead outbound flow or failed owner validation.
    pub(super) fn observe_refresh_failure(
        &mut self,
        endpoint: SocketAddr,
        failure: &RegisterFailureMetadata,
        now: Instant,
        remaining_old_lifetime: Duration,
    ) -> RefreshRetryDecision {
        let admission = self.observe_failure(endpoint, failure, now);
        if !failure.is_temporary() || remaining_old_lifetime.is_zero() {
            return RefreshRetryDecision::InvalidateBinding;
        }
        refresh_wait_decision(admission, now, remaining_old_lifetime)
            .unwrap_or(RefreshRetryDecision::RetainUntilExpiry(remaining_old_lifetime))
    }
}

// Different spellings/flow labels of the same network endpoint are not new
// retry destinations. IPv6 link-local scope IDs remain part of the identity.
fn canonical_endpoint(endpoint: SocketAddr) -> SocketAddr {
    match endpoint {
        SocketAddr::V6(mut address) => {
            if let Some(ipv4) = address.ip().to_ipv4_mapped() {
                SocketAddr::new(ipv4.into(), address.port())
            } else {
                address.set_flowinfo(0);
                SocketAddr::V6(address)
            }
        }
        address => address,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefreshRetryDecision {
    /// Keep the original binding and wake to retry on its current protected
    /// channel. Always recheck state and the old expiry before sending.
    RetryAfter(Duration),
    /// Keep receiving/serving only until the ORIGINAL expiry. Wake then to
    /// invalidate, NOT to send an early REGISTER. This is not a lease extension.
    RetainUntilExpiry(Duration),
    InvalidateBinding,
}

/// Pre-send refresh gate. `None` means the endpoint permits a send; a deferred
/// endpoint need not cause blind teardown of an otherwise valid old binding.
/// Pass an already conservative remaining lifetime (clock/owner uncertainty
/// must give zero), never the profile's proposed new Expires.
pub(super) fn refresh_wait_decision(
    admission: RetryAdmission,
    now: Instant,
    remaining_old_lifetime: Duration,
) -> Option<RefreshRetryDecision> {
    if remaining_old_lifetime.is_zero() {
        return Some(RefreshRetryDecision::InvalidateBinding);
    }
    match admission {
        RetryAdmission::Allowed => None,
        RetryAdmission::Deferred { not_before } => {
            let wait = not_before.saturating_duration_since(now);
            if wait.is_zero() {
                None
            } else if wait < remaining_old_lifetime {
                Some(RefreshRetryDecision::RetryAfter(wait))
            } else {
                Some(RefreshRetryDecision::RetainUntilExpiry(remaining_old_lifetime))
            }
        }
        RetryAdmission::Stopped(_) => Some(RefreshRetryDecision::RetainUntilExpiry(remaining_old_lifetime)),
    }
}

#[cfg(test)]
#[path = "register_retry_tests.rs"]
mod tests;
