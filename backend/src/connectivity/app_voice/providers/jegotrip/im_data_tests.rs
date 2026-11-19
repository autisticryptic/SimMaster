//! Actions-only original IM DATA tests. No SDK, socket, SMS, file attachment
//! write or notification completion is implied by a decoded record/true bool.
use super::*;
use super::super::super::im_data::*;
#[path="im_text_tests.rs"]
mod text;
#[path="im_info_tests.rs"]
mod info;
#[path="im_status_tests.rs"]
mod status;
#[path="im_send_data_tests.rs"]
mod send;
fn bytes(out:&mut Vec<u8>,value:&[u8]){out.extend_from_slice(&(value.len() as u32).to_be_bytes());out.extend_from_slice(value);}
fn map(out:&mut Vec<u8>,fields:&[(&[u8],&[u8])]){out.extend_from_slice(&(fields.len() as u32).to_be_bytes());for(key,value)in fields{bytes(out,key);bytes(out,value);}}
fn record(index:i64,time:i64,kind:&[u8],body:&[u8])->Vec<u8>{
    let mut out=Vec::new();out.extend_from_slice(&index.to_be_bytes());out.extend_from_slice(&time.to_be_bytes());bytes(&mut out,b"01_2");bytes(&mut out,kind);bytes(&mut out,body);
    map(&mut out,&[(b"b",&[0xff,0,7])]);map(&mut out,&[(b"__magic",&[1,2,3])]);out
}
fn one(index:i64,time:i64)->Vec<u8>{let mut out=vec![0];out.extend(record(index,time,b"Text","fixture IM 文本".as_bytes()));out}
fn props<'a>(box_id:&'a[u8],index:&'a[u8])->Vec<RpcProperty<'a>>{vec![RpcProperty{key:b"Notify.Command",value:b"Message"},RpcProperty{key:b"Notify.MsgIdx",value:index},RpcProperty{key:b"Notify.Box",value:box_id}]}
#[test]
fn native_im_box_keeps_original_p2p_uid_and_positive_org_pair_as_opaque_identity(){
    for (value,kind) in [(b"P2P/0_1".as_slice(),BoxKind::P2P),(b"P2P/0001_23",BoxKind::P2P),(b"Org/1-2",BoxKind::Org),(b"Org/12+345",BoxKind::Org)]{
        let box_id=ImBox::parse(value).unwrap();assert_eq!(box_id.bytes(),value);assert_eq!(box_id.kind(),kind);assert_eq!(format!("{box_id:?}"),"ImBox(<redacted>)");}
}
#[test]
fn native_im_box_rejects_aliases_zero_group_parts_nondecimal_or_qualified_references(){
    for value in [b"".as_slice(),b"P2P/1_0",b"P2P/_1",b"P2P/1_",b"P2P/1__2",b"P2P/1_a",b"P2P/1_2\0",b"Org/01-2",b"Org/1-02",b"Org/0+1",b"Org/1-2+3",b"Org/1_2",b"org/1-2",b"Self/1_2",b"AccountEntry:sarc -h 1 -p 1;",b"P2P/1_2@adapter"]{assert!(ImBox::parse(value).is_err());}
    let value=[b"P2P/".as_slice(),&vec![b'1';MAX_BOX_BYTES],b"_2"].concat();assert_eq!(ImBox::parse(&value).err(),Some(CodecError::TooLarge));
}
#[test]
fn native_im_notification_preserves_complete_signed_index_not_cursor_or_notification_id(){
    for index in [i64::MIN,-1,0,0x1_0000_0001,i64::MAX]{let text=index.to_string();let key=MessageKey::notification(&props(b"Org/1-2",text.as_bytes())).unwrap();assert_eq!(key.index,index);assert_eq!(key.box_id.bytes(),b"Org/1-2");assert_eq!(format!("{key:?}"),"ImMessageKey(<redacted>)");}
}
#[test]
fn native_im_notification_invalid_decimal_is_not_silently_zero_or_truncated(){
    for value in [b"".as_slice(),b"+1",b"01",b"-0",b" 1",b"1 ",b"1x",b"9223372036854775808",b"-9223372036854775809",&[0xff,0,1]]{assert!(MessageKey::notification(&props(b"P2P/1_2",value)).is_err());}
}
#[test]
fn native_im_notification_exact_keys_refuse_duplicate_missing_extra_or_invite_shapes(){
    for index in 0..3{let mut fields=props(b"P2P/1_2",b"7");fields.remove(index);assert!(MessageKey::notification(&fields).is_err());}
    let mut fields=props(b"P2P/1_2",b"7");fields.push(RpcProperty{key:b"id",value:b"#Group"});assert!(MessageKey::notification(&fields).is_err());fields.pop();fields[0].value=b"Invite";assert!(MessageKey::notification(&fields).is_err());fields[0].value=b"Message";fields[1]=fields[2];assert_eq!(MessageKey::notification(&fields).err(),Some(CodecError::DuplicateKey));
}
#[test]
fn native_im_record_uses_two_be64_three_byte_strings_and_distinct_attachment_parameter_maps(){
    let wire=one(0x1_2345_6789,-1);let message=ImMessage::decode(&wire).unwrap();assert_eq!(message.index,0x1_2345_6789);assert_eq!(message.time,-1);assert_eq!(message.sender_uid,b"01_2");assert_eq!(message.kind,b"Text");assert_eq!(message.content,"fixture IM 文本".as_bytes());assert_eq!(message.attachments[0].key,b"b");assert_eq!(message.attachments[0].value,&[0xff,0,7]);assert_eq!(message.parameters[0].key,b"__magic");assert_eq!(message.parameters[0].value,&[1,2,3]);assert_eq!(message.encode().unwrap().expose_to_transport(),wire);
}
#[test]
fn native_im_record_signed_extremes_are_lossless_and_not_classified_as_sms_or_ready(){
    for(index,time)in[(i64::MIN,i64::MAX),(i64::MAX,i64::MIN),(-1,-1)]{let wire=one(index,time);let message=ImMessage::decode(&wire).unwrap();assert_eq!((message.index,message.time),(index,time));assert_eq!(message.encode().unwrap().expose_to_transport(),wire);}
}
#[test]
fn native_im_unknown_type_and_binary_content_are_data_not_implicitly_text_or_file_paths(){
    let mut wire=vec![0];wire.extend(record(1,2,b"unknown-private-type",&[0xff,0,7]));let message=ImMessage::decode(&wire).unwrap();assert_eq!(message.kind,b"unknown-private-type");assert_eq!(message.content,&[0xff,0,7]);assert_eq!(format!("{message:?}"),"ImMessage(<redacted>)");assert!(!format!("{message:?}").contains("private"));
}
#[test]
fn native_im_record_every_truncation_wrong_stream_type_and_trailing_data_fail(){
    let wire=one(1,2);for end in 0..wire.len(){assert!(ImMessage::decode(&wire[..end]).is_err(),"truncated at {end}");}for marker in [1,2,255]{let mut bad=wire.clone();bad[0]=marker;assert_eq!(ImMessage::decode(&bad).err(),Some(CodecError::UnsupportedStream));}let mut trailing=wire;trailing.push(0);assert_eq!(ImMessage::decode(&trailing).err(),Some(CodecError::TrailingData));
}
#[test]
fn native_im_record_count_and_size_claims_fail_before_large_allocation(){
    let mut base=vec![0];base.extend_from_slice(&[0;16]);for _ in 0..3{bytes(&mut base,b"");}let start=base.len();base.extend_from_slice(&u32::MAX.to_be_bytes());assert_eq!(ImMessage::decode(&base).err(),Some(CodecError::TooLarge));base.truncate(start);base.extend_from_slice(&1u32.to_be_bytes());assert_eq!(ImMessage::decode(&base).err(),Some(CodecError::Truncated));
    let large=vec![0;super::super::super::rpc_stream::MAX_STREAM_BYTES+1];assert_eq!(ImMessage::decode(&large).err(),Some(CodecError::TooLarge));
}
#[test]
fn native_im_duplicate_attachment_or_parameter_keys_are_not_silently_overwritten(){
    for duplicate_attachment in [true,false]{let mut wire=vec![0];wire.extend_from_slice(&[0;16]);for _ in 0..3{bytes(&mut wire,b"");}let duplicate=[(b"same".as_slice(),b"one".as_slice()),(b"same".as_slice(),b"two".as_slice())];map(&mut wire,if duplicate_attachment{&duplicate}else{&[]});map(&mut wire,if duplicate_attachment{&[]}else{&duplicate});assert_eq!(ImMessage::decode(&wire).err(),Some(CodecError::DuplicateKey));}
}
#[test]
fn native_im_online_moremessage_requires_separate_exact_kind_and_original_box_header(){
    let wire=one(7,8);let headers=[RpcProperty{key:b"Box",value:b"Org/1+2"},RpcProperty{key:b"Group.UpdateTime",value:b"opaque-time"}];let message=decode_more_message(MORE_MESSAGE_KIND,&headers,&wire).unwrap().unwrap();assert_eq!(message.box_id.bytes(),b"Org/1+2");assert_eq!(message.message.index,7);assert_eq!(format!("{message:?}"),"ImMoreMessage(<redacted>)");
    for kind in [b"Sip.Request".as_slice(),b"Message.moreMessage",b"Message.MoreMessage.extra",b"Status.Update"]{assert!(decode_more_message(kind,&[],b"malformed").unwrap().is_none());}assert!(decode_more_message(MORE_MESSAGE_KIND,&[],&wire).is_err());
}
#[test]
fn native_im_online_moremessage_rejects_duplicate_box_and_invalid_outer_properties(){
    let wire=one(1,2);let box_id=RpcProperty{key:b"Box",value:b"P2P/1_2"};assert_eq!(decode_more_message(MORE_MESSAGE_KIND,&[box_id,box_id],&wire).err(),Some(CodecError::DuplicateKey));assert_eq!(decode_more_message(MORE_MESSAGE_KIND,&vec![box_id;129],&wire).err(),Some(CodecError::TooLarge));
}
#[test]
fn native_im_gap_request_and_setread_arguments_have_exact_original_method_version_and_signed64(){
    let box_id=ImBox::parse(b"P2P/01_2").unwrap();let mut expected=vec![0,0,1,0,0];bytes(&mut expected,b"P2P/01_2");expected.extend_from_slice(&(-1i64).to_be_bytes());assert_eq!(encode_set_read(box_id,-1).unwrap().expose_to_transport(),expected);expected.extend_from_slice(&10u32.to_be_bytes());assert_eq!(encode_gap_recv(box_id,-1,10).unwrap().expose_to_transport(),expected);assert_eq!(RECV_METHOD,"recv.Message.Message");assert_eq!(SET_READ_METHOD,"setRead.Message.Message");
}
#[test]
fn native_im_gap_page_limit_is_original_ten_and_range_arithmetic_cannot_wrap(){
    let box_id=ImBox::parse(b"Org/1-2").unwrap();for count in [0,11,u32::MAX]{assert!(encode_gap_recv(box_id,0,count).is_err());}assert!(encode_gap_recv(box_id,i64::MAX,2).is_err());assert!(encode_gap_recv(box_id,i64::MIN,10).is_ok());assert!(encode_gap_recv(box_id,i64::MAX,1).is_ok());
}
fn response(accepted:u8)->Vec<u8>{let mut wire=vec![0,accepted];wire.extend_from_slice(&2u32.to_be_bytes());wire.extend(record(-1,2,b"Text",b"first"));wire.extend(record(i64::MAX,-4,b"System.Text",b"second"));bytes(&mut wire,b"");wire}
#[test]
fn native_im_recv_result_is_bool_then_record_list_then_common_footer_not_sip_offline_cursor(){
    let wire=response(1);let result=decode_recv_result(0,&wire).unwrap();assert!(result.accepted);assert_eq!(result.messages.len(),2);assert_eq!(result.messages[0].index,-1);assert_eq!(result.messages[1].index,i64::MAX);assert_eq!(result.messages[1].kind,b"System.Text");assert!(result.footer.reason.is_empty());assert_eq!(format!("{result:?}"),"ImRecvResult(<redacted>)");
}
#[test]
fn native_im_false_recv_still_validates_list_footer_and_never_becomes_positive(){
    let wire=response(0);let result=decode_recv_result(0,&wire).unwrap();assert!(!result.accepted);assert_eq!(result.messages.len(),2);assert!(decode_recv_result(0,&wire[..wire.len()-1]).is_err());assert_eq!(decode_recv_result(1,&wire).err(),Some(CodecError::RemoteFailure));
}
#[test]
fn native_im_recv_all_truncations_huge_counts_and_duplicate_footer_fail_closed(){
    let wire=response(1);for end in 0..wire.len(){assert!(decode_recv_result(0,&wire[..end]).is_err());}let mut huge=vec![0,1];huge.extend_from_slice(&u32::MAX.to_be_bytes());assert_eq!(decode_recv_result(0,&huge).err(),Some(CodecError::TooLarge));let mut duplicate=wire;map(&mut duplicate,&[(b"same",b"one"),(b"same",b"two")]);assert_eq!(decode_recv_result(0,&duplicate).err(),Some(CodecError::DuplicateKey));
}
#[test]
fn native_im_setread_result_footer_and_remote_error_are_distinct_from_decode_success(){
    for flag in [1,7,255]{let wire=[0,flag,0,0,0,0];assert!(decode_set_read_result(0,&wire).unwrap().accepted);}let mut refused=vec![0,1];bytes(&mut refused,b"private-server-reason");let result=decode_set_read_result(0,&refused).unwrap();assert!(!result.accepted);assert_eq!(result.footer.reason,b"private-server-reason");assert_eq!(format!("{result:?}"),"ImSetReadResult(<redacted>)");assert!(!decode_set_read_result(0,&[0;6]).unwrap().accepted);assert_eq!(decode_set_read_result(42,&[0,1,0,0,0,0]).err(),Some(CodecError::RemoteFailure));
}
#[test]
fn native_im_setread_every_truncation_and_extra_byte_fail_instead_of_acknowledging(){
    let bytes=[0,1,0,0,0,0];for end in 0..bytes.len(){assert!(decode_set_read_result(0,&bytes[..end]).is_err());}assert!(decode_set_read_result(0,&[0,1,0,0,0,0,1]).is_err());
}
