//! Actions-only original bootstrap codec/data and separate TLS-anchor tests.
use super::*;
use super::super::{arc_session::{ArcReply,ArcRpcSession},rpc_envelope::encode_reply,rpc_stream::{RpcProperty,Writer},
    session::HostingSession,tests::imported,tls_engine::tests::Certificates};
fn owner()->AttemptOwner{let mut session=HostingSession::new("fixture-original".into());session.import(imported(),100).unwrap();session.begin_login(100).unwrap()}
fn trust(root:&str)->AccountTrust{AccountTrust::supplied(AccountTrustInputs{authority:"fixture.invalid",root_der_base64:root,enable_sni:true}).unwrap()}
struct Source { scope:RpcConnectionScope, _rpc:ArcRpcSession }
impl std::ops::Deref for Source { type Target=RpcConnectionScope;fn deref(&self)->&Self::Target{&self.scope} }
fn owned(original:&AttemptOwner,properties:&[RpcProperty<'_>],reference:&[u8])->(OwnedBootstrapReply,Source){
    let mut rpc=ArcRpcSession::new(original.clone());let source=rpc.connection_scope();let(id,_)=rpc.prepare_bootstrap(b"fixture-appkey",&[]).unwrap();
    let mut result=Writer::new();result.boolean(true).unwrap();result.properties(properties).unwrap();result.bytes(reference).unwrap();result.bytes(b"optional-account-alive").unwrap();result.bytes(b"optional-user-entry").unwrap();result.bytes(b"").unwrap();
    let reply=encode_reply(id,0,result.finish().expose_to_transport()).unwrap();
    let Some(ArcReply::Bootstrap(proof))=rpc.on_reply(original,&source,reply.expose_to_transport()).unwrap()else{panic!("matched bootstrap required")};
    // Keep the original protocol owner in the fixture; Drop closes its scope.
    (proof,Source{scope:source,_rpc:rpc})
}
fn properties()->[RpcProperty<'static>;4]{[
    RpcProperty{key:b"DomainName",value:b"account-domain.invalid"},RpcProperty{key:b"DomainId",value:b"fixture-domain"},
    RpcProperty{key:b"AppId",value:b"fixture-app"},RpcProperty{key:b"CC",value:b"fixture-cc"},
]}
const REFERENCE:&[u8]=b"#fixture-account:tcp -h fixture.invalid -p 443;";
#[test]
fn matched_bootstrap_fields_use_original_destination_without_domain_name_host_inference(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let anchor=trust(&cert.root);let(reply,source)=owned(&original,&properties(),REFERENCE);let destination=RpcConnectionScope::fresh();
    let plan=ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,None,None).unwrap();
    assert_eq!(plan.bootstrap_call_id(),reply.call_id());assert!(plan.original_source_scope().matches(&source));
    let fields=plan.fields_for(&original,&destination).unwrap();assert_eq!(fields.domain_name,b"account-domain.invalid");assert_eq!(fields.domain_id,b"fixture-domain");assert_eq!(fields.application_id,b"fixture-app");assert_eq!(fields.device_info_cc,b"fixture-cc");
    let connection=plan.connection_for(&original,&destination,&anchor).unwrap();assert_eq!(connection.authority,"fixture.invalid");assert_eq!(connection.server_reference,std::str::from_utf8(REFERENCE).unwrap());assert_eq!(connection.root_der_base64,cert.root);assert!(connection.enable_sni);
    source.close();destination.close();
}
#[test]
fn wrong_owner_source_same_scope_and_closed_destination_fail_before_plan_capture(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let anchor=trust(&cert.root);let(reply,source)=owned(&original,&properties(),REFERENCE);let destination=RpcConnectionScope::fresh();
    assert_eq!(ProvisionedLogin::from_reply(&reply,&owner(),&source,&destination,&anchor,None,None).err(),Some(ProvisionError::Scope));
    assert_eq!(ProvisionedLogin::from_reply(&reply,&original,&RpcConnectionScope::fresh(),&destination,&anchor,None,None).err(),Some(ProvisionError::Scope));
    assert_eq!(ProvisionedLogin::from_reply(&reply,&original,&source,&source,&anchor,None,None).err(),Some(ProvisionError::Scope));
    destination.close();assert_eq!(ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,None,None).err(),Some(ProvisionError::Scope));
    source.close();assert_eq!(ProvisionedLogin::from_reply(&reply,&original,&source,&RpcConnectionScope::fresh(),&anchor,None,None).err(),Some(ProvisionError::Scope));
}
#[test]
fn captured_source_data_survives_source_close_but_never_grants_another_scope_or_trust(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let anchor=trust(&cert.root);let(reply,source)=owned(&original,&properties(),REFERENCE);let destination=RpcConnectionScope::fresh();
    let plan=ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,None,None).unwrap();source.close();
    assert!(plan.connection_for(&original,&destination,&anchor).is_ok());assert!(plan.fields_for(&original,&destination).is_ok());
    assert_eq!(plan.connection_for(&original,&destination,&trust(&cert.root)).err(),Some(ProvisionError::Scope));
    assert_eq!(plan.fields_for(&owner(),&destination).err(),Some(ProvisionError::Scope));
    assert_eq!(plan.connection_for(&original,&RpcConnectionScope::fresh(),&anchor).err(),Some(ProvisionError::Scope));
    destination.close();assert_eq!(plan.fields_for(&original,&destination).err(),Some(ProvisionError::Scope));
}
#[test]
fn absent_cc_is_empty_not_reused_and_required_properties_are_not_guessed(){
    let cert=Certificates::generate("DNS:fixture.invalid");let anchor=trust(&cert.root);
    for missing in 0..4{let original=owner();let props=properties();let filtered=props.iter().enumerate().filter(|(i,_)|*i!=missing).map(|(_,p)|*p).collect::<Vec<_>>();
        let(reply,source)=owned(&original,&filtered,REFERENCE);let destination=RpcConnectionScope::fresh();let result=ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,None,None);
        if missing==3{assert_eq!(result.unwrap().fields_for(&original,&destination).unwrap().device_info_cc,b"");}else{assert_eq!(result.err(),Some(ProvisionError::MissingProperty));}source.close();destination.close();
    }
}
#[test]
fn raw_domain_name_invalid_utf8_controls_and_unresolved_enc_are_not_decoded(){
    let cert=Certificates::generate("DNS:fixture.invalid");let anchor=trust(&cert.root);
    for value in [b"".as_slice(),b"  ",b"ENC(fixture)",b"has@separator",b"[domain]",b"a\0b",&[0xff]]{let original=owner();let mut props=properties();props[0].value=value;
        let(reply,source)=owned(&original,&props,REFERENCE);assert_eq!(ProvisionedLogin::from_reply(&reply,&original,&source,&RpcConnectionScope::fresh(),&anchor,None,None).err(),Some(ProvisionError::InvalidProperty));source.close();
    }
}
#[test]
fn client_property_and_common_reference_decode_keys_remain_separate(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let anchor=trust(&cert.root);
    let client=ConfigDecodeKey::supplied(b"fixture-client-context").unwrap();let common=ConfigDecodeKey::supplied(b"fixture-reference-context").unwrap();
    let encoded_domain=super::super::legacy_config::tests::fixture_envelope(b"fixture-domain",b"fixture-client-context",[5;8]);
    let encoded_app=super::super::legacy_config::tests::fixture_envelope(b"fixture-app",b"fixture-client-context",[6;8]);
    let encoded_ref=super::super::legacy_config::tests::fixture_envelope(REFERENCE,b"fixture-reference-context",[7;8]);
    let mut props=properties();props[1].value=&encoded_domain;props[2].value=&encoded_app;
    let(reply,source)=owned(&original,&props,&encoded_ref);let destination=RpcConnectionScope::fresh();
    let plan=ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,Some(&common),Some(&client)).unwrap();assert_eq!(plan.fields_for(&original,&destination).unwrap().domain_id,b"fixture-domain");
    assert!(ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,Some(&client),Some(&common)).is_err());
    assert!(ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,None,Some(&client)).is_err());
    assert!(ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,Some(&common),None).is_err());source.close();destination.close();
}
#[test]
fn locator_alternate_transport_and_wrong_authority_do_not_bypass_explicit_root_policy(){
    let cert=Certificates::generate("DNS:fixture.invalid");let anchor=trust(&cert.root);
    for reference in [b"#id@adapter$locator".as_slice(),b"#id:udp -h fixture.invalid -p 443;",b"#id:tls -h fixture.invalid -p 443;",b"#id:tcp -h other.invalid -p 443;",b"#id:tcp -h fixture.invalid;"]{
        let original=owner();let(reply,source)=owned(&original,&properties(),reference);assert!(ProvisionedLogin::from_reply(&reply,&original,&source,&RpcConnectionScope::fresh(),&anchor,None,None).is_err());source.close();
    }
    for root in ["","invalid-base64","AA=="]{assert_eq!(AccountTrust::supplied(AccountTrustInputs{authority:"fixture.invalid",root_der_base64:root,enable_sni:true}).err(),Some(ProvisionError::Configuration));}
}
#[test]
fn debug_wrappers_do_not_publish_references_deployment_fields_or_roots(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let anchor=trust(&cert.root);let(reply,source)=owned(&original,&properties(),REFERENCE);let destination=RpcConnectionScope::fresh();
    let plan=ProvisionedLogin::from_reply(&reply,&original,&source,&destination,&anchor,None,None).unwrap();
    assert_eq!(format!("{anchor:?}"),"AccountTrust(<redacted>)");assert_eq!(format!("{plan:?}"),"ProvisionedLogin(<redacted>)");assert_eq!(format!("{:?}",plan.fields_for(&original,&destination).unwrap()),"ProvisionedFields(<redacted>)");source.close();destination.close();
}
#[test]
fn decoded_required_properties_reject_empty_binary_control_and_nested_envelopes(){
    let cert=Certificates::generate("DNS:fixture.invalid");let anchor=trust(&cert.root);
    for index in [1,2]{for value in [b"".as_slice(),b"  ",b"a\0b",&[0xff],b"ENC(unresolved)"]{
        let original=owner();let mut props=properties();props[index].value=value;
        let(reply,source)=owned(&original,&props,REFERENCE);
        assert!(ProvisionedLogin::from_reply(&reply,&original,&source,&RpcConnectionScope::fresh(),&anchor,None,None).is_err());source.close();
    }}
}
#[test]
fn copied_field_limits_fail_before_copy_without_silently_truncating_or_reusing_cc(){
    let mut total=0;assert_eq!(field(&vec![b'x';MAX_FIELD_BYTES+1],false,&mut total).err(),Some(ProvisionError::ResourceLimit));
    let mut total=128*1024;assert_eq!(field(b"x",false,&mut total).err(),Some(ProvisionError::ResourceLimit));
    let mut total=0;assert_eq!(field(b"",true,&mut total).unwrap().as_slice(),b"");
    assert_eq!(field(b"",false,&mut total).err(),Some(ProvisionError::InvalidProperty));
}
