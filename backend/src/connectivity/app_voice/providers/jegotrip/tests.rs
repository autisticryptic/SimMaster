use super::{credentials::*, session::*};

pub(super) fn imported() -> ImportedCredentials {
    serde_json::from_value(serde_json::json!({
        "sdk": {
            "account_identity": "fixture-account",
            "phone_login_token": "fixture-session-token",
            "application_key": "fixture-sdk-application",
            "server_address": "fixture-server.invalid",
            "device_id": "fixture-device",
            "root_der_base64": "fixture-trust",
            "tls_only": true,
            "expires_at_unix_seconds": 200
        },
        "hosting": {
            "sip_number": "fixture-hosted-number",
            "sip_password": "fixture-gateway-password",
            "authname_prefix": "fixture-prefix",
            "channel_id": "fixture-channel",
            "delete_int_code": "fixture-policy",
            "enrollment_confirmed": true
        }
    })).unwrap()
}

fn registered(session: &mut HostingSession) -> AttemptOwner {
    session.import(imported(), 100).unwrap();
    let owner = session.begin_login(100).unwrap();
    assert!(session.on_sdk_login(&owner, true));
    session.begin_hosting(&owner).unwrap();
    assert!(session.on_hosting_ack(&owner, true));
    owner
}

#[test]
fn credential_import_needs_no_account_password_and_grants_no_readiness() {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    assert_eq!(session.phase(), Phase::CredentialsImported);
    assert!(!session.incoming_call_ready());
    assert!(!session.incoming_message_ready());
    assert!(!session.outbound_authorized());
}

#[test]
fn sdk_login_uses_phone_token_not_the_gateway_password() {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    let owner = session.begin_login(100).unwrap();
    let login = session.login_request(&owner).unwrap();
    assert_eq!(login.phone_login_token, "fixture-session-token");
    assert_eq!(login.selected_session_id, None);
    assert_ne!(login.phone_login_token, "fixture-gateway-password");
    assert!(session.begin_hosting(&owner).is_err());
}

#[test]
fn selected_sdk_session_id_is_optional_preserved_and_never_replaced_by_an_auth_token() {
    let mut credentials = imported();
    credentials.sdk.selected_session_id = Some(serde_json::from_str("\"fixture-selected-session-id\"").unwrap());
    let mut session = HostingSession::new("line-a".into());
    session.import(credentials.clone(), 100).unwrap();
    let owner = session.begin_login(100).unwrap();
    let login = session.login_request(&owner).unwrap();
    assert_eq!(login.selected_session_id, Some("fixture-selected-session-id"));
    assert_ne!(login.selected_session_id, Some(login.phone_login_token));
    assert!(!format!("{credentials:?}").contains("fixture-selected-session-id"));
    credentials.sdk.selected_session_id = Some(serde_json::from_str("\"\"").unwrap());
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_session_identity_invalid"));
    credentials.sdk.selected_session_id = Some(serde_json::from_value(serde_json::json!("s".repeat(128))).unwrap());
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_session_identity_invalid"));
}

#[test]
fn hosting_parameters_preserve_the_five_exact_sdk_keys_and_types() {
    let mut session = HostingSession::new("line-a".into());
    session.import(imported(), 100).unwrap();
    let owner = session.begin_login(100).unwrap();
    session.on_sdk_login(&owner, true);
    let request = session.begin_hosting(&owner).unwrap();
    let json = serde_json::to_value(request).unwrap();
    let obj = json.as_object().unwrap();
    assert_eq!(obj.len(), 5);
    for key in ["AuthnamePrefix", "ChannelId", "DeleteIntCode", "SipNumber", "SipPwd"] {
        assert!(obj[key].is_string());
    }
    assert_eq!(obj["SipPwd"], "fixture-gateway-password");
    assert_ne!(obj["SipPwd"], "fixture-session-token");
    assert_eq!((STATUS_REGISTER, STATUS_UNREGISTER), (4, 3));
}

#[test]
fn login_and_registration_ack_are_not_delivery_readiness() {
    let mut session = HostingSession::new("line-a".into());
    let owner = registered(&mut session);
    assert!(!session.incoming_call_ready());
    assert!(!session.incoming_message_ready());
    session.on_channels(&owner, true, false);
    assert!(session.incoming_call_ready());
    assert!(!session.incoming_message_ready());
    session.on_channels(&owner, false, true);
    assert!(!session.incoming_call_ready());
    assert!(session.incoming_message_ready());
}

#[test]
fn revoked_session_cannot_be_revived_by_a_late_success() {
    let mut session = HostingSession::new("line-a".into());
    let owner = registered(&mut session);
    session.on_channels(&owner, true, true);
    assert!(session.on_failure(&owner, BackendFailure::Expired));
    assert!(!session.on_hosting_ack(&owner, true));
    assert!(!session.on_channels(&owner, true, true));
    assert_eq!(session.phase(), Phase::CredentialExpired);
    assert!(!session.incoming_message_ready());
}

#[test]
fn replacing_credentials_invalidates_all_previous_callbacks() {
    let mut session = HostingSession::new("line-a".into());
    let old = registered(&mut session);
    session.import(imported(), 100).unwrap();
    assert!(!session.on_channels(&old, true, true));
    assert!(!session.on_failure(&old, BackendFailure::Disconnected));
    assert_eq!(session.phase(), Phase::CredentialsImported);
}

