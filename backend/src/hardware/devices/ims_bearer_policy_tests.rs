use super::*;

fn authenticated(method: ImsApnAuthMethod) -> ImsBearerRequirements {
    ImsBearerRequirements {
        authentication: ImsApnAuthentication {
            method,
            username: Some(" synthetic-user ".into()),
            password: Some(" synthetic-password ".into()),
        },
        ..Default::default()
    }
}

#[test]
fn default_bearer_is_legacy_and_explicit_none_is_not() {
    let default: ImsBearerRequirements = serde_json::from_str("{}").unwrap();
    assert_eq!(default, DEFAULT_IMS_BEARER_REQUIREMENTS);
    assert!(default.is_default());
    assert!(default.validate().is_ok());
    assert!(default.validate_for_wifi().is_ok());
    let mut explicit_none = default;
    explicit_none.authentication.method = ImsApnAuthMethod::None;
    assert!(!explicit_none.is_default());
    assert!(explicit_none.validate().is_ok());
}

#[test]
fn pap_chap_require_explicit_credentials_but_allow_explicit_empty_password() {
    for method in [ImsApnAuthMethod::Pap, ImsApnAuthMethod::Chap, ImsApnAuthMethod::PapOrChap] {
        let mut bearer = authenticated(method);
        assert!(bearer.validate().is_ok());
        bearer.authentication.password = Some(String::new());
        assert!(bearer.validate().is_ok());
        bearer.authentication.password = None;
        assert_eq!(bearer.validate(), Err("ims_bearer_auth_password_required"));
        bearer.authentication.password = Some("synthetic".into());
        for username in [None, Some(String::new())] {
            bearer.authentication.username = username;
            assert_eq!(bearer.validate(), Err("ims_bearer_auth_username_required"));
        }
    }
}

#[test]
fn absent_or_none_method_never_silently_discards_credentials() {
    for method in [ImsApnAuthMethod::Unspecified, ImsApnAuthMethod::None] {
        let mut bearer = authenticated(method);
        assert_eq!(bearer.validate(), Err("ims_bearer_auth_credentials_conflict"));
        bearer.authentication.username = None;
        assert_eq!(bearer.validate(), Err("ims_bearer_auth_credentials_conflict"));
        bearer.authentication.password = Some(String::new());
        assert!(bearer.validate().is_ok());
        assert!(!bearer.is_default());
    }
}

#[test]
fn credentials_have_byte_and_control_limits_without_trimming() {
    for invalid in ["x".repeat(256), "é".repeat(128)] {
        let mut bearer = authenticated(ImsApnAuthMethod::Pap);
        bearer.authentication.password = Some(invalid);
        assert_eq!(bearer.validate(), Err("ims_bearer_auth_credential_too_long"));
    }
    for invalid in ["a\nb", "a\rb", "a\0b", "a\tb", "a\u{85}b"] {
        let mut bearer = authenticated(ImsApnAuthMethod::Chap);
        bearer.authentication.username = Some(invalid.into());
        assert_eq!(bearer.validate(), Err("ims_bearer_auth_credential_control_character"));
    }
    assert!(authenticated(ImsApnAuthMethod::Pap).validate().is_ok());
}

#[test]
fn debug_and_sanitized_hide_both_credentials_while_storage_is_exact() {
    let bearer = authenticated(ImsApnAuthMethod::PapOrChap);
    let debug = format!("{bearer:?}");
    let safe = serde_json::to_string(&bearer.sanitized()).unwrap();
    for credential in ["synthetic-user", "synthetic-password"] {
        assert!(!debug.contains(credential));
        assert!(!safe.contains(credential));
    }
    assert!(safe.contains("pap_or_chap"));
    assert!(bearer.sanitized().validate().is_err());
    let json = serde_json::to_string(&bearer).unwrap();
    assert_eq!(serde_json::from_str::<ImsBearerRequirements>(&json).unwrap(), bearer);
}

#[test]
fn wifi_does_not_borrow_eap_for_cellular_bearer_requirements() {
    for method in [ImsApnAuthMethod::Pap, ImsApnAuthMethod::Chap, ImsApnAuthMethod::PapOrChap] {
        assert_eq!(authenticated(method).validate_for_wifi(), Err("ims_bearer_wifi_policy_unsupported"));
    }
    for bearer in [
        ImsBearerRequirements { pcscf_via_pco: PcoRequestPolicy::Required, ..Default::default() },
        ImsBearerRequirements { pcscf_via_pco: PcoRequestPolicy::Disabled, ..Default::default() },
        ImsBearerRequirements { pcscf_via_dhcp: DhcpPcscfPolicy::InformationRequest, ..Default::default() },
    ] {
        assert_eq!(bearer.validate_for_wifi(), Err("ims_bearer_wifi_policy_unsupported"));
    }
}

#[test]
fn typed_config_rejects_unknown_fields_methods_and_duplicate_authentication() {
    for json in [
        r#"{"authentication":{"method":"automatic"}}"#,
        r#"{"authentication":{"method":"pap","user":"synthetic"}}"#,
        r#"{"authentication":{},"authentication":{}}"#,
        r#"{"pcscf_via_pco":"auto"}"#,
        r#"{"pcscf_via_dhcp":"required"}"#,
    ] {
        assert!(serde_json::from_str::<ImsBearerRequirements>(json).is_err());
    }
}
