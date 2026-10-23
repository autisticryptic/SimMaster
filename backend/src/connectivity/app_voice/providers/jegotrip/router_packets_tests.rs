use super::*;

fn header(sequence: u16) -> RouterHeader {
    RouterHeader { sequence, word1: 0x2345, word2: 0x6789, byte6: 0xab, packet_kind: 1, fragment_part: 1 }
}
fn routing<'a>(paths: &'a [u16], address1: &'a [u8], address2: &'a [u8]) -> RouterRouting<'a> {
    RouterRouting { tag: 0x12345678, flags: 2, path_words: paths,
        address1: RouterAddress { mask: 0, label: 0x56, bytes: address1 },
        address2: RouterAddress { mask: 0, label: 0x78, bytes: address2 } }
}

#[test]
fn router_header_independent_fixture_preserves_three_be_words_and_nibbles() {
    let h = header(0x0123); let bytes = encode_header(h).unwrap();
    assert_eq!(bytes, [0x01, 0x23, 0x23, 0x45, 0x67, 0x89, 0xab, 0x11]);
    let (decoded, tail) = decode_header(&[0x01, 0x23, 0x23, 0x45, 0x67, 0x89, 0xab, 0x11, 0xff]).unwrap();
    assert_eq!(decoded, h); assert_eq!(tail, &[0xff]);
}

#[test]
fn routing_fixture_has_native_tag_flags_paths_address_order_without_extra_length() {
    let paths = [0x90ab, 0xcdef]; let r = routing(&paths, &[0x10, 0x11], &[0x20, 0x21]);
    let bytes = encode_routed_body(&r, &[0xff, 0xff, 6, 0]).unwrap();
    let expected = [0x12, 0x34, 0x56, 0x78, 2, 2, 0x56, 0x78, 0x90, 0xab, 0xcd, 0xef, 0x10, 0x11, 0x20, 0x21, 0xff, 0xff, 6, 0];
    assert_eq!(bytes.expose_to_transport(), expected);
    let decoded = decode_routed_body(&expected).unwrap();
    assert_eq!(decoded.tag, r.tag); assert_eq!(decoded.path_words, paths);
    assert_eq!(decoded.address1.bytes, r.address1.bytes); assert_eq!(decoded.address2.bytes, r.address2.bytes);
    assert_eq!(decoded.payload, &[0xff, 0xff, 6, 0]);
}

#[test]
fn address_masks_use_recovered_lengths_without_interpreting_endpoint_values() {
    for first_mask in [0, 1, 4, 5] {
        for second_mask in 0..8 {
            let first = vec![0xff; ADDRESS_BYTES[first_mask as usize]];
            let second = vec![0x80; ADDRESS_BYTES[second_mask as usize]];
            let paths = [0x1234, 0, 0xffff];
            let r = RouterRouting { tag: 7, flags: 3, path_words: &paths,
                address1: RouterAddress { mask: first_mask, label: 1, bytes: &first },
                address2: RouterAddress { mask: second_mask, label: 2, bytes: &second } };
            let bytes = encode_routed_body(&r, &[0, 0xff]).unwrap();
            assert_eq!(bytes.expose_to_transport()[5], 3 | (first_mask << 2) | (second_mask << 5));
            let decoded = decode_routed_body(bytes.expose_to_transport()).unwrap();
            assert_eq!(decoded.address1.bytes, first); assert_eq!(decoded.address2.bytes, second);
            assert_eq!(decoded.path_words, paths); assert_eq!(decoded.payload, &[0, 0xff]);
        }
    }
}

