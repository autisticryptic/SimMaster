//! Pure source-derived DATA fixtures only. No SDK, filesystem, transport,
//! notification completion, send permission or R01-R14 signoff is exercised.
//! Independent byte builders below intentionally do not use Writer.
#[path="im_send_version_data_tests.rs"]
mod version_errors;
#[path="im_send_option_data_tests.rs"]
mod options;
use crate::connectivity::app_voice::providers::jegotrip::{
    im_data::{ImBox,ImMessage},
    im_send_data::{encode_user_send,decode_user_send_result,OutgoingMessage,SendKind,USER_SEND_METHOD,MAX_ATTACHMENT_BYTES,MAX_INFO_SUBTYPE_BYTES},
    rpc_stream::{CodecError,Reader,RpcProperty,MAX_FIELD_BYTES,MAX_MAP_ENTRIES,MAX_STREAM_BYTES},
};
use zeroize::Zeroizing;
const ACCOUNT:&[u8]=b"[uid:7_9@example.invalid]";
fn box_id()->ImBox<'static>{ImBox::parse(b"P2P/1_2").unwrap()}
fn text(content:&[u8])->OutgoingMessage<'_>{OutgoingMessage{kind:SendKind::Text,content,attachments:&[],parameters:&[]}}
fn bytes(out:&mut Vec<u8>,value:&[u8]){out.extend_from_slice(&(value.len()as u32).to_be_bytes());out.extend_from_slice(value);}
fn map(out:&mut Vec<u8>,fields:&[(&[u8],&[u8])]){out.extend_from_slice(&(fields.len()as u32).to_be_bytes());for(key,value)in fields{bytes(out,key);bytes(out,value);}}
fn request(kind:&[u8],json:&[u8],attachments:&[(&[u8],&[u8])],parameters:&[(&[u8],&[u8])],notify:&[(&[u8],&[u8])])->Zeroizing<Vec<u8>>{
    let mut out=Zeroizing::new(Vec::with_capacity(MAX_STREAM_BYTES));out.extend_from_slice(&[0,0,1,0,0]);bytes(&mut out,b"P2P/1_2");bytes(&mut out,kind);bytes(&mut out,json);map(&mut out,attachments);map(&mut out,parameters);map(&mut out,notify);out
}
fn result(flag:u8,id:i64,time:i64,reason:&[u8])->Zeroizing<Vec<u8>>{
    let mut out=Zeroizing::new(Vec::with_capacity(MAX_STREAM_BYTES));out.extend_from_slice(&[0,flag]);out.extend_from_slice(&id.to_be_bytes());out.extend_from_slice(&time.to_be_bytes());bytes(&mut out,reason);out
}
struct RequestShape<'a>{box_bytes:&'a[u8],kind:&'a[u8],json:&'a[u8],attachments:Vec<RpcProperty<'a>>,parameters:Vec<RpcProperty<'a>>,notify:Vec<RpcProperty<'a>>}
fn shape(wire:&[u8])->Result<RequestShape<'_>,CodecError>{
    let mut r=Reader::versioned(wire)?;let box_bytes=r.bytes()?;let kind=r.bytes()?;let json=r.bytes()?;let attachments=r.properties()?;let parameters=r.properties()?;let notify=r.properties()?;r.finish()?;Ok(RequestShape{box_bytes,kind,json,attachments,parameters,notify})
}
fn keys<'a>(fields:&[RpcProperty<'a>])->Vec<&'a[u8]>{fields.iter().map(|p|p.key).collect()}

#[test]
fn exact_native_text_request_has_version_box_outgoing_struct_account_and_separate_default_notify(){
    let attachments=[RpcProperty{key:b"localfile",value:&[0xff,0,7]}];
    let parameters=[RpcProperty{key:b"z",value:b"opaque"}];
    let message=OutgoingMessage{kind:SendKind::Text,content:b"hello",attachments:&attachments,parameters:&parameters};
    let encoded=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();
    let expected=request(b"Text",b"{\"text\":\"hello\"}",&[(b"localfile",&[0xff,0,7])],&[(b"z",b"opaque"),(b"account",ACCOUNT)],&[(b"Notify.Text",b"hello")]);
    assert_eq!(encoded.expose_to_transport(),expected.as_slice());
    assert_eq!(&encoded.expose_to_transport()[..5],&[0,0,1,0,0]);
    assert_eq!(USER_SEND_METHOD,"userSend.Message.Message");
}

