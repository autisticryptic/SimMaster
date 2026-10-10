//! Per-line retry ownership. A profile or worker restart is not a new SIM.
use super::*;
use crate::connectivity::modems::ims::cellular_ims::register_retry::{
    EndpointRetryState, RefreshRetryDecision, RetryAdmission,
};

pub(super) type SharedEndpointRetries = Arc<Mutex<EndpointRetryState>>;

#[derive(Default)]
pub(super) struct RetryScope {
    card: Option<UiccCardBinding>,
    state: SharedEndpointRetries,
}

impl RetryScope {
    pub(super) fn for_card(&mut self, card: &UiccCardBinding) -> SharedEndpointRetries {
        // MM owner/generation changes still require the normal admission gates,
        // but cannot shorten a remote wait for the same physical subscription.
        let same_subscription = self.card.as_ref().is_some_and(|old| {
            old.endpoint == card.endpoint && old.slot == card.slot
                && old.iccid == card.iccid && old.imsi == card.imsi
        });
        if !same_subscription {
            self.card = Some(card.clone());
            self.state = Arc::new(Mutex::new(EndpointRetryState::default()));
        }
        self.state.clone()
    }
}

pub(super) fn admission_error(admission: RetryAdmission, now: Instant) -> Option<CellularImsError> {
    match admission {
        RetryAdmission::Allowed => None,
        RetryAdmission::Deferred { not_before } => Some(CellularImsError::with_detail(
            code::REGISTER_RETRY_DEFERRED,
            format!("retry_after_ms={}", not_before.saturating_duration_since(now).as_millis()),
        )),
        RetryAdmission::Stopped(_) => Some(CellularImsError::new(code::REGISTER_RETRY_STOPPED)),
    }
}

pub(super) fn refresh_attempt(
    decision: RefreshRetryDecision,
    error: CellularImsError,
) -> CellularImsRefreshAttempt {
    match decision {
        RefreshRetryDecision::RetryAfter(delay) | RefreshRetryDecision::RetainUntilExpiry(delay) => {
            // The expiry preflight must run again before any request. Waking at
            // expiry is cleanup, never authorization to send early or renew it.
            CellularImsRefreshAttempt {
                outcome: RegistrationRefreshResult::Retry,
                error: Some(error),
                retry_after: Some(delay),
            }
        }
        RefreshRetryDecision::InvalidateBinding => CellularImsRefreshAttempt {
            outcome: RegistrationRefreshResult::RebuildAccess(RegistrationLossReason::Expired),
            error: Some(error),
            retry_after: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectivity::core::register::{RegisterFailureKind, RegisterFailureMetadata, RegisterFailureStage, RetryAfter};

    fn card() -> UiccCardBinding {
        UiccCardBinding { endpoint: "test-reader".into(), slot: 1, iccid: "synthetic-card".into(), imsi: "001010000000001".into(), owner: "owner-a".into() }
    }

    #[tokio::test]
    async fn retry_scope_survives_profile_owner_and_worker_restarts() {
        let mut scope = RetryScope::default();
        let mut binding = card();
        let first = scope.for_card(&binding);
        let endpoint = "192.0.2.1:5060".parse().unwrap();
        let now = Instant::now();
        first.lock().await.observe_failure(endpoint, &RegisterFailureMetadata {
            sip_status: Some(503), stage: RegisterFailureStage::Initial,
            kind: RegisterFailureKind::TemporaryEndpoint, retry_after: RetryAfter::DelaySeconds(120),
        }, now);
        binding.owner = "owner-b".into();
        let again = scope.for_card(&binding);
        assert!(Arc::ptr_eq(&first, &again));
        assert!(!again.lock().await.check(endpoint, now).is_allowed());
        binding.iccid = "another-card".into();
        let other = scope.for_card(&binding);
        assert!(!Arc::ptr_eq(&first, &other));
        assert!(other.lock().await.check(endpoint, now).is_allowed());
        assert!(!first.lock().await.check(endpoint, now).is_allowed());
    }

    #[test]
    fn retry_at_expiry_does_not_claim_a_new_registration() {
        let attempt = refresh_attempt(RefreshRetryDecision::RetainUntilExpiry(Duration::from_secs(2)),
            CellularImsError::new(code::REGISTER_RETRY_DEFERRED));
        assert!(matches!(attempt.outcome, RegistrationRefreshResult::Retry));
        assert_eq!(attempt.retry_after, Some(Duration::from_secs(2)));
    }
}
