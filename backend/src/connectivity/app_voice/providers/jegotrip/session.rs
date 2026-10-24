//! Two independent authentication stages and per-line callback ownership.
//!
//! This is the SDK adapter boundary, not an implementation of the SDK wire
//! protocol. The Android ELF cannot be dlopen'ed as a Linux backend. Missing
//! backends never return Available and imported credentials never grant readiness.
use super::credentials::{ImportedCredentials, Secret};
use serde::Serialize;
use std::{fmt, future::Future, pin::Pin, sync::Arc};

pub const STATUS_REGISTER: i32 = 4;
pub const STATUS_UNREGISTER: i32 = 3;

pub struct SdkLoginRequest<'a> {
    pub account_identity: &'a str,
    pub phone_login_token: &'a str,
    pub application_key: &'a str,
    pub server_address: &'a str,
    pub device_id: Option<&'a str>,
    pub selected_session_id: Option<&'a str>,
    pub root_der_base64: &'a str,
    pub tls_only: bool,
}

impl fmt::Debug for SdkLoginRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SdkLoginRequest(<redacted>)")
    }
}

/// Exact field names observed in JCAccountImpl.getExtraParamsJson.
/// Serialization is only for a verified local SDK backend, not an API response.
#[derive(Serialize)]
pub struct GatewayRegistration<'a> {
    #[serde(rename = "AuthnamePrefix")]
    pub authname_prefix: &'a str,
    #[serde(rename = "ChannelId")]
    pub channel_id: &'a str,
    #[serde(rename = "DeleteIntCode")]
    pub delete_int_code: &'a str,
    #[serde(rename = "SipNumber")]
    pub sip_number: &'a str,
    #[serde(rename = "SipPwd")]
    pub sip_password: &'a str,
}

impl fmt::Debug for GatewayRegistration<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GatewayRegistration(<redacted>)")
    }
}

pub type BackendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), BackendFailure>> + Send + 'a>>;

/// Only stable error kinds cross this boundary; SDK messages may contain tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendFailure {
    Unavailable,
    AuthenticationRejected,
    Expired,
    Disconnected,
    HostingRejected,
}

/// A backend must preserve the two stages. Successful submission is not a
/// successful login/registration: completion arrives as an owned callback.
pub trait HostingBackend: Send + Sync {
    fn login<'a>(&'a self, owner: AttemptOwner, request: SdkLoginRequest<'a>) -> BackendFuture<'a>;
    fn update_status<'a>(&'a self, owner: AttemptOwner, status: i32,
        registration: GatewayRegistration<'a>) -> BackendFuture<'a>;
    /// Synchronously retire new IO/commands for the ORIGINAL attempt. This is
    /// not an IO-release receipt: logout/close still awaits actual teardown.
    /// Legacy offline test adapters have no IO to retire.
    fn retire(&self, _owner: &AttemptOwner) {}
    fn logout(&self, owner: AttemptOwner) -> BackendFuture<'_>;
}

/// Captured when an SDK operation is submitted, never recreated on receipt.
/// The incarnation prevents callbacks from a deleted/recreated line from matching
/// a new controller which happens to use the same line id and generation.
#[derive(Clone)]
pub struct AttemptOwner {
    line_id: String,
    generation: u64,
    incarnation: Arc<()>,
}
impl AttemptOwner {
    pub(super) fn line_id(&self) -> &str { &self.line_id }
}

impl PartialEq for AttemptOwner {
    fn eq(&self, other: &Self) -> bool {
        self.line_id == other.line_id && self.generation == other.generation
            && Arc::ptr_eq(&self.incarnation, &other.incarnation)
    }
}

impl Eq for AttemptOwner {}

impl fmt::Debug for AttemptOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttemptOwner").field("generation", &self.generation).finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Unconfigured,
    CredentialsImported,
    SdkLoginPending,
    SdkLoggedIn,
    HostingPending,
    UnhostingPending,
    Hosted,
    CredentialExpired,
    AuthenticationFailed,
    Disconnected,
}

