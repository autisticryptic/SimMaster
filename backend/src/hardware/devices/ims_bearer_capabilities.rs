//! Admission is based on implemented provider adapters, never the modem name.
//! A configured requirement is not evidence that it was executed.

use serde::Serialize;

use crate::hardware::devices::ims_bearer_policy::{
    DhcpPcscfPolicy, ImsApnAuthMethod, ImsBearerRequirements, PcoRequestPolicy,
};
use super::{ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

/// Static adapter support, not network provisioning or registration readiness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ImsBearerCapabilities {
    pub auth_none: CapabilitySupport,
    pub pap: CapabilitySupport,
    pub chap: CapabilitySupport,
    pub pap_or_chap: CapabilitySupport,
    /// Setting a modem profile's PCO request bit with owned readback/rollback.
    /// Reporting an already-negotiated PCO does NOT implement this capability.
    pub pco_request_policy: CapabilitySupport,
    pub pco_pcscf_addresses: CapabilitySupport,
    pub pco_pcscf_domains: CapabilitySupport,
    /// Information-only DHCP on an already-granted, worker-owned IMS link.
    /// This never claims DHCP address assignment support.
    pub dhcp_information: CapabilitySupport,
}

impl ImsBearerCapabilities {
    pub fn check(self, requirements: &ImsBearerRequirements) -> Result<(), ImsBearerError> {
        requirements.validate().map_err(invalid_requirements)?;
        let auth_support = match requirements.authentication.method {
            ImsApnAuthMethod::Unspecified => None,
            ImsApnAuthMethod::None => Some((self.auth_none, "ims_bearer_explicit_no_auth_unsupported")),
            ImsApnAuthMethod::Pap => Some((self.pap, "ims_bearer_pap_unsupported")),
            ImsApnAuthMethod::Chap => Some((self.chap, "ims_bearer_chap_unsupported")),
            ImsApnAuthMethod::PapOrChap => Some((self.pap_or_chap, "ims_bearer_pap_or_chap_unsupported")),
        };
        if let Some((support, reason)) = auth_support {
            require_support(support, reason)?;
        }
        if requirements.pcscf_via_pco != PcoRequestPolicy::NetworkDefault {
            require_support(self.pco_request_policy, "ims_bearer_pco_request_policy_unsupported")?;
        }
        if requirements.pcscf_via_dhcp == DhcpPcscfPolicy::InformationRequest {
            require_support(self.dhcp_information, "ims_bearer_dhcp_information_unsupported")?;
        }
        Ok(())
    }

    /// Existing retained-MM adapters have no credential/profile-PCO policy
    /// implementation. They do provide their exact-session address observation
    /// and an exclusive link on which the common DHCP information adapter runs.
    pub fn retained_mm() -> Self {
        use CapabilitySupport::{Supported, Unsupported};
        Self {
            auth_none: Unsupported,
            pap: Unsupported,
            chap: Unsupported,
            pap_or_chap: Unsupported,
            pco_request_policy: Unsupported,
            pco_pcscf_addresses: Supported,
            pco_pcscf_domains: Unsupported,
            dhcp_information: if cfg!(target_os = "linux") { Supported } else { Unsupported },
        }
    }
}

fn require_support(support: CapabilitySupport, reason: &'static str) -> Result<(), ImsBearerError> {
    if support == CapabilitySupport::Supported { Ok(()) } else { Err(unsupported_requirements(reason)) }
}

pub(super) fn invalid_requirements(reason: &'static str) -> ImsBearerError {
    ImsBearerError { kind: ImsBearerErrorKind::RequirementsInvalid,
        hint: ImsBearerFailureHint::None, detail: reason.to_owned() }
}

pub(super) fn unsupported_requirements(reason: &'static str) -> ImsBearerError {
    ImsBearerError { kind: ImsBearerErrorKind::RequirementsUnsupported,
        hint: ImsBearerFailureHint::None, detail: reason.to_owned() }
}

/// The legacy establishment signature can consume only the old APN parameters.
/// DHCP is handled by the shared worker layer after successful establishment.
/// Even a provider mistakenly advertising PAP cannot silently discard it here.
pub(super) fn legacy_parameters_only(requirements: &ImsBearerRequirements) -> bool {
    requirements.authentication.method == ImsApnAuthMethod::Unspecified
        && requirements.pcscf_via_pco == PcoRequestPolicy::NetworkDefault
}

#[cfg(test)]
#[path = "ims_bearer_capabilities_tests.rs"]
mod tests;