#[test]
fn outgoing_shape_never_contains_received_index_time_or_sender(){
    let encoded=encode_user_send(box_id(),&text(b"payload"),ACCOUNT,Some(&[])).unwrap();
    let parsed=shape(encoded.expose_to_transport()).unwrap();assert_eq!(parsed.box_bytes,b"P2P/1_2");assert_eq!(parsed.kind,b"Text");assert_eq!(parsed.json,b"{\"text\":\"payload\"}");assert!(parsed.attachments.is_empty());assert_eq!(parsed.parameters.len(),1);assert!(parsed.notify.is_empty());
    // A standalone received stream would need index,time,sender BEFORE kind.
    let mut outgoing_only=Zeroizing::new(Vec::with_capacity(MAX_STREAM_BYTES));outgoing_only.push(0);outgoing_only.extend_from_slice(&encoded.expose_to_transport()[16..]);
    assert!(ImMessage::decode(&outgoing_only).is_err());
    // Independent incoming fixture is valid DATA, but is not this request.
    let mut received=Zeroizing::new(Vec::with_capacity(MAX_STREAM_BYTES));received.push(0);received.extend_from_slice(&(-1i64).to_be_bytes());received.extend_from_slice(&2i64.to_be_bytes());bytes(&mut received,b"7_9");bytes(&mut received,b"Text");bytes(&mut received,b"{\"text\":\"payload\"}");map(&mut received,&[]);map(&mut received,&[]);
    assert!(ImMessage::decode(&received).is_ok());assert_ne!(received.as_slice(),encoded.expose_to_transport());
}

#[test]
fn text_json_uses_serde_escaping_and_default_notification_preserves_original_utf8(){
    let content="quote\" slash\\ line\nreturn\rtab\t\u{8}\u{c}\u{1} 文本";
    let encoded=encode_user_send(box_id(),&text(content.as_bytes()),ACCOUNT,None).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();
    assert_eq!(parsed.json,"{\"text\":\"quote\\\" slash\\\\ line\\nreturn\\rtab\\t\\b\\f\\u0001 文本\"}".as_bytes());
    assert_eq!(parsed.notify,[RpcProperty{key:b"Notify.Text",value:content.as_bytes()}]);
}

#[test]
fn info_is_explicit_unprefixed_ascii_component_with_content_json_and_no_text_default(){
    for subtype in [b"Subtype".as_slice(),b"sub_type-1.v2"]{
        let message=OutgoingMessage{kind:SendKind::Info(subtype),content:b"a\"\nb",attachments:&[],parameters:&[]};
        let encoded=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();
        let mut expected_kind=Zeroizing::new(Vec::with_capacity(5+MAX_INFO_SUBTYPE_BYTES));expected_kind.extend_from_slice(b"Info.");expected_kind.extend_from_slice(subtype);assert_eq!(parsed.kind,expected_kind.as_slice());assert_eq!(parsed.json,b"{\"content\":\"a\\\"\\nb\"}");assert!(parsed.notify.is_empty());
    }
    for subtype in [b"".as_slice(),b".sub",b"sub.",b".",b"..",b"a..b",b"Info.Subtype",b"Info.",b"a/b",b"a\\b",b"a:b",b"a b",b"a\0",b"a\n",&[0xff],"非ASCII".as_bytes()]{let message=OutgoingMessage{kind:SendKind::Info(subtype),content:b"x",attachments:&[],parameters:&[]};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::InvalidField));}
    let subtype=Zeroizing::new(vec![b'a';64]);let message=OutgoingMessage{kind:SendKind::Info(&subtype),content:b"",attachments:&[],parameters:&[]};assert!(encode_user_send(box_id(),&message,ACCOUNT,None).is_ok());let large=Zeroizing::new(vec![b'a';65]);let message=OutgoingMessage{kind:SendKind::Info(&large),content:b"",attachments:&[],parameters:&[]};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::TooLarge));
}

