//! Actions-only relation policy installation/compatibility; no remote WRITE.
use crate::connectivity::app_voice::providers::jegotrip::{self as n,relation_update_policy::NativeRelationUpdateConfig as Policy};
#[test]
fn relation_policy_default_none_is_legacy_canonical_configuration_without_extra_property(){
    let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let config=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",443);assert!(config.profile.relation_updates.is_none());let bytes=config.encode_private().unwrap();let json:serde_json::Value=serde_json::from_slice(&bytes).unwrap();assert!(json["config"]["profile"].get("relation_updates").is_none());let copy=n::account_config::AccountConfig::decode_private(&bytes).unwrap();assert!(copy.profile.relation_updates.is_none());assert_eq!(copy.encode_private().unwrap(),bytes);
}
#[test]
fn relation_policy_is_separate_from_directory_and_im_send_permissions(){
    let policy=Policy{additions:true,updates:false,removals:false};assert!(policy.validate().is_ok());assert!(policy.admits(1,0,0));assert!(!policy.admits(0,1,0));assert!(!policy.admits(0,0,1));assert!(!policy.admits(1,1,0));assert!(!policy.admits(0,0,0));assert!(Policy{additions:false,updates:false,removals:false}.validate().is_err());
}
#[test]
fn relation_policy_unknown_uid_token_scope_or_endpoint_fields_are_rejected(){
    for field in ["uid","token","scope","endpoint","nonce","ready"]{let mut value=serde_json::json!({"additions":true,"updates":false,"removals":false});value[field]=serde_json::json!("private-value");assert!(serde_json::from_value::<Policy>(value).is_err());}assert!(serde_json::from_value::<Policy>(serde_json::json!({"additions":"true","updates":false,"removals":false})).is_err());
}
#[test]
fn relation_policy_requires_the_real_installed_native_client_receiver_not_old_direct_carrier(){
    let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let mut cfg=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",443);cfg.profile.relation_updates=Some(Policy{additions:true,updates:true,removals:true});assert!(cfg.runtime_profile().is_err());cfg.profile.client_ex=true;cfg.profile.incoming=None;cfg.profile.native_receivers=Some(n::native_receiver::NativeReceiverConfig{direct_domains:None,invite_notifications:None,im_messages:Some(n::im_delivery::NativeImConfig{notification_timeout_ms:3000,receive_info:false,directory_queries:false})});assert!(cfg.runtime_profile().is_ok());assert!(cfg.profile.im_send.is_none());assert!(!cfg.profile.native_receivers.as_ref().unwrap().im_messages.as_ref().unwrap().directory_queries);
}
#[test]
fn relation_policy_roundtrip_preserves_explicit_rights_without_changing_native_client_properties(){
    let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let mut cfg=n::account_config::tests::native_fixture(&cert.root,"127.0.0.1",443);cfg.profile.relation_updates=Some(Policy{additions:false,updates:true,removals:false});cfg.profile.client_ex=true;cfg.profile.incoming=None;cfg.profile.native_receivers=Some(n::native_receiver::NativeReceiverConfig{direct_domains:None,invite_notifications:None,im_messages:Some(n::im_delivery::NativeImConfig{notification_timeout_ms:3000,receive_info:false,directory_queries:false})});let before=serde_json::to_value(n::account_config::private_profile_ref(&cfg.profile)).unwrap()["client_properties"].clone();let bytes=cfg.encode_private().unwrap();let copy=n::account_config::AccountConfig::decode_private(&bytes).unwrap();let policy=copy.profile.relation_updates.unwrap();assert!(!policy.additions&&policy.updates&&!policy.removals);assert_eq!(copy.encode_private().unwrap(),bytes);assert_eq!(serde_json::to_value(n::account_config::private_profile_ref(&copy.profile)).unwrap()["client_properties"],before);
}
#[test]
fn relation_policy_debug_does_not_describe_an_authentication_or_business_write_grant(){
    let policy=Policy{additions:true,updates:true,removals:true};assert_eq!(format!("{policy:?}"),"NativeRelationUpdateConfig(<redacted>)");
}
