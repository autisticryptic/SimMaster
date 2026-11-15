//! Actions-only independently assembled original Group status DATA fixtures.
use crate::connectivity::app_voice::providers::jegotrip::{im_status as s,rpc_stream::{CodecError,RpcProperty}};
fn string(out:&mut Vec<u8>,bytes:&[u8]){out.extend_from_slice(&(bytes.len()as u32).to_be_bytes());out.extend_from_slice(bytes);}
fn map(out:&mut Vec<u8>,fields:&[(&[u8],&[u8])]){out.extend_from_slice(&(fields.len()as u32).to_be_bytes());for(key,value)in fields{string(out,key);string(out,value);}}
fn result(fields:&[(&[u8],&[u8])],time:i64,accepted:bool)->Vec<u8>{let mut out=vec![0,u8::from(accepted)];out.extend_from_slice(&1u32.to_be_bytes());string(&mut out,b"1_2");map(&mut out,fields);out.extend_from_slice(&time.to_be_bytes());string(&mut out,b"");out}
#[test]
fn original_status_refresh_method_version_uid_sets_time_are_exact_and_not_message_or_sms_aliases(){
    assert_eq!(s::REFRESH_METHOD,"refreshStatuses.GroupServer.Group");let labels=[s::StatusLabel::parse(b"12_34").unwrap(),s::StatusLabel::parse(b"1-2").unwrap()];let mut expected=vec![0,0,1,0,0];string(&mut expected,b"own-uid");expected.extend_from_slice(&2u32.to_be_bytes());string(&mut expected,b"1-2");string(&mut expected,b"12_34");expected.extend_from_slice(&5u32.to_be_bytes());for key in [b"ML".as_slice(),b"MLB",b"P:P_MR",b"P:P_MV-N",b"P:P_UDMR-N"]{string(&mut expected,key);}expected.extend_from_slice(&(-1i64).to_be_bytes());assert_eq!(s::encode_refresh(b"own-uid",&labels,-1).unwrap().expose_to_transport(),expected);
}
#[test]
fn raw_status_label_uses_original_uid_or_group_grammar_not_caller_box_domain_phone_or_path(){
    for(raw,expected)in[(b"0_1".as_slice(),b"P2P/0_1".as_slice()),(b"001_2",b"P2P/001_2"),(b"1-2",b"Org/1-2"),(b"12+34",b"Org/12+34")]{let label=s::StatusLabel::parse(raw).unwrap();assert_eq!(label.bytes(),raw);assert_eq!(label.box_bytes().as_slice(),expected);}
    for value in [b"".as_slice(),b"P2P/1_2",b"Org/1-2",b"123456789",b"-1_2",b"1_02",b"01-2",b"1-0",b"1_2\0",b"../1_2",b"1_2.extra"]{assert!(s::StatusLabel::parse(value).is_err());}
}
#[test]
fn status_refresh_sets_are_bounded_unique_and_timestamp_negative_one_is_not_a_message_cursor(){
    let label=s::StatusLabel::parse(b"1_2").unwrap();assert!(s::encode_refresh(b"uid",&[label,label],0).is_err());assert!(s::encode_refresh(b"uid",&vec![label;129],0).is_err());for since in [i64::MIN,-2]{assert!(s::encode_refresh(b"uid",&[],since).is_err());}for uid in [b"".as_slice(),b"uid\0"]{assert!(s::encode_refresh(uid,&[],-1).is_err());}
    let encoded=s::encode_refresh(b"uid",&[],i64::MAX).unwrap();assert_eq!(&encoded.expose_to_transport()[encoded.expose_to_transport().len()-8..],&i64::MAX.to_be_bytes());
}
#[test]
fn refresh_result_keeps_full_width_update_time_and_separates_latest_received_read_and_peer_read(){
    let bytes=result(&[(b"ML",b"9223372036854775807"),(b"P:P_MR",b"4294967297"),(b"P:P_MV-N",b"7"),(b"P:P_UDMR-N",b"6")],0x1234_5678_9abc,true);let decoded=s::decode_refresh(0,&bytes).unwrap();assert!(decoded.accepted);assert_eq!(decoded.update_time,0x1234_5678_9abc);let row=&decoded.entries[0];assert_eq!(row.latest,Some(i64::MAX));assert_eq!(row.peer_read,Some(4294967297));assert_eq!(row.last_received,Some(7));assert_eq!(row.last_read,Some(6));assert_eq!(row.reported_range(),Some((4294967297,i64::MAX)));assert!(decoded.footer.reason.is_empty());
}
#[test]
fn missing_status_fields_are_not_zero_minus_one_or_a_local_consumption_grant(){
    let bytes=result(&[(b"ML",b"7")],0,true);let decoded=s::decode_refresh(0,&bytes).unwrap();let row=&decoded.entries[0];assert_eq!(row.peer_read,None);assert_eq!(row.last_received,None);assert_eq!(row.last_read,None);assert_eq!(row.reported_range(),None);
    let bytes=result(&[(b"ML",b"7"),(b"P:P_MR",b"-1")],0,true);assert_eq!(s::decode_refresh(0,&bytes).unwrap().entries[0].reported_range(),Some((-1,7)));
}
#[test]
fn native_reported_range_clips_peer_read_above_latest_without_using_other_status_fields(){
    let bytes=result(&[(b"ML",b"7"),(b"P:P_MR",b"9"),(b"P:P_MV-N",b"6"),(b"P:P_UDMR-N",b"3")],0,true);let decoded=s::decode_refresh(0,&bytes).unwrap();assert_eq!(decoded.entries[0].reported_range(),Some((7,7)));assert_eq!(decoded.entries[0].last_received,Some(6));
}
#[test]
fn malformed_noncanonical_or_overflow_status_indices_do_not_use_native_fallback_sentinels(){
    for value in [b"".as_slice(),b"01",b"-0",b"+1",b" 1",b"1 ",b"-2",b"1.0",b"9223372036854775808",b"1\0"]{for key in [s::LATEST,s::PEER_READ,s::LAST_RECEIVED,s::LAST_READ]{assert!(s::decode_refresh(0,&result(&[(key,value)],0,true)).is_err());}}
}
#[test]
fn refresh_negative_still_validates_payload_and_common_footer_but_never_becomes_accepted(){
    let bytes=result(&[(b"ML",b"7")],5,false);assert!(!s::decode_refresh(0,&bytes).unwrap().accepted);let mut reason=bytes[..bytes.len()-4].to_vec();string(&mut reason,b"private-rejection");assert!(!s::decode_refresh(0,&reason).unwrap().accepted);assert_eq!(s::decode_refresh(7,&bytes).err(),Some(CodecError::RemoteFailure));assert!(s::decode_refresh(0,&result(&[(b"ML",b"invalid")],5,false)).is_err());
}
#[test]
fn every_refresh_truncation_and_trailing_byte_is_rejected_without_partial_rows(){
    let bytes=result(&[(b"ML",b"7"),(b"P:P_MR",b"3")],5,true);for end in 0..bytes.len(){assert!(s::decode_refresh(0,&bytes[..end]).is_err());}let mut trailing=bytes.clone();trailing.push(1);assert!(s::decode_refresh(0,&trailing).is_err());let mut kind=bytes.clone();kind[0]=1;assert_eq!(s::decode_refresh(0,&kind).err(),Some(CodecError::UnsupportedStream));
}
#[test]
fn status_outer_labels_inner_properties_and_counts_reject_duplicates_and_excess_before_allocation(){
    let duplicate=result(&[(b"ML",b"7"),(b"ML",b"8")],0,true);assert_eq!(s::decode_refresh(0,&duplicate).err(),Some(CodecError::DuplicateKey));let mut bytes=vec![0,1];bytes.extend_from_slice(&2u32.to_be_bytes());for _ in 0..2{string(&mut bytes,b"1_2");map(&mut bytes,&[]);}bytes.extend_from_slice(&0i64.to_be_bytes());string(&mut bytes,b"");assert_eq!(s::decode_refresh(0,&bytes).err(),Some(CodecError::DuplicateKey));for count in [129u32,u32::MAX]{let mut bytes=vec![0,1];bytes.extend_from_slice(&count.to_be_bytes());assert_eq!(s::decode_refresh(0,&bytes).err(),Some(CodecError::TooLarge));}
}
#[test]
fn online_status_update_has_its_own_exact_header_and_flat_body_not_a_refresh_or_moremessage_record(){
    let header=[RpcProperty{key:b"Group.GroupId",value:b"1-2"},RpcProperty{key:b"Group.UpdateTime",value:b"4294967297"}];let mut body=vec![0];map(&mut body,&[(s::LATEST,b"12"),(s::LAST_RECEIVED,b"8")]);let update=s::decode_status_update(s::STATUS_KIND,&header,&body).unwrap().unwrap();assert_eq!(update.entry.label.box_bytes().as_slice(),b"Org/1-2");assert_eq!(update.update_time,4294967297);assert_eq!(update.entry.latest,Some(12));assert_eq!(update.entry.last_received,Some(8));assert_eq!(update.entry.reported_range(),None);
    assert!(s::decode_status_update(b"Message.MoreMessage",&header,&body).unwrap().is_none());assert!(s::decode_status_update(b"Sip.Request",&header,&body).unwrap().is_none());assert!(s::decode_refresh(0,&body).is_err());
}
#[test]
fn online_status_update_missing_duplicate_or_invalid_time_headers_never_get_default_values(){
    let good=[RpcProperty{key:b"Group.GroupId",value:b"1_2"},RpcProperty{key:b"Group.UpdateTime",value:b"7"}];let mut body=vec![0];map(&mut body,&[]);assert!(s::decode_status_update(s::STATUS_KIND,&good[..1],&body).is_err());let mut duplicate=good.to_vec();duplicate.push(good[0]);assert!(s::decode_status_update(s::STATUS_KIND,&duplicate,&body).is_err());for value in [b"".as_slice(),b"07",b"-2",b"7\0"]{let header=[good[0],RpcProperty{key:b"Group.UpdateTime",value}];assert!(s::decode_status_update(s::STATUS_KIND,&header,&body).is_err());}
}
#[test]
fn status_private_raw_values_and_unknown_properties_are_not_serialized_or_reflected_in_debug(){
    let bytes=result(&[(s::LATEST,b"7"),(s::BRIEF,b"private-body"),(b"Private.Data",b"private-token")],7,true);let decoded=s::decode_refresh(0,&bytes).unwrap();assert_eq!(decoded.entries[0].properties.len(),3);let rendered=format!("{decoded:?} {:?}",decoded.entries[0]);for value in ["private-body","private-token","1_2"]{assert!(!rendered.contains(value));}assert_ne!(s::PEER_READ_NOTIFICATION,s::LAST_READ);
}
