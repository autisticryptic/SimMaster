//! Priority-ordered SMS send routing with fallback.
//!
//! Sending an SMS is the simpler half of the orchestration problem: pick the
//! highest-priority enabled leg that is ready, try it, and on failure fall
//! through to the next one. This module is the **pure planner** for that: it
//! turns a policy + a per-leg readiness snapshot into an ordered list of
//! candidate legs, and it interprets the result of an attempt to decide whether
//! to advance to the next candidate.
//!
//! The actual send IO lives with the caller (the HTTP handler / a future
//! `AccessLeg` dispatcher), because the VoWiFi and VoLTE send paths need real
//! runtime handles and unix-only sockets. Keeping the *decision* logic here
//! makes selection and fallback fully
//! unit-testable with no IO — matching the design's "enum dispatch, pure
//! planner" approach (§4.3).

use crate::platform::config::{AccessPathKind, SmsPathPolicy};

/// Why a leg was skipped or an attempt was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Leg disabled in the policy.
    Disabled,
    /// Leg enabled but not ready (not registered / no connection).
    NotReady,
    /// Leg was disabled *after* selection, mid-flight.
    DisabledMidFlight,
}

/// The outcome of a single leg send attempt, reported back by the caller so the
/// router can decide whether to fall through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// Sent (accepted by the network / modem). Terminal success.
    Sent,
    /// Failed on the wire (rejected, timeout, transport error). Fall through to
    /// the next candidate.
    Failed,
    /// The leg became unavailable mid-flight and the send had not yet gone out.
    DisabledMidFlight,
}

/// The router's decision after an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteDecision {
    /// Stop; the message was sent on `kind`.
    Delivered { kind: AccessPathKind },
    /// Try the next candidate leg.
    TryNext { next: AccessPathKind },
    /// Stop; all candidates exhausted (or policy said to fail). `attempted` is
    /// the ordered list of legs that were actually tried.
    Exhausted { attempted: Vec<AccessPathKind> },
}

/// Readiness of one send leg at planning time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegSendReadiness {
    pub kind: AccessPathKind,
    pub ready: bool,
}

/// A planned send candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub kind: AccessPathKind,
}

/// Build the ordered candidate list: every enabled+ready leg, in priority
/// order. Disabled or not-ready legs are dropped. The result may be empty (no
/// path can currently send).
pub fn plan_candidates(policy: &SmsPathPolicy, readiness: &[LegSendReadiness]) -> Vec<Candidate> {
    let ready_for = |kind: AccessPathKind| readiness.iter().any(|r| r.kind == kind && r.ready);
    policy
        .enabled_layers()
        .filter(|&kind| ready_for(kind))
        .map(|kind| Candidate { kind })
        .collect()
}

/// A small state machine that drives the fallback walk. The caller creates it
/// from a candidate plan, then repeatedly: reads [`current`](Self::current),
/// performs the send, and feeds the result to [`record`](Self::record) to get
/// the next [`RouteDecision`].
#[derive(Debug, Clone)]
pub struct SendRouter {
    candidates: Vec<Candidate>,
    cursor: usize,
    attempted: Vec<AccessPathKind>,
}

impl SendRouter {
    /// Create a router from a policy + readiness snapshot.
    pub fn new(policy: &SmsPathPolicy, readiness: &[LegSendReadiness]) -> Self {
        Self {
            candidates: plan_candidates(policy, readiness),
            cursor: 0,
            attempted: Vec::new(),
        }
    }

    /// Number of planned candidates.
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    /// The leg the caller should attempt next, or `None` if the plan is
    /// exhausted (caller should treat as failure — nothing to send on).
    pub fn current(&self) -> Option<AccessPathKind> {
        self.candidates.get(self.cursor).map(|c| c.kind)
    }

    /// The ordered list of legs planned (for diagnostics / logging).
    pub fn planned(&self) -> Vec<AccessPathKind> {
        self.candidates.iter().map(|c| c.kind).collect()
    }

    /// Feed the outcome of attempting [`current`](Self::current) and get the
    /// next decision. Advances the internal cursor on fallthrough.
    pub fn record(&mut self, outcome: AttemptOutcome) -> RouteDecision {
        let current = match self.current() {
            Some(kind) => kind,
            None => {
                return RouteDecision::Exhausted {
                    attempted: self.attempted.clone(),
                }
            }
        };
        self.attempted.push(current);

        match outcome {
            AttemptOutcome::Sent => RouteDecision::Delivered { kind: current },
            AttemptOutcome::Failed => self.advance(),
            AttemptOutcome::DisabledMidFlight => self.advance(),
        }
    }

