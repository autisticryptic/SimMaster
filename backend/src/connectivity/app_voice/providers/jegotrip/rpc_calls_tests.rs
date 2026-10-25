//! Actions-only CallEx byte fixtures. No command submission, receiver install,
//! call permission, SDP/crypto guessing, media peer or native target execution.
use super::*;
use super::super::{rpc_envelope::{decode_request, encode_request}, rpc_stream::{MAX_FIELD_BYTES, MAX_STREAM_BYTES}};

const ID: u64 = 0x0123_4567_89ab_cdef;
const ACCEPT_EMPTY: &[u8] = &[0,0,1,0,0,1,0x23,0x45,0x67,0x89,0xab,0xcd,0xef,0,0,0,0];
fn notice(kind: CallNoticeKind, fields: &[RpcProperty<'_>]) -> Vec<u8> {
    let mut writer=Writer::versioned().unwrap();writer.u64(ID).unwrap();
    if kind==CallNoticeKind::Invited{writer.bytes(b"fixture-peer").unwrap();}
    writer.properties(fields).unwrap();writer.finish().expose_to_transport().to_vec()
}
fn result(accepted:bool,fields:&[RpcProperty<'_>],reason:&[u8])->Vec<u8>{
    let mut writer=Writer::new();writer.boolean(accepted).unwrap();writer.properties(fields).unwrap();writer.bytes(reason).unwrap();writer.finish().expose_to_transport().to_vec()
}

#[test]
fn accept_fixture_has_complete_be64_native_id_separate_from_i32_rpc_correlation(){
    let arguments=encode_accept(NativeCallId::from_bits(ID),&[]).unwrap();assert_eq!(arguments.expose_to_transport(),ACCEPT_EMPTY);
    let envelope=encode_request(37,&[],ACCEPT_METHOD.as_bytes(),arguments.expose_to_transport()).unwrap();
    let call=decode_request(envelope.expose_to_transport()).unwrap();assert_eq!(call.call_id,37);
    let mut reader=Reader::versioned(call.arguments).unwrap();assert_eq!(reader.u64().unwrap(),ID);assert!(reader.properties().unwrap().is_empty());reader.finish().unwrap();
    for id in [0,u64::MAX,u32::MAX as u64+1]{
        let bytes=encode_accept(NativeCallId::from_bits(id),&[]).unwrap();assert_eq!(&bytes.expose_to_transport()[5..13],&id.to_be_bytes());
    }
}
#[test]
fn invited_independent_fixture_preserves_original_binary_peer_without_text_or_sdp_inference(){
    let mut fixture=ACCEPT_EMPTY[..13].to_vec();fixture.extend_from_slice(&[0,0,0,3,0xff,0,7,0,0,0,0]);
    let value=decode_notice(INVITED_METHOD.as_bytes(),&fixture).unwrap().unwrap();
    assert_eq!(value.kind,CallNoticeKind::Invited);assert_eq!(value.call_id.expose_to_backend(),ID);
    assert_eq!(value.peer,Some([0xff,0,7].as_slice()));assert!(value.properties.is_empty());
}
#[test]
fn released_and_accepted_are_be64_map_without_invitation_peer_or_media_success(){
    let properties=[RpcProperty{key:&[0xff,0],value:&[0,0xfe,3]},RpcProperty{key:b"RseEx.SDP",value:b"opaque-not-parsed-as-sdp"}];
    for (kind,method) in [(CallNoticeKind::Released,RELEASED_METHOD),(CallNoticeKind::Accepted,ACCEPTED_METHOD)]{
        let bytes=notice(kind,&properties);let value=decode_notice(method.as_bytes(),&bytes).unwrap().unwrap();
        assert_eq!(value.call_id.expose_to_backend(),ID);assert_eq!(value.kind,kind);assert!(value.peer.is_none());
        assert!(value.properties.contains(&properties[0]));assert!(value.properties.contains(&properties[1]));
    }
}
#[test]
fn call_notification_truncation_bad_version_and_trailing_data_never_produce_partial_events(){
    for (kind,method) in [(CallNoticeKind::Invited,INVITED_METHOD),(CallNoticeKind::Released,RELEASED_METHOD),(CallNoticeKind::Accepted,ACCEPTED_METHOD)]{
        let valid=notice(kind,&[]);
        for end in 0..valid.len(){assert!(decode_notice(method.as_bytes(),&valid[..end]).is_err(),"prefix {end}");}
        let mut bad=valid.clone();bad.push(0);assert_eq!(decode_notice(method.as_bytes(),&bad).err(),Some(CodecError::TrailingData));
        let mut bad=valid.clone();bad[2]=2;assert_eq!(decode_notice(method.as_bytes(),&bad).err(),Some(CodecError::UnsupportedVersion));
        let mut bad=valid;bad[0]=1;assert_eq!(decode_notice(method.as_bytes(),&bad).err(),Some(CodecError::UnsupportedStream));
    }
}
#[test]
fn duplicate_maps_and_oversized_call_fields_are_rejected_instead_of_truncated(){
    let mut duplicate=ACCEPT_EMPTY[..13].to_vec();duplicate.extend_from_slice(&[0,0,0,2]);
    for _ in 0..2{duplicate.extend_from_slice(&[0,0,0,1,b'a',0,0,0,1,b'x']);}
    assert_eq!(decode_notice(RELEASED_METHOD.as_bytes(),&duplicate).err(),Some(CodecError::DuplicateKey));
    assert_eq!(encode_accept(NativeCallId::from_bits(ID),&[RpcProperty{key:b"a",value:b"x"},RpcProperty{key:b"a",value:b"y"}]).err(),Some(CodecError::DuplicateKey));
    let large=vec![0;MAX_FIELD_BYTES+1];assert_eq!(encode_accept(NativeCallId::from_bits(ID),&[RpcProperty{key:b"a",value:&large}]).err(),Some(CodecError::TooLarge));
    assert_eq!(decode_notice(INVITED_METHOD.as_bytes(),&vec![0;MAX_STREAM_BYTES+1]).err(),Some(CodecError::TooLarge));
}
#[test]
fn accept_result_requires_zero_external_code_true_boolean_map_and_terminal_footer(){
    let fixture=[0,1,0,0,0,0,0,0,0,0];assert!(decode_accept_result(0,&fixture).unwrap().properties.is_empty());
    let mut optional=fixture.to_vec();optional.extend_from_slice(&[0,0,0,0]);assert!(decode_accept_result(0,&optional).unwrap().footer.properties.is_empty());
    assert_eq!(decode_accept_result(0,&result(false,&[],b"")).err(),Some(CodecError::RemoteFailure));
    assert_eq!(decode_accept_result(0,&result(true,&[],b"fixture-rejected")).err(),Some(CodecError::RemoteFailure));
    for code in [1,0x1_0000,u32::MAX]{assert_eq!(decode_accept_result(code,&fixture).err(),Some(CodecError::RemoteFailure));}
    // Native bool is nonzero, not an alternate request version marker.
    let mut compatible=fixture;compatible[1]=7;assert!(decode_accept_result(0,&compatible).is_ok());
}
#[test]
fn accept_result_missing_footer_trailing_or_duplicate_properties_is_never_success(){
    let valid=result(true,&[RpcProperty{key:b"endpoint",value:b"private-opaque-reference"}],b"");
    for end in 0..valid.len(){assert!(decode_accept_result(0,&valid[..end]).is_err());}
    let mut trailing=valid.clone();trailing.push(0xff);assert!(decode_accept_result(0,&trailing).is_err());
    let mut duplicate=vec![0,1,0,0,0,2];for _ in 0..2{duplicate.extend_from_slice(&[0,0,0,1,b'a',0,0,0,1,b'x']);}duplicate.extend_from_slice(&[0,0,0,0]);
    assert_eq!(decode_accept_result(0,&duplicate).err(),Some(CodecError::DuplicateKey));
    assert!(decode_accept_result(0,ACCEPT_EMPTY).is_err());
}
#[test]
fn call_data_debug_wrappers_do_not_expose_original_id_peer_properties_or_endpoints(){
    let fields=[RpcProperty{key:b"endpoint",value:b"fixture-private-reference"}];let raw=notice(CallNoticeKind::Invited,&fields);
    let incoming=decode_notice(INVITED_METHOD.as_bytes(),&raw).unwrap().unwrap();let accepted=result(true,&fields,b"");
    assert_eq!(format!("{:?}",NativeCallId::from_bits(ID)),"NativeCallId(<redacted>)");
    assert_eq!(format!("{incoming:?}"),"CallNotice(<redacted>)");assert_eq!(format!("{:?}",decode_accept_result(0,&accepted).unwrap()),"AcceptResult(<redacted>)");
}
#[test]
fn unknown_call_method_is_refused_before_parsing_or_guessing_an_invitation(){
    for method in [b"onInvited".as_slice(),b"recvOnlineMessage.MessageReceiver.Client",b"onReplaced.CallClientEx.CallEx"]{
        assert!(decode_notice(method,&[0xff]).unwrap().is_none());
    }
}
#[test]
fn inform_and_release_arguments_share_original_be64_map_but_results_do_not_require_accept_map(){
    for operation in [CallOperation::Inform,CallOperation::Accept,CallOperation::Release]{
        assert_eq!(encode_operation(operation,NativeCallId::from_bits(ID),&[]).unwrap().expose_to_transport(),ACCEPT_EMPTY);
        assert!(!operation.method().is_empty());
    }
    for operation in [CallOperation::Inform,CallOperation::Release]{
        let result=decode_operation_result(operation,0,&[0,1,0,0,0,0]).unwrap();assert!(result.accepted&&result.properties.is_empty());
        assert!(!decode_operation_result(operation,0,&[0,0,0,0,0,0]).unwrap().accepted);
        assert!(decode_operation_result(operation,1,&[0,1,0,0,0,0]).is_err());
    }
}
#[test]
fn notice_replies_preserve_the_native_invited_released_and_accepted_format_distinction(){
    for kind in [CallNoticeKind::Invited,CallNoticeKind::Released]{
        assert_eq!(encode_notice_result(kind,true,&[]).unwrap().expose_to_transport(),&[0,1,0,0,0,0]);
        assert_eq!(encode_notice_result(kind,false,&[]).unwrap().expose_to_transport(),&[0,0,0,0,0,0]);
        assert_eq!(encode_notice_result(kind,true,&[RpcProperty{key:b"p",value:b"v"}]).err(),Some(CodecError::InvalidField));
    }
    assert_eq!(encode_notice_result(CallNoticeKind::Accepted,true,&[]).unwrap().expose_to_transport(),&[0,1,0,0,0,0,0,0,0,0]);
    assert!(encode_notice_result(CallNoticeKind::Accepted,true,&[RpcProperty{key:b"p",value:b"v"}]).is_ok());
}
#[test]
fn call_operation_rejections_and_malformed_results_never_become_sdk_success_or_media_state(){
    let values=[RpcProperty{key:b"endpoint",value:b"opaque"}];
    let bytes=result(false,&values,b"");let rejected=decode_operation_result(CallOperation::Accept,0,&bytes).unwrap();assert!(!rejected.accepted);assert!(rejected.properties.contains(&values[0]));
    let bytes=result(true,&values,b"rejected");assert!(!decode_operation_result(CallOperation::Accept,0,&bytes).unwrap().accepted);
    assert!(decode_operation_result(CallOperation::Accept,0,&[0,1,0,0,0,0]).is_err());
    assert_eq!(format!("{:?}",decode_operation_result(CallOperation::Inform,0,&[0,1,0,0,0,0]).unwrap()),"OperationResult(<redacted>)");
}
