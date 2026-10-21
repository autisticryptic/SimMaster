//! Drives the verified SDK boundary; never invents an on-wire protocol.
//! A backend must actually submit operations and deliver the matching callback.
//! Default construction has no backend and fails explicitly before any login.
use super::{credentials::ImportedCredentials, session::*};
use std::{sync::Arc, time::{Duration, Instant}};

pub struct HostingController {
    session: HostingSession,
    backend: Option<Arc<dyn HostingBackend>>,
    pending_deadline: Option<Instant>,
    operation_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerFailure {
    Credentials(&'static str),
    Backend(BackendFailure),
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callback {
    SdkLogin { success: bool },
    HostingAck { success: bool },
    Channels { calls: bool, messages: bool },
    Failure(BackendFailure),
}

impl HostingController {
    pub fn new(line_id: String, backend: Option<Arc<dyn HostingBackend>>) -> Self {
        Self { session: HostingSession::new(line_id), backend, pending_deadline: None,
            operation_timeout: Duration::from_secs(30) }
    }

    /// Only local configuration. Import never submits a network operation.
    /// Active credentials must be stopped before replacement.
    pub fn import(&mut self, credentials: ImportedCredentials, now: u64)
        -> Result<(), ControllerFailure> {
        if !matches!(self.session.phase(), Phase::Unconfigured | Phase::CredentialsImported
            | Phase::CredentialExpired | Phase::AuthenticationFailed | Phase::Disconnected) {
            return Err(ControllerFailure::Credentials("jegotrip_stop_before_replace"));
        }
        self.session.import(credentials, now).map_err(ControllerFailure::Credentials)?;
        self.pending_deadline = None;
        Ok(())
    }

    /// Submission success keeps SdkLoginPending; readiness comes from callbacks.
    pub async fn start(&mut self, now: u64) -> Result<AttemptOwner, ControllerFailure> {
        if !matches!(self.session.phase(), Phase::CredentialsImported | Phase::CredentialExpired
            | Phase::AuthenticationFailed | Phase::Disconnected) {
            return Err(ControllerFailure::Credentials("jegotrip_start_state_invalid"));
        }
        let backend = self.backend.clone()
            .ok_or(ControllerFailure::Backend(BackendFailure::Unavailable))?;
        let owner = self.session.begin_login(now).map_err(ControllerFailure::Credentials)?;
        let request = self.session.login_request(&owner).map_err(ControllerFailure::Credentials)?;
        let result = tokio::time::timeout(self.operation_timeout,
            backend.login(owner.clone(), request)).await;
        match result {
            Ok(Ok(())) => {
                self.pending_deadline = Some(Instant::now() + self.operation_timeout);
                Ok(owner)
            }
            Ok(Err(reason)) => {
                self.session.on_failure(&owner, reason);
                Err(ControllerFailure::Backend(reason))
            }
            Err(_) => {
                self.session.on_failure(&owner, BackendFailure::Disconnected);
                Err(ControllerFailure::Timeout)
            }
        }
    }

    /// Returns false for an old owner or a callback for the wrong stage.
    pub async fn on_callback(&mut self, owner: &AttemptOwner, callback: Callback)
        -> Result<bool, ControllerFailure> {
        match callback {
            Callback::SdkLogin { success } => {
                if !self.session.on_sdk_login(owner, success) { return Ok(false); }
                self.pending_deadline = None;
                if !success { return Ok(true); }
                let backend = self.backend.clone()
                    .ok_or(ControllerFailure::Backend(BackendFailure::Unavailable))?;
                let request = self.session.begin_hosting(owner).map_err(ControllerFailure::Credentials)?;
                let result = tokio::time::timeout(self.operation_timeout,
                    backend.update_status(owner.clone(), STATUS_REGISTER, request)).await;
                match result {
                    Ok(Ok(())) => self.pending_deadline = Some(Instant::now() + self.operation_timeout),
                    Ok(Err(reason)) => {
                        self.session.on_failure(owner, reason);
                        return Err(ControllerFailure::Backend(reason));
                    }
                    Err(_) => {
                        self.session.on_failure(owner, BackendFailure::Disconnected);
                        return Err(ControllerFailure::Timeout);
                    }
                }
                Ok(true)
            }
            Callback::HostingAck { success } => {
                let accepted = self.session.on_hosting_ack(owner, success);
                if accepted { self.pending_deadline = None; }
                Ok(accepted)
            }
            Callback::Channels { calls, messages } => Ok(self.session.on_channels(owner, calls, messages)),
            Callback::Failure(reason) => {
                let accepted = self.session.on_failure(owner, reason);
                if accepted { self.pending_deadline = None; }
                Ok(accepted)
            }
        }
    }

    /// Called by a scheduler. Known expiry and missing callback revoke readiness.
    pub fn tick(&mut self, now: u64, monotonic_now: Instant) {
        self.session.check_expiry(now);
        if self.pending_deadline.is_some_and(|at| monotonic_now >= at) {
            let owner = self.session.current_owner();
            self.session.on_failure(&owner, BackendFailure::Disconnected);
            self.pending_deadline = None;
        }
    }

    /// Stop always retires the old callback owner. Failed unregister still runs
    /// logout; a failed network teardown is returned, never reported as success.
    pub async fn stop(&mut self) -> Result<(), ControllerFailure> {
        self.pending_deadline = None;
        let backend = self.backend.clone();
        let mut failure = None;
        if let Some(backend) = backend {
            if self.session.phase() != Phase::Unconfigured {
                let (owner, request) = self.session.begin_unhosting()
                    .map_err(ControllerFailure::Credentials)?;
                match tokio::time::timeout(self.operation_timeout,
                    backend.update_status(owner.clone(), STATUS_UNREGISTER, request)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(reason)) => failure = Some(ControllerFailure::Backend(reason)),
                    Err(_) => failure = Some(ControllerFailure::Timeout),
                }
                match tokio::time::timeout(self.operation_timeout, backend.logout(owner)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(reason)) => { failure.get_or_insert(ControllerFailure::Backend(reason)); }
                    Err(_) => { failure.get_or_insert(ControllerFailure::Timeout); }
                }
            }
        }
        self.session.clear().map_err(ControllerFailure::Credentials)?;
        failure.map_or(Ok(()), Err)
    }

    pub fn phase(&self) -> Phase { self.session.phase() }
    pub fn incoming_call_ready(&self) -> bool { self.session.incoming_call_ready() }
    pub fn incoming_message_ready(&self) -> bool { self.session.incoming_message_ready() }
    pub fn outbound_authorized(&self) -> bool { self.session.outbound_authorized() }
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
