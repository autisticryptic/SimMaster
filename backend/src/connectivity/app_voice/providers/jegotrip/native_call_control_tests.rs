//! Actions-only original default CallServerEx path. Legacy assertions stay separate.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{native_receiver::{NativeReceiverConfig,NativeReceiverPolicy,NativeReceiverSetup},rpc_stream::Reader};

fn native_context()->(ArcRpcSession,AttemptOwner,RpcConnectionScope,CallDispatcher){
    let original=owner();let mut rpc=ArcRpcSession::new(original.clone());rpc.claim_common_transport().unwrap();rpc.bind_common_identity(b"AccountEntry").unwrap();
    let scope=rpc.connection_scope();assert!(rpc.configure_native_call_control().is_err());
    let(id,_)=rpc.prepare_login(ArcLoginInputs{typed_identity:b"username:fixture",domain_name:b"fixture.invalid",selected_session_id:b"fixture-selector",
        phone_login_token:b"fixture-pwd",application_key:b"fixture-appkey",domain_id:b"fixture-domain",application_id:b"fixture-app",device_info_cc:b"",client_properties:&[]},&[]).unwrap();
    let mut result=Writer::new();result.boolean(true).unwrap();result.bytes(b"fixture-user").unwrap();result.bytes(b"fixture-auth-token").unwrap();result.properties(&[]).unwrap();result.bytes(b"").unwrap();
    let reply=encode_reply(id,0,result.finish().expose_to_transport()).unwrap();
    assert!(matches!(rpc.on_reply(&original,&scope,reply.expose_to_transport()).unwrap(),Some(ArcReply::Login(_))));
    rpc.configure_native_call_control().unwrap();let policy=NativeReceiverPolicy::capture(&NativeReceiverConfig{direct_domains:None}).unwrap();
    let dispatcher=rpc.native_call_dispatcher(policy).unwrap();(rpc,original,scope,dispatcher)
}
fn native_notice(d:&CallDispatcher,original:&AttemptOwner,scope:&RpcConnectionScope,kind:CallNoticeKind,id:u64,oid:Option<&[u8]>)->OwnedCallNotice{
    let mut w=Writer::versioned().unwrap();w.u64(id).unwrap();if kind==CallNoticeKind::Invited{w.bytes(b"native-peer").unwrap();}
    let properties=oid.map(|value|vec![RpcProperty{key:b"callServerEx.oid",value}]).unwrap_or_default();w.properties(&properties).unwrap();let args=w.finish();
    let method=match kind{CallNoticeKind::Invited=>n::rpc_calls::INVITED_METHOD,CallNoticeKind::Released=>n::rpc_calls::RELEASED_METHOD,CallNoticeKind::Accepted=>n::rpc_calls::ACCEPTED_METHOD};
    let request=RpcRequest{call_id:37,method:method.as_bytes(),arguments:args.expose_to_transport(),metadata:vec![RpcProperty{key:b"id",value:b"CallClientEx"}]};
    d.on_direct_request(original,scope,&request).unwrap().unwrap()
}
fn assert_target(bytes:&RpcBytes,target:&[u8],method:CallOperation){
    let request=decode_request(bytes.expose_to_transport()).unwrap();assert_eq!(request.method,method.method().as_bytes());
    assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,target);
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth-token");
    assert!(!request.metadata.iter().any(|p|p.key==b"domain"||p.key==b"pwd"));
    let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.u64().unwrap(),ID);
}
fn native_ticket(rpc:&mut ArcRpcSession,original:&AttemptOwner,scope:&RpcConnectionScope,d:&CallDispatcher,oid:Option<&[u8]>)->CallControlTicket{
    let notice=native_notice(d,original,scope,CallNoticeKind::Invited,ID,oid);rpc.register_call_notice(&notice).unwrap().unwrap()
}
#[test]
fn native_default_control_requires_common_login_and_cannot_replace_legacy_or_existing_control(){
    let(mut legacy,_,_,_)=context();assert!(legacy.configure_native_call_control().is_err());
    let(mut native,_,scope,_)=native_context();assert!(native.configure_native_call_control().is_err());
    assert!(native.configure_call_control(&[RpcProperty{key:b"id",value:b"#other"},RpcProperty{key:b"domain",value:b"other"}]).is_err());assert!(scope.is_open());
    assert_eq!(validate_routing(&[RpcProperty{key:b"id",value:NATIVE_CALL_OBJECT}]),Err(CodecError::InvalidField));
}
#[test]
fn native_existence_inform_uses_default_id_only_full_call_id_and_exact_original_property(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,Some(b"#per-call"));
    assert!(rpc.prepare_call(&ticket,CallOperation::Inform,&[]).is_err());rpc.call_acknowledged(&ticket,true).unwrap();
    let(sequence,bytes)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();assert_eq!(sequence,2);assert_target(&bytes,NATIVE_CALL_OBJECT,CallOperation::Inform);
    let request=decode_request(bytes.expose_to_transport()).unwrap();let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.u64().unwrap(),ID);
    let props=reader.properties().unwrap();assert_eq!(props.len(),1);assert_eq!(props[0].key,b"RseEx.Info");assert_eq!(props[0].value,b"Invited callId is exist?");reader.finish().unwrap();
    assert!(rpc.prepare_call(&ticket,CallOperation::Accept,&[]).is_err());assert!(finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,true).accepted());
}
#[test]
fn native_original_invitation_oid_is_used_only_after_existence_for_accept_and_release(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,Some(b"#per-call"));rpc.call_acknowledged(&ticket,true).unwrap();
    let(sequence,bytes)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();assert_target(&bytes,NATIVE_CALL_OBJECT,CallOperation::Inform);
    finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,true);
    let(sequence,bytes)=rpc.prepare_call(&ticket,CallOperation::Accept,&[RpcProperty{key:b"RseEx.SDP",value:b"data-not-media"}]).unwrap();assert_target(&bytes,b"#per-call",CallOperation::Accept);
    let accepted=finish(&mut rpc,&original,&scope,sequence,CallOperation::Accept,true);assert!(accepted.accepted());assert!(accepted.original_call(&ticket));
    let(sequence,bytes)=rpc.prepare_call(&ticket,CallOperation::Release,&[]).unwrap();assert_target(&bytes,b"#per-call",CallOperation::Release);
    assert!(finish(&mut rpc,&original,&scope,sequence,CallOperation::Release,true).accepted());assert!(!ticket.is_live());
}
#[test]
fn native_absent_per_call_override_keeps_original_default_for_followup_control(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,None);rpc.call_acknowledged(&ticket,true).unwrap();
    let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,true);
    let(sequence,bytes)=rpc.prepare_call(&ticket,CallOperation::Accept,&[]).unwrap();assert_target(&bytes,NATIVE_CALL_OBJECT,CallOperation::Accept);
    finish(&mut rpc,&original,&scope,sequence,CallOperation::Accept,true);
    let(_,bytes)=rpc.prepare_call(&ticket,CallOperation::Release,&[]).unwrap();assert_target(&bytes,NATIVE_CALL_OBJECT,CallOperation::Release);
}
#[test]
fn native_endpoint_adapter_facet_cloud_or_malformed_oid_is_not_downgraded_to_a_bare_id(){
    for bad in [b"".as_slice(),b"#object:tcp -h fixture.invalid -p 443;",b"#object@adapter",b"#object$locator",b"facet/#object",b"[cloud]/#object",
        b"{group}",b"<cloud>",b"wild*",b"ENC(unresolved)",b"with space",b"quote\"id",b"slash\\id"]{
        let(mut rpc,original,scope,d)=native_context();let notice=native_notice(&d,&original,&scope,CallNoticeKind::Invited,ID,Some(bad));
        assert_eq!(rpc.register_call_notice(&notice).err(),Some(CodecError::InvalidField));assert!(scope.is_open());
    }
}
#[test]
fn native_existence_pending_rejects_foreign_owner_scope_sequence_and_duplicate_result(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,None);rpc.call_acknowledged(&ticket,true).unwrap();
    let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();let body=result(CallOperation::Inform,true);let wire=encode_reply(sequence,0,body.expose_to_transport()).unwrap();
    assert!(rpc.on_reply(&owner(),&scope,wire.expose_to_transport()).unwrap().is_none());
    assert!(rpc.on_reply(&original,&RpcConnectionScope::fresh(),wire.expose_to_transport()).unwrap().is_none());
    let other=encode_reply(sequence+11,0,body.expose_to_transport()).unwrap();assert!(rpc.on_reply(&original,&scope,other.expose_to_transport()).unwrap().is_none());
    assert!(rpc.is_pending(sequence));assert!(finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,true).accepted());
    assert!(rpc.on_reply(&original,&scope,wire.expose_to_transport()).unwrap().is_none());
}
#[test]
fn native_false_existence_keeps_sdk_but_never_grants_accept_or_second_inform(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,None);rpc.call_acknowledged(&ticket,true).unwrap();
    let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();assert!(!finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,false).accepted());
    assert!(!ticket.is_live());assert!(scope.is_open());assert!(rpc.prepare_call(&ticket,CallOperation::Inform,&[]).is_err());
    assert!(rpc.prepare_call(&ticket,CallOperation::Accept,&[]).is_err());assert_eq!(rpc.phase(),n::arc_session::ArcPhase::Authenticated);
}
#[test]
fn native_remote_release_revokes_per_call_identity_before_late_positive_accept(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,Some(b"#per-call"));rpc.call_acknowledged(&ticket,true).unwrap();
    let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,true);
    let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Accept,&[]).unwrap();
    let notice=native_notice(&d,&original,&scope,CallNoticeKind::Released,ID,None);rpc.register_call_notice(&notice).unwrap();assert!(!ticket.is_live());
    assert!(!finish(&mut rpc,&original,&scope,sequence,CallOperation::Accept,true).accepted());assert_eq!(ticket.with_media_gate(||1),None);
}
#[test]
fn duplicate_native_invitation_cannot_replace_the_original_per_call_target(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,Some(b"#original"));
    let duplicate=native_notice(&d,&original,&scope,CallNoticeKind::Invited,ID,Some(b"#replacement"));assert!(rpc.register_call_notice(&duplicate).unwrap().is_none());
    rpc.call_acknowledged(&ticket,true).unwrap();let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();finish(&mut rpc,&original,&scope,sequence,CallOperation::Inform,true);
    let(_,bytes)=rpc.prepare_call(&ticket,CallOperation::Accept,&[]).unwrap();assert_target(&bytes,b"#original",CallOperation::Accept);
}
#[test]
fn native_existence_properties_cannot_be_overridden_or_consume_a_pending_id_on_rejection(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,None);rpc.call_acknowledged(&ticket,true).unwrap();
    for property in [RpcProperty{key:b"RseEx.Info",value:b"other"},RpcProperty{key:b"token",value:b"substitute"},RpcProperty{key:b"keepAlive",value:b"1"}]{
        assert!(rpc.prepare_call(&ticket,CallOperation::Inform,&[property]).is_err());
    }
    let(sequence,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();assert_eq!(sequence,2);
}
#[test]
fn native_rejection_ack_and_closed_scope_do_not_allow_call_control_or_restore_old_tickets(){
    let(mut rpc,original,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&original,&scope,&d,None);rpc.call_acknowledged(&ticket,false).unwrap();
    assert!(!ticket.is_live());assert!(rpc.prepare_call(&ticket,CallOperation::Inform,&[]).is_err());
    scope.close();assert!(rpc.prepare_call(&ticket,CallOperation::Release,&[]).is_err());assert_eq!(ticket.with_media_gate(||1),None);
}
#[test]
fn native_default_setup_is_bounded_prelogin_data_not_a_scope_or_ready_proof(){
    let policy=NativeReceiverPolicy::capture(&NativeReceiverConfig{direct_domains:None}).unwrap();
    let setup=NativeReceiverSetup::capture_native_control(policy,false).unwrap();assert!(setup.native_call_control);assert!(setup.control().is_none());
    assert_eq!(setup.retained_bytes(),NATIVE_CONTROL_BYTES);assert_eq!(format!("{setup:?}"),"NativeReceiverSetup(<redacted>)");
    let legacy=NativeReceiverSetup::capture(NativeReceiverPolicy::capture(&NativeReceiverConfig{direct_domains:None}).unwrap(),false,None).unwrap();assert!(!legacy.native_call_control);
}