#[test]
fn mandatory_account_is_exact_supplied_data_never_derived_from_mainaccount_or_caller_parameters(){
    let caller=[RpcProperty{key:b"mainAccount",value:b"caller-not-own-uri"}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&caller};
    for own in [ACCOUNT,b"[username:own_session@example.invalid]".as_slice()]{let encoded=encode_user_send(box_id(),&message,own,Some(&[])).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();assert_eq!(parsed.parameters.iter().find(|p|p.key==b"account").unwrap().value,own);assert_eq!(parsed.parameters.iter().find(|p|p.key==b"mainAccount").unwrap().value,b"caller-not-own-uri");}
    let collision=[RpcProperty{key:b"account",value:ACCOUNT}];let collision_message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&collision};assert_eq!(encode_user_send(box_id(),&collision_message,ACCOUNT,None).err(),Some(CodecError::DuplicateKey));
    for own in [b"".as_slice(),b"a\0b",b"a\nb",b"a\rb",b"a\tb",&[0xff]]{assert_eq!(encode_user_send(box_id(),&message,own,None).err(),Some(CodecError::InvalidField));}
    let bound=Zeroizing::new(vec![b'a';MAX_FIELD_BYTES]);assert!(encode_user_send(box_id(),&text(b""),&bound,Some(&[])).is_ok());let large=Zeroizing::new(vec![b'a';MAX_FIELD_BYTES+1]);assert_eq!(encode_user_send(box_id(),&message,&large,None).err(),Some(CodecError::TooLarge));
    // Accepted byte bounds above do NOT attest that an input is SDK-owned.
}

#[test]
fn caller_content_and_escaped_json_have_distinct_field_bounds_and_nul_is_not_escaped_into_acceptance(){
    for content in [b"a\0b".as_slice(),&[0xff,0xfe]]{assert_eq!(encode_user_send(box_id(),&text(content),ACCOUNT,None).err(),Some(CodecError::InvalidField));}
    let content=Zeroizing::new(vec![b'a';MAX_FIELD_BYTES-11]);let encoded=encode_user_send(box_id(),&text(&content),ACCOUNT,Some(&[])).unwrap();assert_eq!(shape(encoded.expose_to_transport()).unwrap().json.len(),MAX_FIELD_BYTES);
    for len in [MAX_FIELD_BYTES-10,MAX_FIELD_BYTES,MAX_FIELD_BYTES+1]{let content=Zeroizing::new(vec![b'a';len]);assert_eq!(encode_user_send(box_id(),&text(&content),ACCOUNT,Some(&[])).err(),Some(CodecError::TooLarge));}
    let escaped=Zeroizing::new(vec![1u8;MAX_FIELD_BYTES/6]);assert_eq!(encode_user_send(box_id(),&text(&escaped),ACCOUNT,Some(&[])).err(),Some(CodecError::TooLarge));
    assert!(encode_user_send(box_id(),&text(b"\n\r\t"),ACCOUNT,None).is_ok());
}

#[test]
fn attachments_are_explicit_binary_bytes_of_one_through_10240_without_path_operations(){
    for len in [1,MAX_ATTACHMENT_BYTES]{let data=Zeroizing::new(vec![0xff;len]);let fields=[RpcProperty{key:b"localfile",value:&data}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&fields,parameters:&[]};let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&[])).unwrap();assert_eq!(shape(encoded.expose_to_transport()).unwrap().attachments[0].value,data.as_slice());}
    for(len,error)in[(0,CodecError::InvalidField),(MAX_ATTACHMENT_BYTES+1,CodecError::TooLarge)]{let data=Zeroizing::new(vec![0;len]);let fields=[RpcProperty{key:b"localfile",value:&data}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&fields,parameters:&[]};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(error));}
    // Metadata looking like a path is still only metadata; no file is opened.
    let fields=[RpcProperty{key:b"../fixture.bin",value:&[0,0xff,7]}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&fields,parameters:&[]};assert!(encode_user_send(box_id(),&message,ACCOUNT,Some(&[])).is_ok());
}

