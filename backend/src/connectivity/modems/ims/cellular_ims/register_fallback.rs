//! Attempt-local invariants for static REGISTER fallbacks at one P-CSCF.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RegisterFallbackState {
    empty_aka: bool,
    advertise_sec_agree: bool,
    server_required_sec_agree: bool,
    // Unlike the legacy dynamic-ladder flag, this excludes timeout probes.
    confirmed_security_required: bool,
}

impl RegisterFallbackState {
    pub(super) fn new(first: CellularImsRegisterVariant) -> Self {
        Self {
            empty_aka: first.authorization == CellularImsInitialAuthorization::UriFirstEmptyAka,
            advertise_sec_agree: first.policy.advertise_sec_agree,
            // A proactive local Require is not evidence of a server demand.
            server_required_sec_agree: first.server_required_sec_agree,
            confirmed_security_required: false,
        }
    }

    pub(super) fn observe(
        &mut self,
        profile: &CarrierProfile,
        current: CellularImsRegisterVariant,
        failure: &RegisterFailure,
    ) -> Result<CellularImsRegisterVariant, &'static str> {
        // This helper never authorizes a retry or changes the caller's stop rules.
        if !pre_authentication_variant_failure(failure) {
            return Ok(current);
        }
        let mut required = self.server_required_sec_agree
            || profile.ims.register.sec_agree_mode == "required";
        let mut confirmed = self.confirmed_security_required;
        if matches!(register_failure_status(failure), Some(421 | 494)) {
            let response = failure.response.as_deref().unwrap_or_default();
            let demand = register_failure_status(failure) == Some(494);
            required |= demand;
            confirmed |= demand;
            for header in ["Require", "Proxy-Require"] {
                let values = sip::header_values(response, header);
                if !values.is_empty() {
                    // Every required token must be understood. Do not let an
                    // empty duplicate header/list member disappear in parsing.
                    if values.iter().any(|value| value.split(',').any(|token|
                        !token.trim().eq_ignore_ascii_case("sec-agree"))) {
                        return Err("unsupported_required_extension");
                    }
                    required = true;
                    confirmed = true;
                }
            }
            // Security-Server alone on 421 is only an offer, not a requirement.
        }
        if register_failure_status(failure) == Some(420)
            && crate::connectivity::modems::ims::vowifi::profiles::is_standard_derived_profile(profile)
            && failure.response.as_deref().is_some_and(|response|
                response_has_only_extension(response, "Unsupported", "sec-agree")
                    && response_warns_missing_sec_agree(response))
        {
            required = true;
            confirmed = true;
        }
        if (required || current.server_required_sec_agree)
            && profile.ims.register.sec_agree_mode == "disabled"
        {
            return Err("required_security_disabled");
        }
        self.empty_aka |= current.authorization == CellularImsInitialAuthorization::UriFirstEmptyAka;
        self.advertise_sec_agree |= current.policy.advertise_sec_agree;
        self.server_required_sec_agree = required;
        self.confirmed_security_required = confirmed;
        // A probe retains the legacy dynamic format ladder, but is not learned
        // as a mandatory condition for later static candidates or AKA.
        // Preserve the actual sent header policy, including require-only syntax.
        Ok(CellularImsRegisterVariant {
            server_required_sec_agree: required || current.server_required_sec_agree,
            ..current
        })
    }

    pub(super) fn requires_protection(&self, profile: &CarrierProfile) -> bool {
        self.confirmed_security_required || profile.ims.register.sec_agree_mode == "required"
    }

    /// Apply only to NEW STATIC candidates, never pending dynamic variants.
    pub(super) fn apply(&self, mut next: CellularImsRegisterVariant) -> CellularImsRegisterVariant {
        if self.empty_aka {
            next.authorization = CellularImsInitialAuthorization::UriFirstEmptyAka;
        }
        next.policy.advertise_sec_agree |= self.advertise_sec_agree;
        if self.server_required_sec_agree && !next.server_required_sec_agree {
            next = next.requiring_sec_agree();
        }
        // Already-required candidates retain their configured Require-only
        // syntax. In particular applying the seed must not rewrite SM1.
        // Offer formatting/mechanism restrictions and all other fields survive.
        next
    }
}

/// Semantic candidates, not labels or fresh transaction IDs/SPI/ports. Apply
/// inheritance BEFORE recording; repeats must not consume the wire budget.
#[derive(Default)]
pub(super) struct RegisterCandidateHistory {
    tried: Vec<(CellularImsRegisterVariant, bool)>,
}

impl RegisterCandidateHistory {
    pub(super) fn record(&mut self, candidate: CellularImsRegisterVariant, protected: bool) -> bool {
        if self.tried.iter().any(|(old, old_protected)|
            *old_protected == protected
                && old.authorization == candidate.authorization
                && old.policy == candidate.policy
                && old.security_client_offer == candidate.security_client_offer
                && old.security_mechanism == candidate.security_mechanism)
        {
            return false;
        }
        self.tried.push((candidate, protected));
        true
    }
}

#[cfg(test)]
#[path = "register_fallback_tests.rs"]
mod tests;
