use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    arc_session::{ArcReply, ArcRpcSession, ArcPhase}, rpc_envelope::encode_reply,
    rpc_stream::{RpcProperty, Writer}, session::HostingSession, tests::imported,
    legacy_config::tests::fixture_envelope,
};

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("fixture-deployment-line".into()); session.import(imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn setup(reference: &[u8], properties: &[RpcProperty<'_>]) -> (ArcRpcSession, AttemptOwner, RpcConnectionScope, OwnedBootstrapReply) {
    let original = owner(); let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_bootstrap(b"fixture-appkey", &[]).unwrap();
    let mut result = Writer::new(); result.boolean(true).unwrap(); result.properties(properties).unwrap();
    result.bytes(reference).unwrap(); result.bytes(b"fixture-account-alive").unwrap(); result.bytes(b"fixture-user-entry").unwrap(); result.bytes(b"").unwrap(); let result = result.finish();
    let reply = encode_reply(id, 0, result.expose_to_transport()).unwrap();
    let configuration = match rpc.on_reply(&original, &scope, reply.expose_to_transport()).unwrap().unwrap() {
        ArcReply::Bootstrap(configuration) => configuration, _ => panic!("matched bootstrap required"),
    };
    (rpc, original, scope, configuration)
}

#[test]
fn matched_bootstrap_retains_original_scope_and_native_configuration_without_authentication() {
    let (rpc, original, scope, result) = setup(b"#fixture-entry:tcp -h fixture.invalid -p 443;", &[RpcProperty { key: b"DomainName", value: b"fixture.invalid" }]);
    let plan = BootstrapDeployment::from_reply(&result, &original, &scope, None).unwrap();
    assert_eq!(plan.call_id(), result.call_id()); assert_eq!(plan.account_reference_for(&original, &scope).unwrap().identity(), b"#fixture-entry");
    let candidates = plan.raw_tcp_candidates_for(&original, &scope).unwrap(); assert_eq!(candidates[0].host(), b"fixture.invalid"); assert_eq!(candidates[0].port(), 443);
    assert_eq!(plan.property_for(&original, &scope, b"DomainName", None).unwrap().unwrap().expose_to_backend(), b"fixture.invalid");
    assert!(plan.property_for(&original, &scope, b"unknown", None).unwrap().is_none());
    assert_eq!(plan.auxiliary_references_for(&original, &scope), Some((b"fixture-account-alive".as_slice(), b"fixture-user-entry".as_slice())));
    assert_eq!(rpc.phase(), ArcPhase::Initial);
}

#[test]
fn original_owner_and_scope_are_checked_before_decrypting_or_parsing_wrong_data() {
    let (_rpc, original, scope, result) = setup(b"ENC(unclosed", &[]);
    assert!(result.material_for(&owner(), &scope).is_none());
    assert!(result.material_for(&original, &RpcConnectionScope::fresh()).is_none());
    assert_eq!(BootstrapDeployment::from_reply(&result, &owner(), &scope, None).err(), Some(DeploymentError::Scope));
    assert_eq!(BootstrapDeployment::from_reply(&result, &original, &RpcConnectionScope::fresh(), None).err(), Some(DeploymentError::Scope));
    assert_eq!(BootstrapDeployment::from_reply(&result, &original, &scope, None).err(), Some(DeploymentError::Configuration(ConfigError::MalformedEnvelope)));
}

#[test]
fn client_property_and_common_reference_keys_are_independent_without_password_substitution() {
    let reference_password = b"fixture-reference-key"; let client_password = b"fixture-client-key";
    let reference = fixture_envelope(b"#fixture-entry:tcp -h fixture.invalid -p 443;", reference_password, [1; 8]);
    let property = fixture_envelope(b"fixture.invalid", client_password, [2; 8]);
    let (_rpc, original, scope, result) = setup(&reference, &[RpcProperty { key: b"DomainName", value: &property }]);
    assert_eq!(BootstrapDeployment::from_reply(&result, &original, &scope, None).err(), Some(DeploymentError::Configuration(ConfigError::KeyRequired)));
    let reference_key = ConfigDecodeKey::supplied(reference_password).unwrap(); let client_key = ConfigDecodeKey::supplied(client_password).unwrap();
    let plan = BootstrapDeployment::from_reply(&result, &original, &scope, Some(&reference_key)).unwrap();
    assert_eq!(plan.property_for(&original, &scope, b"DomainName", None).err(), Some(DeploymentError::Configuration(ConfigError::KeyRequired)));
    assert_eq!(plan.property_for(&original, &scope, b"DomainName", Some(&client_key)).unwrap().unwrap().expose_to_backend(), b"fixture.invalid");
    if let Ok(wrong) = BootstrapDeployment::from_reply(&result, &original, &scope, Some(&client_key)) {
        assert_ne!(wrong.account_reference_for(&original, &scope).unwrap().identity(), b"#fixture-entry");
    }
    if let Ok(Some(value)) = plan.property_for(&original, &scope, b"DomainName", Some(&reference_key)) {
        assert_ne!(value.expose_to_backend(), b"fixture.invalid");
    }
}

#[test]
fn incomplete_adapter_or_different_transport_is_not_guessed_as_raw_tcp_target() {
    for reference in [b"#fixture".as_slice(), b"#fixture@adapter$locator", b"#fixture:tls -h fixture.invalid -p 443;"] {
        let (_rpc, original, scope, result) = setup(reference, &[]);
        let plan = BootstrapDeployment::from_reply(&result, &original, &scope, None).unwrap();
        assert!(plan.raw_tcp_candidates_for(&original, &scope).is_err());
    }
}

#[test]
fn close_drop_or_another_connection_invalidates_plan_access_not_borrowed_byte_snapshots() {
    let (mut rpc, original, scope, result) = setup(b"#fixture:tcp -h fixture.invalid -p 443;", &[]);
    let plan = BootstrapDeployment::from_reply(&result, &original, &scope, None).unwrap();
    rpc.close(); assert!(result.material_for(&original, &scope).is_none()); assert!(plan.account_reference_for(&original, &scope).is_none());
    assert_eq!(plan.raw_tcp_candidates_for(&original, &scope).err(), Some(DeploymentError::Scope));
    let (rpc, original, scope, result) = setup(b"#fixture", &[]); drop(rpc);
    assert_eq!(BootstrapDeployment::from_reply(&result, &original, &scope, None).err(), Some(DeploymentError::Scope));
}

#[test]
fn failed_and_unmatched_bootstrap_never_produces_owned_configuration() {
    let original = owner(); let mut rpc = ArcRpcSession::new(original.clone()); let scope = rpc.connection_scope();
    let (id, _) = rpc.prepare_bootstrap(b"fixture-appkey", &[]).unwrap();
    let mut result = Writer::new(); result.boolean(false).unwrap(); result.properties(&[]).unwrap();
    for value in [b"#fixture".as_slice(), b"", b"", b""] { result.bytes(value).unwrap(); } let result = result.finish();
    let unknown = encode_reply(id + 1, 0, result.expose_to_transport()).unwrap(); assert!(rpc.on_reply(&original, &scope, unknown.expose_to_transport()).unwrap().is_none());
    let rejected = encode_reply(id, 0, result.expose_to_transport()).unwrap(); assert!(matches!(rpc.on_reply(&original, &scope, rejected.expose_to_transport()).unwrap(), Some(ArcReply::Failure(_))));
    assert_eq!(rpc.phase(), ArcPhase::Closed);
}

#[test]
fn bootstrap_and_plan_debug_redact_values_and_auxiliary_references_do_not_grant_channels() {
    let (rpc, original, scope, result) = setup(b"#fixture-account:tcp -h fixture.invalid -p 443;", &[RpcProperty { key: b"private", value: b"fixture-secret" }]);
    let plan = BootstrapDeployment::from_reply(&result, &original, &scope, None).unwrap();
    assert!(!format!("{result:?}").contains("fixture")); assert!(!format!("{plan:?}").contains("fixture"));
    assert_eq!(rpc.phase(), ArcPhase::Initial);
}

#[test]
fn new_decode_key_imports_are_optional_redacted_bounded_and_not_phone_or_gateway_credentials() {
    let mut credentials = imported(); assert!(credentials.sdk.client_config_decode_key.is_none()); assert!(credentials.sdk.reference_decode_key.is_none());
    credentials.sdk.client_config_decode_key = Some(serde_json::from_str("\"fixture-client-key\"").unwrap());
    credentials.sdk.reference_decode_key = Some(serde_json::from_str("\"fixture-reference-key\"").unwrap());
    assert!(credentials.validate(100).is_ok()); assert!(!format!("{credentials:?}").contains("fixture-reference-key"));
    credentials.sdk.reference_decode_key = Some(serde_json::from_str("\"\"").unwrap()); assert_eq!(credentials.validate(100).err(), Some("jegotrip_config_decode_key_invalid"));
    credentials.sdk.reference_decode_key = Some(serde_json::from_value(serde_json::json!("x".repeat(4097))).unwrap());
    assert_eq!(credentials.validate(100).err(), Some("jegotrip_config_decode_key_invalid"));
}