#[test]
fn all_three_maps_preserve_native_byte_length_first_then_equal_length_byte_order(){
    let attachments=[RpcProperty{key:b"aa",value:b"3"},RpcProperty{key:b"z",value:b"2"},RpcProperty{key:b"a",value:b"1"}];
    let parameters=[RpcProperty{key:b"zzzzzzzz",value:b"4"},RpcProperty{key:b"aa",value:b"3"},RpcProperty{key:b"z",value:b"2"},RpcProperty{key:b"a",value:b"1"}];
    let notify=[RpcProperty{key:b"Notify.AA",value:b"3"},RpcProperty{key:b"Notify.Z",value:b"2"},RpcProperty{key:b"Notify.A",value:b"1"}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&attachments,parameters:&parameters};let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&notify)).unwrap();
    let expected=request(b"Text",b"{\"text\":\"\"}",&[(b"a",b"1"),(b"z",b"2"),(b"aa",b"3")],&[(b"a",b"1"),(b"z",b"2"),(b"aa",b"3"),(b"account",ACCOUNT),(b"zzzzzzzz",b"4")],&[(b"Notify.A",b"1"),(b"Notify.Z",b"2"),(b"Notify.AA",b"3")]);assert_eq!(encoded.expose_to_transport(),expected.as_slice());
    let parsed=shape(encoded.expose_to_transport()).unwrap();assert_eq!(keys(&parsed.attachments),vec![b"a".as_slice(),b"z",b"aa"]);assert_eq!(keys(&parsed.parameters),vec![b"a".as_slice(),b"z",b"aa",b"account",b"zzzzzzzz"]);assert_eq!(keys(&parsed.notify),vec![b"Notify.A".as_slice(),b"Notify.Z",b"Notify.AA"]);
}

#[test]
fn duplicate_keys_in_each_caller_map_and_invalid_string_fields_are_rejected(){
    let duplicate=[RpcProperty{key:b"same",value:b"one"},RpcProperty{key:b"same",value:b"two"}];
    for attachments in [true,false]{let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:if attachments{&duplicate}else{&[]},parameters:if attachments{&[]}else{&duplicate}};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::DuplicateKey));}
    let notify=[RpcProperty{key:b"Notify.Text",value:b"one"},RpcProperty{key:b"Notify.Text",value:b"two"}];assert_eq!(encode_user_send(box_id(),&text(b""),ACCOUNT,Some(&notify)).err(),Some(CodecError::DuplicateKey));
    for bad_key in [b"".as_slice(),b"a\0b",b"a\nb",&[0xff]]{let fields=[RpcProperty{key:bad_key,value:b"x"}];for attachments in [true,false]{let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:if attachments{&fields}else{&[]},parameters:if attachments{&[]}else{&fields}};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::InvalidField));}}
    for value in [b"a\0b".as_slice(),&[0xff]]{let fields=[RpcProperty{key:b"opaque",value}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&fields};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::InvalidField));}
    let large=Zeroizing::new(vec![b'a';MAX_FIELD_BYTES+1]);for field in [RpcProperty{key:&large,value:b"x"},RpcProperty{key:b"k",value:&large}]{let fields=[field];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&fields};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,None).err(),Some(CodecError::TooLarge));}
}

