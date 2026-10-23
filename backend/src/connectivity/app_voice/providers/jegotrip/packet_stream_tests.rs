use super::*;

#[test]
fn frame_fixture_writes_original_big_endian_length_not_inclusive_length() {
    let packet = [0xff, 0xff, 6, 0, 0x80];
    let frame = encode_packet_stream_frame(&packet).unwrap();
    assert_eq!(frame.expose_to_transport(), &[0, 0, 0, 5, 0xff, 0xff, 6, 0, 0x80]);
    let decoded = PacketStreamDecoder::default().on_bytes(frame.expose_to_transport()).unwrap();
    assert_eq!(decoded[0].expose_to_transport(), packet);
}

#[test]
fn independent_coalesced_fixture_emits_each_announced_packet_in_order() {
    let bytes = [0, 0, 0, 2, 0xff, 1, 0, 0, 0, 3, 0, 0xff, 9];
    let mut decoder = PacketStreamDecoder::default();
    let packets = decoder.on_bytes(&bytes).unwrap();
    assert_eq!(packets.iter().map(|p| p.expose_to_transport()).collect::<Vec<_>>(), vec![&[0xff, 1][..], &[0, 0xff, 9][..]]);
    assert!(!decoder.has_partial()); assert!(decoder.finish_eof().is_ok());
}

#[test]
fn every_split_in_prefix_and_body_preserves_binary_packet_bytes() {
    let bytes = [0, 0, 0, 4, 0, 0xff, 0x80, 2, 0, 0, 0, 1, 3];
    for split in 0..=bytes.len() {
        let mut decoder = PacketStreamDecoder::default();
        let mut packets = decoder.on_bytes(&bytes[..split]).unwrap();
        packets.extend(decoder.on_bytes(&bytes[split..]).unwrap());
        assert_eq!(packets.iter().map(|p| p.expose_to_transport()).collect::<Vec<_>>(), vec![&[0, 0xff, 0x80, 2][..], &[3][..]], "split {split}");
        assert!(decoder.finish_eof().is_ok());
    }
    let mut decoder = PacketStreamDecoder::default(); let mut packets = vec![];
    for byte in bytes { packets.extend(decoder.on_bytes(&[byte]).unwrap()); }
    assert_eq!(packets.len(), 2); assert!(!decoder.has_partial());
}

#[test]
fn partial_second_packet_waits_after_complete_first_without_merging_them() {
    let mut decoder = PacketStreamDecoder::default();
    let packets = decoder.on_bytes(&[0, 0, 0, 1, 9, 0, 0, 0, 3, 7]).unwrap();
    assert_eq!(packets[0].expose_to_transport(), &[9]); assert!(decoder.has_partial());
    assert!(decoder.on_bytes(&[]).unwrap().is_empty());
    let packets = decoder.on_bytes(&[0xff, 0]).unwrap();
    assert_eq!(packets[0].expose_to_transport(), &[7, 0xff, 0]); assert!(!decoder.has_partial());
}

#[test]
fn zero_and_hostile_lengths_fail_before_reserving_packet_and_close_decoder() {
    for prefix in [0u32, (MAX_NETWORK_PACKET_BYTES + 1) as u32, u32::MAX] {
        let mut decoder = PacketStreamDecoder::default();
        let expected = if prefix == 0 { TransportError::InvalidPacket } else { TransportError::TooLarge };
        assert_eq!(decoder.on_bytes(&prefix.to_be_bytes()).err(), Some(expected));
        assert!(decoder.is_closed()); assert!(!decoder.has_partial());
        assert_eq!(decoder.on_bytes(&[0, 0, 0, 1, 1]).err(), Some(TransportError::Closed));
    }
}

#[test]
fn local_maximum_packet_roundtrips_without_native_sixty_four_megabyte_allocation() {
    let packet = (0..MAX_NETWORK_PACKET_BYTES).map(|i| i as u8).collect::<Vec<_>>();
    let frame = encode_packet_stream_frame(&packet).unwrap(); let mut decoder = PacketStreamDecoder::default(); let mut output = vec![];
    for chunk in frame.expose_to_transport().chunks(2048) { output.extend(decoder.on_bytes(chunk).unwrap()); }
    assert_eq!(output.len(), 1); assert_eq!(output[0].expose_to_transport(), packet);
    assert!(encode_packet_stream_frame(&[]).is_err());
    assert_eq!(encode_packet_stream_frame(&vec![0; MAX_NETWORK_PACKET_BYTES + 1]).err(), Some(TransportError::TooLarge));
}

#[test]
fn oversized_input_and_tiny_frame_flood_are_explicit_terminal_local_limits() {
    let mut decoder = PacketStreamDecoder::default();
    assert_eq!(decoder.on_bytes(&vec![0; MAX_STREAM_INPUT_BYTES + 1]).err(), Some(TransportError::TooLarge));
    let packet = [0, 0, 0, 1, 9];
    let accepted = packet.repeat(MAX_PACKETS_PER_INPUT);
    assert_eq!(PacketStreamDecoder::default().on_bytes(&accepted).unwrap().len(), MAX_PACKETS_PER_INPUT);
    let mut decoder = PacketStreamDecoder::default();
    assert_eq!(decoder.on_bytes(&packet.repeat(MAX_PACKETS_PER_INPUT + 1)).err(), Some(TransportError::TooLarge));
    assert!(decoder.is_closed()); assert!(!decoder.has_partial());
}

#[test]
fn bad_later_frame_discards_outputs_from_the_same_input_call() {
    let mut decoder = PacketStreamDecoder::default();
    assert_eq!(decoder.on_bytes(&[0, 0, 0, 1, 9, 0, 0, 0, 0]).err(), Some(TransportError::InvalidPacket));
    assert!(decoder.is_closed());
    assert_eq!(decoder.on_bytes(&[0, 0, 0, 1, 9]).err(), Some(TransportError::Closed));
}

#[test]
fn eof_with_prefix_or_body_is_truncated_and_empty_feed_is_not_eof() {
    for bytes in [&[0][..], &[0, 0, 0][..], &[0, 0, 0, 2][..], &[0, 0, 0, 2, 0xff][..]] {
        let mut decoder = PacketStreamDecoder::default();
        assert!(decoder.on_bytes(bytes).unwrap().is_empty()); assert!(decoder.has_partial());
        assert!(decoder.on_bytes(&[]).unwrap().is_empty()); assert!(!decoder.is_closed());
        assert_eq!(decoder.finish_eof(), Err(TransportError::Truncated));
        assert!(!decoder.has_partial()); assert_eq!(decoder.finish_eof(), Err(TransportError::Closed));
    }
    let mut decoder = PacketStreamDecoder::default(); assert_eq!(decoder.finish_eof(), Ok(()));
    assert_eq!(decoder.on_bytes(&[]).err(), Some(TransportError::Closed));
}

#[test]
fn debug_redacts_partial_and_completed_network_payload() {
    let frame = encode_packet_stream_frame(b"fixture-private-wire").unwrap(); let mut decoder = PacketStreamDecoder::default();
    decoder.on_bytes(&frame.expose_to_transport()[..12]).unwrap();
    assert!(!format!("{decoder:?}").contains("fixture")); assert!(!format!("{frame:?}").contains("fixture"));
    decoder.close(); assert!(!decoder.has_partial());
}
