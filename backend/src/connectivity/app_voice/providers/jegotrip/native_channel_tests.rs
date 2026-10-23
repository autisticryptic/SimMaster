use super::*;

fn p(sequence:u16)->ChannelPacket<'static> { ChannelPacket { timestamp:None,sequence,kind:PacketKind::Data,data_gap:0,
    ack_sequence:0,delivered_sequence:0,inclusive_ack:false,ack_mask:&[],body:&[] } }

#[test]
fn independent_basic_native_fixture_has_three_be16_fields_flags_mask_and_body() {
    let raw=[0x12,0x34,0x22,0x81,0xab,0xcd,0x65,0x43,5,0xff,0,3];
    let packet=decode_packet(HeaderMode::Basic,&raw).unwrap();assert_eq!(packet.sequence,0x1234);assert_eq!(packet.kind,PacketKind::Data);
    assert_eq!(packet.data_gap,2);assert_eq!(packet.ack_sequence,0xabcd);assert_eq!(packet.delivered_sequence,0x6543);
    assert!(packet.inclusive_ack);assert_eq!(packet.ack_mask,&[5]);assert_eq!(packet.body,&[0xff,0,3]);
    assert_eq!(encode_packet(HeaderMode::Basic,&packet).unwrap().expose_to_transport(),raw);
}

#[test]
fn independent_timestamped_fixture_keeps_native_prefix_outside_the_eight_byte_header() {
    let raw=[0x55,0xaa,0,1,0x60,0,0,2,0,3,0x89,0x87,0x22,0x83];
    let packet=decode_packet(HeaderMode::Timestamped,&raw).unwrap();assert_eq!(packet.timestamp,Some(0x55aa));
    assert_eq!(packet.sequence,1);assert_eq!(packet.kind,PacketKind::Probe);assert_eq!(packet.ack_sequence,2);
    assert_eq!(packet.delivered_sequence,3);assert_eq!(packet.body,&[0x89,0x87,0x22,0x83]);
    assert_eq!(encode_packet(HeaderMode::Timestamped,&packet).unwrap().expose_to_transport(),raw);
}

#[test]
fn header_mode_missing_timestamp_bad_flags_masks_short_and_oversize_are_not_invented() {
    let mut packet=p(1);assert!(encode_packet(HeaderMode::Timestamped,&packet).is_err());packet.timestamp=Some(1);
    assert!(encode_packet(HeaderMode::Basic,&packet).is_err());packet.timestamp=None;packet.data_gap=32;
    assert!(encode_packet(HeaderMode::Basic,&packet).is_err());packet.data_gap=0;packet.ack_mask=&[0;128];
    assert!(encode_packet(HeaderMode::Basic,&packet).is_err());assert!(acknowledged_sequences(&packet,0).is_err());
    packet.ack_mask=&[];packet.data_gap=32;assert!(previous_data(&packet).is_err());
    for raw in [vec![0;7],vec![0,0,0xe0,0,0,0,0,0],vec![0,0,0x20,4,0,0,0,0,1],vec![0;MAX_NETWORK_PACKET_BYTES+1]] {
        assert!(decode_packet(HeaderMode::Basic,&raw).is_err());
    }
}

#[test]
fn all_recovered_packet_classes_roundtrip_without_interpreting_fec_or_probes_as_messages() {
    for kind in [PacketKind::Start,PacketKind::Data,PacketKind::Skip,PacketKind::Probe,PacketKind::Fec] {
        let mut packet=p(65535);packet.kind=kind;let wire=encode_packet(HeaderMode::Basic,&packet).unwrap();
        let decoded=decode_packet(HeaderMode::Basic,wire.expose_to_transport()).unwrap();assert_eq!(decoded.kind,kind);
    }
}

#[test]
fn native_ack_bits_are_lsb_first_and_final_ack_sequence_is_independent() {
    let mut packet=p(500);packet.ack_sequence=19;packet.ack_mask=&[0b0000_0101];
    assert_eq!(acknowledged_sequences(&packet,10).unwrap(),vec![16,18,19]);
    packet.ack_sequence=17;packet.ack_mask=&[0,0b0000_0001];
    assert_eq!(acknowledged_sequences(&packet,10).unwrap(),vec![16,17]);
    packet.ack_mask=&[];assert_eq!(acknowledged_sequences(&packet,10).unwrap(),vec![17]);
}