#[test]
fn another_line_cannot_reuse_a_callback_owner() {
    let mut a = HostingSession::new("line-a".into());
    let owner = registered(&mut a);
    let mut b = HostingSession::new("line-b".into());
    registered(&mut b);
    assert!(!b.on_channels(&owner, true, true));
    assert!(!b.on_failure(&owner, BackendFailure::Expired));
}

#[test]
fn clearing_a_line_revokes_channels_and_pending_callbacks() {
    let mut session = HostingSession::new("line-a".into());
    let owner = registered(&mut session);
    session.on_channels(&owner, true, true);
    session.clear().unwrap();
    assert!(!session.on_channels(&owner, true, true));
    assert_eq!(session.phase(), Phase::Unconfigured);
    assert!(session.begin_login(100).is_err());
}

#[test]
fn expired_imports_and_missing_enrollment_are_rejected() {
    let mut credentials = imported();
    assert_eq!(credentials.validate(200).err(), Some("jegotrip_credential_expired"));
    credentials.hosting.enrollment_confirmed = false;
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_hosting_enrollment_required"));
}

#[test]
fn weak_tls_empty_token_and_control_characters_fail_closed() {
    let mut credentials = imported();
    credentials.sdk.tls_only = false;
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_tls_required"));
    credentials.sdk.tls_only = true;
    credentials.sdk.phone_login_token = serde_json::from_str("\"\"").unwrap();
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_credential_invalid"));
    credentials.sdk.phone_login_token = serde_json::from_str("\"token\\n\"").unwrap();
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_credential_invalid"));
}

#[test]
fn expiry_revokes_a_live_hosted_session_and_unknown_expiry_is_not_invented() {
    let mut session = HostingSession::new("line-a".into());
    let owner = registered(&mut session);
    session.on_channels(&owner, true, true);
    session.check_expiry(200);
    assert_eq!(session.phase(), Phase::CredentialExpired);
    assert!(!session.incoming_call_ready());
    let mut credentials = imported();
    credentials.sdk.expires_at_unix_seconds = None;
    assert!(credentials.validate(u64::MAX).is_ok());
}

#[test]
fn debug_and_contract_never_expose_imported_credentials() {
    let credentials = imported();
    let debug = format!("{credentials:?}");
    for secret in ["fixture-account", "fixture-session-token", "fixture-gateway-password",
        "fixture-sdk-application", "fixture-server.invalid", "fixture-hosted-number"] {
        assert!(!debug.contains(secret));
    }
    let public = serde_json::to_string(&contract()).unwrap();
    assert!(!public.contains("fixture-"));
    assert!(public.contains("\"transport_implemented\":false"));
    assert!(public.contains("\"account_login_ui_required\":false"));
    assert!(public.contains("\"service_credentials_required\":true"));
}

#[test]
fn unverified_outbound_traffic_is_never_implicitly_authorized() {
    let mut session = HostingSession::new("line-a".into());
    let owner = registered(&mut session);
    session.on_channels(&owner, true, true);
    assert!(!session.outbound_authorized());
}

#[test]
fn expiry_is_idempotent_and_retires_the_original_owner_only_once() {
    let mut session = HostingSession::new("line-a".into());
    let owner = registered(&mut session);
    session.on_channels(&owner, true, true);
    session.check_expiry(200);
    let expired_owner = session.current_owner();
    assert_ne!(expired_owner, owner);
    session.check_expiry(201);
    session.check_expiry(u64::MAX);
    assert_eq!(session.current_owner(), expired_owner);
    assert_eq!(session.phase(), Phase::CredentialExpired);
    assert!(!session.on_failure(&expired_owner, BackendFailure::Disconnected));
    assert!(!session.on_channels(&owner, true, true));
}

#[test]
fn a_notification_cannot_fail_an_imported_but_not_started_session() {
    let mut session = HostingSession::new("line-a".into());
    let owner = session.import(imported(), 100).unwrap();
    assert!(!session.on_failure(&owner, BackendFailure::Disconnected));
    assert_eq!(session.phase(), Phase::CredentialsImported);
    assert_eq!(session.current_owner(), owner);
}

#[test]
fn owner_debug_redacts_line_identity_and_recreated_sessions_are_distinct() {
    let mut first = HostingSession::new("fixture-private-line".into());
    let mut second = HostingSession::new("fixture-private-line".into());
    let a = first.import(imported(), 100).unwrap();
    let b = second.import(imported(), 100).unwrap();
    assert_ne!(a, b);
    assert!(!format!("{a:?}").contains("fixture-private-line"));
}

#[test]
fn android_cached_password_field_cannot_silently_replace_the_decoded_password() {
    let mut value = serde_json::to_value(serde_json::json!({
        "sipPassword": "ciphertext", "sip_number": "fixture-number",
        "authname_prefix": "fixture-prefix", "channel_id": "fixture-channel",
        "delete_int_code": "fixture-policy", "enrollment_confirmed": true
    })).unwrap();
    assert!(serde_json::from_value::<HostingCredentials>(value.clone()).is_err());
    value.as_object_mut().unwrap().remove("sipPassword");
    value["sip_password"] = serde_json::json!("decoded");
    assert!(serde_json::from_value::<HostingCredentials>(value).is_ok());
}
