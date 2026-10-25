//! Actions-only original-call state/shared-ARC pending tests. No IO/SDK/media.
use super::*;
use super::super::{arc_session::{ArcLoginInputs,ArcReply,ArcRpcSession},call_binding::{CallDispatcher,CallIncomingBinding},
    rpc_calls::{CallOperation,CallNoticeKind,NativeCallId},rpc_envelope::{RpcRequest,encode_reply,decode_request},
    rpc_stream::{Writer,RpcBytes},session::HostingSession,tests::imported};
const ID:u64=0x0123_4567_89ab_cdef;
fn owner()->AttemptOwner{let mut s=HostingSession::new("line-call-owner".into());s.import(imported(),100).unwrap();s.begin_login(100).unwrap()}
fn context()->(ArcRpcSession,AttemptOwner,RpcConnectionScope,CallDispatcher){
    let original=owner();let mut rpc=ArcRpcSession::new(original.clone());let scope=rpc.connection_scope();
    let(id,_)=rpc.prepare_login(ArcLoginInputs{typed_identity:b"username:fixture",domain_name:b"fixture.invalid",selected_session_id:b"fixture-selector",phone_login_token:b"fixture-pwd",application_key:b"fixture-appkey",domain_id:b"fixture-domain",application_id:b"fixture-app",device_info_cc:b"",client_properties:&[]},&[]).unwrap();
    let mut result=Writer::new();result.boolean(true).unwrap();result.bytes(b"fixture-user").unwrap();result.bytes(b"fixture-auth-token").unwrap();result.properties(&[]).unwrap();result.bytes(b"").unwrap();
    let reply=encode_reply(id,0,result.finish().expose_to_transport()).unwrap();assert!(matches!(rpc.on_reply(&original,&scope,reply.expose_to_transport()).unwrap(),Some(ArcReply::Login(_))));
    rpc.configure_call_control(&[RpcProperty{key:b"id",value:b"#fixture-base-server"},RpcProperty{key:b"domain",value:b"fixture-domain"}]).unwrap();
    let d=rpc.incoming_call_dispatcher(CallIncomingBinding{object_id:b"#fixture-call-client",domain:b"fixture-domain"}).unwrap();(rpc,original,scope,d)
}
fn notice(d:&CallDispatcher,original:&AttemptOwner,scope:&RpcConnectionScope,kind:CallNoticeKind,id:u64)->OwnedCallNotice{
    let mut w=Writer::versioned().unwrap();w.u64(id).unwrap();if kind==CallNoticeKind::Invited{w.bytes(b"fixture-peer").unwrap();}w.properties(&[RpcProperty{key:b"callServerEx.oid",value:b"#fixture-original-server"}]).unwrap();let args=w.finish();
    let method=match kind{CallNoticeKind::Invited=>super::super::rpc_calls::INVITED_METHOD,CallNoticeKind::Released=>super::super::rpc_calls::RELEASED_METHOD,CallNoticeKind::Accepted=>super::super::rpc_calls::ACCEPTED_METHOD};
    let request=RpcRequest{call_id:37,method:method.as_bytes(),arguments:args.expose_to_transport(),metadata:vec![RpcProperty{key:b"id",value:b"#fixture-call-client"},RpcProperty{key:b"domain",value:b"fixture-domain"}]};d.on_direct_request(original,scope,&request).unwrap().unwrap()
}
fn result(operation:CallOperation,accepted:bool)->RpcBytes{let mut w=Writer::new();w.boolean(accepted).unwrap();if operation==CallOperation::Accept{w.properties(&[RpcProperty{key:b"endpoint",value:b"opaque-not-media-ready"}]).unwrap();}w.bytes(b"").unwrap();w.finish()}
fn finish(rpc:&mut ArcRpcSession,original:&AttemptOwner,scope:&RpcConnectionScope,id:i32,operation:CallOperation,accepted:bool)->OwnedCallOperationReply{
    let bytes=result(operation,accepted);let wire=encode_reply(id,0,bytes.expose_to_transport()).unwrap();let Some(ArcReply::Call(reply))=rpc.on_reply(original,scope,wire.expose_to_transport()).unwrap()else{panic!("matched original call reply")};reply
}
#[test]
fn shared_allocator_preserves_original_native_id_token_target_and_signal_only_results(){
    let(mut rpc,original,scope,d)=context();let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();
    assert!(rpc.prepare_call(&ticket,CallOperation::Inform,&[]).is_err());rpc.call_acknowledged(&ticket,true).unwrap();
    let(check,bytes)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();assert_eq!(check,2);let request=decode_request(bytes.expose_to_transport()).unwrap();
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth-token");assert!(!request.metadata.iter().any(|p|p.key==b"pwd"));
    assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#fixture-base-server");
    let reply=finish(&mut rpc,&original,&scope,check,CallOperation::Inform,true);assert!(reply.accepted());
    let(accept,bytes)=rpc.prepare_call(&ticket,CallOperation::Accept,&[RpcProperty{key:b"RseEx.SDP",value:b"still-data"}]).unwrap();assert_eq!(accept,3);
    let request=decode_request(bytes.expose_to_transport()).unwrap();assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#fixture-original-server");
    let reply=finish(&mut rpc,&original,&scope,accept,CallOperation::Accept,true);assert!(reply.accepted());assert_eq!(reply.native_id(),NativeCallId::from_bits(ID));assert!(reply.properties_for(&original,&scope).unwrap().iter().any(|p|p.value==b"opaque-not-media-ready"));
    assert!(rpc.prepare_call(&ticket,CallOperation::Accept,&[]).is_err());let(release,_)=rpc.prepare_call(&ticket,CallOperation::Release,&[]).unwrap();assert_eq!(release,4);
    assert!(finish(&mut rpc,&original,&scope,release,CallOperation::Release,true).accepted());assert!(!ticket.is_live());
}
#[test]
fn wrong_original_owner_scope_and_rpc_sequence_never_complete_call_pending_or_rebind(){
    let(mut rpc,original,scope,d)=context();let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();rpc.call_acknowledged(&ticket,true).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();let bytes=result(CallOperation::Inform,true);let wire=encode_reply(id,0,bytes.expose_to_transport()).unwrap();
    assert!(rpc.on_reply(&owner(),&scope,wire.expose_to_transport()).unwrap().is_none());assert!(rpc.on_reply(&original,&RpcConnectionScope::fresh(),wire.expose_to_transport()).unwrap().is_none());assert!(rpc.is_pending(id));
    let other=encode_reply(id+9,0,bytes.expose_to_transport()).unwrap();assert!(rpc.on_reply(&original,&scope,other.expose_to_transport()).unwrap().is_none());assert!(rpc.is_pending(id));
    assert!(finish(&mut rpc,&original,&scope,id,CallOperation::Inform,true).accepted());assert!(!rpc.is_pending(id));assert!(rpc.on_reply(&original,&scope,wire.expose_to_transport()).unwrap().is_none());
}
#[test]
fn remote_release_revokes_before_late_true_accept_and_never_revives_original_call(){
    let(mut rpc,original,scope,d)=context();let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();rpc.call_acknowledged(&ticket,true).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();finish(&mut rpc,&original,&scope,id,CallOperation::Inform,true);
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Accept,&[]).unwrap();let released=notice(&d,&original,&scope,CallNoticeKind::Released,ID);rpc.register_call_notice(&released).unwrap();assert!(!ticket.is_live());
    let late=finish(&mut rpc,&original,&scope,id,CallOperation::Accept,true);assert!(!late.accepted());assert_eq!(rpc.phase(),super::super::arc_session::ArcPhase::Authenticated);assert!(rpc.prepare_call(&ticket,CallOperation::Accept,&[]).is_err());
}
#[test]
fn false_call_result_is_call_rejection_not_sdk_authentication_failure_or_retry_grant(){
    let(mut rpc,original,scope,d)=context();let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();rpc.call_acknowledged(&ticket,true).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();assert!(!finish(&mut rpc,&original,&scope,id,CallOperation::Inform,false).accepted());assert!(scope.is_open());assert!(!ticket.is_live());
    assert!(rpc.prepare_call(&ticket,CallOperation::Accept,&[]).is_err());let(id,_)=rpc.prepare_call(&ticket,CallOperation::Release,&[]).unwrap();assert!(finish(&mut rpc,&original,&scope,id,CallOperation::Release,true).accepted());
}
#[test]
fn duplicate_invitation_unknown_release_and_unadmitted_control_do_not_create_permissions(){
    let(mut rpc,original,scope,d)=context();let unknown=notice(&d,&original,&scope,CallNoticeKind::Released,ID);assert!(rpc.register_call_notice(&unknown).unwrap().is_none());
    let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();assert!(rpc.register_call_notice(&incoming).unwrap().is_none());
    assert!(rpc.prepare_call(&ticket,CallOperation::Accept,&[]).is_err());rpc.call_acknowledged(&ticket,false).unwrap();assert!(!ticket.is_live());assert!(rpc.prepare_call(&ticket,CallOperation::Inform,&[]).is_err());
}
#[test]
fn call_configuration_rejects_auth_target_substitution_and_does_not_mint_a_new_scope(){
    for bad in [b"".as_slice(),b"wild*",b"with space",b"#id:tcp -h host -p 443;",b"ENC(unresolved)"]{
        assert!(validate_routing(&[RpcProperty{key:b"id",value:bad},RpcProperty{key:b"domain",value:b"fixture"}]).is_err());
    }
    assert!(validate_routing(&[RpcProperty{key:b"id",value:b"#id"},RpcProperty{key:b"domain",value:b"fixture"},RpcProperty{key:b"token",value:b"not-original"}]).is_err());
    let(mut rpc,_original,scope,_d)=context();assert!(rpc.configure_call_control(&[]).is_err());assert!(scope.is_open());
}
#[test]
fn call_scope_drop_and_malformed_reply_never_expose_positive_data_or_original_ticket_after_close(){
    let(mut rpc,original,scope,d)=context();let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();rpc.call_acknowledged(&ticket,true).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();let bad=encode_reply(id,0,&[0,1]).unwrap();assert!(rpc.on_reply(&original,&scope,bad.expose_to_transport()).is_err());assert!(!ticket.is_live());assert!(!scope.is_open());
}
#[test]
fn debug_wrappers_and_checked_reply_borrows_do_not_publish_peer_call_ids_or_keys(){
    let(mut rpc,original,scope,d)=context();let incoming=notice(&d,&original,&scope,CallNoticeKind::Invited,ID);let ticket=rpc.register_call_notice(&incoming).unwrap().unwrap();rpc.call_acknowledged(&ticket,true).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();let reply=finish(&mut rpc,&original,&scope,id,CallOperation::Inform,true);
    assert_eq!(format!("{ticket:?}"),"CallControlTicket(<redacted>)");assert_eq!(format!("{reply:?}"),"OwnedCallOperationReply(<redacted>)");assert!(reply.properties_for(&owner(),&scope).is_none());scope.close();assert!(reply.properties_for(&original,&scope).is_none());
}
