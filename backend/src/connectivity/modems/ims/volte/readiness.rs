//! QMI provisioning readiness gate (beta2 `/run/qmi_auto_activate.ready`).
//!
//! # Why this exists (beta2 alignment)
//!
//! beta2 waits for an initial QMI UIM provisioning step to settle before it
//! starts driving the modem for VoLTE. It does this by polling for a marker
//! file, `/run/qmi_auto_activate.ready`, that a separate one-shot writes once the
//! SIM's automatic data activation has completed. The observed log lines are:
//!   - `Waiting for initial QMI UIM provisioning to settle`
//!   - `QMI auto-activate ready marker did not appear; continuing with modem
//!     readiness checks`
//!
//! The gate is **best-effort, not a hard prerequisite**: if the marker never
//! appears within the timeout, beta2 continues anyway and relies on the
//! subsequent modem-readiness checks (registration/attach state). Blocking VoLTE
//! forever on a marker that a given deployment may never write would be worse
//! than proceeding, so the timeout falls through rather than failing.
//!
//! # Testability
//!
//! The polling loop and the fall-through decision are separated from the clock
//! and the filesystem: [`await_ready_marker`] takes a `now`-style predicate and a
//! sleep, so the timeout/observed/fell-through outcomes are unit-tested without
//! touching a real `/run` or a real timer.

use std::path::Path;
use std::time::Duration;

use tokio::time::sleep;

/// Marker file a separate QMI auto-activate one-shot writes once initial UIM
/// provisioning has settled. Matches beta2's path exactly.
pub const READY_MARKER_PATH: &str = "/run/qmi_auto_activate.ready";

/// How long to wait for the marker before continuing without it.
pub const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// How often to poll for the marker while waiting.
pub const READY_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Outcome of waiting for the provisioning-ready marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessOutcome {
    /// The marker was present (either immediately or before the timeout).
    Ready,
    /// The marker never appeared within the timeout. Per beta2, the caller
    /// continues with the ordinary modem-readiness checks anyway.
    TimedOut,
}

impl ReadinessOutcome {
    /// The beta2 log line for this outcome.
    pub fn log_message(self) -> &'static str {
        match self {
            ReadinessOutcome::Ready => "Initial QMI UIM provisioning settled",
            ReadinessOutcome::TimedOut => {
                "QMI auto-activate ready marker did not appear; continuing with modem readiness checks"
            }
        }
    }
}

/// Wait for the real `/run/qmi_auto_activate.ready` marker, then continue
/// regardless of the outcome (the marker is advisory, matching beta2).
pub async fn wait_for_qmi_ready() -> ReadinessOutcome {
    await_ready_marker(
        || Path::new(READY_MARKER_PATH).exists(),
        READY_TIMEOUT,
        READY_POLL_INTERVAL,
        sleep,
    )
    .await
}

/// Poll `is_present` until it returns true or `timeout` elapses, sleeping
/// `interval` between polls via the injected `sleep_for`.
///
/// Separated from the concrete filesystem/clock so the three outcomes
/// (immediately ready, ready after polling, timed out) are unit-testable. The
/// total wait is bounded by `timeout`; a final check is performed right at the
/// deadline so a marker that lands on the last tick is still observed.
pub async fn await_ready_marker<P, S, Fut>(
    mut is_present: P,
    timeout: Duration,
    interval: Duration,
    mut sleep_for: S,
) -> ReadinessOutcome
where
    P: FnMut() -> bool,
    S: FnMut(Duration) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    if is_present() {
        return ReadinessOutcome::Ready;
    }
    // Guard against a zero interval turning this into a busy-spin.
    let step = if interval.is_zero() {
        READY_POLL_INTERVAL
    } else {
        interval
    };
    let mut waited = Duration::ZERO;
    while waited < timeout {
        let remaining = timeout - waited;
        let this_step = step.min(remaining);
        sleep_for(this_step).await;
        waited += this_step;
        if is_present() {
            return ReadinessOutcome::Ready;
        }
    }
    ReadinessOutcome::TimedOut
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    async fn no_sleep(_: Duration) {}

    #[tokio::test]
    async fn marker_present_immediately_is_ready_without_sleeping() {
        let slept = Cell::new(false);
        let outcome = await_ready_marker(
            || true,
            Duration::from_secs(20),
            Duration::from_secs(1),
            |d| {
                slept.set(true);
                async move {
                    let _ = d;
                }
            },
        )
        .await;
        assert_eq!(outcome, ReadinessOutcome::Ready);
        assert!(
            !slept.get(),
            "must not sleep when the marker is already present"
        );
    }

    #[tokio::test]
    async fn marker_appearing_after_a_few_polls_is_observed() {
        let polls = Cell::new(0u32);
        let outcome = await_ready_marker(
            || {
                let n = polls.get();
                polls.set(n + 1);
                // Absent on the first two checks, present on the third.
                n >= 2
            },
            Duration::from_secs(20),
            Duration::from_secs(1),
            no_sleep,
        )
        .await;
        assert_eq!(outcome, ReadinessOutcome::Ready);
    }

    #[tokio::test]
    async fn marker_never_appearing_times_out_and_falls_through() {
        let outcome = await_ready_marker(
            || false,
            Duration::from_secs(3),
            Duration::from_secs(1),
            no_sleep,
        )
        .await;
        assert_eq!(outcome, ReadinessOutcome::TimedOut);
        assert_eq!(
            outcome.log_message(),
            "QMI auto-activate ready marker did not appear; continuing with modem readiness checks"
        );
    }

    #[tokio::test]
    async fn a_zero_interval_does_not_busy_spin_forever() {
        // With a zero interval the loop must still terminate at the timeout.
        let outcome =
            await_ready_marker(|| false, Duration::from_secs(2), Duration::ZERO, no_sleep).await;
        assert_eq!(outcome, ReadinessOutcome::TimedOut);
    }
}