#[test]
fn map_count_128_includes_the_mandatory_account_and_never_allows_129(){
    let names:Vec<_>=(0..=MAX_MAP_ENTRIES).map(|i|Zeroizing::new(format!("Notify.K{i}"))).collect();let fields:Vec<_>=names.iter().map(|name|RpcProperty{key:name.as_bytes(),value:b"x"}).collect();
    for(count,ok)in[(MAX_MAP_ENTRIES,true),(MAX_MAP_ENTRIES+1,false)]{let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&fields[..count],parameters:&[]};let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&[]));assert_eq!(encoded.is_ok(),ok);if !ok{assert_eq!(encoded.err(),Some(CodecError::TooLarge));}let encoded=encode_user_send(box_id(),&text(b""),ACCOUNT,Some(&fields[..count]));assert_eq!(encoded.is_ok(),ok);if !ok{assert_eq!(encoded.err(),Some(CodecError::TooLarge));}}
    for(count,ok)in[(MAX_MAP_ENTRIES-1,true),(MAX_MAP_ENTRIES,false)]{let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&fields[..count]};let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&[]));assert_eq!(encoded.is_ok(),ok);match encoded{Ok(encoded)=>assert_eq!(shape(encoded.expose_to_transport()).unwrap().parameters.len(),MAX_MAP_ENTRIES),Err(error)=>assert_eq!(error,CodecError::TooLarge)}}
}

#[test]
fn entire_stream_exact_128k_limit_counts_headers_account_and_notification_map(){
    let prototype=request(b"Text",b"{\"text\":\"\"}",&[],&[(b"a",b""),(b"b",b""),(b"account",ACCOUNT)],&[]);
    let first=Zeroizing::new(vec![b'a';MAX_FIELD_BYTES]);let remaining=MAX_STREAM_BYTES-prototype.len()-first.len();assert!(remaining<MAX_FIELD_BYTES);
    let mut second=Zeroizing::new(Vec::with_capacity(MAX_FIELD_BYTES));second.resize(remaining,b'b');
    {let parameters=[RpcProperty{key:b"a",value:&first},RpcProperty{key:b"b",value:&second}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&parameters};let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&[])).unwrap();assert_eq!(encoded.expose_to_transport().len(),MAX_STREAM_BYTES);}
    second.push(b'b');let parameters=[RpcProperty{key:b"a",value:&first},RpcProperty{key:b"b",value:&second}];let message=OutgoingMessage{kind:SendKind::Text,content:b"",attachments:&[],parameters:&parameters};assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,Some(&[])).err(),Some(CodecError::TooLarge));
    let oversized=Zeroizing::new(vec![0;MAX_STREAM_BYTES+1]);assert_eq!(decode_user_send_result(0,&oversized).err(),Some(CodecError::TooLarge));
}

#[test]
fn notification_option_is_explicit_exact_override_and_only_notify_components_are_supported(){
    let message=text(b"original\ntext");let default=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();assert_eq!(shape(default.expose_to_transport()).unwrap().notify,[RpcProperty{key:b"Notify.Text",value:b"original\ntext"}]);let empty=encode_user_send(box_id(),&message,ACCOUNT,Some(&[])).unwrap();assert!(shape(empty.expose_to_transport()).unwrap().notify.is_empty());
    let explicit=[RpcProperty{key:b"Notify.Text",value:b"explicit"},RpcProperty{key:b"Notify.Token",value:b"opaque-not-a-grant"}];let encoded=encode_user_send(box_id(),&message,ACCOUNT,Some(&explicit)).unwrap();assert_eq!(shape(encoded.expose_to_transport()).unwrap().notify,explicit);
    let info=OutgoingMessage{kind:SendKind::Info(b"Subtype"),content:b"info",attachments:&[],parameters:&[]};let encoded=encode_user_send(box_id(),&info,ACCOUNT,Some(&explicit)).unwrap();assert_eq!(shape(encoded.expose_to_transport()).unwrap().notify,explicit);
    for name in [b"Text".as_slice(),b"notify.Text",b"Notify.",b"Notify..Text",b"Notify.a/b",b"Notify.a\\b",b"Notify.a\0",b"endpoint",b"token",b"grant"]{let fields=[RpcProperty{key:name,value:b"opaque"}];assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,Some(&fields)).err(),Some(CodecError::InvalidField));}
    let large=Zeroizing::new(vec![b'a';MAX_FIELD_BYTES]);let fields=[RpcProperty{key:b"Notify.Text",value:&large}];assert!(encode_user_send(box_id(),&text(b""),ACCOUNT,Some(&fields)).is_ok());let invalid=[RpcProperty{key:b"Notify.Text",value:b"a\0b"}];assert_eq!(encode_user_send(box_id(),&message,ACCOUNT,Some(&invalid)).err(),Some(CodecError::InvalidField));
}

