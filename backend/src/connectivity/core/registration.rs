//! Access-neutral state produced by a successful IMS registration.
//!
//! The access adapters own bearer, socket and security lifetimes. This module
//! only owns the registration facts that have identical meaning on VoLTE and
//! VoWiFi: the network-selected expiry, route set and public identities.

use std::time::{Duration, SystemTime};

use super::{
    register::RegisterFailure, register_response::RegisterArtifacts, sip_frame::parse_status,
};

/// Access on which the IMS registration was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImsRegistrationAccess {
    Volte,
    Vowifi,
}

/// Network registration lease derived from the successful REGISTER response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationLease {
    /// Effective network lifetime. A response Contact `expires` wins over the
    /// response `Expires`, which in turn wins over the carrier-profile default.
    pub expires_seconds: u32,
    /// Delay after which an adapter should refresh the registration. Scheduling
    /// remains adapter-owned because a failed refresh rebuilds different access
    /// resources on VoLTE and VoWiFi.
    pub refresh_after: Duration,
    /// Full lifetime advertised by the network. Readiness must never be kept
    /// alive beyond this duration merely because a local cache has a minimum.
    pub expires_after: Duration,
}

impl RegistrationLease {
    pub fn from_artifacts(artifacts: &RegisterArtifacts, default_expires_seconds: u32) -> Self {
        Self::from_expires(artifacts.expires_seconds.unwrap_or(default_expires_seconds))
    }

    pub fn from_expires(expires_seconds: u32) -> Self {
        let expires_seconds = expires_seconds.max(1);
        let expires_after = Duration::from_secs(u64::from(expires_seconds));
        // Preserve the established VoLTE policy: refresh at 11/12 of the
        // negotiated lease. It leaves a bounded retry window without inventing
        // a local minimum that could outlive a short network registration.
        let refresh_seconds = (u64::from(expires_seconds).saturating_mul(11) / 12).max(1);
        Self {
            expires_seconds,
            refresh_after: Duration::from_secs(refresh_seconds),
            expires_after,
        }
    }
}

/// Facts shared by all services using one successful IMS registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredImsContext {
    pub access: ImsRegistrationAccess,
    pub registered_at: SystemTime,
    pub lease: RegistrationLease,
    pub service_route: Option<String>,
    pub associated_uris: Vec<String>,
}

impl RegisteredImsContext {
    pub fn from_response(
        access: ImsRegistrationAccess,
        response: &[u8],
        default_expires_seconds: u32,
    ) -> Self {
        Self::from_artifacts(
            access,
            RegisterArtifacts::parse(response),
            default_expires_seconds,
        )
    }

    pub fn from_artifacts(
        access: ImsRegistrationAccess,
        artifacts: RegisterArtifacts,
        default_expires_seconds: u32,
    ) -> Self {
        Self {
            access,
            registered_at: SystemTime::now(),
            lease: RegistrationLease::from_artifacts(&artifacts, default_expires_seconds),
            service_route: artifacts.service_route,
            associated_uris: artifacts.associated_uris,
        }
    }

    pub fn default_associated_uri(&self) -> Option<&str> {
        self.associated_uris.first().map(String::as_str)
    }
}

/// Shared reason why an adapter can no longer keep a registration usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationLossReason {
    Expired,
    AuthenticationRejected,
    NetworkRejected,
    SignalingTransportLost,
    AccessTransportLost,
}

impl RegistrationLossReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::AuthenticationRejected => "authentication_rejected",
            Self::NetworkRejected => "network_rejected",
            Self::SignalingTransportLost => "signaling_transport_lost",
            Self::AccessTransportLost => "access_transport_lost",
        }
    }

    /// Classify a shared REGISTER failure without depending on an access
    /// adapter's error vocabulary. A received final SIP response is a network
    /// decision; failure before any complete response is a signaling-path
    /// failure. Authentication challenges are kept separate so callers can
    /// avoid retrying a rejected SIM credential indefinitely.
    pub fn from_register_failure(failure: &RegisterFailure) -> Self {
        if failure.error.code() == "ims_register_auth_rejected" {
            return Self::AuthenticationRejected;
        }

        if let Some(status) = failure
            .response
            .as_deref()
            .and_then(|response| parse_status(response).ok())
        {
            return match status {
                401 | 403 | 407 => Self::AuthenticationRejected,
                _ => Self::NetworkRejected,
            };
        }

        Self::SignalingTransportLost
    }
}

/// Common semantic result of a refresh attempt. Adapters map their concrete
/// errors to this result before deciding which bearer/tunnel resources to
/// rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationRefreshResult {
    Refreshed(RegisteredImsContext),
    RebuildAccess(RegistrationLossReason),
}

/// Common semantic result of an unregister attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnregisterResult {
    Confirmed,
    AlreadyExpired,
    Rejected,
    AccessLost,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_expiry_drives_the_shared_lease() {
        let response = concat!(
            "SIP/2.0 200 OK\r\n",
            "Contact: <sip:user@10.0.0.2>;expires=120\r\n",
            "Expires: 3600\r\n",
            "Service-Route: <sip:pcscf.example;lr>\r\n",
            "P-Associated-URI: <sip:user@ims.example>\r\n\r\n",
        );
        let context = RegisteredImsContext::from_response(
            ImsRegistrationAccess::Vowifi,
            response.as_bytes(),
            7200,
        );

        assert_eq!(context.lease.expires_seconds, 120);
        assert_eq!(context.lease.refresh_after, Duration::from_secs(110));
        assert_eq!(context.lease.expires_after, Duration::from_secs(120));
        assert_eq!(
            context.service_route.as_deref(),
            Some("<sip:pcscf.example;lr>")
        );
        assert_eq!(
            context.default_associated_uri(),
            Some("sip:user@ims.example")
        );
    }

    #[test]
    fn profile_expiry_is_only_the_missing_header_fallback() {
        let context = RegisteredImsContext::from_response(
            ImsRegistrationAccess::Volte,
            b"SIP/2.0 200 OK\r\n\r\n",
            600,
        );
        assert_eq!(context.lease.expires_seconds, 600);
        assert_eq!(context.lease.refresh_after, Duration::from_secs(550));
    }

    #[test]
    fn zero_expiry_never_creates_a_zero_duration_timer() {
        let lease = RegistrationLease::from_expires(0);
        assert_eq!(lease.expires_seconds, 1);
        assert_eq!(lease.refresh_after, Duration::from_secs(1));
        assert_eq!(lease.expires_after, Duration::from_secs(1));
    }

    #[test]
    fn register_failure_classification_is_access_neutral() {
        let auth = RegisterFailure {
            error: super::super::ImsError::new("ims_register_auth_rejected"),
            response: Some(b"SIP/2.0 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec()),
            auth_rounds: 2,
        };
        assert_eq!(
            RegistrationLossReason::from_register_failure(&auth),
            RegistrationLossReason::AuthenticationRejected
        );

        let rejected = RegisterFailure {
            error: super::super::ImsError::new("ims_register_authenticated_unexpected_status"),
            response: Some(
                b"SIP/2.0 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n".to_vec(),
            ),
            auth_rounds: 1,
        };
        assert_eq!(
            RegistrationLossReason::from_register_failure(&rejected),
            RegistrationLossReason::NetworkRejected
        );

        let transport = RegisterFailure {
            error: super::super::ImsError::new("ims_register_initial_receive_failed"),
            response: None,
            auth_rounds: 0,
        };
        assert_eq!(
            RegistrationLossReason::from_register_failure(&transport),
            RegistrationLossReason::SignalingTransportLost
        );
    }
}
