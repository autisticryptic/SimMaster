use super::*;
use crate::hardware::devices::ims_bearer_policy::{
    ImsApnAuthentication, ImsApnAuthMethod, PcoRequestPolicy,
};
use crate::connectivity::modems::ims::vowifi::{
    profile_record::CarrierProfileRecord,
    profiles::{derive_standard_3gpp_profile, Standard3gppAccess},
};

fn baseline() -> &'static CarrierProfile {
    derive_standard_3gpp_profile("001", "01", Standard3gppAccess::LteEpc).unwrap()
}

fn authenticated() -> ImsBearerRequirements {
    ImsBearerRequirements {
        authentication: ImsApnAuthentication {
            method: ImsApnAuthMethod::Pap,
            username: Some("synthetic-effective-user".into()),
            password: Some("synthetic-effective-password".into()),
        },
        pcscf_via_pco: PcoRequestPolicy::Required,
        ..Default::default()
    }
}

fn catalog() -> &'static CarrierProfile {
    let mut record = CarrierProfileRecord::from_profile(baseline());
    record.meta.profile_id = "synthetic-effective-bearer".into();
    record.ims.bearer = authenticated();
    record.intern()
}

#[test]
fn bearer_override_is_atomic_not_a_credential_field_merge() {
    let mut override_ = SimOverride::default();
    override_.ims_cellular.bearer = Some(ImsBearerRequirements::default());
    let profile = resolve_effective_ims_profile(catalog(), Some(&override_));
    assert!(profile.bearer.is_default());
    assert_eq!(profile.bearer_source, OverrideSource::SimOverride);
    assert!(profile.validate_bearer().is_ok());

    let mut incomplete = authenticated();
    incomplete.authentication.password = None;
    override_.ims_cellular.bearer = Some(incomplete);
    let profile = resolve_effective_ims_profile(catalog(), Some(&override_));
    assert!(profile.bearer.authentication.password.is_none());
    assert_eq!(profile.validate_bearer(), Err("ims_bearer_auth_password_required"));
}

#[test]
fn apn_edit_cannot_forward_catalog_credentials_without_explicit_bearer() {
    let mut override_ = SimOverride::default();
    override_.ims_cellular.apn = Some("different-ims-apn".into());
    let profile = resolve_effective_ims_profile(catalog(), Some(&override_));
    assert_eq!(profile.validate_bearer(), Err("ims_bearer_apn_override_requires_explicit_bearer"));
    // The error is explicit, not a silent fallback to a no-auth request.
    assert_eq!(profile.bearer, authenticated());
    override_.ims_cellular.bearer = Some(ImsBearerRequirements::default());
    assert!(resolve_effective_ims_profile(catalog(), Some(&override_)).validate_bearer().is_ok());
    override_.ims_cellular.bearer = Some(authenticated());
    assert!(resolve_effective_ims_profile(catalog(), Some(&override_)).validate_bearer().is_ok());
}

#[test]
fn unchanged_apn_or_uncredentialed_catalog_keeps_legacy_admission() {
    let mut override_ = SimOverride::default();
    override_.ims_cellular.apn = catalog().epdg.apn.map(str::to_string);
    assert!(resolve_effective_ims_profile(catalog(), Some(&override_)).validate_bearer().is_ok());
    override_.ims_cellular.apn = Some("different-ims-apn".into());
    let profile = resolve_effective_ims_profile(baseline(), Some(&override_));
    assert!(profile.validate_bearer().is_ok());
    assert!(profile.bearer.is_default());
}

#[test]
fn bearer_override_stays_with_its_sim_and_access() {
    let mut first = SimOverride::default();
    first.ims_cellular.bearer = Some(authenticated());
    let second = SimOverride::default();
    let cellular = resolve_effective_ims_profile(baseline(), Some(&first));
    assert_eq!(cellular.bearer, authenticated());
    assert_eq!(cellular.bearer_source, OverrideSource::SimOverride);
    for profile in [
        resolve_effective_vowifi_ims_profile(baseline(), Some(&first)),
        resolve_effective_ims_profile(baseline(), Some(&second)),
    ] {
        assert!(profile.bearer.is_default());
        assert_eq!(profile.bearer_source, OverrideSource::Catalog);
        assert!(profile.validate_bearer().is_ok());
    }
}

#[test]
fn wifi_gate_rejects_cellular_catalog_and_override_policy_without_using_eap() {
    assert_eq!(resolve_effective_vowifi_ims_profile(catalog(), None).validate_bearer(),
        Err("ims_bearer_wifi_policy_unsupported"));
    let mut override_ = SimOverride::default();
    override_.ims_vowifi.bearer = Some(authenticated());
    assert_eq!(resolve_effective_vowifi_ims_profile(baseline(), Some(&override_)).validate_bearer(),
        Err("ims_bearer_wifi_policy_unsupported"));
    assert!(validate_override(&override_).contains(&"ims_vowifi.ims_bearer_wifi_policy_unsupported".to_string()));
    override_.ims_vowifi.bearer = Some(ImsBearerRequirements::default());
    assert!(resolve_effective_vowifi_ims_profile(catalog(), Some(&override_)).validate_bearer().is_ok());
}

#[test]
fn effective_profile_debug_never_contains_bearer_credentials() {
    let debug = format!("{:?}", resolve_effective_ims_profile(catalog(), None));
    assert!(!debug.contains("synthetic-effective-user"));
    assert!(!debug.contains("synthetic-effective-password"));
}
