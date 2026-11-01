use super::*;

// Independent arithmetic vectors from the recovered u32 recurrence, not an
// encoder-generated fixture. Seed0 blocks: a=3249a234,b=0,c=0,s=10923487;
// next a=42dbd6bb,b=1032389d,c=28af389d,s=098b5192.
const ZERO_STREAM:[u8;32]=[
 0x32,0x49,0xa2,0x34,0,0,0,0,0,0,0,0,0x10,0x92,0x34,0x87,
 0x42,0xdb,0xd6,0xbb,0x10,0x32,0x38,0x9d,0x28,0xaf,0x38,0x9d,0x09,0x8b,0x51,0x92,
];

#[test]
fn seeded_zero_record_matches_two_independent_native_blocks_and_be_seed_prefix(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(0)).unwrap();let mut plain=vec![9];plain.extend_from_slice(&[0;32]);
 let encoded=sender.encode(&plain).unwrap();let mut expected=vec![9,0,0,0,0];expected.extend_from_slice(&ZERO_STREAM);
 assert_eq!(encoded.expose_to_transport(),expected);assert_eq!(sender.cipher.as_ref().unwrap().counter,32);
 let mut receiver=RecordReceiver::new(RecordSecurity::SeededXor);
 assert_eq!(receiver.decode(&expected).unwrap().expose_to_transport(),plain);assert_eq!(receiver.cipher.as_ref().unwrap().counter,32);
}

#[test]
fn changing_record_type_does_not_restart_direction_state_or_repeat_seed(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(1)).unwrap();
 assert_eq!(sender.encode(&[7,b'a',b'b',b'c']).unwrap().expose_to_transport(),[7,0,0,0,1,0x53,0x2b,0xc1]);
 assert_eq!(sender.encode(&[8,0,0,0]).unwrap().expose_to_transport(),[8,0x35,0x32,0x49]);
 assert_eq!(sender.encode(&[9,0,0,0,0,0,0,0,0,0,0]).unwrap().expose_to_transport(),[9,0xa2,0x35,0x90,0x7c,0xa2,0x35,0x80,0xee,0x96,0xbd]);
 assert_eq!(sender.cipher.as_ref().unwrap().counter,16);
 let mut receiver=RecordReceiver::new(RecordSecurity::SeededXor);
 assert_eq!(receiver.decode(&[7,0,0,0,1,0x53,0x2b,0xc1]).unwrap().expose_to_transport(),[7,b'a',b'b',b'c']);
 assert_eq!(receiver.decode(&[8,0x35,0x32,0x49]).unwrap().expose_to_transport(),[8,0,0,0]);
 assert_eq!(receiver.decode(&[9,0xa2,0x35,0x90,0x7c,0xa2,0x35,0x80,0xee,0x96,0xbd]).unwrap().expose_to_transport(),[9,0,0,0,0,0,0,0,0,0,0]);
}

#[test]
fn seed_max_uses_u32_wrap_without_sign_extension_or_host_endian_bytes(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(u32::MAX)).unwrap();let plain=[3,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0];
 assert_eq!(sender.encode(&plain).unwrap().expose_to_transport(),[3,0xff,0xff,0xff,0xff,0x32,0x49,0xa2,0x33,0xcd,0xb6,0x5d,0xcd,0x90,0x7b,0x5d,0xcd,0x80,0xe9,0x69,0x4b]);
}

#[test]
fn type_and_seed_bytes_do_not_advance_cipher_payload_counter(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(1)).unwrap();
 assert_eq!(sender.encode(&[0xfe]).unwrap().expose_to_transport(),[0xfe,0,0,0,1]);assert_eq!(sender.cipher.as_ref().unwrap().counter,0);
 assert_eq!(sender.encode(&[8,0]).unwrap().expose_to_transport(),[8,0x32]);
 let mut receiver=RecordReceiver::new(RecordSecurity::SeededXor);assert_eq!(receiver.decode(&[0xfe,0,0,0,1]).unwrap().expose_to_transport(),[0xfe]);
 assert_eq!(receiver.cipher.as_ref().unwrap().counter,0);assert_eq!(receiver.decode(&[8,0x32]).unwrap().expose_to_transport(),[8,0]);
}

#[test]
fn direction_states_are_independent_even_when_both_records_arrive_in_one_turn(){
 let mut outgoing=RecordSender::new(RecordSecurity::SeededXor,Some(0)).unwrap();let mut incoming=RecordReceiver::new(RecordSecurity::SeededXor);
 outgoing.encode(&[9,0,0,0]).unwrap();assert_eq!(incoming.decode(&[7,0,0,0,1,0x53,0x2b,0xc1]).unwrap().expose_to_transport(),[7,b'a',b'b',b'c']);
 assert_eq!(outgoing.encode(&[10,0]).unwrap().expose_to_transport(),[10,0x34]);
 assert_eq!(incoming.decode(&[8,0x35]).unwrap().expose_to_transport(),[8,0]);
}

#[test]
fn record_boundaries_do_not_change_continuous_xor_block_position(){
 for split in 1..32 {
  let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(0)).unwrap();
  let mut first=vec![1];first.extend_from_slice(&vec![0;split]);let first=sender.encode(&first).unwrap();
  let mut second=vec![2];second.extend_from_slice(&vec![0;32-split]);let second=sender.encode(&second).unwrap();
  assert_eq!(&first.expose_to_transport()[5..],&ZERO_STREAM[..split]);assert_eq!(&second.expose_to_transport()[1..],&ZERO_STREAM[split..]);
 }
}