/// Created/retired by the caller for one no-modem line. No global credential set.
pub struct HostingSession {
    line_id: String,
    generation: u64,
    incarnation: Arc<()>,
    credentials: Option<ImportedCredentials>,
    phase: Phase,
    incoming_call_channel: bool,
    incoming_message_channel: bool,
}

impl fmt::Debug for HostingSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostingSession")
            .field("generation", &self.generation)
            .field("phase", &self.phase)
            .field("credentials_present", &self.credentials.is_some())
            .finish()
    }
}

impl HostingSession {
    pub fn new(line_id: String) -> Self {
        Self { line_id, generation: 0, incarnation: Arc::new(()), credentials: None, phase: Phase::Unconfigured,
            incoming_call_channel: false, incoming_message_channel: false }
    }

    fn invalidate(&mut self) {
        self.incoming_call_channel = false;
        self.incoming_message_channel = false;
    }

    fn next_owner(&mut self) -> Result<AttemptOwner, &'static str> {
        let Some(next) = self.generation.checked_add(1) else {
            self.invalidate();
            self.credentials = None;
            self.phase = Phase::Unconfigured;
            return Err("jegotrip_generation_exhausted");
        };
        self.generation = next;
        Ok(self.current_owner())
    }

    pub fn import(&mut self, credentials: ImportedCredentials, now: u64) -> Result<AttemptOwner, &'static str> {
        credentials.validate(now)?;
        let owner = self.next_owner()?;
        self.invalidate();
        self.credentials = Some(credentials);
        self.phase = Phase::CredentialsImported;
        Ok(owner)
    }

    /// Starts a new callback scope. Does not call any service by itself.
    pub fn begin_login(&mut self, now: u64) -> Result<AttemptOwner, &'static str> {
        let credentials = self.credentials.as_ref().ok_or("jegotrip_credentials_required")?;
        if let Err(code) = credentials.validate(now) {
            self.invalidate();
            self.phase = Phase::CredentialExpired;
            return Err(code);
        }
        let owner = self.next_owner()?;
        self.invalidate();
        self.phase = Phase::SdkLoginPending;
        Ok(owner)
    }

    pub(super) fn owns(&self, owner: &AttemptOwner) -> bool {
        owner.line_id == self.line_id && owner.generation == self.generation
            && Arc::ptr_eq(&owner.incarnation, &self.incarnation)
    }

    pub fn login_request(&self, owner: &AttemptOwner) -> Result<SdkLoginRequest<'_>, &'static str> {
        if !self.owns(owner) || self.phase != Phase::SdkLoginPending {
            return Err("jegotrip_callback_scope_invalid");
        }
        let sdk = &self.credentials.as_ref().ok_or("jegotrip_credentials_required")?.sdk;
        Ok(SdkLoginRequest {
            account_identity: sdk.account_identity.expose_to_backend(),
            phone_login_token: sdk.phone_login_token.expose_to_backend(),
            application_key: sdk.application_key.expose_to_backend(),
            server_address: sdk.server_address.expose_to_backend(),
            device_id: sdk.device_id.as_ref().map(Secret::expose_to_backend),
            selected_session_id: sdk.selected_session_id.as_ref().map(Secret::expose_to_backend),
            root_der_base64: sdk.root_der_base64.expose_to_backend(),
            tls_only: sdk.tls_only,
        })
    }

    pub fn on_sdk_login(&mut self, owner: &AttemptOwner, success: bool) -> bool {
        if !self.owns(owner) || self.phase != Phase::SdkLoginPending { return false; }
        self.phase = if success { Phase::SdkLoggedIn } else { Phase::AuthenticationFailed };
        self.invalidate();
        true
    }

    pub fn begin_hosting(&mut self, owner: &AttemptOwner) -> Result<GatewayRegistration<'_>, &'static str> {
        if !self.owns(owner) || self.phase != Phase::SdkLoggedIn {
            return Err("jegotrip_sdk_login_required");
        }
        let hosting = &self.credentials.as_ref().ok_or("jegotrip_credentials_required")?.hosting;
        self.phase = Phase::HostingPending;
        Ok(GatewayRegistration {
            authname_prefix: hosting.authname_prefix.expose_to_backend(),
            channel_id: hosting.channel_id.expose_to_backend(),
            delete_int_code: &hosting.delete_int_code,
            sip_number: hosting.sip_number.expose_to_backend(),
            sip_password: hosting.sip_password.expose_to_backend(),
        })
    }

    /// Registration acknowledgement alone does not prove either delivery channel.
    pub fn on_hosting_ack(&mut self, owner: &AttemptOwner, success: bool) -> bool {
        if !self.owns(owner) || self.phase != Phase::HostingPending { return false; }
        self.phase = if success { Phase::Hosted } else { Phase::AuthenticationFailed };
        self.invalidate();
        true
    }

    pub fn on_channels(&mut self, owner: &AttemptOwner, calls: bool, messages: bool) -> bool {
        if !self.owns(owner) || self.phase != Phase::Hosted { return false; }
        self.incoming_call_channel = calls;
        self.incoming_message_channel = messages;
        true
    }

    pub fn on_failure(&mut self, owner: &AttemptOwner, reason: BackendFailure) -> bool {
        if !self.owns(owner) || !matches!(self.phase, Phase::SdkLoginPending
            | Phase::SdkLoggedIn | Phase::HostingPending | Phase::Hosted | Phase::UnhostingPending) {
            return false;
        }
        self.phase = match reason {
            BackendFailure::Expired => Phase::CredentialExpired,
            BackendFailure::AuthenticationRejected | BackendFailure::HostingRejected => Phase::AuthenticationFailed,
            BackendFailure::Unavailable | BackendFailure::Disconnected => Phase::Disconnected,
        };
        self.invalidate();
        // Retire the scope so a delayed success cannot undo this failure.
        if let Some(next) = self.generation.checked_add(1) { self.generation = next; }
        else { self.credentials = None; }
        true
    }

    pub fn check_expiry(&mut self, now: u64) {
        if self.phase != Phase::CredentialExpired
            && self.credentials.as_ref().and_then(|c| c.sdk.expires_at_unix_seconds)
                .is_some_and(|at| at <= now) {
            self.invalidate();
            self.phase = Phase::CredentialExpired;
            if let Some(next) = self.generation.checked_add(1) { self.generation = next; }
            else { self.credentials = None; }
        }
    }

    /// Retire login/hosting callbacks before teardown is submitted. The caller
    /// still needs the gateway parameters to send STATUS_UNREGISTER.
    pub fn begin_unhosting(&mut self) -> Result<(AttemptOwner, GatewayRegistration<'_>), &'static str> {
        let owner = self.current_owner();
        self.next_owner()?;
        self.invalidate();
        self.phase = Phase::UnhostingPending;
        let hosting = &self.credentials.as_ref().ok_or("jegotrip_credentials_required")?.hosting;
        Ok((owner, GatewayRegistration {
            authname_prefix: hosting.authname_prefix.expose_to_backend(),
            channel_id: hosting.channel_id.expose_to_backend(),
            delete_int_code: &hosting.delete_int_code,
            sip_number: hosting.sip_number.expose_to_backend(),
            sip_password: hosting.sip_password.expose_to_backend(),
        }))
    }

    pub fn current_owner(&self) -> AttemptOwner {
        AttemptOwner { line_id: self.line_id.clone(), generation: self.generation,
            incarnation: self.incarnation.clone() }
    }

    pub fn clear(&mut self) -> Result<(), &'static str> {
        self.next_owner()?;
        self.credentials = None;
        self.phase = Phase::Unconfigured;
        self.invalidate();
        Ok(())
    }

    pub fn phase(&self) -> Phase { self.phase }
    pub fn incoming_call_ready(&self) -> bool { self.phase == Phase::Hosted && self.incoming_call_channel }
    pub fn incoming_message_ready(&self) -> bool { self.phase == Phase::Hosted && self.incoming_message_channel }
    pub fn accepts_incoming_message(&self, owner: &AttemptOwner) -> bool {
        self.owns(owner) && self.incoming_message_ready()
    }
    /// Provider tariffs differ from IMS; outbound SMS capability is unverified.
    /// Import/registration must not silently authorize billable outbound traffic.
    pub fn outbound_authorized(&self) -> bool { false }
}
