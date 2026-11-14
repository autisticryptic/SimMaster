//! Actions-only native control activation through the original ARC protocol.
use super::*;
use n::rpc_calls::CallOperation;
const CALL_ID:u64=0x1234_5678_9abc;
fn prepared_native_control(policy:Arc<NativeReceiverPolicy>)->(AttemptOwner,RpcConnectionScope,OfflineBackend){
    let original=owner();let mut backend=OfflineBackend::new(original.clone());backend.claim_common_transport().unwrap();backend.bind_common_identity(b"AccountEntry").unwrap();
    let scope=backend.connection_scope();backend.arm_native_receivers(NativeReceiverSetup::capture_native_control(policy,true).unwrap()).unwrap();(original,scope,backend)
}
fn invitation()->Vec<u8>{let mut writer=Writer::versioned().unwrap();writer.u64(CALL_ID).unwrap();writer.bytes(b"native-caller").unwrap();
    writer.properties(&[RpcProperty{key:b"callServerEx.oid",value:b"#native-per-call"}]).unwrap();writer.finish().expose_to_transport().to_vec()}
#[test]
fn default_native_call_book_activates_on_matched_login_before_late_consumer_configuration(){
    let(original,scope,mut backend)=prepared_native_control(default_policy());assert!(backend.configure_native_receivers(17).is_err());
    let login=backend.prepare_login(inputs(),&[]).unwrap();backend.on_plaintext_packet(&original,&scope,&login_reply(login.call_id),0).unwrap();
    let packet=request(91,n::rpc_calls::INVITED_METHOD.as_bytes(),&invitation(),CALL_OBJECT,None,None);
    let events=backend.on_plaintext_packet(&original,&scope,&packet,1).unwrap();
    let call=events.into_iter().find_map(|event|if let OfflineEvent::Call(call)=event{Some(call)}else{None}).unwrap();
    let ticket=call.control_ticket().unwrap();assert_eq!(ticket.native_id().expose_to_backend(),CALL_ID);
    assert!(backend.prepare_call(ticket,CallOperation::Inform,&[]).is_err());
    assert!(backend.complete_call(call.ticket(),true,&[]).unwrap().is_some());
    let outgoing=backend.prepare_call(ticket,CallOperation::Inform,&[]).unwrap();let request=decode_request(&outgoing.packets[0].expose_to_transport()[1..]).unwrap();
    assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#CallServerEx");
    assert!(!request.metadata.iter().any(|p|p.key==b"domain"));
    let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.u64().unwrap(),CALL_ID);let props=reader.properties().unwrap();
    assert_eq!(props.len(),1);assert_eq!(props[0].key,b"RseEx.Info");assert_eq!(props[0].value,b"Invited callId is exist?");reader.finish().unwrap();
    backend.configure_native_receivers(17).unwrap();assert!(scope.is_open());
}
#[test]
fn default_native_batch_call_retains_original_ack_boundary_and_does_not_apply_direct_domain_filter(){
    let(original,scope,mut backend)=prepared_native_control(policy(DomainMode::AllowList));let login=backend.prepare_login(inputs(),&[]).unwrap();
    backend.on_plaintext_packet(&original,&scope,&login_reply(login.call_id),0).unwrap();backend.configure_native_receivers(17).unwrap();
    let outer=backend.prepare_swap(1,&[]).unwrap();let packet=batch(&outer,n::rpc_calls::INVITED_METHOD.as_bytes(),&invitation(),CALL_OBJECT,None);
    let events=backend.on_plaintext_packet(&original,&scope,&packet,2).unwrap();let call=events.into_iter().find_map(|event|if let OfflineEvent::Call(call)=event{Some(call)}else{None}).unwrap();
    let ticket=call.control_ticket().unwrap();assert!(ticket.matches(&original,&scope));
    assert!(matches!(backend.complete_call(call.ticket(),true,&[]).unwrap(),Some(n::offline_backend::CallCompletion::BatchQueued)));
    assert!(!backend.call_acknowledgement_ready(ticket));assert!(backend.prepare_call(ticket,CallOperation::Inform,&[]).is_err());assert!(scope.is_open());
}
#[test]
fn default_native_control_does_not_turn_client_ex_check_alive_or_session_revocation_into_readiness(){
    let(original,scope,mut backend)=prepared_native_control(default_policy());let login=backend.prepare_login(inputs(),&[]).unwrap();
    backend.on_plaintext_packet(&original,&scope,&login_reply(login.call_id),0).unwrap();backend.configure_native_receivers(17).unwrap();
    let empty=Writer::versioned().unwrap().finish();let alive=request(90,n::rpc_client_ex::CHECK_ALIVE_METHOD.as_bytes(),empty.expose_to_transport(),MESSAGE_OBJECT,None,None);
    assert_eq!(direct_result(&backend.on_plaintext_packet(&original,&scope,&alive,1).unwrap()),[0;10]);
    let events=backend.on_plaintext_packet(&original,&scope,&request(91,n::rpc_calls::INVITED_METHOD.as_bytes(),&invitation(),CALL_OBJECT,None,None),2).unwrap();
    let call=events.into_iter().find_map(|event|if let OfflineEvent::Call(call)=event{Some(call)}else{None}).unwrap();
    backend.complete_call(call.ticket(),true,&[]).unwrap();let ticket=call.control_ticket().unwrap();assert!(ticket.is_live());
    let mut reason=Writer::versioned().unwrap();reason.bytes(b"server-kick-off").unwrap();let reason=reason.finish();
    let kicked=request(92,n::rpc_client_ex::NOTIFY_DISCONNECT_METHOD.as_bytes(),reason.expose_to_transport(),MESSAGE_OBJECT,None,Some(b"session-token"));
    backend.on_plaintext_packet(&original,&scope,&kicked,3).unwrap();assert!(scope.session_revoked());assert!(!ticket.is_live());
    assert!(backend.prepare_call(ticket,CallOperation::Inform,&[]).is_err());assert_eq!(ticket.with_media_gate(||1),None);
}
