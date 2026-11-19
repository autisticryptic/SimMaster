//! Pure original 5C347C/5C38F4 default notification DATA. No IO or permissions.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::im_send_data::{encode_user_send_with_digest,default_user_send_notifications,validate_user_send_capture};
#[test]
fn legacy_capture_bounds_remain_metadata_replay_compatible_while_fresh_derived_wire_is_bounded(){
    let value=vec![b'a';MAX_FIELD_BYTES];let fields=[RpcProperty{key:b"userData",value:&value}];let message=OutgoingMessage{kind:SendKind::Text,content:b"body",attachments:&[],parameters:&fields};
    assert!(validate_user_send_capture(&message,None).is_ok());
    assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::TooLarge));
}
#[test]
fn original_present_userdata_online_and_imdn_are_copied_with_exact_native_keys_and_empty_values(){
    let fields=[RpcProperty{key:b"userData",value:b""},RpcProperty{key:b"OnlineMsg",value:b"01"},RpcProperty{key:b"imdnId",value:b"opaque-id"},RpcProperty{key:b"other",value:b"not-notified"}];
    let message=OutgoingMessage{kind:SendKind::Text,content:b"original body",attachments:&[],parameters:&fields};
    let encoded=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();
    assert_eq!(parsed.notify,vec![RpcProperty{key:b"OnlineMsg",value:b"01"},RpcProperty{key:b"Notify.Text",value:b"original body"},RpcProperty{key:b"Notify.ImdnId",value:b"opaque-id"},RpcProperty{key:b"Notify.ImUserData",value:b""}]);
    assert!(parsed.parameters.iter().any(|p|p.key==b"other"&&p.value==b"not-notified"));assert!(!parsed.notify.iter().any(|p|p.key==b"Notify.OnlineMsg"||p.key==b"other"));
}
#[test]
fn original_info_defaults_have_common_metadata_but_never_invent_text(){
    let fields=[RpcProperty{key:b"userData",value:b"private data"},RpcProperty{key:b"OnlineMsg",value:b"false"},RpcProperty{key:b"imdnId",value:b"id"}];
    let message=OutgoingMessage{kind:SendKind::Info(b"Subtype"),content:b"info body",attachments:&[],parameters:&fields};
    let encoded=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();assert_eq!(parsed.kind,b"Info.Subtype");assert_eq!(parsed.notify.len(),3);assert!(!parsed.notify.iter().any(|p|p.key==b"Notify.Text"));assert_eq!(parsed.notify[0],RpcProperty{key:b"OnlineMsg",value:b"false"});
}
#[test]
fn original_nonempty_digest_is_notification_text_data_and_never_changes_json_body_or_account(){
    let message=text(b"actual body");let encoded=encode_user_send_with_digest(box_id(),&message,ACCOUNT,None,Some("digest \"summary\"\n文本".as_bytes())).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();assert_eq!(parsed.json,br#"{"text":"actual body"}"#);assert_eq!(parsed.parameters,vec![RpcProperty{key:b"account",value:ACCOUNT}]);assert_eq!(parsed.notify,vec![RpcProperty{key:b"Notify.Text",value:"digest \"summary\"\n文本".as_bytes()}]);
}
#[test]
fn original_empty_digest_falls_back_to_actual_text_and_missing_metadata_stays_missing(){
    let message=text(b"body");let none=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();let empty=encode_user_send_with_digest(box_id(),&message,ACCOUNT,None,Some(b"")).unwrap();assert_eq!(none.expose_to_transport(),empty.expose_to_transport());assert_eq!(shape(empty.expose_to_transport()).unwrap().notify.len(),1);
}
#[test]
fn explicit_notification_override_remains_exact_and_cannot_silently_mix_digest_sources(){
    let fields=[RpcProperty{key:b"userData",value:b"implicit"},RpcProperty{key:b"OnlineMsg",value:b"1"}];let message=OutgoingMessage{kind:SendKind::Text,content:b"body",attachments:&[],parameters:&fields};
    let empty=encode_user_send(box_id(),&message,ACCOUNT,Some(&[])).unwrap();assert!(shape(empty.expose_to_transport()).unwrap().notify.is_empty());
    let explicit=[RpcProperty{key:b"Notify.Text",value:b"explicit"}];let exact=encode_user_send(box_id(),&message,ACCOUNT,Some(&explicit)).unwrap();assert_eq!(shape(exact.expose_to_transport()).unwrap().notify,explicit);
    assert_eq!(encode_user_send_with_digest(box_id(),&message,ACCOUNT,Some(&explicit),Some(b"digest")).err(),Some(CodecError::InvalidField));
}
#[test]
fn exact_online_msg_key_is_string_data_not_boolean_readiness_auth_or_a_generic_namespace_escape(){
    let message=text(b"body");for value in [b"".as_slice(),b"0",b"01",b"false",b"opaque"]{let fields=[RpcProperty{key:b"OnlineMsg",value}];let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&fields)).unwrap();assert_eq!(shape(encoded.expose_to_transport()).unwrap().notify,fields);}
    for key in [b"onlineMsg".as_slice(),b"Onlinemsg",b"OnlineMsg.Other",b"OnlineMsg/endpoint",b"token",b"ready"]{let fields=[RpcProperty{key,value:b"true"}];assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,Some(&fields)).err(),Some(CodecError::InvalidField));}
}
#[test]
fn notification_digest_rejects_nul_invalid_utf8_oversize_and_info_without_any_io(){
    let message=text(b"body");for bad in [b"a\0b".as_slice(),&[255]]{assert_eq!(encode_user_send_with_digest(box_id(),&message,ACCOUNT,None,Some(bad)).err(),Some(CodecError::InvalidField));}
    let large=vec![b'x';MAX_FIELD_BYTES+1];assert_eq!(encode_user_send_with_digest(box_id(),&message,ACCOUNT,None,Some(&large)).err(),Some(CodecError::TooLarge));
    let info=OutgoingMessage{kind:SendKind::Info(b"Subtype"),content:b"body",attachments:&[],parameters:&[]};assert_eq!(encode_user_send_with_digest(box_id(),&info,ACCOUNT,None,Some(b"digest")).err(),Some(CodecError::InvalidField));
}
#[test]
fn derived_notification_metadata_obeys_whole_stream_bounds_and_original_duplicate_parameter_guard(){
    let large=vec![b'x';MAX_FIELD_BYTES];let fields=[RpcProperty{key:b"userData",value:&large}];let message=OutgoingMessage{kind:SendKind::Text,content:b"body",attachments:&[],parameters:&fields};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::TooLarge));
    let duplicate=[RpcProperty{key:b"OnlineMsg",value:b"0"},RpcProperty{key:b"OnlineMsg",value:b"1"}];let message=OutgoingMessage{kind:SendKind::Text,content:b"body",attachments:&[],parameters:&duplicate};assert_eq!(default_user_send_notifications(&message,None).err(),Some(CodecError::DuplicateKey));
}
