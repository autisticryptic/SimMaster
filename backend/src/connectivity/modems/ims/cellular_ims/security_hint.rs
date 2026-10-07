//! Initial 421/494 security hints are not AKA challenges or installed SAs.
//! One constrained reoffer; never drop to generic after accepting this branch.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Decision {
    NotApplicable,
    Retry(CellularImsRegisterVariant),
    Stop(&'static str),
}

pub(super) fn decide(
    profile: &CarrierProfile,
    variant: CellularImsRegisterVariant,
    failure: &RegisterFailure,
) -> Decision {
    if variant.security_mechanism.is_some() {
        // No loop, formatting/identity experiment, or generic fallback after
        // this one-shot reoffer. Normal authenticated refresh is independent.
        return Decision::Stop("security_hint_reoffer_finished");
    }
    if failure.auth_rounds != 0 || !matches!(register_failure_status(failure), Some(421 | 494))
        || !crate::connectivity::modems::ims::vowifi::profiles::is_standard_derived_profile(profile)
    {
        return Decision::NotApplicable;
    }
    let Some(response) = failure.response.as_deref() else { return Decision::NotApplicable; };
    let offers = sip::header_values(response, "Security-Server");
    if offers.is_empty() { return Decision::NotApplicable; }
    if profile.ims.register.sec_agree_mode == "disabled" {
        return Decision::Stop("security_hint_conflicts_with_disabled_policy");
    }
    // The historical Require-only escalation is unchanged. This new path is
    // for a complete proactively-declared, empty-AKA, full initial request.
    if variant.authorization != CellularImsInitialAuthorization::UriFirstEmptyAka
        || !variant.policy.advertise_sec_agree || !variant.policy.require_sec_agree
        || !variant.policy.proxy_require_sec_agree
        || variant.security_client_offer != CellularImsSecurityClientOffer::Full
    {
        return Decision::NotApplicable;
    }
    for name in ["Require", "Proxy-Require"] {
        if !sip::header_values(response, name).is_empty()
            && !response_has_only_extension(response, name, "sec-agree")
        {
            return Decision::Stop("security_hint_requires_unknown_extension");
        }
    }
    if !sip::header_values(response, "WWW-Authenticate").is_empty()
        || !sip::header_values(response, "Proxy-Authenticate").is_empty()
    {
        return Decision::Stop("security_hint_is_not_an_aka_challenge");
    }
    // A bare 421 (without named required extensions) does not establish a
    // mandatory security transition. Let the ordinary identity-preserving
    // fallback try the historical request envelope before guessing a cipher.
    if register_failure_status(failure) == Some(421)
        && !response_has_only_extension(response, "Require", "sec-agree")
        && !response_has_only_extension(response, "Proxy-Require", "sec-agree")
    {
        return Decision::NotApplicable;
    }
    let allowed = profile.ims.register.security_client_mechanisms;
    let unique = allowed.iter().map(|s| s.to_ascii_lowercase()).collect::<std::collections::HashSet<_>>();
    if unique.len() <= 1 {
        return Decision::Stop("security_hint_would_repeat_same_offer");
    }
    let agreement = match super::super::security_agreement::select(&offers, allowed, true) {
        Ok(Some(agreement)) => agreement,
        _ => return Decision::Stop("security_hint_unusable_or_unoffered"),
    };
    let Some(index) = agreement.matched_mechanism else {
        return Decision::Stop("security_hint_unusable_or_unoffered");
    };
    // An unauthenticated hint must not cause a newly narrowed integrity-only
    // offer. Keep the existing preferred AES alternative; null remains allowed
    // only in the original full-offer/challenge path, not this extra retry.
    if index != 0 || !allowed[index].split('/').nth(1).is_some_and(|e| e.eq_ignore_ascii_case("aes-cbc")) {
        return Decision::Stop("security_hint_cannot_narrow_to_weaker_policy");
    }
    Decision::Retry(CellularImsRegisterVariant {
        label: "standard_3gpp_security_hint_reoffer",
        security_mechanism: Some(index),
        server_required_sec_agree: true,
        ..variant
    })
}

#[cfg(test)]
#[path = "security_hint_tests.rs"]
mod tests;
