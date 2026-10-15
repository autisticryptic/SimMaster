use super::*;
use crate::hardware::devices::ims_bearer_policy::{ImsApnAuthMethod, ImsApnAuthentication};
use crate::connectivity::modems::ims::effective_profile::resolve_effective_ims_profile;
use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};

fn baseline() -> &'static CarrierProfile {
    derive_standard_3gpp_profile("001", "01", Standard3gppAccess::LteEpc).unwrap()
}

fn record_with_credentials() -> CarrierProfileRecord {
    let mut record = CarrierProfileRecord::from_profile(baseline());
    record.meta.profile_id = "synthetic-bearer-record".into();
    record.ims.bearer.authentication = ImsApnAuthentication {
        method: ImsApnAuthMethod::Chap,
        username: Some(" synthetic-record-user ".into()),
        password: Some(" synthetic-record-password ".into()),
    };
    record
}

#[test]
fn standard_derived_profiles_keep_legacy_bearer_and_no_credentials() {
    use crate::connectivity::modems::ims::vowifi::profiles::{
        derive_standard_3gpp_profile, Standard3gppAccess,
    };
    for access in [Standard3gppAccess::LteEpc, Standard3gppAccess::WifiEpdg] {
        let profile = derive_standard_3gpp_profile("001", "01", access).unwrap();
        assert!(profile.ims.bearer.is_default());
        let record = CarrierProfileRecord::from_profile(profile);
        assert!(record.ims.bearer.is_default());
        assert!(resolve_effective_ims_profile(profile, None).bearer.is_default());
    }
}

#[test]
fn old_record_without_bearer_preserves_default_execution_policy() {
    let record = CarrierProfileRecord::from_profile(baseline());
    let mut value = serde_json::to_value(&record).unwrap();
    value["ims"].as_object_mut().unwrap().remove("bearer");
    let loaded = CarrierProfileRecord::from_database_json(&value.to_string()).unwrap();
    assert!(loaded.ims.bearer.is_default());
    assert!(loaded.intern().ims.bearer.is_default());
    assert_eq!(loaded, record);
}

#[test]
fn record_intern_and_effective_projection_preserve_exact_credentials() {
    let record = record_with_credentials();
    record.validate_ims_only().unwrap();
    let json = serde_json::to_string(&record).unwrap();
    let loaded = CarrierProfileRecord::from_database_json(&json).unwrap();
    let profile = loaded.intern();
    assert!(std::ptr::eq(profile, loaded.intern()));
    assert_eq!(*profile.ims.bearer, record.ims.bearer);
    assert_eq!(CarrierProfileRecord::from_profile(profile), loaded);
    let effective = resolve_effective_ims_profile(profile, None);
    assert_eq!(effective.bearer, record.ims.bearer);
    assert!(effective.validate_bearer().is_ok());
}

#[test]
fn record_empty_password_survives_interning_instead_of_becoming_absent() {
    let mut record = record_with_credentials();
    record.meta.profile_id = "synthetic-empty-bearer-password".into();
    record.ims.bearer.authentication.password = Some(String::new());
    record.validate_ims_only().unwrap();
    assert_eq!(record.intern().ims.bearer.authentication.password.as_deref(), Some(""));
}

#[test]
fn record_debug_response_and_invalid_input_errors_do_not_echo_credentials() {
    let record = record_with_credentials();
    let safe = serde_json::to_string(&record.sanitized()).unwrap();
    let debug = format!("{record:?} {:?}", record.intern());
    let public_runtime_profile = serde_json::to_string(record.intern()).unwrap();
    for value in ["synthetic-record-user", "synthetic-record-password"] {
        assert!(!safe.contains(value));
        assert!(!debug.contains(value));
        assert!(!public_runtime_profile.contains(value));
    }
    let mut malformed = serde_json::to_value(&record).unwrap();
    malformed["ims"]["bearer"]["authentication"]["method"] = "misplaced-synthetic-secret".into();
    for error in [
        CarrierProfileRecord::from_database_json(&malformed.to_string()).unwrap_err(),
        CarrierProfileRecord::from_api_value(malformed).unwrap_err(),
    ] {
        assert_eq!(error, "carrier_profile_json_invalid");
        assert!(!error.contains("misplaced-synthetic-secret"));
    }
    assert!(record.sanitized().validate_ims_only().is_err());
}

#[test]
fn malformed_record_authentication_is_rejected_before_interning() {
    let mut record = record_with_credentials();
    record.ims.bearer.authentication.password = None;
    assert_eq!(record.validate_ims_only().unwrap_err(), "ims_bearer_auth_password_required");
    record.ims.bearer.authentication.method = ImsApnAuthMethod::None;
    assert_eq!(record.validate_ims_only().unwrap_err(), "ims_bearer_auth_credentials_conflict");
}