    fn advance(&mut self) -> RouteDecision {
        self.cursor += 1;
        match self.current() {
            Some(next) => RouteDecision::TryNext { next },
            None => RouteDecision::Exhausted {
                attempted: self.attempted.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(kind: AccessPathKind, ready: bool) -> LegSendReadiness {
        LegSendReadiness { kind, ready }
    }

    #[test]
    fn automatic_mode_uses_fixed_order() {
        let p = SmsPathPolicy::default();
        let plan = plan_candidates(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
                ready(AccessPathKind::Cs, true),
            ],
        );
        assert_eq!(
            plan.iter().map(|c| c.kind).collect::<Vec<_>>(),
            vec![
                AccessPathKind::Vowifi,
                AccessPathKind::Volte,
                AccessPathKind::Cs
            ]
        );
    }

    #[test]
    fn automatic_mode_skips_unready_legs() {
        let p = SmsPathPolicy::default();
        let plan = plan_candidates(
            &p,
            &[
                ready(AccessPathKind::Vowifi, false),
                ready(AccessPathKind::Volte, false),
                ready(AccessPathKind::Cs, true),
            ],
        );
        assert_eq!(
            plan.iter().map(|c| c.kind).collect::<Vec<_>>(),
            vec![AccessPathKind::Cs]
        );
    }

    #[test]
    fn forced_vowifi_never_plans_chargeable_fallbacks() {
        let p = SmsPathPolicy {
            force_vowifi_send: true,
            ..SmsPathPolicy::default()
        };
        let plan = plan_candidates(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
                ready(AccessPathKind::Cs, true),
            ],
        );
        assert_eq!(
            plan.iter()
                .map(|candidate| candidate.kind)
                .collect::<Vec<_>>(),
            vec![AccessPathKind::Vowifi]
        );
    }

    #[test]
    fn first_leg_success_delivers_without_fallthrough() {
        let p = SmsPathPolicy::default();
        let mut r = SendRouter::new(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
                ready(AccessPathKind::Cs, true),
            ],
        );
        assert_eq!(r.current(), Some(AccessPathKind::Vowifi));
        assert_eq!(
            r.record(AttemptOutcome::Sent),
            RouteDecision::Delivered {
                kind: AccessPathKind::Vowifi
            }
        );
    }

    #[test]
    fn failure_falls_through_to_next_leg() {
        let p = SmsPathPolicy::default();
        let mut r = SendRouter::new(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
                ready(AccessPathKind::Cs, true),
            ],
        );
        assert_eq!(
            r.record(AttemptOutcome::Failed),
            RouteDecision::TryNext {
                next: AccessPathKind::Volte
            }
        );
        assert_eq!(r.current(), Some(AccessPathKind::Volte));
        assert_eq!(
            r.record(AttemptOutcome::Sent),
            RouteDecision::Delivered {
                kind: AccessPathKind::Volte
            }
        );
    }

    #[test]
    fn all_failures_exhaust_with_attempt_history() {
        let p = SmsPathPolicy::default();
        let mut r = SendRouter::new(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
                ready(AccessPathKind::Cs, true),
            ],
        );
        assert_eq!(
            r.record(AttemptOutcome::Failed),
            RouteDecision::TryNext {
                next: AccessPathKind::Volte
            }
        );
        assert_eq!(
            r.record(AttemptOutcome::Failed),
            RouteDecision::TryNext {
                next: AccessPathKind::Cs
            }
        );
        assert_eq!(
            r.record(AttemptOutcome::Failed),
            RouteDecision::Exhausted {
                attempted: vec![
                    AccessPathKind::Vowifi,
                    AccessPathKind::Volte,
                    AccessPathKind::Cs
                ]
            }
        );
        assert_eq!(r.current(), None);
    }

    #[test]
    fn mid_flight_unavailability_falls_through() {
        let p = SmsPathPolicy::default();
        let mut r = SendRouter::new(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
            ],
        );
        assert_eq!(
            r.record(AttemptOutcome::DisabledMidFlight),
            RouteDecision::TryNext {
                next: AccessPathKind::Volte
            }
        );
    }

    #[test]
    fn forced_vowifi_failure_is_terminal() {
        let p = SmsPathPolicy {
            force_vowifi_send: true,
            ..SmsPathPolicy::default()
        };
        let mut r = SendRouter::new(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
            ],
        );
        assert_eq!(
            r.record(AttemptOutcome::Failed),
            RouteDecision::Exhausted {
                attempted: vec![AccessPathKind::Vowifi]
            }
        );
    }

    #[test]
    fn empty_plan_reports_exhausted_immediately() {
        let p = SmsPathPolicy::default();
        let mut r = SendRouter::new(
            &p,
            &[
                ready(AccessPathKind::Vowifi, false),
                ready(AccessPathKind::Volte, false),
                ready(AccessPathKind::Cs, false),
            ],
        );
        assert_eq!(r.current(), None);
        assert_eq!(r.candidate_count(), 0);
        assert_eq!(
            r.record(AttemptOutcome::Failed),
            RouteDecision::Exhausted { attempted: vec![] }
        );
    }
}