#[test]
fn original_statistics_namespace_is_data_in_notification_map_not_notify_or_auth_override(){let statistics=[RpcProperty{key:b"Statistics.AppId",value:b"original-app"},RpcProperty{key:b"Statistics.DomainId",value:b"original-domain"},RpcProperty{key:b"Statistics.CountryCode",value:b""},RpcProperty{key:b"Notify.Text",value:b"text"}];let encoded=encode_user_send(box_id(),&text(b"text"),ACCOUNT,Some(&statistics)).unwrap();let parsed=shape(encoded.expose_to_transport()).unwrap();assert_eq!(parsed.parameters.iter().find(|p|p.key==b"account").unwrap().value,ACCOUNT);assert_eq!(parsed.notify.iter().find(|p|p.key==b"Statistics.AppId").unwrap().value,b"original-app");assert!(!parsed.notify.iter().any(|p|p.key==b"Notify.AppId"));for key in [b"Statistics.".as_slice(),b"statistics.AppId",b"Statistics..AppId"]{let fields=[RpcProperty{key,value:b"data"}];assert!(encode_user_send(box_id(),&text(b"text"),ACCOUNT,Some(&fields)).is_err());}}
#[test]
fn exact_result_vector_is_bool_then_full_signed64_id_time_then_common_footer(){
    let literal=[0,1,0x80,0,0,0,0,0,0,0,0x7f,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0,0,0,0];let parsed=decode_user_send_result(0,&literal).unwrap();assert!(parsed.accepted);assert_eq!((parsed.msg_id,parsed.time),(i64::MIN,i64::MAX));assert!(parsed.footer.reason.is_empty());assert!(parsed.footer.properties.is_empty());
    for(id,time)in[(i64::MAX,i64::MIN),(-1,-1),(0,0),(0x1_2345_6789,-0x1_2345_6789)]{for flag in [1,7,255]{let wire=result(flag,id,time,b"");let parsed=decode_user_send_result(0,&wire).unwrap();assert!(parsed.accepted);assert_eq!((parsed.msg_id,parsed.time),(id,time));}}
}

#[test]
fn false_and_nonempty_error_reason_are_negative_data_and_remote_code_is_rejected_first(){
    let false_wire=result(0,-1,i64::MIN,b"");let parsed=decode_user_send_result(0,&false_wire).unwrap();assert!(!parsed.accepted);assert_eq!((parsed.msg_id,parsed.time),(-1,i64::MIN));assert!(decode_user_send_result(0,&false_wire[..false_wire.len()-1]).is_err());
    for flag in [0,1,255]{let mut wire=result(flag,9,-4,b"private-server-reason");map(&mut wire,&[(b"token",b"opaque-no-grant")]);let parsed=decode_user_send_result(0,&wire).unwrap();assert!(!parsed.accepted);assert_eq!(parsed.footer.reason,b"private-server-reason");assert_eq!(parsed.footer.properties[0].value,b"opaque-no-grant");}
    let mut opaque=result(1,9,10,b"");map(&mut opaque,&[(b"token",b"opaque-no-grant")]);let parsed=decode_user_send_result(0,&opaque).unwrap();assert!(parsed.accepted);assert_eq!(parsed.footer.properties.len(),1); // NOT a grant
    for code in [1,42,65537,u32::MAX]{assert_eq!(decode_user_send_result(code,b"").err(),Some(CodecError::RemoteFailure));assert_eq!(decode_user_send_result(code,&false_wire).err(),Some(CodecError::RemoteFailure));}
}

