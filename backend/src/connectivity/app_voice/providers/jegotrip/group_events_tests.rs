//! Actions-only original Group.Update codec/policy tests, no SDK or peer execution.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::rpc_stream::Writer;
fn body()->Vec<u8>{let mut w=Writer::new();w.u32(2).unwrap();for(label,flags)in[(b"original-label".as_slice(),0x81234567),(b"removed".as_slice(),20)]{w.bytes(label).unwrap();w.u32(flags).unwrap();w.bytes(b"first\0bytes").unwrap();w.bytes(b"second").unwrap();w.properties(&[RpcProperty{key:b"opaque",value:b"private-data"}]).unwrap();}w.finish().expose_to_transport().to_vec()}
fn header()->[RpcProperty<'static>;3]{[RpcProperty{key:b"Group.GroupId",value:b"1-2"},RpcProperty{key:b"Group.LastUpdateTime",value:b"-9223372036854775808"},RpcProperty{key:b"Group.UpdateTime",value:b"9223372036854775807"}]}
#[test]fn group_event_original_kind_is_not_local_notification_outbound_method_or_status_map(){
 let bytes=body();assert_eq!(GROUP_UPDATE_KIND,b"Group.Update");for kind in [b"MtcGroupChangedNotification".as_slice(),b"setRelations.GroupServer.Group",b"Status.Update",b"Message.MoreMessage",b"System.OrgRelSync"]{assert!(decode_group_update(kind,&header(),&bytes,b"fixture-user").unwrap().is_none());}
}
#[test]fn group_event_original_full_relation_stream_preserves_u32_two_strings_properties_and_signed64(){
 let bytes=body();let result=decode_group_update(GROUP_UPDATE_KIND,&header(),&bytes,b"fixture-user").unwrap().unwrap();assert_eq!(result.target,RelationEventTarget::Group);assert_eq!(result.identifier,b"1-2");assert_eq!((result.base_time,result.update_time),(i64::MIN,i64::MAX));assert_eq!(result.relations[0].flags,0x81234567);assert_eq!(result.relations[0].first,b"first\0bytes");assert_eq!(result.relations[1].flags,20);assert_eq!(result.relations[0].properties[0].value,b"private-data");
}
#[test]fn group_event_missing_identifier_falls_back_only_to_supplied_original_sdk_uid_data(){
 let h=[header()[1],header()[2]];let bytes=body();let value=decode_group_update(GROUP_UPDATE_KIND,&h,&bytes,b"current-private-uid").unwrap().unwrap();assert_eq!(value.identifier,b"current-private-uid");assert_eq!(value.target,RelationEventTarget::Buddy);assert!(decode_group_update(GROUP_UPDATE_KIND,&h,&bytes,b"").is_err());
}
#[test]fn group_event_another_uid_phone_box_uri_or_endpoint_cannot_be_a_current_buddy_target(){
 let bytes=body();for target in [b"another-user".as_slice(),b"15900000000",b"P2P/1_2",b"Org/1-2",b"Group:tcp -h x -p 1;",b"01-2",b""]{let mut h=header();h[0].value=target;assert!(decode_group_update(GROUP_UPDATE_KIND,&h,&bytes,b"fixture-user").is_err());}
}
#[test]fn group_event_two_original_time_fields_are_required_and_not_inferred_as_zero(){
 let bytes=body();let h=header();for missing in [1,2]{let header=h.iter().enumerate().filter_map(|(i,p)|(i!=missing).then_some(*p)).collect::<Vec<_>>();assert!(decode_group_update(GROUP_UPDATE_KIND,&header,&bytes,b"fixture-user").is_err());}
}
#[test]fn group_event_signed_time_strings_are_exact_noncoercible_and_order_checked(){
 let bytes=body();for text in [b"01".as_slice(),b"+1",b"-0",b"9223372036854775808",b"private-token"]{let mut h=header();h[2].value=text;assert!(decode_group_update(GROUP_UPDATE_KIND,&h,&bytes,b"fixture-user").is_err());}let mut h=header();h[1].value=b"10";h[2].value=b"10";assert!(decode_group_update(GROUP_UPDATE_KIND,&h,&bytes,b"fixture-user").is_err());
}
#[test]fn group_event_all_stream_truncations_and_trailing_data_are_refused_without_footer_invention(){
 let bytes=body();for end in 0..bytes.len(){assert!(decode_relation_map(&bytes[..end]).is_err());}let mut trailing=bytes;trailing.push(0);assert!(decode_relation_map(&trailing).is_err());let mut bad=body();bad[0]=1;assert_eq!(decode_relation_map(&bad).err(),Some(CodecError::UnsupportedStream));
}
#[test]fn group_event_duplicate_labels_or_properties_are_not_accepted_as_last_writer_wins(){
 let mut w=Writer::new();w.u32(2).unwrap();for _ in 0..2{w.bytes(b"same").unwrap();w.u32(1).unwrap();w.bytes(b"").unwrap();w.bytes(b"").unwrap();w.properties(&[]).unwrap();}assert_eq!(decode_relation_map(w.finish().expose_to_transport()).err(),Some(CodecError::DuplicateKey));let mut h=header().to_vec();h.push(header()[0]);assert_eq!(decode_group_update(GROUP_UPDATE_KIND,&h,&body(),b"fixture-user").err(),Some(CodecError::DuplicateKey));
}
#[test]fn group_event_zero_map_and_header_projection_are_data_not_relationship_write_ack(){
 let mut w=Writer::new();w.u32(0).unwrap();let bytes=w.finish();let value=decode_group_update(GROUP_UPDATE_KIND,&header(),bytes.expose_to_transport(),b"fixture-user").unwrap().unwrap();assert!(value.relations.is_empty());assert_eq!(relation_map_scratch(bytes.expose_to_transport()),Ok(0));
}
#[test]fn group_event_count_field_and_scratch_headers_are_bounded_before_vector_allocation(){
 let mut w=Writer::new();w.u32((MAX_MAP_ENTRIES+1)as u32).unwrap();assert!(relation_map_scratch(w.finish().expose_to_transport()).is_err());let bytes=body();assert!(relation_map_scratch(&bytes).unwrap()>=2*std::mem::size_of::<Relation<'_>>());
}
#[test]fn group_event_installation_defaults_absent_and_strict_policy_has_no_other_rights(){
 assert!(NativeRelationEventsConfig{group_updates:true}.validate().is_ok());assert!(NativeRelationEventsConfig{group_updates:false}.validate().is_err());assert!(serde_json::from_str::<NativeRelationEventsConfig>(r#"{"group_updates":true,"write":true}"#).is_err());assert!(serde_json::from_str::<NativeRelationEventsConfig>(r#"{"group_updates":"true"}"#).is_err());
}
#[test]fn group_event_debug_never_exports_original_targets_relation_bodies_or_signed_times(){
 let bytes=body();let value=decode_group_update(GROUP_UPDATE_KIND,&header(),&bytes,b"fixture-user").unwrap().unwrap();let text=format!("{value:?} {:?} {:?}",value.target,NativeRelationEventsConfig{group_updates:true});for secret in ["1-2","original-label","private-data","first","922337","812345"]{assert!(!text.contains(secret));}
}
