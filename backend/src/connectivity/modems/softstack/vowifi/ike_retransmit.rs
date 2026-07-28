#![allow(dead_code)]

use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetransmitDecision {
    Wait,
    Retransmit,
    GiveUp,
}

impl RetransmitDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Retransmit => "retransmit",
            Self::GiveUp => "give_up",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetransmitPolicy {
    pub initial_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub max_attempts: u8,
    pub exchange_deadline_ms: u64,
}

impl Default for RetransmitPolicy {
    fn default() -> Self {
        Self {
            initial_timeout_ms: 500,
            max_timeout_ms: 8_000,
            max_attempts: 5,
            exchange_deadline_ms: 30_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetransmitState {
    pub message_id: u32,
    pub attempts: u8,
    pub elapsed_ms: u64,
    pub next_timeout_ms: u64,
    pub decision: &'static str,
}

impl RetransmitPolicy {
    pub fn initial_state(&self, message_id: u32) -> RetransmitState {
        RetransmitState {
            message_id,
            attempts: 0,
            elapsed_ms: 0,
            next_timeout_ms: self.initial_timeout_ms,
            decision: RetransmitDecision::Wait.as_str(),
        }
    }

    pub fn evaluate(&self, state: &RetransmitState, elapsed: Duration) -> RetransmitState {
        let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        if elapsed_ms >= self.exchange_deadline_ms || state.attempts >= self.max_attempts {
            return RetransmitState {
                message_id: state.message_id,
                attempts: state.attempts,
                elapsed_ms,
                next_timeout_ms: state.next_timeout_ms,
                decision: RetransmitDecision::GiveUp.as_str(),
            };
        }

        if elapsed_ms < state.next_timeout_ms {
            return RetransmitState {
                message_id: state.message_id,
                attempts: state.attempts,
                elapsed_ms,
                next_timeout_ms: state.next_timeout_ms,
                decision: RetransmitDecision::Wait.as_str(),
            };
        }

        let next_attempts = state.attempts.saturating_add(1);
        let next_timeout_ms = state
            .next_timeout_ms
            .saturating_mul(2)
            .min(self.max_timeout_ms);
        RetransmitState {
            message_id: state.message_id,
            attempts: next_attempts,
            elapsed_ms,
            next_timeout_ms,
            decision: if next_attempts >= self.max_attempts {
                RetransmitDecision::GiveUp.as_str()
            } else {
                RetransmitDecision::Retransmit.as_str()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_until_initial_timeout() {
        let policy = RetransmitPolicy::default();
        let state = policy.initial_state(7);

        let next = policy.evaluate(&state, Duration::from_millis(499));

        assert_eq!(next.message_id, 7);
        assert_eq!(next.attempts, 0);
        assert_eq!(next.decision, "wait");
        assert_eq!(next.next_timeout_ms, 500);
    }

    #[test]
    fn retransmits_with_capped_exponential_backoff() {
        let policy = RetransmitPolicy {
            initial_timeout_ms: 500,
            max_timeout_ms: 2_000,
            max_attempts: 5,
            exchange_deadline_ms: 30_000,
        };
        let state = policy.initial_state(1);
        let first = policy.evaluate(&state, Duration::from_millis(500));
        let second = policy.evaluate(&first, Duration::from_millis(1_000));
        let third = policy.evaluate(&second, Duration::from_millis(2_000));
        let fourth = policy.evaluate(&third, Duration::from_millis(4_000));

        assert_eq!(first.decision, "retransmit");
        assert_eq!(first.attempts, 1);
        assert_eq!(first.next_timeout_ms, 1_000);
        assert_eq!(second.next_timeout_ms, 2_000);
        assert_eq!(third.next_timeout_ms, 2_000);
        assert_eq!(fourth.next_timeout_ms, 2_000);
    }

    #[test]
    fn gives_up_at_attempt_limit_or_deadline() {
        let policy = RetransmitPolicy {
            initial_timeout_ms: 100,
            max_timeout_ms: 800,
            max_attempts: 2,
            exchange_deadline_ms: 1_000,
        };
        let state = policy.initial_state(3);
        let first = policy.evaluate(&state, Duration::from_millis(100));
        let second = policy.evaluate(&first, Duration::from_millis(200));

        assert_eq!(second.decision, "give_up");

        let state = policy.initial_state(4);
        let deadline = policy.evaluate(&state, Duration::from_millis(1_000));
        assert_eq!(deadline.decision, "give_up");
    }

    #[test]
    fn serialized_policy_and_state_have_no_sensitive_fields() {
        let policy = RetransmitPolicy::default();
        let state = policy.evaluate(&policy.initial_state(9), Duration::from_millis(500));
        let json = serde_json::to_string(&(policy, state)).expect("serialize retransmit state");

        for forbidden_key in [
            "imsi",
            "iccid",
            "msisdn",
            "spi",
            "ck",
            "ik",
            "nonce",
            "payload",
            "packet",
            "key_material",
        ] {
            assert!(
                !json
                    .to_ascii_lowercase()
                    .contains(&format!("\"{forbidden_key}\"")),
                "retransmit state must not contain a {forbidden_key} field"
            );
        }
    }
}
