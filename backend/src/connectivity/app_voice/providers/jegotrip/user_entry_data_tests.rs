//! Actions-only source-derived resolver DATA checks, no SDK or network execute.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::user_entry_data as d;
#[test]
fn resolver_uid_grammar_is_original_digits_underscore_nonzero_suffix(){
    for uid in [b"0_1".as_slice(),b"00_1",b"1_999"]{assert!(d::valid_uid(uid));}
    for uid in [b"1_0".as_slice(),b"1_01",b"_1",b"1_",b"1_1_1",b"user",b"1-2"]{assert!(!d::valid_uid(uid));}
}
#[test]
fn resolver_uri_retains_explicit_aid_kind_value_and_authority_without_routing(){
    for(raw,kind,value)in[(b"[phone:+123@example.invalid]".as_slice(),d::AidKind::Phone,b"+123".as_slice()),(b"[username:alice+1@example.invalid]",d::AidKind::Username,b"alice+1"),(b"[email:a@b.invalid@example.invalid]",d::AidKind::Email,b"a@b.invalid")]{let uri=d::RecipientUri::parse(raw).unwrap();assert_eq!(uri.kind(),kind);assert_eq!(uri.value(),value);assert_eq!(uri.authority(),Some(b"example.invalid".as_slice()));}
}
#[test]
fn resolver_uid_and_bracketed_uid_are_data_not_a_phone_or_group_label(){
    for raw in [b"1_2".as_slice(),b"[uid:1_2]"]{let uri=d::RecipientUri::parse(raw).unwrap();assert_eq!(uri.kind(),d::AidKind::Uid);assert_eq!(uri.value(),b"1_2");assert_eq!(uri.authority(),None);}
}
#[test]
fn resolver_bare_alias_url_missing_realm_and_invalid_alias_are_not_guessed(){
    for raw in [b"alice".as_slice(),b"+123456",b"a@b.invalid",b"scheme://example.invalid",b"[phone:+123]",b"[username:.alice@example.invalid]",b"[email:a@b@example.invalid]",b"[phone:12@example.invalid]",b"[uid:1_0]",b"[other:value@example.invalid]"]{assert!(d::RecipientUri::parse(raw).is_err());}
}
#[test]
fn resolver_pairset_uses_length_first_type_then_value_order_not_lexicographic(){
    let inputs=[d::Aid::original(d::AidKind::Phone,b"1234").unwrap(),d::Aid::original(d::AidKind::Uid,b"1_2").unwrap(),d::Aid::original(d::AidKind::Phone,b"123").unwrap()];let bytes=d::encode_aid_to_uid(&inputs).unwrap();let mut r=crate::connectivity::app_voice::providers::jegotrip::rpc_stream::Reader::versioned(bytes.expose_to_transport()).unwrap();assert_eq!(r.u32().unwrap(),3);for(kind,value)in[(b"uid".as_slice(),b"1_2".as_slice()),(b"phone",b"123"),(b"phone",b"1234")]{assert_eq!(r.bytes().unwrap(),kind);assert_eq!(r.bytes().unwrap(),value);}r.finish().unwrap();assert_eq!(d::AID_TO_UID_METHOD,"aidToUid.UserEntry.User");
}
#[test]
fn resolver_duplicate_pair_and_oversized_request_fail_before_any_request_id(){
    let aid=d::Aid::original(d::AidKind::Phone,b"123").unwrap();assert!(d::encode_aid_to_uid(&[aid,aid]).is_err());assert!(d::encode_aid_to_uid(&[]).is_err());assert!(d::encode_aid_to_uid(&vec![aid;129]).is_err());assert!(d::RecipientUri::parse(&vec![b'x';513]).is_err());
}
fn result(accepted:bool,uid:&[u8],kind:&[u8],value:&[u8])->Vec<u8>{let mut w=crate::connectivity::app_voice::providers::jegotrip::rpc_stream::Writer::new();w.boolean(accepted).unwrap();w.u32(1).unwrap();w.bytes(uid).unwrap();w.bytes(kind).unwrap();w.bytes(value).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
#[test]
fn resolver_reply_map_key_is_resolved_uid_and_inner_pair_is_original_aid(){
    let raw=result(true,b"7_9",b"phone",b"123");let reply=d::decode_aid_to_uid(0,&raw).unwrap();assert!(reply.accepted);assert_eq!(reply.entries[0].uid,b"7_9");assert_eq!(reply.entries[0].aid.kind(),d::AidKind::Phone);assert_eq!(reply.entries[0].aid.value(),b"123");assert!(reply.footer.reason.is_empty());
}
#[test]
fn resolver_reply_false_version_truncation_and_trailing_bytes_never_become_uid_grants(){
    let raw=result(false,b"7_9",b"phone",b"123");assert!(!d::decode_aid_to_uid(0,&raw).unwrap().accepted);assert!(d::decode_aid_to_uid(65537,&raw).is_err());for n in 0..raw.len(){assert!(d::decode_aid_to_uid(0,&raw[..n]).is_err());}let mut trailing=raw.clone();trailing.push(9);assert!(d::decode_aid_to_uid(0,&trailing).is_err());
}
#[test]
fn resolver_invalid_returned_uid_or_original_aid_is_not_silently_dropped(){
    for(uid,kind,value)in[(b"7_0".as_slice(),b"phone".as_slice(),b"123".as_slice()),(b"7_9",b"unknown",b"123"),(b"7_9",b"phone",b"x23")]{assert!(d::decode_aid_to_uid(0,&result(true,uid,kind,value)).is_err());}
}
#[test]
fn resolver_debug_never_exports_aid_value_uid_or_realm(){
    let uri=d::RecipientUri::parse(b"[username:private_user@example.invalid]").unwrap();let aid=uri.aid();let raw=result(true,b"7_9",b"username",b"private_user");let reply=d::decode_aid_to_uid(0,&raw).unwrap();let debug=format!("{uri:?} {aid:?} {reply:?} {:?}",reply.entries[0]);for secret in ["private_user","7_9","example.invalid"]{assert!(!debug.contains(secret));}
}