#[test]
fn cumulative_native_ack_is_bounded_by_original_channel_window_and_wraps_locally() {
    let mut packet=p(100);packet.ack_sequence=19;packet.ack_mask=&[1];packet.inclusive_ack=true;
    assert_eq!(acknowledged_sequences(&packet,10).unwrap(),vec![11,12,13,14,15,16,19]);
    packet.ack_sequence=1;packet.ack_mask=&[1];assert_eq!(acknowledged_sequences(&packet,65534).unwrap(),vec![65535,0,1]);
    packet.ack_sequence=5000;packet.ack_mask=&[];assert!(acknowledged_sequences(&packet,0).is_err());
    packet.ack_sequence=9;assert!(acknowledged_sequences(&packet,10).unwrap().is_empty());
}

#[test]
fn native_data_gap_and_escape31_reference_preceding_data_not_last_packet_or_rpc_id() {
    let mut packet=p(101);packet.data_gap=3;packet.body=b"fixture";
    assert_eq!(previous_data(&packet).unwrap(),(97,b"fixture".as_slice()));
    packet.sequence=0;packet.data_gap=0;assert_eq!(previous_data(&packet).unwrap().0,65535);
    packet.data_gap=31;packet.body=&[0x12,0x34,0xff,7];assert_eq!(previous_data(&packet).unwrap(),(0x1234,[0xff,7].as_slice()));
    packet.body=&[0];assert!(previous_data(&packet).is_err());packet.kind=PacketKind::Probe;assert!(previous_data(&packet).is_err());
}

#[test]
fn native_start_skip_continuation_counts_and_unknown_run_types_fail_explicitly() {
    let mut packet=p(100);packet.kind=PacketKind::Start;assert_eq!(starting_sequence(&packet).unwrap(),Some(100));
    packet.kind=PacketKind::Probe;assert!(starting_sequence(&packet).unwrap().is_none());
    packet.kind=PacketKind::Skip;packet.body=&[0x80,0x20,0,0x10];assert_eq!(starting_sequence(&packet).unwrap(),Some(52));
    packet.sequence=0;packet.body=&[0,1];assert_eq!(starting_sequence(&packet).unwrap(),Some(65535));
    for raw in [b"".as_slice(),&[0],&[0x80,1],&[0x40,0],&[0,1,0,1]] { packet.body=raw;assert!(starting_sequence(&packet).is_err()); }
}

#[test]
fn independent_native_unbatched_record_is_ff_payload_not_an_added_inner_be32() {
    let packets=encode_record(&[0,0xff,3],100).unwrap();assert_eq!(packets.len(),1);
    assert_eq!(packets[0].expose_to_transport(),&[0xff,0,0xff,3]);
    let mut receiver=Records::default();let complete=receiver.on_payload(packets[0].expose_to_transport()).unwrap();
    assert_eq!(complete.len(),1);assert_eq!(complete[0].expose_to_transport(),&[0,0xff,3]);
}

#[test]
fn independent_native_be16_batched_records_preserve_binary_bytes_and_order() {
    let mut receiver=Records::default();let complete=receiver.on_payload(&[0,3,b'A',0,0xff,0,2,b'd',b'e']).unwrap();
    assert_eq!(complete.len(),2);assert_eq!(complete[0].expose_to_transport(),&[b'A',0,0xff]);
    assert_eq!(complete[1].expose_to_transport(),b"de");assert!(!receiver.has_partial());
}

