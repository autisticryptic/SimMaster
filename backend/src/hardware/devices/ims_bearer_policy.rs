//! Configured IMS bearer requirements, independent of driver capability.
//!
//! Serialization is for private persistence, not API responses. PAP/CHAP
//! credentials are never inferred from SIM/AKA material or ordinary data APNs.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImsApnAuthMethod {
    #[default]
    Unspecified,
    None,
    Pap,
    Chap,
    PapOrChap,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImsApnAuthentication {
    pub method: ImsApnAuthMethod,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for ImsApnAuthentication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImsApnAuthentication")
            .field("method", &self.method)
            .field("username", &self.username.as_ref().map(|_| "[REDACTED]"))
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

impl ImsApnAuthentication {
    pub fn validate(&self) -> Result<(), &'static str> {
        // Byte bounds are deliberate: these values ultimately enter bounded
        // modem protocol fields. Drivers may impose stricter CLI-safe bounds.
        for value in [self.username.as_deref(), self.password.as_deref()]
            .into_iter()
            .flatten()
        {
            if value.len() > 255 {
                return Err("ims_bearer_auth_credential_too_long");
            }
            if value.chars().any(char::is_control) {
                return Err("ims_bearer_auth_credential_control_character");
            }
        }
        match self.method {
            ImsApnAuthMethod::Unspecified | ImsApnAuthMethod::None => {
                if self.has_nonempty_credentials() {
                    return Err("ims_bearer_auth_credentials_conflict");
                }
            }
            ImsApnAuthMethod::Pap | ImsApnAuthMethod::Chap | ImsApnAuthMethod::PapOrChap => {
                if self.username.as_deref().is_none_or(str::is_empty) {
                    return Err("ims_bearer_auth_username_required");
                }
                // Some deployments explicitly use an empty password. Missing
                // and empty must remain different; never trim either value.
                if self.password.is_none() {
                    return Err("ims_bearer_auth_password_required");
                }
            }
        }
        Ok(())
    }

    pub fn has_nonempty_credentials(&self) -> bool {
        [self.username.as_deref(), self.password.as_deref()]
            .into_iter()
            .flatten()
            .any(|value| !value.is_empty())
    }

    /// Display projection only; a PAP/CHAP result is intentionally not valid
    /// execution input. It cannot be used to silently retain old credentials.
    pub fn sanitized(&self) -> Self {
        Self {
            method: self.method,
            username: None,
            password: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcoRequestPolicy {
    #[default]
    NetworkDefault,
    Required,
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DhcpPcscfPolicy {
    #[default]
    Disabled,
    InformationRequest,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImsBearerRequirements {
    pub authentication: ImsApnAuthentication,
    pub pcscf_via_pco: PcoRequestPolicy,
    pub pcscf_via_dhcp: DhcpPcscfPolicy,
}

impl fmt::Debug for ImsBearerRequirements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImsBearerRequirements")
            .field("authentication", &self.authentication)
            .field("pcscf_via_pco", &self.pcscf_via_pco)
            .field("pcscf_via_dhcp", &self.pcscf_via_dhcp)
            .finish()
    }
}

pub const DEFAULT_IMS_BEARER_REQUIREMENTS: ImsBearerRequirements = ImsBearerRequirements {
    authentication: ImsApnAuthentication {
        method: ImsApnAuthMethod::Unspecified,
        username: None,
        password: None,
    },
    pcscf_via_pco: PcoRequestPolicy::NetworkDefault,
    pcscf_via_dhcp: DhcpPcscfPolicy::Disabled,
};

impl ImsBearerRequirements {
    pub fn validate(&self) -> Result<(), &'static str> {
        self.authentication.validate()
    }

    /// Exact legacy requirement, for fail-closed admission by old providers.
    /// Explicit `none` is not equivalent to leaving modem policy unchanged.
    pub fn is_default(&self) -> bool {
        self == &DEFAULT_IMS_BEARER_REQUIREMENTS
    }

    /// WiFi EAP/USIM is a separate authentication protocol. Never borrow it to
    /// satisfy a cellular PAP/CHAP or modem-profile PCO/DHCP requirement.
    pub fn validate_for_wifi(&self) -> Result<(), &'static str> {
        self.validate()?;
        if !matches!(
            self.authentication.method,
            ImsApnAuthMethod::Unspecified | ImsApnAuthMethod::None
        ) || self.pcscf_via_pco != PcoRequestPolicy::NetworkDefault
            || self.pcscf_via_dhcp != DhcpPcscfPolicy::Disabled
        {
            return Err("ims_bearer_wifi_policy_unsupported");
        }
        Ok(())
    }

    pub fn sanitized(&self) -> Self {
        Self {
            authentication: self.authentication.sanitized(),
            pcscf_via_pco: self.pcscf_via_pco,
            pcscf_via_dhcp: self.pcscf_via_dhcp,
        }
    }
}

#[cfg(test)]
#[path = "ims_bearer_policy_tests.rs"]
mod tests;
