//! Drives the verified SDK boundary; never invents an on-wire protocol.
//! A backend must actually submit operations and deliver the matching callback.
//! Default construction has no backend and fails explicitly before any login.
use super::{credentials::ImportedCredentials, session::*};
use std::{sync::Arc, time::{Duration, Instant}};

pub struct HostingController {
    session: HostingSession,
    backend: Option<Arc<dyn HostingBackend>>,
    // Retained original IO owner even after the session retires its callbacks.
    backend_owner: Option<AttemptOwner>,
    pending_deadline: Option<Instant>,
    operation_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerFailure {
    Credentials(&'static str),
    Notification(&'static str),
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
        Self { session: HostingSession::new(line_id), backend, backend_owner: None, pending_deadline: None,
            operation_timeout: Duration::from_secs(30) }
    }

    /// Only local configuration. Import never submits a network operation.
    /// Active credentials must be stopped before replacement.
    pub fn import(&mut self, credentials: ImportedCredentials, now: u64)
        -> Result<(), ControllerFailure> {
        if self.backend_owner.is_some() || !matches!(self.session.phase(), Phase::Unconfigured | Phase::CredentialsImported
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
        if self.backend_owner.is_some() { return Err(ControllerFailure::Credentials("jegotrip_stop_before_start")); }
        let backend = self.backend.clone()
            .ok_or(ControllerFailure::Backend(BackendFailure::Unavailable))?;
        let owner = self.session.begin_login(now).map_err(ControllerFailure::Credentials)?;
        self.backend_owner = Some(owner.clone());
        self.pending_deadline = Some(Instant::now() + self.operation_timeout);
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
                backend.retire(&owner);
                Err(ControllerFailure::Backend(reason))
            }
            Err(_) => {
                self.session.on_failure(&owner, BackendFailure::Disconnected);
                backend.retire(&owner);
                Err(ControllerFailure::Timeout)
            }
        }
    }

    /// Returns false for an old owner or a callback for the wrong stage.
    /// `now` is supplied by the scheduler's Unix clock; expiry and monotonic
    /// deadlines are checked on delivery, not only during periodic polling.
    pub async fn on_callback(&mut self, owner: &AttemptOwner, callback: Callback, now: u64)
        -> Result<bool, ControllerFailure> {
        self.tick(now, Instant::now());
        match callback {
            Callback::SdkLogin { success } => {
                if !self.session.on_sdk_login(owner, success) { return Ok(false); }
                self.pending_deadline = None;
                if !success { self.retire_backend(); return Ok(true); }
                let backend = self.backend.clone()
                    .ok_or(ControllerFailure::Backend(BackendFailure::Unavailable))?;
                let request = self.session.begin_hosting(owner).map_err(ControllerFailure::Credentials)?;
                self.pending_deadline = Some(Instant::now() + self.operation_timeout);
                let result = tokio::time::timeout(self.operation_timeout,
                    backend.update_status(owner.clone(), STATUS_REGISTER, request)).await;
                match result {
                    Ok(Ok(())) => self.pending_deadline = Some(Instant::now() + self.operation_timeout),
                    Ok(Err(reason)) => {
                        self.session.on_failure(owner, reason);
                        backend.retire(owner);
                        return Err(ControllerFailure::Backend(reason));
                    }
                    Err(_) => {
                        self.session.on_failure(owner, BackendFailure::Disconnected);
                        backend.retire(owner);
                        return Err(ControllerFailure::Timeout);
                    }
                }
                Ok(true)
            }
            Callback::HostingAck { success } => {
                let accepted = self.session.on_hosting_ack(owner, success);
                if accepted { self.pending_deadline = None; if !success { self.retire_backend(); } }
                Ok(accepted)
            }
            Callback::Channels { calls, messages } => Ok(self.session.on_channels(owner, calls, messages)),
            Callback::Failure(reason) => {
                let accepted = self.session.on_failure(owner, reason);
                if accepted { self.pending_deadline = None; self.retire_backend(); }
                Ok(accepted)
            }
        }
    }

    /// Native notifications arrive from the local SDK adapter, not HTTP callers.
    /// The owner and operation must be captured at submission. Messages preserve
    /// bytes and must belong to a hosted ready channel; reject stale input before
    /// decoding/copying so unrelated malformed notifications have no effect.
    pub async fn on_native_notification(&mut self, owner: &AttemptOwner,
        notification: super::native_events::NativeNotification<'_>, now: u64)
        -> Result<Option<super::native_events::IncomingMessage>, ControllerFailure> {
        use super::native_events::{decode, NativeEvent};
        self.tick(now, Instant::now());
        if !self.session.owns(owner) { return Ok(None); }
        if notification.name == "MtcSmgwMessageRecvNotification"
            && !self.session.accepts_incoming_message(owner) { return Ok(None); }
        match decode(notification).map_err(ControllerFailure::Notification)? {
            Some(NativeEvent::Callback(callback)) => {
                self.on_callback(owner, callback, now).await?;
                Ok(None)
            }
            Some(NativeEvent::IncomingMessage(message)) => Ok(Some(message)),
            None => Ok(None),
        }
    }

    /// Called by a scheduler and by callback/message delivery. Expiry takes
    /// precedence over a simultaneous callback timeout and is idempotent.
    pub fn tick(&mut self, now: u64, monotonic_now: Instant) {
        self.session.check_expiry(now);
        if !matches!(self.session.phase(), Phase::SdkLoginPending | Phase::HostingPending) {
            self.pending_deadline = None;
            if matches!(self.session.phase(), Phase::CredentialExpired | Phase::AuthenticationFailed | Phase::Disconnected) { self.retire_backend(); }
            return;
        }
        if self.pending_deadline.is_some_and(|at| monotonic_now >= at) {
            let owner = self.session.current_owner();
            self.session.on_failure(&owner, BackendFailure::Disconnected);
            self.pending_deadline = None;
            self.retire_backend();
        }
    }

    fn retire_backend(&self) {
        if let (Some(backend), Some(owner)) = (&self.backend, &self.backend_owner) { backend.retire(owner); }
    }

    /// Retire callback admission first, but issue teardown on the ORIGINAL
    /// captured backend/IO owner. Imports and already failed/logged-out sessions
    /// do not invent gateway unregister operations. Failed unregister still
    /// waits for original logout; cancellation retains owner for another wait.
    pub async fn stop(&mut self) -> Result<(), ControllerFailure> {
        self.pending_deadline = None;
        let phase = self.session.phase(); let backend = self.backend.clone(); let original = self.backend_owner.clone();
        let mut released = original.is_none(); let mut failure = None;
        if phase != Phase::Unconfigured {
            match self.session.begin_unhosting() {
                Ok((_, request)) => {
                    if matches!(phase, Phase::SdkLoggedIn | Phase::HostingPending | Phase::Hosted) {
                        if let (Some(backend), Some(owner)) = (&backend, &original) {
                            match tokio::time::timeout(self.operation_timeout, backend.update_status(owner.clone(), STATUS_UNREGISTER, request)).await {
                                Ok(Ok(())) => {},
                                Ok(Err(reason)) => { backend.retire(owner); failure = Some(ControllerFailure::Backend(reason)); }
                                Err(_) => { backend.retire(owner); failure = Some(ControllerFailure::Timeout); },
                            }
                        }
                    }
                }
                Err(reason) => { failure = Some(ControllerFailure::Credentials(reason)); }
            }
        }
        if let (Some(backend), Some(owner)) = (backend, original) {
            match tokio::time::timeout(self.operation_timeout, backend.logout(owner.clone())).await {
                Ok(Ok(())) => { released = true; },
                Ok(Err(reason)) => { backend.retire(&owner); failure.get_or_insert(ControllerFailure::Backend(reason)); }
                Err(_) => { backend.retire(&owner); failure.get_or_insert(ControllerFailure::Timeout); }
            }
        }
        if released { self.backend_owner = None; }
        if let Err(reason) = self.session.clear() { failure.get_or_insert(ControllerFailure::Credentials(reason)); }
        failure.map_or(Ok(()), Err)
    }

    pub fn phase(&self) -> Phase { self.session.phase() }
    pub fn incoming_call_ready(&self) -> bool { self.session.incoming_call_ready() }
    pub fn incoming_message_ready(&self) -> bool { self.session.incoming_message_ready() }
    pub fn outbound_authorized(&self) -> bool { self.session.outbound_authorized() }
}

impl Drop for HostingController {
    fn drop(&mut self) { self.retire_backend(); }
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