fn native_account_config()->n::account_config::AccountConfig{
    let certificate=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");
    let mut config=n::account_config::tests::native_fixture(&certificate.root,"127.0.0.1",12345);
    config.profile.incoming=None;config.profile.native_receivers=Some(NativeReceiverConfig{direct_domains:None});config
}
#[test]
fn native_call_control_config_defaults_off_and_does_not_change_old_private_envelopes(){
    let config=native_account_config();assert!(!config.profile.native_call_control);let bytes=config.encode_private().unwrap();
    let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();assert!(value["config"]["profile"].get("native_call_control").is_none());
    let loaded=n::account_config::AccountConfig::decode_private(&bytes).unwrap();assert!(!loaded.profile.native_call_control);assert_eq!(loaded.encode_private().unwrap().as_slice(),bytes.as_slice());
}
#[test]
fn native_call_control_config_roundtrips_only_as_explicit_native_flag_without_routing_or_domain(){
    let mut config=native_account_config();config.profile.native_call_control=true;config.validate(100).unwrap();
    let bytes=config.encode_private().unwrap();let value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["format_version"],1);assert_eq!(value["config"]["profile"]["native_call_control"],true);assert!(value["config"]["profile"]["call_control"].is_null());
    let loaded=n::account_config::AccountConfig::decode_private(&bytes).unwrap();assert!(loaded.profile.native_call_control);loaded.runtime_profile().unwrap();assert!(loaded.runtime_calls().unwrap().is_none());
}
#[test]
fn native_call_control_requires_native_receivers_and_cannot_mix_with_legacy_control_maps(){
    let mut config=native_account_config();config.profile.native_call_control=true;config.profile.native_receivers=None;assert!(config.validate(100).is_err());
    let mut config=native_account_config();config.profile.native_call_control=true;
    config.profile.call_control=Some(vec![n::account_config::ProfileProperty{key:b"id".to_vec(),value:b"#explicit".to_vec()},n::account_config::ProfileProperty{key:b"domain".to_vec(),value:b"explicit-domain".to_vec()}]);
    assert!(config.validate(100).is_err());
}
#[test]
fn native_call_control_flag_does_not_allow_a_legacy_tcp_profile_or_imply_media_configuration(){
    let certificate=n::tls_engine::tests::Certificates::generate("DNS:fixture.invalid");let mut config=n::account_test_support::config(&certificate.root);
    config.profile.native_call_control=true;assert!(config.runtime_profile().is_err());
    let mut config=native_account_config();config.profile.native_call_control=true;assert!(config.runtime_calls().unwrap().is_none());
}
#[test]
fn native_call_control_profile_and_explicit_control_are_mutually_exclusive_in_both_orders(){
    let config=native_account_config();let routing=[RpcProperty{key:b"id",value:b"#explicit"},RpcProperty{key:b"domain",value:b"domain"}];
    assert!(config.runtime_profile().unwrap().with_native_call_control().unwrap().with_call_control(&routing).is_err());
    assert!(config.runtime_profile().unwrap().with_call_control(&routing).unwrap().with_native_call_control().is_err());
    assert!(config.runtime_profile().unwrap().with_native_call_control().unwrap().with_native_call_control().is_err());
}
#[test]
fn native_call_control_false_or_true_never_bypasses_unsupported_media_or_plain_transport_policy(){
    let mut config=native_account_config();config.profile.native_call_control=true;
    let bytes=config.encode_private().unwrap();let mut value:serde_json::Value=serde_json::from_slice(&bytes).unwrap();
    value["config"]["call_media"]=serde_json::json!({"authority":"fixture.invalid","root_der_base64":"not-a-root","enable_sni":true,
        "routing":[],"features":[],"properties":[],"reference_decode_key":null,"audio_bind_ip":"127.0.0.1","allow_plain_media":false,"trunk":null});
    assert!(n::account_config::AccountConfig::decode_private(&serde_json::to_vec(&value).unwrap()).unwrap().validate(100).is_err());
}
