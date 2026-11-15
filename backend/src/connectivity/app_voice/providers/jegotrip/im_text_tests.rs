//! Actions-only structured IM Text JSON. No message consumption or IO grant.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::im_text::{ImText,TextField};
fn decode(kind:&[u8],content:&[u8])->Result<Option<ImText>,CodecError>{
    ImText::decode(&ImMessage{index:1,time:2,sender_uid:b"1_2",kind,content,attachments:vec![],parameters:vec![]})
}
#[test]
fn native_im_text_uses_original_json_text_key_not_raw_record_content(){
    let data=br#"{"text":"fixture text","displayName":"fixture display"}"#;let text=decode(b"Text",data).unwrap().unwrap();assert_eq!(text.text(),"fixture text");assert_eq!(text.field(TextField::DisplayName),Some("fixture display"));assert_ne!(text.text().as_bytes(),data);
}
#[test]
fn native_im_text_preserves_five_original_metadata_strings_without_turning_online_into_readiness(){
    let text=decode(b"Text",br#"{"text":"","imdnId":"opaque-id","csnId":"opaque-conversation","displayName":"name","userData":"opaque-user-data","OnlineMsg":"true"}"#).unwrap().unwrap();
    assert_eq!(text.text(),"");assert_eq!(text.field(TextField::ImdnId),Some("opaque-id"));assert_eq!(text.field(TextField::ConversationId),Some("opaque-conversation"));assert_eq!(text.field(TextField::UserData),Some("opaque-user-data"));assert_eq!(text.field(TextField::OnlineMessage),Some("true"));assert_eq!(format!("{text:?}"),"ImText(<redacted>)");
}
#[test]
fn native_im_text_and_system_text_share_content_shape_other_types_are_not_reinterpreted(){
    assert_eq!(decode(b"System.Text",br#"{"text":"system"}"#).unwrap().unwrap().text(),"system");
    for kind in [b"text".as_slice(),b"Info.Text",b"Image",b"System.Info.Text",b"Text.extra"]{assert!(decode(kind,b"not-json").unwrap().is_none());}
}
#[test]
fn native_im_text_json_unicode_escapes_brackets_quotes_and_newlines_preserve_actual_text(){
    let text=decode(b"Text",r#"{"text":"中文\n\"[{}]\" \uD83E\uDD58","te\u0073t":{"ignored":"value"}}"#.as_bytes()).unwrap().unwrap();assert_eq!(text.text(),"中文\n\"[{}]\" 🥘");
    assert_eq!(decode(b"Text",br#"{"te\u0078t":"escaped key"}"#).unwrap().unwrap().text(),"escaped key");
}
#[test]
fn native_im_text_missing_text_wrong_root_or_known_nonstring_cannot_fake_empty_message(){
    for data in [b"".as_slice(),br#"{}"#,br#"[]"#,br#""raw text""#,br#"null"#,br#"{"Text":"wrong key"}"#,br#"{"text":false}"#,br#"{"text":1}"#,br#"{"text":null}"#,br#"{"text":"ok","OnlineMsg":true}"#]{assert!(decode(b"Text",data).is_err());}
}
#[test]
fn native_im_text_duplicate_known_escaped_or_unknown_keys_are_rejected_not_overwritten(){
    for data in [br#"{"text":"one","text":"two"}"#.as_slice(),br#"{"text":"one","te\u0078t":"two"}"#,br#"{"text":"one","opaque":1,"opaque":2}"#]{assert!(decode(b"Text",data).is_err());}
}
#[test]
fn native_im_text_unknown_json_data_is_ignored_but_never_rendered_or_promoted(){
    let text=decode(b"Text",br#"{"text":"only this","token":"private-token","grant":true,"endpoint":{"host":"private-route"},"nested":[1,null,false]}"#).unwrap().unwrap();assert_eq!(text.text(),"only this");
    for field in [TextField::ImdnId,TextField::ConversationId,TextField::DisplayName,TextField::UserData,TextField::OnlineMessage]{assert!(text.field(field).is_none());}let debug=format!("{text:?}");assert!(!debug.contains("private-token"));assert!(!debug.contains("private-route"));
}
#[test]
fn native_im_text_embedded_nul_invalid_unicode_truncation_and_trailing_values_are_rejected(){
    for data in [br#"{"text":"a\u0000b"}"#.as_slice(),br#"{"text":"\uD800"}"#,br#"{"text":"ok"}{}"#,br#"{"text":"open"#, &[b'{',b'"',255,b'"',b':',b'1',b'}']]{assert!(decode(b"Text",data).is_err());}
    let data=br#"{"text":"fixture","OnlineMsg":"1"}"#;for end in 0..data.len(){assert!(decode(b"Text",&data[..end]).is_err());}
}
#[test]
fn native_im_text_total_bytes_and_unknown_field_count_are_bounded_before_semantic_use(){
    let large=format!("{{\"text\":\"{}\"}}","a".repeat(super::super::super::super::rpc_stream::MAX_FIELD_BYTES));assert_eq!(decode(b"Text",large.as_bytes()).err(),Some(CodecError::TooLarge));
    for(count,accepted)in[(127,true),(128,false)]{let mut content=String::from("{\"text\":\"bounded\"");for n in 0..count{content.push_str(&format!(",\"unknown{n}\":0"));}content.push('}');assert_eq!(decode(b"Text",content.as_bytes()).is_ok(),accepted);}
}
#[test]
fn native_im_text_unknown_nesting_is_bounded_before_ignored_any_and_string_braces_do_not_count(){
    for(depth,accepted)in[(31,true),(32,false),(1000,false)]{let content=format!("{{\"text\":\"bounded\",\"unknown\":{}0{}}}","[".repeat(depth),"]".repeat(depth));assert_eq!(decode(b"Text",content.as_bytes()).is_ok(),accepted);}
    let content=format!("{{\"text\":\"{}\"}}","[{}]".repeat(1000));assert!(decode(b"Text",content.as_bytes()).is_ok());
}
