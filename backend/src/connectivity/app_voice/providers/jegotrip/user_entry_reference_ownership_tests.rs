//! Actions-only source/destination handoff of immutable original reference DATA.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
fn router()->n::router_connector::RouterPolicy{let cert=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");n::netstream_runtime::tests::policy(&cert.root,12345)}
#[test]
fn matched_bootstrap_keeps_decoded_factory_input_separate_from_legacy_canonical_encoding(){
    let raw=b"\t prefix/UserEntry:arc -p 00098 -h 42.7 \r\n";let(rpc,who,source,reply)=auxiliary(raw);
    let reference=UserEntryReference::from_bootstrap(&reply,&who,&source,None).unwrap().unwrap();
    assert_eq!(reference.reference_for(&who,&source).unwrap().original_factory_input(),raw);
    assert_eq!(reference.reference_for(&who,&source).unwrap().encode_native().unwrap().expose_to_backend(),b"prefix/UserEntry:arc -h 42.7 -p 98;");
    assert_eq!(rpc.phase(),ArcPhase::Initial);assert!(source.is_open());
}
#[test]
fn configuration_decryption_retains_plain_factory_input_not_envelope_or_other_key_context(){
    let raw=b" prefix/UserEntry@adapter$locator ";let encrypted=fixture_envelope(raw,b"fixture-reference-key",[8;8]);let(_rpc,who,source,reply)=auxiliary(&encrypted);
    let key=ConfigDecodeKey::supplied(b"fixture-reference-key").unwrap();let r=UserEntryReference::from_bootstrap(&reply,&who,&source,Some(&key)).unwrap().unwrap();
    assert_eq!(r.reference_for(&who,&source).unwrap().original_factory_input(),raw);assert_ne!(r.reference_for(&who,&source).unwrap().original_factory_input(),encrypted);
    assert!(UserEntryReference::from_bootstrap(&reply,&who,&source,None).is_err());assert!(!format!("{r:?}").contains("adapter"));
}
#[test]
fn exact_reference_transfer_checks_source_before_move_and_cannot_rebind_foreign_or_closed_scope(){
    for foreign in [true,false]{let(mut rpc,who,source,reply)=auxiliary(b"UserEntry@adapter$locator");let r=UserEntryReference::from_bootstrap(&reply,&who,&source,None).unwrap().unwrap();
        if foreign{assert!(r.into_reference(&owner(),&source).is_err());}else{rpc.close();assert!(r.into_reference(&who,&source).is_err());}
    }
    let(_rpc,who,source,reply)=auxiliary(b"UserEntry");let r=UserEntryReference::from_bootstrap(&reply,&who,&source,None).unwrap().unwrap();source.mark_session_revoked();assert!(r.into_reference(&who,&source).is_err());
}
#[test]
fn qualified_destination_snapshot_survives_source_join_with_exact_input_and_no_new_authority(){
    let raw=b" prefix/UserEntry:sarc -p 00098 -h 42.7 ";let(mut rpc,who,source,reply)=auxiliary(raw);let parent=RpcConnectionScope::fresh();let p=router();
    let target=n::user_entry_target::UserEntryTarget::from_bootstrap(&reply,&who,&source,&parent,&p,None,b"fixture.invalid",b"cc").unwrap().unwrap();rpc.close();
    let r=target.reference_for(&who,&parent,&p).unwrap();assert_eq!(r.original_factory_input(),raw);assert_eq!(r.identity(),b"prefix/UserEntry");
    assert_eq!(target.retained_bytes(),r.retained_bytes()+b"fixture.invalid".len()+2);assert_eq!(target.original_bootstrap_id(),reply.call_id());
    assert!(target.reference_for(&who,&parent,&router()).is_none());assert!(target.reference_for(&owner(),&parent,&p).is_none());parent.close();assert!(target.reference_for(&who,&parent,&p).is_none());
}
#[test]
fn exact_snapshot_retention_does_not_enable_identity_adapter_locator_or_wrong_transport_queries(){
    let p=router();for raw in [b"UserEntry".as_slice(),b" prefix/UserEntry@adapter$locator ",b"UserEntry:tls -p 00443 -h fixture.invalid"]{
        let(rpc,who,source,reply)=auxiliary(raw);let parent=RpcConnectionScope::fresh();let r=UserEntryReference::from_bootstrap(&reply,&who,&source,None).unwrap().unwrap();
        assert_eq!(r.reference_for(&who,&source).unwrap().original_factory_input(),raw);
        assert!(n::user_entry_target::UserEntryTarget::from_bootstrap(&reply,&who,&source,&parent,&p,None,b"fixture.invalid",b"").is_err());
        assert_eq!(rpc.phase(),ArcPhase::Initial);assert!(source.is_open());assert!(parent.is_open());
    }
}
