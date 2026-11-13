//! Actions-only native client adapter policies, not service or device evidence.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{session::HostingSession, tests::imported};
#[path = "native_receiver_protocol_tests.rs"]
mod protocol;
fn owner() -> AttemptOwner { let mut s = HostingSession::new("fixture-native-receiver".into()); s.import(imported(), 100).unwrap(); s.begin_login(100).unwrap() }
fn default_policy() -> Arc<NativeReceiverPolicy> { NativeReceiverPolicy::capture(&NativeReceiverConfig { direct_domains: None }).unwrap() }
fn policy(mode: DomainMode) -> Arc<NativeReceiverPolicy> { NativeReceiverPolicy::capture(&NativeReceiverConfig { direct_domains: Some(DirectDomainConfig { mode, values: vec![b"listed".to_vec()] }) }).unwrap() }
fn metadata(object: ReceiverObject, domain: Option<&[u8]>) -> Vec<RpcProperty<'_>> {
    let mut fields = vec![RpcProperty { key: b"id", value: object.identity() }];
    if let Some(value) = domain { fields.push(RpcProperty { key: b"domain", value }); }
    fields
}
#[test]
fn native_default_direct_domain_policy_is_empty_deny_list_and_not_an_invented_outbound_domain() {
    let p = default_policy();
    for object in [ReceiverObject::Message, ReceiverObject::Call] { for domain in [None, Some(b"".as_slice()), Some(b"arbitrary-original-peer-domain")] {
        assert_eq!(p.validate(object, &metadata(object, domain), ReceiverTransport::Direct), Ok(()));
    } }
    assert_eq!(p.retained_bytes(), 0);
}
#[test]
fn explicit_direct_allow_and_deny_lists_match_native_missing_empty_and_membership_semantics() {
    for (mode, values) in [(DomainMode::DenyList, [true, true, false, true]), (DomainMode::AllowList, [false, false, true, false])] {
        let p = policy(mode);
        for (domain, allowed) in [None, Some(b"".as_slice()), Some(b"listed"), Some(b"other")].into_iter().zip(values) {
            assert_eq!(p.validate(ReceiverObject::Message, &metadata(ReceiverObject::Message, domain), ReceiverTransport::Direct).is_ok(), allowed);
        }
    }
}
#[test]
fn authenticated_batch_adapter_does_not_reuse_the_direct_domain_filter() {
    for mode in [DomainMode::DenyList, DomainMode::AllowList] { let p = policy(mode);
        for domain in [None, Some(b"".as_slice()), Some(b"listed"), Some(b"other")] {
            for object in [ReceiverObject::Message, ReceiverObject::Call] {
                assert_eq!(p.validate(object, &metadata(object, domain), ReceiverTransport::Batch), Ok(()));
            }
        }
    }
}
#[test]
fn native_dispatch_requires_exact_unqualified_objects_not_aliases_facets_adapters_or_clouds() {
    let p = default_policy();
    for value in [b"".as_slice(), b"#MessageReceiver", b"messageReceiver", b"MessageReceiver@foreign", b"MessageReceiver@x$cloud",
        b"facet/MessageReceiver", b"MessageReceiver:sarc -h 42.7 -p 98;", b"MessageReceiver\0", b"CallClientEx", b"*"] {
        for transport in [ReceiverTransport::Direct, ReceiverTransport::Batch] {
            assert!(p.validate(ReceiverObject::Message, &[RpcProperty { key: b"id", value }], transport).is_err());
        }
    }
    assert!(p.validate(ReceiverObject::Call, &metadata(ReceiverObject::Message, None), ReceiverTransport::Batch).is_err());
}
#[test]
fn native_policy_never_accepts_duplicate_oversize_or_unbounded_metadata_before_dispatch() {
    let p = default_policy(); let mut duplicate = metadata(ReceiverObject::Message, None); duplicate.push(duplicate[0]);
    assert_eq!(p.validate(ReceiverObject::Message, &duplicate, ReceiverTransport::Batch), Err(CodecError::DuplicateKey));
    let large = vec![1; MAX_FIELD_BYTES + 1];
    assert_eq!(p.validate(ReceiverObject::Message, &[RpcProperty { key: b"id", value: &large }], ReceiverTransport::Direct), Err(CodecError::TooLarge));
    assert_eq!(p.validate(ReceiverObject::Message, &vec![RpcProperty { key: b"a", value: b"b" }; MAX_MAP_ENTRIES + 1], ReceiverTransport::Batch), Err(CodecError::TooLarge));
}
#[test]
fn native_domain_configuration_rejects_duplicate_empty_control_and_unresolved_values() {
    for values in [vec![b"".to_vec()], vec![b"a\0b".to_vec()], vec![b"ENC(unresolved)".to_vec()], vec![b"listed".to_vec(), b"listed".to_vec()], vec![vec![1; MAX_FIELD_BYTES + 1]]] {
        let config = NativeReceiverConfig { direct_domains: Some(DirectDomainConfig { mode: DomainMode::AllowList, values }) };
        assert!(NativeReceiverPolicy::capture(&config).is_err());
    }
    assert!(serde_json::from_str::<NativeReceiverConfig>(r#"{"unsafe_accept_all":true}"#).is_err());
    assert!(serde_json::from_str::<NativeReceiverConfig>(r#"{"direct_domains":{"mode":"unknown","values":[]}}"#).is_err());
    assert!(serde_json::from_str::<NativeReceiverConfig>(r#"true"#).is_err());
}
#[test]
fn empty_allow_list_denies_direct_without_changing_authenticated_batch_object_checks() {
    let p = NativeReceiverPolicy::capture(&NativeReceiverConfig { direct_domains: Some(DirectDomainConfig { mode: DomainMode::AllowList, values: vec![] }) }).unwrap();
    let fields = metadata(ReceiverObject::Message, None);
    assert!(p.validate(ReceiverObject::Message, &fields, ReceiverTransport::Direct).is_err());
    assert!(p.validate(ReceiverObject::Message, &fields, ReceiverTransport::Batch).is_ok());
}
#[test]
fn original_native_receiver_binding_is_retired_with_its_owner_scope_and_does_not_expose_policy_values() {
    let original = owner(); let scope = RpcConnectionScope::fresh(); let policy = policy(DomainMode::DenyList);
    let binding = NativeReceiverBinding::original(original.clone(), scope.clone(), policy.clone()).unwrap();
    assert!(binding.matches(&original, &scope)); assert!(!binding.matches(&owner(), &scope));
    assert!(!binding.matches(&original, &RpcConnectionScope::fresh()));
    assert!(!format!("{binding:?} {policy:?}").contains("listed")); scope.close(); assert!(!binding.matches(&original, &scope));
    assert!(NativeReceiverBinding::original(original, scope, policy).is_err());
}
#[test]
fn native_receiver_option_roundtrips_without_fake_receiver_domain_or_legacy_changes() {
    use crate::connectivity::app_voice::providers::jegotrip::{account_config::{self, AccountConfig}, tls_engine::tests::Certificates};
    let cert = Certificates::generate("DNS:fixture.invalid"); let mut config = account_config::tests::native_fixture(&cert.root, "fixture.invalid", 443);
    assert!(config.profile.native_receivers.is_none()); let old = config.encode_private().unwrap(); assert!(!std::str::from_utf8(&old).unwrap().contains("native_receivers"));
    config.profile.incoming = None; config.profile.native_receivers = Some(NativeReceiverConfig { direct_domains: None });
    config.profile.client_ex = true; config.profile.offline_reception = true;
    assert!(config.validate(100).is_ok()); let encoded = config.encode_private().unwrap(); let restored = AccountConfig::decode_private(&encoded).unwrap();
    assert!(restored.profile.incoming.is_none()); assert!(restored.profile.native_receivers.is_some()); assert!(restored.runtime_profile().is_ok());
    assert_eq!(encoded.as_slice(), restored.encode_private().unwrap().as_slice());
    assert!(!std::str::from_utf8(&encoded).unwrap().contains("MessageReceiver"), "known local namespace is runtime code, not a fabricated imported value");
}
#[test]
fn native_option_cannot_replace_a_legacy_receiver_or_run_on_the_wrong_transport() {
    use crate::connectivity::app_voice::providers::jegotrip::{account_config::{self, ProfileRoute}, tls_engine::tests::Certificates};
    let cert = Certificates::generate("DNS:fixture.invalid"); let mut config = account_config::tests::native_fixture(&cert.root, "fixture.invalid", 443);
    config.profile.native_receivers = Some(NativeReceiverConfig { direct_domains: None });
    assert!(config.runtime_profile().is_err(), "legacy explicit incoming cannot be silently replaced");
    config.profile.incoming = None; assert!(config.runtime_profile().is_ok());
    config.profile.route = ProfileRoute::DirectTlsCarrier; assert!(config.runtime_profile().is_err());
}