#[test]
fn extension_flags_legacy_address_decoration_and_invalid_masks_are_explicitly_unsupported() {
    for flags in [4, 0x20, 0x40, 0x80, 0xff] {
        let r = RouterRouting { flags, ..routing(&[], &[0, 1], &[2, 3]) };
        assert_eq!(encode_routed_body(&r, &[0]).err(), Some(TransportError::UnsupportedRoute));
        assert_eq!(decode_routed_body(&[0, 0, 0, 0, flags, 0, 0, 0, 0, 1, 2, 3, 0]).err(), Some(TransportError::UnsupportedRoute));
    }
    for mask in [2, 3, 6, 7, 8, 255] {
        let r = RouterRouting { address1: RouterAddress { mask, label: 0, bytes: &[0; 18] }, ..routing(&[], &[0, 1], &[2, 3]) };
        assert!(encode_routed_body(&r, &[0]).is_err());
    }
    let r = RouterRouting { address2: RouterAddress { mask: 8, label: 0, bytes: &[0; 2] }, ..routing(&[], &[0, 1], &[2, 3]) };
    assert_eq!(encode_routed_body(&r, &[0]).err(), Some(TransportError::UnsupportedRoute));
}

#[test]
fn routing_truncation_paths_empty_payload_and_local_bounds_fail_before_unbounded_copy() {
    let r = routing(&[], &[1, 2], &[3, 4]); let bytes = encode_routed_body(&r, &[9]).unwrap();
    for length in 0..bytes.expose_to_transport().len() {
        assert_eq!(decode_routed_body(&bytes.expose_to_transport()[..length]).err(), Some(TransportError::Truncated));
    }
    assert!(encode_routed_body(&routing(&[1, 2, 3, 4], &[1, 2], &[3, 4]), &[9]).is_err());
    assert!(encode_routed_body(&routing(&[], &[1], &[3, 4]), &[9]).is_err());
    assert!(encode_routed_body(&r, &[]).is_err());
    assert_eq!(encode_routed_body(&r, &vec![0; MAX_ROUTED_BODY_BYTES]).err(), Some(TransportError::TooLarge));
    assert_eq!(decode_routed_body(&vec![0; MAX_ROUTED_BODY_BYTES + 1]).err(), Some(TransportError::TooLarge));
}

#[test]
fn exact_mtu_uses_two_balanced_native_halves_and_adjacent_router_sequences() {
    let body = (0..13).collect::<Vec<u8>>();
    let packets = encode_router_packets(header(10), &body, 21).unwrap();
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].expose_to_transport(), &[0, 10, 0x23, 0x45, 0x67, 0x89, 0xab, 0x21, 0, 1, 2, 3, 4, 5]);
    assert_eq!(packets[1].expose_to_transport(), &[0, 11, 0x23, 0x45, 0x67, 0x89, 0xab, 0x31, 6, 7, 8, 9, 10, 11, 12]);
    let single = encode_router_packets(header(10), &body, 22).unwrap(); assert_eq!(single.len(), 1);
    assert_eq!(single[0].expose_to_transport()[7], 0x11);
}

#[test]
fn ordered_fragment_pair_reassembles_before_routing_and_preserves_each_header() {
    let body = encode_routed_body(&routing(&[], &[1, 2], &[3, 4]), &[0xff, 0xff, 6, 0xff, 0]).unwrap();
    let packets = encode_router_packets(header(10), body.expose_to_transport(), 24).unwrap();
    let mut assembler = RouterPacketAssembler::default();
    assert!(assembler.on_packet(packets[0].expose_to_transport()).unwrap().is_none());
    let message = assembler.on_packet(packets[1].expose_to_transport()).unwrap().unwrap();
    assert_eq!(message.first_header.sequence, 10); assert_eq!(message.first_header.fragment_part, 2);
    assert_eq!(message.continuation_header.unwrap().sequence, 11);
    assert_eq!(decode_routed_body(message.routing_and_payload.expose_to_transport()).unwrap().payload, &[0xff, 0xff, 6, 0xff, 0]);
    assert!(assembler.finish_eof().is_ok());
}