#[test]
fn payload_counter_wrap_retains_evolved_key_and_state_not_a_new_seed(){
 let mut state=LegacyState::new(0);state.counter=u32::MAX-15;let mut bytes=[0;16];state.apply(&mut bytes);
 assert_eq!(bytes,[0x32,0x49,0xa2,0x34,0,0,0,0,0,0,0,0,0x20,0x92,0x34,0x86]);
 assert_eq!(state.counter,0);assert_eq!(state.key,0x10923487);assert_eq!(state.state,0x20923486);
}

#[test]
fn malformed_first_seed_or_oversized_peer_record_closes_without_resynchronization(){
 for first in [vec![],vec![1],vec![1,0],vec![1,0,0],vec![1,0,0,0],vec![0;MAX_PLAIN_RECORD+5]] {
  let mut receiver=RecordReceiver::new(RecordSecurity::SeededXor);assert!(receiver.decode(&first).is_err());
  assert!(receiver.cipher.is_none());assert_eq!(receiver.decode(&[7,0,0,0,1]).err(),Some(TransportError::Closed));
 }
}

#[test]
fn sender_bounds_fail_before_advancing_or_consuming_first_security_prefix(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(1)).unwrap();
 assert_eq!(sender.encode(&[]).err(),Some(TransportError::InvalidPacket));
 assert_eq!(sender.encode(&vec![0;MAX_PLAIN_RECORD+1]).err(),Some(TransportError::TooLarge));assert!(sender.cipher.is_none());
 assert_eq!(sender.encode(&[7,b'a',b'b',b'c']).unwrap().expose_to_transport(),[7,0,0,0,1,0x53,0x2b,0xc1]);
}

#[test]
fn plain_mode_is_explicit_and_never_sniffs_a_seed_from_record_contents(){
 assert!(RecordSender::new(RecordSecurity::Plain,Some(1)).is_err());assert!(RecordSender::new(RecordSecurity::SeededXor,None).is_err());
 let mut sender=RecordSender::new(RecordSecurity::Plain,None).unwrap();let mut receiver=RecordReceiver::new(RecordSecurity::Plain);
 let bytes=[7,0,0,0,1,0x53,0x2b,0xc1];assert_eq!(sender.encode(&bytes).unwrap().expose_to_transport(),bytes);
 assert_eq!(receiver.decode(&bytes).unwrap().expose_to_transport(),bytes);assert!(sender.cipher.is_none());assert!(receiver.cipher.is_none());
}

#[test]
fn local_maximum_record_is_bounded_independently_of_native_megabyte_record_limit(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(0)).unwrap();let plain=vec![0;MAX_PLAIN_RECORD];
 let encoded=sender.encode(&plain).unwrap();assert_eq!(encoded.expose_to_transport().len(),MAX_PLAIN_RECORD+4);
 let mut receiver=RecordReceiver::new(RecordSecurity::SeededXor);assert_eq!(receiver.decode(encoded.expose_to_transport()).unwrap().expose_to_transport(),plain);
}

#[test]
fn close_wipes_direction_state_and_refuses_reuse_for_another_stream(){
 let mut sender=RecordSender::new(RecordSecurity::SeededXor,Some(1)).unwrap();sender.encode(&[1,2,3]).unwrap();sender.close();
 assert!(sender.seed.is_none());assert!(sender.cipher.is_none());assert_eq!(sender.encode(&[1]).err(),Some(TransportError::Closed));
 let mut receiver=RecordReceiver::new(RecordSecurity::SeededXor);receiver.decode(&[7,0,0,0,1]).unwrap();receiver.close();
 assert!(receiver.cipher.is_none());assert_eq!(receiver.decode(&[7]).err(),Some(TransportError::Closed));
}

#[test]
fn private_sender_clone_can_stage_a_record_without_committing_on_queue_failure(){
 let mut original=RecordSender::new(RecordSecurity::SeededXor,Some(1)).unwrap();let mut staged=original.clone();staged.encode(&[1,2,3]).unwrap();drop(staged);
 assert!(original.cipher.is_none());assert_eq!(original.encode(&[7,b'a',b'b',b'c']).unwrap().expose_to_transport(),[7,0,0,0,1,0x53,0x2b,0xc1]);
}

#[test]
fn channel_escape_vectors_preserve_fe_ff_and_do_not_select_inner_tls(){
 for (plain,wire) in [(&[1,2][..],&[0xff,0xff,1,2][..]),(&[0xfe,2][..],&[0xff,0xff,0xfe,0xfe,2][..]),
  (&[0xff,2][..],&[0xff,0xff,0xfe,0xff,2][..]),(&[6,2][..],&[0xff,0xff,6,2][..])]{
  assert_eq!(escape_channel(plain).unwrap().expose_to_transport(),wire);assert_eq!(unescape_channel(wire).unwrap(),plain);
 }
}

#[test]
fn raw_common_escape_bad_prefix_and_truncation_cannot_enter_channel_decoder(){
 for bytes in [&[][..],&[0xff][..],&[0xff,0xff][..],&[0xff,0xff,0xfe][..],&[0xff,0xff,0xff,1][..],&[1,2,3][..],&[0xff,0xff,0xfe,1][..]]{
  assert!(unescape_channel(bytes).is_err());
 }
 assert!(escape_channel(&[]).is_err());assert!(escape_channel(&vec![0;MAX_NETWORK_PACKET_BYTES-1]).is_err());
 assert!(unescape_channel(&vec![0;MAX_NETWORK_PACKET_BYTES+1]).is_err());
}

#[test]
fn debug_redacts_record_state_seed_and_contents(){
 let sender=RecordSender::new(RecordSecurity::SeededXor,Some(1234567)).unwrap();let receiver=RecordReceiver::new(RecordSecurity::SeededXor);
 let text=format!("{sender:?} {receiver:?}");assert!(text.contains("<redacted>"));assert!(!text.contains("1234567"));
}
