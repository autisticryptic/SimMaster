use super::*;

fn h(sequence: u16, kind: u8, part: u8) -> RouterHeader { RouterHeader { sequence, word1: 0x1234, word2: 0x5678,
    byte6: 9, packet_kind: kind, fragment_part: part } }
fn wire(sequence: u16, kind: u8, part: u8, body: &[u8]) -> Vec<u8> { packet(h(sequence,kind,part),body).unwrap().expose_to_transport().to_vec() }
fn data(value: PathInput) -> Option<PathMessage> { match value { PathInput::Data { message, .. } => message, _ => panic!("data required") } }

#[test]
fn independent_control_header_and_optional_probe_words_match_native_fixture() {
    let probe = encode_probe(h(0xabcd,2,1), Some(TimingProbe { peer_send_time: 0x0203, peer_sequence: 0x0405 })).unwrap();
    assert_eq!(probe.expose_to_transport(), [0xab,0xcd,0x12,0x34,0x56,0x78,9,0x12,2,3,4,5]);
    let (header,body)=decode_path_header(probe.expose_to_transport()).unwrap();assert_eq!(header.packet_kind,2); assert_eq!(header.fragment_part,1);
    let echo=decode_probe(body).unwrap().unwrap();assert_eq!(echo.peer_send_time,0x0203);assert_eq!(echo.peer_sequence,0x0405);
    assert_eq!(encode_feedback(h(0xabcd,3,1)).unwrap().expose_to_transport(),[0xab,0xcd,0x12,0x34,0x56,0x78,9,0x13]);
    assert!(decode_probe(&[]).unwrap().is_none());
}

#[test]
fn path_controls_never_become_application_data_or_message_acknowledgements() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(matches!(receiver.on_packet(&wire(1,2,1,&[]),now).unwrap(),PathInput::Probe{echo:None,..}));
    assert!(matches!(receiver.on_packet(&wire(2,3,1,&[]),now).unwrap(),PathInput::Feedback{..}));
    assert!(!receiver.has_partial());
}

#[test]
fn malformed_control_kind_part_and_probe_lengths_close_only_this_decoder() {
    for bytes in [vec![0,1,0,0,0,0,0,0x14], vec![0,1,0,0,0,0,0,0x22], wire(1,2,1,&[1]), wire(1,3,1,&[1])] {
        let mut receiver=PathReceiver::default();assert!(receiver.on_packet(&bytes,Instant::now()).is_err());
        assert!(matches!(receiver.on_packet(&wire(1,2,1,&[]),Instant::now()),Err(TransportError::Closed)));
    }
}

#[test]
fn reserved_short_and_oversized_headers_fail_before_payload_allocation() {
    for bytes in [vec![0;7], vec![0;MAX_NETWORK_PACKET_BYTES+1],vec![0,0,0,0,0,0,0,0x18]] { assert!(decode_path_header(&bytes).is_err()); }
    assert!(encode_probe(h(1,1,1),None).is_err()); assert!(encode_feedback(h(1,2,1)).is_err());
}

#[test]
fn native_sequence_bitmap_uses_first_minus512_and_32word_sliding_not_arc_ids() {
    let mut book=SequenceBookkeeping::default();assert_eq!(book.observe(100),ReceiveObservation::FirstOrReset);
    assert_eq!(book.base,Some(100u16.wrapping_sub(512)));assert_eq!(book.head,0);
    assert_eq!(book.observe(100),ReceiveObservation::Duplicate);assert_eq!(book.observe(611),ReceiveObservation::New);assert_eq!(book.head,0);
    assert_eq!(book.observe(612),ReceiveObservation::New);assert_eq!(book.head,1);assert_eq!(book.base,Some(100u16.wrapping_sub(480)));
}

#[test]
fn native_u16_sequence_wrap_remains_one_path_and_duplicates_are_bookkeeping_only() {
    let mut book=SequenceBookkeeping::default();assert_eq!(book.observe(65534),ReceiveObservation::FirstOrReset);
    for sequence in [65535,0,1] { assert_eq!(book.observe(sequence),ReceiveObservation::New); }
    assert_eq!(book.observe(65535),ReceiveObservation::Duplicate);
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(data(receiver.on_packet(&wire(1,1,1,b"fixture"),now).unwrap()).is_some());
    let duplicate=receiver.on_packet(&wire(1,1,1,b"fixture"),now).unwrap();
    assert!(matches!(&duplicate,PathInput::Data{observation:ReceiveObservation::Duplicate,message:Some(_)}));
    assert_eq!(data(duplicate).unwrap().body.expose_to_transport(),b"fixture");
}

#[test]
fn native_window_reset_clears_original_partial_cache_without_silent_data_rebinding() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(data(receiver.on_packet(&wire(1,1,2,b"old"),now).unwrap()).is_none());assert!(receiver.has_partial());
    assert!(data(receiver.on_packet(&wire(5000,1,1,b"new"),now).unwrap()).is_some());assert!(!receiver.has_partial());
    assert!(data(receiver.on_packet(&wire(5001,1,3,b"new tail"),now).unwrap()).is_none());
}

