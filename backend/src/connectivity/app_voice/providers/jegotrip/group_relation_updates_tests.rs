//! Actions-only strict Group relation-change codec coverage; no service/SDK operations.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::rpc_stream::RpcProperty;
fn relation(label:&[u8])->Relation<'_>{Relation{label,flags:0x81234567,first:b"first\0bytes",second:b"second",properties:vec![RpcProperty{key:b"opaque",value:b"private-value"}]}}
fn result(accepted:bool,base:i64,update:i64,reason:&[u8])->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.i64(base).unwrap();w.i64(update).unwrap();w.bytes(reason).unwrap();w.finish().expose_to_transport().to_vec()}
fn read_relation<'a>(r:&mut Reader<'a>)->(&'a[u8],u32,&'a[u8],&'a[u8],Vec<RpcProperty<'a>>){(r.bytes().unwrap(),r.u32().unwrap(),r.bytes().unwrap(),r.bytes().unwrap(),r.properties().unwrap())}
#[test]
fn relation_updates_exact_version_zero_identifier_and_three_empty_maps(){
    let bytes=encode_updates(b"I",&[],&[],&[]).unwrap();let expected=[0,0,1,0,0,0,0,0,1,b'I',0,0,0,0,0,0,0,0,0,0,0,0];assert_eq!(bytes.expose_to_transport(),expected);assert_eq!(SET_RELATIONS_METHOD,"setRelations.GroupServer.Group");assert_ne!(SET_RELATIONS_METHOD,super::super::group_directory_data::RELATIONS_METHOD);
}
#[test]
fn relation_updates_measured_writer_keeps_exact_capacity_without_secret_reallocation(){
    let rows=[relation(b"buddy")];let size=encoded_size(b"original-uid",&rows,&[],&[]).unwrap();let bytes=encode_updates(b"original-uid",&rows,&[],&[]).unwrap();assert_eq!(bytes.expose_to_transport().len(),size);assert_eq!(bytes.allocated_capacity(),size);
}
#[test]
fn relation_updates_preserve_opaque_labels_flags_strings_properties_in_both_maps(){
    let add=[relation(b"new\0label")];let updates=[relation(b"updated")];let bytes=encode_updates(b"original-uid",&add,&updates,&[]).unwrap();let mut r=Reader::versioned(bytes.expose_to_transport()).unwrap();assert_eq!(r.bytes().unwrap(),b"original-uid");
    for label in [b"new\0label".as_slice(),b"updated"]{assert_eq!(r.u32().unwrap(),1);let(l,flags,first,second,properties)=read_relation(&mut r);assert_eq!(l,label);assert_eq!(flags,0x81234567);assert_eq!(first,b"first\0bytes");assert_eq!(second,b"second");assert_eq!(properties[0].value,b"private-value");}assert_eq!(r.u32().unwrap(),0);r.finish().unwrap();
}
#[test]
fn relation_updates_use_native_length_then_byte_map_order_not_caller_order(){
    let rows=[relation(b"aa"),relation(b"z"),relation(b"a")];let bytes=encode_updates(b"uid",&rows,&[],&[]).unwrap();let mut r=Reader::versioned(bytes.expose_to_transport()).unwrap();r.bytes().unwrap();assert_eq!(r.u32().unwrap(),3);for want in [b"a".as_slice(),b"z",b"aa"]{assert_eq!(read_relation(&mut r).0,want);}assert_eq!(r.u32().unwrap(),0);assert_eq!(r.u32().unwrap(),0);r.finish().unwrap();
}
#[test]
fn relation_updates_mtc_buddy_remove_preserves_original_false_flag_not_true_default(){
    let removals=[Removal{label:b"z",flag:true},Removal{label:b"a",flag:false}];let bytes=encode_updates(b"uid",&[],&[],&removals).unwrap();let mut r=Reader::versioned(bytes.expose_to_transport()).unwrap();r.bytes().unwrap();assert_eq!(r.u32().unwrap(),0);assert_eq!(r.u32().unwrap(),0);assert_eq!(r.u32().unwrap(),2);assert_eq!(r.bytes().unwrap(),b"a");assert!(!r.boolean().unwrap());assert_eq!(r.bytes().unwrap(),b"z");assert!(r.boolean().unwrap());r.finish().unwrap();
}
#[test]
fn relation_updates_duplicate_keys_within_each_map_are_not_last_writer_wins(){
    let rows=[relation(b"same"),relation(b"same")];assert_eq!(encoded_size(b"uid",&rows,&[],&[]),Err(CodecError::DuplicateKey));assert_eq!(encoded_size(b"uid",&[],&rows,&[]),Err(CodecError::DuplicateKey));let removals=[Removal{label:b"same",flag:false},Removal{label:b"same",flag:true}];assert_eq!(encoded_size(b"uid",&[],&[],&removals),Err(CodecError::DuplicateKey));
}
#[test]
fn relation_updates_duplicate_property_keys_fail_before_allocating_the_writer(){
    let mut row=relation(b"buddy");row.properties.push(RpcProperty{key:b"opaque",value:b"different"});assert_eq!(encoded_size(b"uid",&[row],&[],&[]),Err(CodecError::DuplicateKey));
}
#[test]
fn relation_updates_keep_independent_native_maps_but_reject_high_level_conflicting_action(){
    let add=[relation(b"same")];let updates=[relation(b"same")];assert!(encode_updates(b"uid",&add,&updates,&[]).is_ok());assert_eq!(reject_conflicting_changes(&add,&updates,&[]),Err(CodecError::InvalidField));let removals=[Removal{label:b"same",flag:false}];assert_eq!(reject_conflicting_changes(&add,&[],&removals),Err(CodecError::InvalidField));assert_eq!(reject_conflicting_changes(&[],&updates,&removals),Err(CodecError::InvalidField));assert_eq!(reject_conflicting_changes(&[relation(b"a")],&[relation(b"u")],&[Removal{label:b"d",flag:false}]),Ok(()));
}
#[test]
fn relation_updates_empty_identifier_oversized_fields_and_total_limits_are_distinct(){
    assert_eq!(encoded_size(b"",&[],&[],&[]),Err(CodecError::InvalidField));let large=vec![7;MAX_FIELD_BYTES+1];assert_eq!(encoded_size(&large,&[],&[],&[]),Err(CodecError::TooLarge));let field=vec![7;MAX_FIELD_BYTES];let mut row=relation(b"buddy");row.first=&field;row.second=&field;assert_eq!(encoded_size(b"uid",&[row],&[],&[]),Err(CodecError::TooLarge));
}
#[test]
fn relation_updates_per_map_count_limit_is_checked_before_items_or_allocation(){
    let rows=(0..=MAX_MAP_ENTRIES).map(|_|relation(b"item")).collect::<Vec<_>>();assert_eq!(encoded_size(b"uid",&rows,&[],&[]),Err(CodecError::TooLarge));let removals=vec![Removal{label:b"same",flag:false};MAX_MAP_ENTRIES+1];assert_eq!(encoded_size(b"uid",&[],&[],&removals),Err(CodecError::TooLarge));
}
#[test]
fn relation_updates_native_api_type_mapping_preserves_all_recovered_values(){
    for(source,wire)in [(11,11),(12,12),(13,13),(14,14),(15,15),(16,16),(18,18),(257,0),(258,1),(259,2),(4097,10)]{assert_eq!(native_type_from_mtc(source),Ok(wire));}for source in [0,1,10,17,19,20,256,260,4096,u32::MAX]{assert_eq!(native_type_from_mtc(source),Err(CodecError::InvalidField));}
}
#[test]
fn relation_update_reply_reads_bool_two_full_signed64_times_and_complete_footer(){
    let bytes=result(true,i64::MIN,i64::MAX,b"");let value=decode_updates(0,&bytes).unwrap();assert!(value.accepted);assert_eq!(value.base_time,i64::MIN);assert_eq!(value.update_time,i64::MAX);assert!(value.footer.reason.is_empty());
}
#[test]
fn relation_update_negative_or_nonempty_footer_never_becomes_positive(){
    for(accepted,reason)in [(false,b"".as_slice()),(true,b"private-reason")]{let bytes=result(accepted,5,6,reason);let value=decode_updates(0,&bytes).unwrap();assert!(!value.accepted);assert_eq!(value.base_time,5);assert_eq!(value.update_time,6);assert_eq!(value.footer.reason,reason);}
}
#[test]
fn relation_update_nonzero_and_version_class_results_never_synthesize_normal_success(){
    let bytes=result(true,1,2,b"");for code in [1,65536,65537,u32::MAX]{assert_eq!(decode_updates(code,&bytes).err(),Some(CodecError::RemoteFailure));}
}
#[test]
fn relation_update_every_truncation_and_trailing_byte_is_rejected(){
    let bytes=result(true,1,2,b"");for end in 0..bytes.len(){assert!(decode_updates(0,&bytes[..end]).is_err(),"accepted truncated field at{end}");}let mut more=bytes;more.push(0);assert!(decode_updates(0,&more).is_err());
}
#[test]
fn relation_update_bool_is_original_nonzero_native_byte_without_json_coercion(){
    let mut bytes=result(true,1,2,b"");bytes[1]=255;assert!(decode_updates(0,&bytes).unwrap().accepted);bytes[0]=1;assert_eq!(decode_updates(0,&bytes).err(),Some(CodecError::UnsupportedStream));
}
#[test]
fn relation_update_optional_footer_properties_remain_opaque_not_uid_or_write_grants(){
    let mut w=Writer::new();w.boolean(true).unwrap();w.i64(1).unwrap();w.i64(2).unwrap();w.bytes(b"").unwrap();w.properties(&[RpcProperty{key:b"grant",value:b"private-claim"}]).unwrap();let bytes=w.finish();let value=decode_updates(0,bytes.expose_to_transport()).unwrap();assert!(value.accepted);assert_eq!(value.footer.properties[0].value,b"private-claim");
}
#[test]
fn relation_update_debug_never_exports_peer_labels_property_values_or_footer_reason(){
    let removed=Removal{label:b"private-buddy",flag:false};let bytes=result(true,111,222,b"private-reason");let value=decode_updates(0,&bytes).unwrap();let message=format!("{removed:?} {value:?}");for private in ["private-","111","222"]{assert!(!message.contains(private));}
}