#[test]
fn second_half_first_wrong_kind_gap_or_interleaved_start_closes_safe_subset() {
    let original = encode_router_packets(header(10), &[1; 20], 24).unwrap();
    assert_eq!(RouterPacketAssembler::default().on_packet(original[1].expose_to_transport()).err(), Some(TransportError::InvalidFragment));
    for next in [packet(RouterHeader { sequence: 12, fragment_part: 3, ..header(10) }, &[2; 10]).unwrap(),
        packet(RouterHeader { sequence: 11, packet_kind: 0, fragment_part: 3, ..header(10) }, &[2; 10]).unwrap(),
        packet(header(11), &[2; 10]).unwrap(), TransportBytes::from_bytes(original[0].expose_to_transport().to_vec())] {
        let mut assembler = RouterPacketAssembler::default(); assembler.on_packet(original[0].expose_to_transport()).unwrap();
        assert_eq!(assembler.on_packet(next.expose_to_transport()).err(), Some(TransportError::InvalidFragment));
        assert_eq!(assembler.on_packet(original[1].expose_to_transport()).err(), Some(TransportError::Closed));
    }
}

#[test]
fn unknown_controls_reserved_header_bits_and_sequence_wrap_do_not_downgrade() {
    for byte in [0, 2, 0x10, 0x12, 0x40, 0x18, 0x91, 0xff] {
        let mut bytes = [0; 9]; bytes[7] = byte; bytes[8] = 1;
        if byte == 0x10 { assert!(decode_header(&bytes).is_ok()); }
        else { assert_eq!(decode_header(&bytes).err(), Some(TransportError::UnsupportedKind)); }
    }
    for length in 0..8 { assert_eq!(decode_header(&[0; 8][..length]).err(), Some(TransportError::Truncated)); }
    assert_eq!(encode_router_packets(header(u16::MAX), &[0; 20], 24).err(), Some(TransportError::InvalidFragment));
    let first = packet(RouterHeader { fragment_part: 2, ..header(u16::MAX) }, &[1]).unwrap();
    assert_eq!(RouterPacketAssembler::default().on_packet(first.expose_to_transport()).err(), Some(TransportError::InvalidFragment));
}

#[test]
fn oversized_pair_and_incomplete_eof_never_emit_a_router_message() {
    let first = packet(RouterHeader { fragment_part: 2, ..header(1) }, &vec![0; MAX_ROUTED_BODY_BYTES]).unwrap();
    let second = packet(RouterHeader { fragment_part: 3, ..header(2) }, &[0]).unwrap();
    let mut assembler = RouterPacketAssembler::default(); assert!(assembler.on_packet(first.expose_to_transport()).unwrap().is_none());
    assert_eq!(assembler.on_packet(second.expose_to_transport()).err(), Some(TransportError::TooLarge));
    let mut assembler = RouterPacketAssembler::default();
    assembler.on_packet(packet(RouterHeader { fragment_part: 2, ..header(1) }, &[0]).unwrap().expose_to_transport()).unwrap();
    assert_eq!(assembler.finish_eof(), Err(TransportError::Truncated));
    assert_eq!(assembler.on_packet(&[0]).err(), Some(TransportError::Closed));
    assert_eq!(encode_router_packets(header(1), &[0; 50], 16).err(), Some(TransportError::TooLarge));
}

#[test]
fn debug_redacts_address_path_tag_timing_and_partial_router_body() {
    let r = routing(&[], b"xy", b"zz"); let bytes = encode_routed_body(&r, b"fixture-private").unwrap();
    let packets = encode_router_packets(header(1), bytes.expose_to_transport(), 24).unwrap();
    let mut assembler = RouterPacketAssembler::default(); assembler.on_packet(packets[0].expose_to_transport()).unwrap();
    for value in [format!("{r:?}"), format!("{:?}", r.address1), format!("{:?}", header(1)), format!("{assembler:?}")] {
        assert!(!value.contains("fixture")); assert!(!value.contains("xy"));
    }
}