#[test]
fn adjacent_part3_before_part2_pairs_without_reordering_body_bytes() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    assert!(data(receiver.on_packet(&wire(11,1,3,b"second"),now).unwrap()).is_none());
    let message=data(receiver.on_packet(&wire(10,1,2,b"first"),now).unwrap()).unwrap();
    assert_eq!(message.first_header.sequence,10);assert_eq!(message.last_header.unwrap().sequence,11);
    assert_eq!(message.body.expose_to_transport(),b"firstsecond");assert!(!receiver.has_partial());
}

#[test]
fn both_pair_orders_cross_65535_zero_without_global_sequence_reassignment() {
    for order in [[(65535,2,b"first".as_slice()),(0,3,b"second".as_slice())],[(0,3,b"second".as_slice()),(65535,2,b"first".as_slice())]] {
        let mut receiver=PathReceiver::default();let now=Instant::now();
        assert!(data(receiver.on_packet(&wire(order[0].0,1,order[0].1,order[0].2),now).unwrap()).is_none());
        let message=data(receiver.on_packet(&wire(order[1].0,1,order[1].1,order[1].2),now).unwrap()).unwrap();
        assert_eq!(message.body.expose_to_transport(),b"firstsecond");assert_eq!(message.first_header.sequence,65535);assert_eq!(message.last_header.unwrap().sequence,0);
    }
}

#[test]
fn interleaved_adjacent_pairs_stay_separate_and_duplicate_half_does_not_replace_original() {
    let mut receiver=PathReceiver::default();let now=Instant::now();
    for (sequence,body) in [(10,b"A".as_slice()),(12,b"B"),(10,b"A")] { assert!(data(receiver.on_packet(&wire(sequence,1,2,body),now).unwrap()).is_none()); }
    assert_eq!(data(receiver.on_packet(&wire(13,1,3,b"b"),now).unwrap()).unwrap().body.expose_to_transport(),b"Bb");
    assert_eq!(data(receiver.on_packet(&wire(11,1,3,b"a"),now).unwrap()).unwrap().body.expose_to_transport(),b"Aa");
}

#[test]
fn mismatching_adjacent_kind_part_or_retained_body_is_terminal_not_a_new_pair() {
    for second in [wire(11,0,3,b"b"),wire(10,1,3,b"b"),wire(10,1,2,b"changed")] {
        let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"a"),now).unwrap();
        assert!(receiver.on_packet(&second,now).is_err());assert!(!receiver.has_partial());
    }
}

#[test]
fn partial_age_window_resource_caps_and_eof_never_emit_unconfirmed_complete_message() {
    let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"a"),now).unwrap();
    assert!(data(receiver.on_packet(&wire(11,1,3,b"b"),now+HALF_AGE).unwrap()).is_none());
    assert!(matches!(receiver.finish_eof(),Err(TransportError::Truncated)));
    let mut receiver=PathReceiver::default();
    for sequence in (0..MAX_CACHED_HALVES as u16*2).step_by(2) { receiver.on_packet(&wire(sequence,1,2,b"a"),now).unwrap(); }
    assert!(receiver.on_packet(&wire(200,1,2,b"a"),now).is_err());
}

#[test]
fn expired_halves_are_not_retained_by_full_data_probe_or_feedback_traffic() {
    for final_packet in [wire(11,1,1,b"whole"),wire(11,2,1,&[]),wire(11,3,1,&[])] {
        let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"old private half"),now).unwrap();
        assert!(receiver.has_partial());receiver.on_packet(&final_packet,now+HALF_AGE).unwrap();
        assert!(!receiver.has_partial());assert_eq!(receiver.cached_bytes,0);assert!(receiver.finish_eof().is_ok());
    }
}

#[test]
fn clock_rollback_and_combined_body_overflow_close_and_clear_retention() {
    let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"a"),now).unwrap();
    assert!(receiver.on_packet(&wire(11,1,3,b"b"),now-Duration::from_millis(1)).is_err());assert!(!receiver.has_partial());
    let mut receiver=PathReceiver::default();let large=vec![0;MAX_ROUTED_BODY_BYTES];receiver.on_packet(&wire(10,1,2,&large),now).unwrap();
    assert!(receiver.on_packet(&wire(11,1,3,&[0]),now).is_err());assert!(!receiver.has_partial());
}

#[test]
fn debug_is_redacted_for_route_controls_retained_halves_and_reassembled_bytes() {
    let mut receiver=PathReceiver::default();let now=Instant::now();receiver.on_packet(&wire(10,1,2,b"fixture-private-route"),now).unwrap();
    assert!(!format!("{receiver:?}").contains("fixture"));
    let value=receiver.on_packet(&wire(11,1,3,b"private"),now).unwrap();assert!(!format!("{value:?}").contains("fixture"));
}
