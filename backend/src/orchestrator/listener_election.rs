//! Active MT-SMS listener election.
//!
//! Receiving SMS is fundamentally different from sending: to receive, *some*
//! leg must be registered and listening, but the same number must **not** be
//! registered on two IMS legs at once or the network delivers the message
//! twice. So at any instant at most **one** IMS leg is the "active listener".
//!
//! This module is pure decision logic: given the configured priority policy and
//! a readiness report per leg, it elects the active listener and decides what
//! the CS/modem listener should do. The caller applies the decision (registers
//! the elected leg, pauses/keeps the CS scan). Keeping it pure makes the
//! election fully unit-testable with no IO.

use crate::infra::config::{AccessPathKind, SmsPathPolicy};

/// Whether a leg is currently able to receive MT SMS (registered + listening).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegReceiveReadiness {
    pub kind: AccessPathKind,
    /// The leg is registered and can take over MT reception right now.
    pub ready: bool,
}

/// What the CS/modem listener should do given the elected IMS listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsListenerAction {
    /// No IMS leg is active; the CS listener owns MT reception.
    Active,
    /// An IMS leg is active; pause the CS scan entirely (no duplicates).
    Paused,
    /// An IMS leg is active but the policy keeps CS as a fallback receiver;
    /// keep scanning but rely on cross-transport dedup to drop duplicates.
    ActiveWithDedup,
}

/// The outcome of an election pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElectionOutcome {
    /// The elected IMS listener, if any IMS leg is enabled + ready.
    pub active_ims_listener: Option<AccessPathKind>,
    /// What the CS listener should do.
    pub cs_action: CsListenerAction,
}

/// Elect the single active MT-SMS listener.
///
/// Rules (mirrors design §9.2):
/// 1. Walk the policy's enabled IMS layers in priority order.
/// 2. The first one whose readiness report says `ready` becomes the active
///    listener.
/// 3. The CS listener stands down (`Paused`) while an IMS leg is active, unless
///    the policy sets `cs_fallback_receiver`, in which case it keeps scanning
///    with dedup (`ActiveWithDedup`).
/// 4. If no IMS leg is ready, the CS listener is `Active` (owns reception),
///    provided CS is enabled in the policy; otherwise nobody receives and
///    `cs_action` is `Paused`.
pub fn elect_listener(
    policy: &SmsPathPolicy,
    readiness: &[LegReceiveReadiness],
) -> ElectionOutcome {
    let ready_for = |kind: AccessPathKind| readiness.iter().any(|r| r.kind == kind && r.ready);

    // First enabled IMS layer, in priority order, that is ready.
    let active_ims_listener = policy.enabled_ims_layers().find(|&kind| ready_for(kind));

    let cs_enabled = policy.is_enabled(AccessPathKind::Cs);

    let cs_action = match active_ims_listener {
        Some(_) => {
            if policy.cs_fallback_receiver && cs_enabled {
                CsListenerAction::ActiveWithDedup
            } else {
                CsListenerAction::Paused
            }
        }
        None => {
            if cs_enabled {
                CsListenerAction::Active
            } else {
                CsListenerAction::Paused
            }
        }
    };

    ElectionOutcome {
        active_ims_listener,
        cs_action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::config::{MidFlightDisablePolicy, PathLayerConfig};

    fn policy(order: Vec<(AccessPathKind, bool)>, cs_fallback: bool) -> SmsPathPolicy {
        SmsPathPolicy {
            priority: order
                .into_iter()
                .map(|(kind, enabled)| PathLayerConfig { kind, enabled })
                .collect(),
            dedupe_enabled: true,
            cs_fallback_receiver: cs_fallback,
            mid_flight_disable: MidFlightDisablePolicy::AutoSwitch,
            dedup_retention_days: 30,
            message_retention_limit: 10_000,
        }
        .normalized()
    }

    fn ready(kind: AccessPathKind, ready: bool) -> LegReceiveReadiness {
        LegReceiveReadiness { kind, ready }
    }

    #[test]
    fn highest_priority_ready_ims_leg_wins() {
        let p = policy(
            vec![
                (AccessPathKind::Vowifi, true),
                (AccessPathKind::Volte, true),
                (AccessPathKind::Cs, true),
            ],
            false,
        );
        let out = elect_listener(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
            ],
        );
        assert_eq!(out.active_ims_listener, Some(AccessPathKind::Vowifi));
        assert_eq!(out.cs_action, CsListenerAction::Paused);
    }

    #[test]
    fn falls_through_to_next_ims_leg_when_top_not_ready() {
        let p = policy(
            vec![
                (AccessPathKind::Vowifi, true),
                (AccessPathKind::Volte, true),
                (AccessPathKind::Cs, true),
            ],
            false,
        );
        let out = elect_listener(
            &p,
            &[
                ready(AccessPathKind::Vowifi, false),
                ready(AccessPathKind::Volte, true),
            ],
        );
        assert_eq!(out.active_ims_listener, Some(AccessPathKind::Volte));
    }

    #[test]
    fn disabled_ims_leg_is_skipped_even_when_ready() {
        let p = policy(
            vec![
                (AccessPathKind::Vowifi, false),
                (AccessPathKind::Volte, true),
                (AccessPathKind::Cs, true),
            ],
            false,
        );
        let out = elect_listener(
            &p,
            &[
                ready(AccessPathKind::Vowifi, true),
                ready(AccessPathKind::Volte, true),
            ],
        );
        assert_eq!(out.active_ims_listener, Some(AccessPathKind::Volte));
    }

    #[test]
    fn no_ready_ims_leg_gives_cs_reception() {
        let p = policy(
            vec![
                (AccessPathKind::Vowifi, true),
                (AccessPathKind::Volte, true),
                (AccessPathKind::Cs, true),
            ],
            false,
        );
        let out = elect_listener(
            &p,
            &[
                ready(AccessPathKind::Vowifi, false),
                ready(AccessPathKind::Volte, false),
            ],
        );
        assert_eq!(out.active_ims_listener, None);
        assert_eq!(out.cs_action, CsListenerAction::Active);
    }

    #[test]
    fn cs_fallback_receiver_keeps_cs_scanning_with_dedup() {
        let p = policy(
            vec![
                (AccessPathKind::Vowifi, true),
                (AccessPathKind::Volte, true),
                (AccessPathKind::Cs, true),
            ],
            true,
        );
        let out = elect_listener(&p, &[ready(AccessPathKind::Vowifi, true)]);
        assert_eq!(out.active_ims_listener, Some(AccessPathKind::Vowifi));
        assert_eq!(out.cs_action, CsListenerAction::ActiveWithDedup);
    }

    #[test]
    fn cs_disabled_and_no_ims_means_nobody_receives() {
        let p = policy(
            vec![
                (AccessPathKind::Vowifi, true),
                (AccessPathKind::Volte, true),
                (AccessPathKind::Cs, false),
            ],
            false,
        );
        let out = elect_listener(&p, &[ready(AccessPathKind::Vowifi, false)]);
        assert_eq!(out.active_ims_listener, None);
        assert_eq!(out.cs_action, CsListenerAction::Paused);
    }
}
