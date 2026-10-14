//! Registered WiFi refresh on the existing operator-owned channel. Failure
//! metadata, wait and original lease travel together; no raw response escapes.
use super::*;
use super::super::operator::RegisteredRefreshFailure;
use crate::connectivity::core::{
    register::{RegisterNegotiationBudget, RegisterFailureKind, run_register_observed_with_budget},
    register_retry::RetryAdmission,
    registration_diagnostics::{FailureScope, RegistrationProcedure},
};

impl VowifiUnregisterFactory {
    pub(super) async fn refresh_with_recovery(&self, channel: &mut SipChannel, original_expiry: Option<Instant>)
        -> Result<RegisteredImsContext, RegisteredRefreshFailure> {
        self.context.diagnostic_scope.update(|state| state.facts.procedure = RegistrationProcedure::Refresh).await;
        if original_expiry.is_some_and(|expiry| expiry <= Instant::now()) {
            return Err(ImsError::new("vowifi_registration_expired").into());
        }
        if let Err(error) = self.context.verify_worker(&self.line_id) {
            self.context.diagnostic_scope.core_failure(&error, FailureScope::Ownership).await;
            return Err(error.into());
        }
        let Some(uicc) = self.context.uicc.as_ref() else {
            let error = ImsError::new("ims_uicc_binding_invalid");
            self.context.diagnostic_scope.core_failure(&error, FailureScope::Ownership).await;
            return Err(error.into());
        };
        let retries = match register_retry_live::Scope::for_owner(&self.line_id, uicc).await {
            Ok(scope) => scope,
            Err(error) => {
                self.context.diagnostic_scope.core_failure(&error, FailureScope::Ownership).await;
                return Err(error.into());
            }
        };
        // route_addr/profile are captured before IPsec replaces remote ports.
        let logical_endpoint = SocketAddr::new(self.context.route_addr, self.profile.ims.local_port);
        let admission = retries.check(logical_endpoint).await;
        if let Some(error) = register_retry_live::admission_error(admission) {
            self.context.diagnostic_scope.admission(admission).await;
            return Err(RegisteredRefreshFailure {
                error: ImsError::new(if matches!(admission, RetryAdmission::Stopped(_)) { register_retry_live::STOPPED } else { register_retry_live::DEFERRED }),
                metadata: None, retry_admission: error.retry_admission,
                retained_until: original_expiry.filter(|expiry| *expiry > Instant::now()),
            });
        }
        let mut auth = VowifiRefreshAuthenticator {
            factory: self, resync_sent: false,
            authorization: self.refresh_authorization.lock().unwrap_or_else(|error| error.into_inner()).clone(),
            expires: self.expires_seconds.load(std::sync::atomic::Ordering::Relaxed),
        };
        let initial = auth.request()?;
        self.context.diagnostic_scope.offer(&initial, auth.authorization.as_ref()).await;
        let mut budget = RegisterNegotiationBudget::default();
        let result = run_register_observed_with_budget(channel, &initial, &mut auth, &mut budget).await;
        if let Err(reason) = uicc.verify(&self.line_id).await {
            let error = ImsError::new(reason);
            self.context.diagnostic_scope.core_failure(&error, FailureScope::Ownership).await;
            return Err(error.into());
        }
        self.context.verify_worker(&self.line_id)?;
        let result = match result {
            Ok(result) => result,
            Err(failure) => {
                let metadata = budget.failure_metadata(&failure);
                self.context.diagnostic_scope.update(|state| state.failure(failure.error.code(), Some(metadata))).await;
                let admission = if metadata.is_temporary() || register_retry_live::response_requires_wait(metadata) {
                    let admission = retries.observe(logical_endpoint, &metadata).await;
                    // The installed flow cannot jump to another profile/worker
                    // while it waits; preserve the gate outside its caches.
                    retries.pause_restart().await;
                    self.context.diagnostic_scope.admission(admission).await;
                    Some(admission)
                } else { None };
                let error = if is_live_digest_authentication_failure(failure.error.code())
                    || failure.error.code().starts_with("ims_outbound_")
                    || failure.error.code() == "vowifi_refresh_security_update_required" {
                    failure.error
                } else if matches!(metadata.sip_status, Some(403 | 404 | 410 | 481 | 401 | 407))
                    && metadata.kind != RegisterFailureKind::Transport {
                    ImsError::new("vowifi_refresh_rejected")
                } else { failure.error };
                return Err(RegisteredRefreshFailure {
                    error, metadata: Some(metadata), retry_admission: admission,
                    retained_until: original_expiry.filter(|expiry| metadata.is_temporary() && *expiry > Instant::now()),
                });
            }
        };
        auth.validate_success(&result.response)?;
        if let Some(state) = auth.authorization.as_mut() { state.apply_success(&result.response)?; }
        retries.accepted(logical_endpoint).await?;
        self.context.verify_worker(&self.line_id)?;
        let artifacts = channel.outbound_registered(&result.response, auth.expires)?;
        if artifacts.expires_seconds == Some(0) { return Err(ImsError::new("vowifi_refresh_rejected").into()); }
        self.context.diagnostic_scope.accepted(channel.route(), channel.security_verify(), auth.authorization.as_ref(), &result.response).await;
        *self.refresh_authorization.lock().unwrap_or_else(|error| error.into_inner()) = auth.authorization;
        let registration = RegisteredImsContext::from_artifacts(ImsRegistrationAccess::Vowifi, artifacts, auth.expires);
        self.expires_seconds.store(registration.lease.expires_seconds, std::sync::atomic::Ordering::Relaxed);
        Ok(registration)
    }
}