#[test]
fn strict_truncation_invalid_tags_lengths_duplicates_and_trailing_bytes_fail_closed(){
    let wire=result(1,1,2,b"");for end in 0..wire.len(){assert!(decode_user_send_result(0,&wire[..end]).is_err(),"truncated result at {end}");}
    for marker in [1,2,255]{let mut bad=result(1,1,2,b"");bad[0]=marker;assert_eq!(decode_user_send_result(0,&bad).err(),Some(CodecError::UnsupportedStream));}
    let mut trailing=result(1,1,2,b"");trailing.push(1);assert!(decode_user_send_result(0,&trailing).is_err());let mut complete_map=result(1,1,2,b"");map(&mut complete_map,&[]);complete_map.push(0);assert_eq!(decode_user_send_result(0,&complete_map).err(),Some(CodecError::TrailingData));
    let mut duplicate=result(1,1,2,b"");map(&mut duplicate,&[(b"same",b"one"),(b"same",b"two")]);assert_eq!(decode_user_send_result(0,&duplicate).err(),Some(CodecError::DuplicateKey));
    let mut huge=result(1,1,2,b"");huge.extend_from_slice(&u32::MAX.to_be_bytes());assert_eq!(decode_user_send_result(0,&huge).err(),Some(CodecError::TooLarge));let mut missing=result(1,1,2,b"");missing.extend_from_slice(&1u32.to_be_bytes());assert_eq!(decode_user_send_result(0,&missing).err(),Some(CodecError::Truncated));let mut huge_reason=result(1,1,2,b"");huge_reason[18..22].copy_from_slice(&u32::MAX.to_be_bytes());assert_eq!(decode_user_send_result(0,&huge_reason).err(),Some(CodecError::TooLarge));
    let mut footer=result(1,1,2,b"");let base=footer.len();map(&mut footer,&[(b"k",b"value")]);for end in base+1..footer.len(){assert!(decode_user_send_result(0,&footer[..end]).is_err());}assert!(decode_user_send_result(0,&footer).is_ok());
    let encoded=encode_user_send(box_id(),&text(b"x"),ACCOUNT,Some(&[])).unwrap();for end in 0..encoded.expose_to_transport().len(){assert!(shape(&encoded.expose_to_transport()[..end]).is_err());}
    for prefix in [[2,0,1,0,0],[0,0,2,0,0],[0,0,1,0,1]]{assert!(shape(&prefix).is_err());}
}

#[test]
fn debug_is_always_redacted_for_kinds_content_attachments_parameters_account_and_result(){
    let attachments=[RpcProperty{key:b"secret-file",value:b"secret-attachment"}];let parameters=[RpcProperty{key:b"secret-param",value:b"secret-value"}];let message=OutgoingMessage{kind:SendKind::Info(b"secret-subtype"),content:b"secret-content",attachments:&attachments,parameters:&parameters};let encoded=encode_user_send(box_id(),&message,ACCOUNT,None).unwrap();let mut wire=result(1,0x1_2345_6789,-123456789,b"secret-reason");map(&mut wire,&[(b"token",b"secret-token")]);let parsed=decode_user_send_result(0,&wire).unwrap();
    assert_eq!(format!("{:?}",SendKind::Text),"ImSendKind(<redacted>)");assert_eq!(format!("{:?}",message.kind),"ImSendKind(<redacted>)");assert_eq!(format!("{message:?}"),"ImOutgoingMessage(<redacted>)");assert_eq!(format!("{parsed:?}"),"ImUserSendResult(<redacted>)");
    let debug=Zeroizing::new(format!("{:?} {message:?} {encoded:?} {parsed:?} {:?} {:?} {:?}",message.kind,parsed.footer,attachments[0],parameters[0]));for secret in ["secret-","example.invalid","7_9","123456789","4886718345"]{assert!(!debug.contains(secret));}assert!(debug.contains("<redacted>"));
}
