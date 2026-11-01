use super::*;
use super::super::common_compression::tests::reference_container;
fn assembler(now:Instant)->CommonAssembler{CommonAssembler::new(Duration::from_secs(2),now).unwrap()}
fn compressed(kind:CallKind,payload:&[u8])->Vec<u8>{let mut bytes=vec![kind as u8|COMPRESSED];bytes.extend_from_slice(&reference_container(payload,1));bytes}
fn fragment(body:&[u8],header:u8,split:usize)->Vec<TransportBytes>{
 let mut first=vec![header|FRAGMENTED];first.extend_from_slice(&(body.len() as u32).to_be_bytes());first.extend_from_slice(&body[..split]);
 let mut last=vec![3];last.extend_from_slice(&body[split..]);vec![TransportBytes::from_bytes(first),TransportBytes::from_bytes(last)]
}

#[test]
fn native_mode_one_threshold_differs_from_unchanged_mode_three_codec(){
 let data=vec![7;NATIVE_FRAGMENT_THRESHOLD];let encoded=encode(CallKind::Request,&data,NATIVE_FRAGMENT_THRESHOLD,None).unwrap();
 assert_eq!(encoded.len(),1);assert_eq!(encoded[0].expose_to_transport().len(),NATIVE_FRAGMENT_THRESHOLD+1);
 assert!(super::super::transport_packets::encode_call_packets(CallKind::Request,&data,NATIVE_FRAGMENT_THRESHOLD).is_err());
 assert_eq!(super::super::transport_packets::CallAssembler::default().on_packet(&[0x10,0]).err(),Some(super::super::transport_packets::TransportError::UnsupportedCompression));
}
#[test]
fn native_large_uncompressed_fragments_balance_last_two_and_keep_original_be_length(){
 let now=Instant::now();let data=vec![0x42;MAX_STREAM_BYTES];let encoded=encode(CallKind::Reply,&data,NATIVE_FRAGMENT_THRESHOLD,None).unwrap();
 assert_eq!(encoded.iter().map(|p|p.expose_to_transport().len()-1).collect::<Vec<_>>(),[65536,32770,32770]);
 assert_eq!(&encoded[0].expose_to_transport()[..5],&[0x21,0,2,0,0]);assert_eq!(encoded[1].expose_to_transport()[0],3);
 let result=assembler(now).on_group(&encoded,now+Duration::from_secs(4),now).unwrap();assert_eq!(result.len(),1);assert_eq!(result[0].payload.expose_to_transport(),data);
}
#[test]
fn small_independent_fragment_vector_matches_native_prefix_and_balanced_tail(){
 let encoded=encode(CallKind::Request,&[0,1,2,3,4,5,6,7,8,9,10,11,12],8,None).unwrap();
 assert_eq!(encoded.iter().map(|p|p.expose_to_transport()).collect::<Vec<_>>(),vec![&[0x20,0,0,0,13,0,1,2,3][..],&[3,4,5,6,7][..],&[3,8,9,10,11,12][..]]);
}
#[test]
fn independent_compressed_request_and_reply_decode_after_common_header(){
 let now=Instant::now();for kind in [CallKind::Request,CallKind::Reply]{let bytes=compressed(kind,b"native compressed call");let result=assembler(now).on_packet(&bytes,now+Duration::from_secs(4),now).unwrap().unwrap();
  assert_eq!(result.kind,kind);assert_eq!(result.payload.expose_to_transport(),b"native compressed call");}
}
#[test]
fn compressed_fragments_are_reassembled_before_any_bzip2_parse(){
 let now=Instant::now();let container=reference_container(b"complete compressed payload",1);let packets=fragment(&container,0x11,7);let mut state=assembler(now);
 assert!(state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(5),now).unwrap().is_none());
 let result=state.on_packet(packets[1].expose_to_transport(),now+Duration::from_secs(5),now).unwrap().unwrap();assert_eq!(result.payload.expose_to_transport(),b"complete compressed payload");
}
#[test]
fn corrupt_compression_is_reported_only_at_complete_original_fragment_boundary(){
 let now=Instant::now();let mut container=reference_container(b"original partial boundary",1);container[4]=0;let packets=fragment(&container,0x10,8);let mut state=assembler(now);
 assert!(state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(5),now).unwrap().is_none());
 assert!(matches!(state.on_packet(packets[1].expose_to_transport(),now+Duration::from_secs(5),now),Err(CommonError::Compression(_))));assert!(state.closed);
}
#[test]
fn explicit_compression_policy_never_guesses_a_default_or_emits_partial_output(){
 let now=Instant::now();let data=vec![b'x';4096];let plain=encode(CallKind::Reply,&data,NATIVE_FRAGMENT_THRESHOLD,None).unwrap();assert_eq!(plain[0].expose_to_transport()[0],1);
 let zipped=encode(CallKind::Reply,&data,NATIVE_FRAGMENT_THRESHOLD,Some(64)).unwrap();assert_eq!(zipped[0].expose_to_transport()[0],0x11);
 assert_eq!(assembler(now).on_group(&zipped,now+Duration::from_secs(5),now).unwrap()[0].payload.expose_to_transport(),data);
}
#[test]
fn compressed_output_can_itself_fragment_without_putting_compression_on_continuations(){
 let now=Instant::now();let data=vec![b'x';4096];let packets=encode(CallKind::Request,&data,8,Some(64)).unwrap();assert!(packets.len()>1);assert_eq!(packets[0].expose_to_transport()[0],0x30);
 assert!(packets.iter().skip(1).all(|packet|packet.expose_to_transport()[0]==3));assert_eq!(assembler(now).on_group(&packets,now+Duration::from_secs(5),now).unwrap()[0].payload.expose_to_transport(),data);
}
#[test]
fn control_packet_shapes_are_exact_and_not_compressed_or_fragmented(){
 let now=Instant::now();for kind in [CallKind::Ping,CallKind::Pong,CallKind::Release]{let encoded=encode(kind,&[],NATIVE_FRAGMENT_THRESHOLD,Some(8)).unwrap();assert_eq!(encoded[0].expose_to_transport(),[kind as u8]);
  assert_eq!(assembler(now).on_group(&encoded,now+Duration::from_secs(5),now).unwrap()[0].kind,kind);assert!(encode(kind,b"x",8,None).is_err());}
 for header in [0x12,0x1e,0x14,0x22,0x24,0x2e]{assert!(assembler(now).on_packet(&[header,0],now+Duration::from_secs(5),now).is_err());}
}
#[test]
fn unsupported_kx_inner_tls_reserved_bits_and_fake_continuations_fail_closed(){
 let now=Instant::now();for header in [5,6,7,15,0x13,0x23,0x33,0x40,0x80,0xff]{let mut state=assembler(now);assert!(state.on_packet(&[header,0],now+Duration::from_secs(5),now).is_err());assert_eq!(state.on_packet(&[2],now+Duration::from_secs(5),now).err(),Some(CommonError::Closed));}
}
#[test]
fn original_partial_deadline_is_not_replaced_by_a_later_caller_budget(){
 let now=Instant::now();let packets=fragment(b"abcdefgh",0,2);let mut state=assembler(now);let end=now+Duration::from_millis(50);
 state.on_packet(packets[0].expose_to_transport(),end,now).unwrap();assert_eq!(state.deadline(),Some(end));
 let result=state.on_packet(packets[1].expose_to_transport(),now+Duration::from_secs(20),now+Duration::from_millis(49)).unwrap().unwrap();assert_eq!(result.deadline,end);
}
#[test]
fn timeout_rollback_and_empty_group_cannot_reset_an_old_partial(){
 let now=Instant::now();let packets=fragment(b"abcdefgh",0,2);let mut state=assembler(now);state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(5),now).unwrap();
 assert!(state.on_group(&[],now+Duration::from_secs(20),now+Duration::from_secs(1)).unwrap().is_empty());assert_eq!(state.deadline(),Some(now+Duration::from_secs(2)));
 assert_eq!(state.on_group(&[],now+Duration::from_secs(20),now+Duration::from_secs(2)).err(),Some(CommonError::Timeout));
 let mut state=assembler(now);assert_eq!(state.tick(now-Duration::from_millis(1)),Err(CommonError::ClockRollback));
}
#[test]
fn control_or_new_fragment_cannot_silently_discard_an_incomplete_original_call(){
 let now=Instant::now();let packets=fragment(b"abcdefgh",0,2);
 for next in [&[2][..],&[14][..],&[4][..],packets[0].expose_to_transport(),&[1,b'x'][..]]{let mut state=assembler(now);state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(5),now).unwrap();
  assert_eq!(state.on_packet(next,now+Duration::from_secs(5),now).err(),Some(CommonError::Fragment));assert_eq!(state.retained_bytes(),0);}
}
#[test]
fn empty_or_overlong_continuations_and_unbounded_lengths_never_allocate_from_peer_claim(){
 let now=Instant::now();for first in [&[0x20,0xff,0xff,0xff,0xff,1][..],&[0x20,0,0,0,0][..],&[0x20,0,0,0,1,1][..],&[0x20,0,0,0,1][..]]{assert!(assembler(now).on_packet(first,now+Duration::from_secs(5),now).is_err());}
 assert!(assembler(now).on_packet(&[3,1],now+Duration::from_secs(5),now).is_err());
 for continuation in [&[3][..],&[3,1,2,3,4,5,6,7][..]]{let mut state=assembler(now);state.on_packet(&[0x20,0,0,0,4,1],now+Duration::from_secs(5),now).unwrap();assert!(state.on_packet(continuation,now+Duration::from_secs(5),now).is_err());}
}
#[test]
fn common_group_failure_discards_an_earlier_valid_message(){
 let now=Instant::now();let packets=vec![TransportBytes::from_bytes(vec![1,b'a']),TransportBytes::from_bytes(vec![0x11,0])];let mut state=assembler(now);
 assert!(state.on_group(&packets,now+Duration::from_secs(5),now).is_err());assert!(state.closed);
}
#[test]
fn batch_uncompressed_total_is_bounded_even_for_tiny_valid_compressed_inputs(){
 let now=Instant::now();let data=vec![b'x';MAX_STREAM_BYTES];let packet=compressed(CallKind::Reply,&data);
 let packets:Vec<_>=(0..9).map(|_|TransportBytes::from_bytes(packet.clone())).collect();assert!(packets.iter().map(|p|p.expose_to_transport().len()).sum::<usize>()<MAX_GROUP_BYTES);
 let mut state=assembler(now);assert_eq!(state.on_group(&packets,now+Duration::from_secs(5),now).err(),Some(CommonError::Bound));assert!(state.closed);
}
#[test]
fn packet_count_and_large_packet_bounds_are_checked_before_group_publication(){
 let now=Instant::now();let packets:Vec<_>=(0..257).map(|_|TransportBytes::from_bytes(vec![2])).collect();assert!(assembler(now).on_group(&packets,now+Duration::from_secs(5),now).is_err());
 assert_eq!(assembler(now).on_packet(&vec![0;MAX_STREAM_BYTES+2],now+Duration::from_secs(5),now).err(),Some(CommonError::Bound));
 assert!(encode(CallKind::Request,&vec![1;MAX_STREAM_BYTES],8,None).is_err());
}
#[test]
fn malformed_compressed_length_and_trailing_stream_do_not_become_rpc_payload(){
 let now=Instant::now();let mut packet=compressed(CallKind::Reply,b"abc");packet.extend_from_slice(b"trailing");assert!(assembler(now).on_packet(&packet,now+Duration::from_secs(5),now).is_err());
 let mut packet=compressed(CallKind::Reply,b"abc");packet[1..5].copy_from_slice(&4u32.to_be_bytes());assert!(assembler(now).on_packet(&packet,now+Duration::from_secs(5),now).is_err());
}
#[test]
fn eof_cannot_publish_an_unfinished_common_fragment(){
 let now=Instant::now();let mut state=assembler(now);state.on_packet(&[0x20,0,0,0,4,1],now+Duration::from_secs(5),now).unwrap();assert_eq!(state.finish_eof(),Err(CommonError::Fragment));
 assert_eq!(state.finish_eof(),Err(CommonError::Closed));let mut empty=assembler(now);assert_eq!(empty.finish_eof(),Ok(()));
}
#[test]
fn partial_buffer_is_preallocated_and_debug_is_redacted(){
 let now=Instant::now();let packets=fragment(b"fixture-private-fragment",0,7);let mut state=assembler(now);state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(5),now).unwrap();
 let part=state.partial.as_ref().unwrap();assert!(part.bytes.capacity()>=part.expected);assert!(!format!("{state:?}").contains("private-fragment"));
}
#[test]
fn deadlines_and_compression_threshold_parameters_fail_without_implicit_policy(){
 let now=Instant::now();assert!(CommonAssembler::new(Duration::ZERO,now).is_err());assert!(CommonAssembler::new(Duration::from_secs(31),now).is_err());
 for threshold in [0,7,NATIVE_FRAGMENT_THRESHOLD+1]{assert!(encode(CallKind::Reply,b"x",threshold,None).is_err());}
 assert!(encode(CallKind::Reply,b"x",8,Some(0)).is_err());assert!(encode(CallKind::Reply,b"x",8,Some(MAX_STREAM_BYTES+1)).is_err());
 assert_eq!(assembler(now).on_packet(&[2],now,now).err(),Some(CommonError::Timeout));
}
#[test]
fn shorter_continuation_caller_tightens_original_fragment_and_completed_deadline(){
 let now=Instant::now();let mut state=assembler(now);let packets=fragment(b"abcdefgh",0,2);let short=now+Duration::from_millis(50);
 state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(3),now).unwrap();
 let result=state.on_packet(packets[1].expose_to_transport(),short,now).unwrap().unwrap();assert_eq!(result.deadline,short);
 let mut state=assembler(now);state.on_packet(packets[0].expose_to_transport(),now+Duration::from_secs(3),now).unwrap();
 state.on_packet(&[3,b'c'],short,now).unwrap();assert_eq!(state.deadline(),Some(short));assert_eq!(state.tick(short),Err(CommonError::Timeout));
}
#[test]
fn expanded_output_and_retained_partial_share_the_same_group_budget(){
 let now=Instant::now();let data=vec![b'x';MAX_STREAM_BYTES];let packet=compressed(CallKind::Reply,&data);
 let mut packets:Vec<_>=(0..8).map(|_|TransportBytes::from_bytes(packet.clone())).collect();packets.push(TransportBytes::from_bytes(vec![0x20,0,0,0,8,b'a']));
 let mut state=assembler(now);assert_eq!(state.on_group(&packets,now+Duration::from_secs(5),now).err(),Some(CommonError::Bound));assert!(state.closed);assert_eq!(state.retained_bytes(),0);
}
#[test]
fn announced_expansion_is_admitted_before_decoder_even_with_a_broken_crc(){
 let now=Instant::now();let mut packet=compressed(CallKind::Reply,&[b'x';32]);let last=packet.len()-4;packet[last]^=0x80;
 let mut state=assembler(now);assert_eq!(state.on_packet_budgeted(&packet,now+Duration::from_secs(1),now,now,31).err(),Some(CommonError::Bound));
 assert!(state.closed);assert_eq!(state.retained_bytes(),0);
}
#[test]
fn announced_fragment_reservation_not_just_filled_bytes_is_charged_before_allocation(){
 let now=Instant::now();let mut state=assembler(now);let packet=[0x20,0,2,0,0,b'x'];
 assert_eq!(state.on_packet_budgeted(&packet,now+Duration::from_secs(1),now,now,1).err(),Some(CommonError::Bound));assert_eq!(state.retained_bytes(),0);
 let mut state=assembler(now);state.on_packet(&packet,now+Duration::from_secs(1),now).unwrap();assert_eq!(state.retained_bytes(),MAX_STREAM_BYTES);
}
#[test]
fn fragmented_call_count_is_bounded_across_separate_input_groups(){
 let now=Instant::now();let mut state=assembler(now);state.on_packet(&[0x20,0,0,1,1,b'x'],now+Duration::from_secs(1),now).unwrap();
 for _ in 0..255{assert!(state.on_packet(&[3,b'x'],now+Duration::from_secs(1),now).unwrap().is_none());}
 assert_eq!(state.on_packet(&[3,b'x'],now+Duration::from_secs(1),now).err(),Some(CommonError::Bound));assert!(state.closed);
}
#[test]
fn retained_group_receipt_time_bounds_new_common_partial_not_later_processing_time(){
 let now=Instant::now();let mut state=assembler(now);
 assert_eq!(state.on_packet_budgeted(&[0x20,0,0,0,4,b'x'],now+Duration::from_secs(10),now,now+Duration::from_secs(2),MAX_GROUP_BYTES).err(),Some(CommonError::Timeout));
 let mut state=assembler(now);assert_eq!(state.on_packet_budgeted(&[2],now+Duration::from_secs(10),now+Duration::from_millis(1),now,MAX_GROUP_BYTES).err(),Some(CommonError::ClockRollback));
}
