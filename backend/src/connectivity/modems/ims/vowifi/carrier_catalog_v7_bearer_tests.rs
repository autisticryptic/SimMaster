use super::*;
use serde_json::json;
use crate::connectivity::modems::ims::effective_profile::{
    resolve_effective_ims_profile, resolve_effective_vowifi_ims_profile,
};

fn project(access_config: Value, access: CatalogAccessKind) -> Result<CarrierProfileRecord, String> {
    let meta = profile_meta("Synthetic", "Synthetic", "", "00101").unwrap();
    let release = CatalogRelease {
        release_id: "synthetic-bearer-release".into(),
        generated_at: "2026-01-01T00:00:00Z".into(),
        sealed: true,
    };
    let mut config = json!({"protocol_baseline": PROTOCOL_BASELINE, "access": {}});
    config["access"][access.v7_config_key()] = access_config;
    project_config("synthetic-bearer-projector", &meta, Vec::new(), &release, access, &config)
}

#[test]
fn catalog_projector_preserves_all_pixel_auth_methods_and_exact_credentials() {
    for (token, method) in [
        ("pap", ImsApnAuthMethod::Pap),
        ("chap", ImsApnAuthMethod::Chap),
        ("pap_or_chap", ImsApnAuthMethod::PapOrChap),
    ] {
        let record = project(json!({
            "apn": "synthetic-ims", "auth_type": token,
            "username": " synthetic-catalog-user ", "password": " synthetic-catalog-password "
        }), CatalogAccessKind::LteEpc).unwrap();
        assert_eq!(record.ims.bearer.authentication.method, method);
        assert_eq!(record.ims.bearer.authentication.username.as_deref(), Some(" synthetic-catalog-user "));
        assert_eq!(record.ims.bearer.authentication.password.as_deref(), Some(" synthetic-catalog-password "));
        let profile = record.intern();
        let effective = resolve_effective_ims_profile(profile, None);
        assert_eq!(effective.bearer, record.ims.bearer);
        assert!(effective.validate_bearer().is_ok());
    }
    let empty_password = project(json!({
        "auth_type": "chap", "username": "synthetic-user", "password": ""
    }), CatalogAccessKind::LteEpc).unwrap();
    assert_eq!(empty_password.intern().ims.bearer.authentication.password.as_deref(), Some(""));
}

#[test]
fn catalog_discovery_hints_do_not_become_mandatory_modem_profile_writes() {
    for config in [json!({}), json!({"pcscf_discovery": ["pco", "dhcp"]}), json!({"auth_type": "unspecified"})] {
        let record = project(config, CatalogAccessKind::LteEpc).unwrap();
        assert!(record.ims.bearer.is_default());
        assert!(resolve_effective_ims_profile(record.intern(), None).bearer.is_default());
    }
    let explicit_none = project(json!({"auth_type": "none"}), CatalogAccessKind::LteEpc).unwrap();
    assert_eq!(explicit_none.ims.bearer.authentication.method, ImsApnAuthMethod::None);
    assert!(!explicit_none.ims.bearer.is_default());
}

#[test]
fn catalog_explicit_pco_dhcp_policies_survive_record_and_effective_projection() {
    for config in [
        json!({"pcscf_via_pco": "required", "pcscf_via_dhcp": "information_request"}),
        json!({"pcscf_via_pco": true, "pcscf_via_dhcp": true}),
    ] {
        let record = project(config, CatalogAccessKind::LteEpc).unwrap();
        let effective = resolve_effective_ims_profile(record.intern(), None);
        assert_eq!(effective.bearer.pcscf_via_pco, PcoRequestPolicy::Required);
        assert_eq!(effective.bearer.pcscf_via_dhcp, DhcpPcscfPolicy::InformationRequest);
    }
    for config in [
        json!({"pcscf_via_pco": "disabled", "pcscf_via_dhcp": "disabled"}),
        json!({"pcscf_via_pco": false, "pcscf_via_dhcp": false}),
    ] {
        let record = project(config, CatalogAccessKind::LteEpc).unwrap();
        assert_eq!(record.ims.bearer.pcscf_via_pco, PcoRequestPolicy::Disabled);
        assert_eq!(record.ims.bearer.pcscf_via_dhcp, DhcpPcscfPolicy::Disabled);
    }
}

#[test]
fn catalog_malformed_authentication_never_falls_back_or_echoes_input() {
    for config in [
        json!({"auth_type": "misplaced-synthetic-secret"}),
        json!({"auth_type": 1}),
        json!({"auth_type": null}),
        json!({"auth_type": "PAP"}),
        json!({"auth_type": "pap"}),
        json!({"auth_type": "chap", "username": "synthetic-user"}),
        json!({"auth_type": "pap", "username": "", "password": "synthetic-password"}),
        json!({"auth_type": "pap", "username": 7, "password": "synthetic-password"}),
        json!({"auth_type": "chap", "username": "synthetic-user", "password": null}),
        json!({"auth_type": "none", "username": "misplaced-synthetic-secret"}),
        json!({"username": "misplaced-synthetic-secret"}),
        json!({"auth_type": "unspecified", "password": "misplaced-synthetic-secret"}),
        json!({"auth_type": "pap", "username": "synthetic-user", "password": "bad\nsecret"}),
        json!({"pcscf_via_pco": "misplaced-synthetic-secret"}),
        json!({"pcscf_via_dhcp": ["misplaced-synthetic-secret"]}),
    ] {
        let error = project(config, CatalogAccessKind::LteEpc).unwrap_err();
        assert!(error.starts_with("carrier_catalog_bearer_invalid:"));
        for forbidden in ["misplaced-synthetic-secret", "synthetic-password", "bad\nsecret"] {
            assert!(!error.contains(forbidden));
        }
    }
}

#[test]
fn wifi_catalog_rejects_cellular_auth_without_changing_eap_usim_defaults() {
    let baseline = project(json!({"pcscf_discovery": ["ike_cfg"]}), CatalogAccessKind::WifiEpdg).unwrap();
    assert!(baseline.ims.bearer.is_default());
    assert_eq!(baseline.ikev2.aka_challenge_mode, "eap_aka");
    assert!(resolve_effective_vowifi_ims_profile(baseline.intern(), None).validate_bearer().is_ok());
    for config in [
        json!({"auth_type": "pap", "username": "synthetic-user", "password": "synthetic-password"}),
        json!({"pcscf_via_pco": true}),
        json!({"pcscf_via_dhcp": true}),
    ] {
        let error = project(config, CatalogAccessKind::WifiEpdg).unwrap_err();
        assert!(error.ends_with("ims_bearer_wifi_policy_unsupported"));
    }
}
