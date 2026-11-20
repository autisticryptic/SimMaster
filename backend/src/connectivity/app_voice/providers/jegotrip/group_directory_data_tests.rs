//! Actions-only original Group directory DATA; no SDK/device/permission.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{group_directory_data as g,rpc_stream::{Reader,Writer,RpcProperty,CodecError,MAX_MAP_ENTRIES}};
fn relation(w:&mut Writer,label:&[u8],flags:u32,first:&[u8],second:&[u8]){w.bytes(label).unwrap();w.u32(flags).unwrap();w.bytes(first).unwrap();w.bytes(second).unwrap();w.properties(&[RpcProperty{key:b"private-property",value:b"private-value"}]).unwrap();}
fn relations(yes:bool,reported:bool,reason:&[u8])->Vec<u8>{let mut w=Writer::new();w.boolean(yes).unwrap();w.boolean(reported).unwrap();w.u32(1).unwrap();relation(&mut w,b"private-label",0xf1234567,b"private-first",b"private-second");w.u32(0).unwrap();w.u32(1).unwrap();w.bytes(b"private-name").unwrap();w.i64(i64::MIN).unwrap();w.bytes(reason).unwrap();w.finish().expose_to_transport().to_vec()}
fn properties(yes:bool,reason:&[u8])->Vec<u8>{let mut w=Writer::new();w.boolean(yes).unwrap();w.properties(&[RpcProperty{key:b"private-key",value:b"private-value"}]).unwrap();w.bytes(reason).unwrap();w.finish().expose_to_transport().to_vec()}
#[test]
fn group_directory_original_relations_request_is_version_one_zero_identifier_and_signed64_since(){
 let out=g::encode_relations(b"current-sdk-uid",i64::MAX).unwrap();let mut expected=vec![0,0,1,0,0];expected.extend_from_slice(&15u32.to_be_bytes());expected.extend_from_slice(b"current-sdk-uid");expected.extend_from_slice(&i64::MAX.to_be_bytes());assert_eq!(out.expose_to_transport(),expected);assert_eq!(g::RELATIONS_METHOD,"getRelations2.GroupServer.Group");assert_ne!(g::RELATIONS_METHOD,"refreshStatuses.GroupServer.Group");}
#[test]
fn group_directory_properties_request_preserves_original_empty_prefix_one_entry_set(){
 let out=g::encode_properties(g::GroupId::parse(b"1-2").unwrap()).unwrap();let mut r=Reader::versioned(out.expose_to_transport()).unwrap();assert_eq!(r.bytes().unwrap(),b"1-2");assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"");r.finish().unwrap();assert_eq!(g::PROPERTIES_METHOD,"getPropsPrefixs.GroupServer.Group");}
#[test]
fn group_directory_group_identifier_is_not_a_uid_box_phone_uri_endpoint_or_missing_id(){
 for good in [b"1-2".as_slice(),b"1+2",b"123-456"]{assert_eq!(g::GroupId::parse(good).unwrap().bytes(),good);}
 for bad in [b"".as_slice(),b"0-2",b"1-0",b"01-2",b"1-02",b"1_2",b"Org/1-2",b"P2P/1_2",b"[phone:+1000]",b"Group:tcp -h x -p 1;",b"1-2\0"]{assert!(g::GroupId::parse(bad).is_err());}}
#[test]
fn group_directory_relations_shape_rejects_missing_or_oversized_identifier_and_invalid_since(){
 assert!(g::encode_relations(b"",-1).is_err());assert!(g::encode_relations(b"uid",-2).is_err());assert!(g::encode_relations(&vec![b'a';65537],0).is_err());assert!(g::GroupId::parse(&vec![b'1';509]).is_err());assert!(g::encode_relations(b"uid",-1).is_ok());}
#[test]
fn group_directory_relations_result_preserves_opaque_flags_two_strings_names_and_time(){
 let bytes=relations(true,false,b"");let r=g::decode_relations(0,&bytes).unwrap();assert!(r.accepted);assert!(!r.reported);assert_eq!(r.first.len(),1);assert!(r.second.is_empty());assert_eq!(r.first[0].label,b"private-label");assert_eq!(r.first[0].flags,0xf1234567);assert_eq!(r.first[0].first,b"private-first");assert_eq!(r.first[0].second,b"private-second");assert_eq!(r.first[0].properties[0].value,b"private-value");assert_eq!(r.names,[b"private-name".as_slice()]);assert_eq!(r.time,i64::MIN);assert_eq!(g::record_count(&r).unwrap(),1);}
#[test]
fn group_directory_properties_result_is_bool_and_string_map_not_receipt_or_auth(){
 let bytes=properties(true,b"");let r=g::decode_properties(0,&bytes).unwrap();assert!(r.accepted);assert_eq!(r.properties.len(),1);assert_eq!(r.properties[0].key,b"private-key");assert_eq!(r.properties[0].value,b"private-value");assert!(r.footer.reason.is_empty());}