#[test]
fn native_fe_ff_fragmentation_reassembles_and_eof_before_final_is_truncated() {
    let packets=encode_record(b"0123456789",5).unwrap();assert_eq!(packets.len(),3);
    assert_eq!(packets[0].expose_to_transport(),b"\xfe0123");assert_eq!(packets[1].expose_to_transport(),b"\xfe4567");
    assert_eq!(packets[2].expose_to_transport(),b"\xff89");
    let mut receiver=Records::default();for chunk in &packets[..2] { assert!(receiver.on_payload(chunk.expose_to_transport()).unwrap().is_empty()); }
    assert!(receiver.has_partial());let complete=receiver.on_payload(packets[2].expose_to_transport()).unwrap();
    assert_eq!(complete[0].expose_to_transport(),b"0123456789");assert!(receiver.finish_eof().is_ok());
    let mut receiver=Records::default();receiver.on_payload(packets[0].expose_to_transport()).unwrap();
    assert!(matches!(receiver.finish_eof(),Err(TransportError::Truncated)));assert!(!receiver.has_partial());
}

#[test]
fn partial_fe_then_be16_prefix_and_following_final_record_do_not_guess_read_boundaries() {
    let mut receiver=Records::default();receiver.on_payload(b"\xfeab").unwrap();
    let complete=receiver.on_payload(b"\0\x02cd\xffef").unwrap();assert_eq!(complete.len(),2);
    assert_eq!(complete[0].expose_to_transport(),b"abcd");assert_eq!(complete[1].expose_to_transport(),b"ef");
}

#[test]
fn malformed_later_record_suppresses_same_input_outputs_and_clears_retention() {
    for bad in [b"\0\0".as_slice(),b"\0",b"\0\x03ab",b"\xfe",b"\xff"] {
        let mut receiver=Records::default();let input=[b"\0\x01a".as_slice(),bad].concat();assert!(receiver.on_payload(&input).is_err());
        assert!(!receiver.has_partial());assert!(matches!(receiver.on_payload(b"\xffok"),Err(TransportError::Closed)));
    }
}

#[test]
fn resource_caps_are_local_and_never_allocate_hostile_unlimited_record_or_packet_flood() {
    assert!(encode_record(&[],10).is_err());assert!(encode_record(&vec![0;MAX_RECORD+1],10).is_err());assert!(encode_record(b"x",1).is_err());
    assert!(encode_record(&vec![0;8192],2).is_err());
    let mut receiver=Records::default();let flood=b"\0\x01x".repeat(MAX_RECORDS_PER_PACKET+1);assert!(receiver.on_payload(&flood).is_err());
    let mut receiver=Records::default();assert!(receiver.on_payload(&vec![0;MAX_NETWORK_PACKET_BYTES+1]).is_err());
}

#[test]
fn large_native_record_roundtrip_uses_explicit_frame_budget_not_transport_read_lengths() {
    let bytes=(0..MAX_RECORD).map(|i|(i%251) as u8).collect::<Vec<_>>();let packets=encode_record(&bytes,8192).unwrap();
    let mut receiver=Records::default();let mut complete=Vec::new();for packet in packets { complete.extend(receiver.on_payload(packet.expose_to_transport()).unwrap()); }
    assert_eq!(complete.len(),1);assert_eq!(complete[0].expose_to_transport(),bytes);assert!(!receiver.has_partial());
}

#[test]
fn tiny_partial_fragments_use_bounded_geometric_growth_and_keep_exact_bytes() {
    let mut records=Records::default();
    for i in 0..2048 {
        records.on_payload(&[0xfe,(i%251) as u8]).unwrap();
        assert!(records.partial.capacity() >= records.partial.len());assert!(records.partial.capacity() <= MAX_RECORD);
        assert!(records.partial.capacity() <= 2 * records.partial.len());
    }
    let output=records.on_payload(&[0xff,9]).unwrap();let mut expected=(0..2048).map(|i|(i%251) as u8).collect::<Vec<_>>();expected.push(9);
    assert_eq!(output[0].expose_to_transport(),expected);assert!(!records.has_partial());
}

#[test]
fn debug_redacts_channel_metadata_record_contents_and_timing_not_as_authentication() {
    let mut packet=p(100);packet.body=b"fixture-private-message";assert!(!format!("{packet:?}").contains("fixture"));
    let mut records=Records::default();records.on_payload(b"\xfefixture-private-message").unwrap();assert!(!format!("{records:?}").contains("fixture"));
}