#[test]
fn group_directory_original_negative_and_nonempty_footer_do_not_become_positive(){
 for(yes,reason)in[(false,b"".as_slice()),(true,b"private-reason".as_slice())]{assert!(!g::decode_relations(0,&relations(yes,true,reason)).unwrap().accepted);assert!(!g::decode_properties(0,&properties(yes,reason)).unwrap().accepted);}}
#[test]
fn group_directory_nonzero_remote_code_and_version_refusal_are_not_normal_success(){
 for code in [1,65536,65537,u32::MAX]{assert_eq!(g::decode_relations(code,&relations(true,true,b"")).err(),Some(CodecError::RemoteFailure));assert_eq!(g::decode_properties(code,&properties(true,b"")).err(),Some(CodecError::RemoteFailure));}}
#[test]
fn group_directory_every_truncation_trailing_data_and_bad_bool_is_rejected(){
 let bytes=relations(true,true,b"");for end in 0..bytes.len(){assert!(g::decode_relations(0,&bytes[..end]).is_err());}let mut more=bytes.clone();more.push(0);assert!(g::decode_relations(0,&more).is_err());
 // Native bool is any nonzero byte, not the frontend's JSON bool. Keep the
 // incomplete-field and oversized-map refusals; do not change the old Reader.
 more=bytes.clone();more[1]=2;{let value=g::decode_relations(0,&more).unwrap();assert!(value.accepted&&value.reported);assert_eq!(value.time,i64::MIN);}more.truncate(2);assert!(g::decode_relations(0,&more).is_err());
 more=bytes.clone();more[2]=2;{let value=g::decode_relations(0,&more).unwrap();assert!(value.accepted&&value.reported);assert_eq!(value.first.len(),1);}more.truncate(3);assert!(g::decode_relations(0,&more).is_err());
 more=bytes;more[3]=2;assert!(g::decode_relations(0,&more).is_err());
 let bytes=properties(true,b"");for end in 0..bytes.len(){assert!(g::decode_properties(0,&bytes[..end]).is_err());}let mut more=bytes;more.push(0);assert!(g::decode_properties(0,&more).is_err());}
#[test]
fn group_directory_relation_and_name_duplicate_keys_are_refused_not_last_write_wins(){
 let mut w=Writer::new();w.boolean(true).unwrap();w.boolean(true).unwrap();w.u32(2).unwrap();relation(&mut w,b"same",0,b"",b"");relation(&mut w,b"same",1,b"",b"");w.u32(0).unwrap();w.u32(0).unwrap();w.i64(0).unwrap();w.bytes(b"").unwrap();assert_eq!(g::decode_relations(0,w.finish().expose_to_transport()).err(),Some(CodecError::DuplicateKey));
 let mut w=Writer::new();w.boolean(true).unwrap();w.boolean(true).unwrap();w.u32(0).unwrap();w.u32(0).unwrap();w.u32(2).unwrap();w.bytes(b"same").unwrap();w.bytes(b"same").unwrap();w.i64(0).unwrap();w.bytes(b"").unwrap();assert_eq!(g::decode_relations(0,w.finish().expose_to_transport()).err(),Some(CodecError::DuplicateKey));}
#[test]
fn group_directory_map_counts_and_field_lengths_remain_original_bounded_before_allocating(){
 let mut w=Writer::new();w.boolean(true).unwrap();w.boolean(true).unwrap();w.u32((MAX_MAP_ENTRIES+1)as u32).unwrap();assert!(g::decode_relations(0,w.finish().expose_to_transport()).is_err());
 let mut w=Writer::new();w.boolean(true).unwrap();w.u32((MAX_MAP_ENTRIES+1)as u32).unwrap();assert!(g::decode_properties(0,w.finish().expose_to_transport()).is_err());
 let mut bad=properties(true,b"");bad[6..10].copy_from_slice(&65537u32.to_be_bytes());assert!(g::decode_properties(0,&bad).is_err());}
#[test]
fn group_directory_optional_footer_properties_are_consumed_as_opaque_data_not_grants(){
 let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();w.properties(&[RpcProperty{key:b"grant",value:b"private-claim"}]).unwrap();let bytes=w.finish();let r=g::decode_properties(0,bytes.expose_to_transport()).unwrap();assert!(r.accepted);assert!(r.properties.is_empty());assert_eq!(r.footer.properties[0].value,b"private-claim");}
#[test]
fn group_directory_debug_never_exposes_group_id_label_flags_property_name_or_value(){
 let id=g::GroupId::parse(b"123-456").unwrap();let bytes=relations(true,true,b"");let r=g::decode_relations(0,&bytes).unwrap();let bytes=properties(true,b"");let p=g::decode_properties(0,&bytes).unwrap();let printed=format!("{id:?} {r:?} {:?} {p:?}",r.first[0]);for secret in ["123-456","private-","f1234567"]{assert!(!printed.contains(secret));}}
