use super::*;

#[test]
fn fragment_fixture_uses_original_be_length_and_balances_the_last_two_chunks() {
    let payload = (0u8..13).collect::<Vec<_>>();
    let packets = encode_call_packets(CallKind::Request, &payload, 8).unwrap();
    assert_eq!(packets.iter().map(|packet| packet.expose_to_transport()).collect::<Vec<_>>(), vec![
        &[0x20, 0, 0, 0, 13, 0, 1, 2, 3][..],
        &[3, 4, 5, 6, 7][..],
        &[3, 8, 9, 10, 11, 12][..],
    ]);
    let mut receiver = CallAssembler::default();
    assert!(receiver.on_packet(packets[0].expose_to_transport()).unwrap().is_none());
    assert!(receiver.on_packet(packets[1].expose_to_transport()).unwrap().is_none());
    let message = receiver.on_packet(packets[2].expose_to_transport()).unwrap().unwrap();
    assert_eq!(message.kind, CallKind::Request);
    assert_eq!(message.payload.expose_to_transport(), payload);
}

#[test]
fn exact_threshold_and_control_kinds_have_no_extra_length_or_version_bytes() {
    let payload = vec![0xff; 8];
    let packets = encode_call_packets(CallKind::Reply, &payload, 8).unwrap();
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].expose_to_transport(), &[1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    for kind in [CallKind::Ping, CallKind::Pong, CallKind::Release] {
        let packets = encode_call_packets(kind, &[], TLS_FRAGMENT_THRESHOLD).unwrap();
        assert_eq!(packets[0].expose_to_transport(), &[kind as u8]);
        assert_eq!(CallAssembler::default().on_packet(packets[0].expose_to_transport()).unwrap().unwrap().kind, kind);
        assert!(encode_call_packets(kind, b"unexpected", 8).is_err());
    }
    assert!(encode_call_packets(CallKind::Request, b"", 8).is_err());
}

#[test]
fn fragmentation_roundtrips_binary_boundaries_and_never_adds_a_small_final_chunk() {
    for length in [9, 12, 13, 15, 16, 17, TLS_FRAGMENT_THRESHOLD + 1, MAX_STREAM_BYTES] {
        let payload = (0..length).map(|index| index as u8).collect::<Vec<_>>();
        let threshold = if length < 20 { 8 } else { TLS_FRAGMENT_THRESHOLD };
        let packets = encode_call_packets(CallKind::Reply, &payload, threshold).unwrap();
        let lengths = packets.iter().map(|packet| packet.expose_to_transport().len() - 1).collect::<Vec<_>>();
        let expected = match length {
            9 => vec![6, 7], 12 => vec![8, 8], 13 => vec![8, 4, 5],
            15 => vec![8, 5, 6], 16 => vec![8, 6, 6], 17 => vec![8, 6, 7],
            value if value == TLS_FRAGMENT_THRESHOLD + 1 => vec![8193, 8194],
            MAX_STREAM_BYTES => vec![TLS_FRAGMENT_THRESHOLD; 7].into_iter().chain([8201, 8201]).collect(),
            _ => unreachable!("fixture boundary"),
        };
        assert_eq!(lengths, expected);
        let mut receiver = CallAssembler::default();
        for (index, packet) in packets.iter().enumerate() {
            let message = receiver.on_packet(packet.expose_to_transport()).unwrap();
            if index + 1 != packets.len() { assert!(message.is_none()); }
            else { assert_eq!(message.unwrap().payload.expose_to_transport(), payload); }
        }
    }
}

#[test]
fn hostile_fragment_length_is_bounded_before_copy_and_poisoning_is_terminal() {
    let mut receiver = CallAssembler::default();
    assert_eq!(receiver.on_packet(&[0x20, 0xff, 0xff, 0xff, 0xff, 0]).err(), Some(TransportError::TooLarge));
    assert_eq!(receiver.on_packet(&[2]).err(), Some(TransportError::Closed));
    for bytes in [&[0x20][..], &[0x20, 0, 0, 0, 0][..], &[0x20, 0, 0, 0, 1, 0][..], &[0x20, 0, 0, 0, 2][..]] {
        assert!(CallAssembler::default().on_packet(bytes).is_err());
    }
}

#[test]
fn orphan_overlong_empty_continuation_or_new_start_during_fragment_fail_closed() {
    assert_eq!(CallAssembler::default().on_packet(&[3, 0]).err(), Some(TransportError::InvalidFragment));
    for next in [&[3][..], &[3, 1, 2, 3, 4, 5][..], &[2][..], &[0x20, 0, 0, 0, 4, 0][..]] {
        let mut receiver = CallAssembler::default();
        assert!(receiver.on_packet(&[0x20, 0, 0, 0, 4, 0]).unwrap().is_none());
        assert_eq!(receiver.on_packet(next).err(), Some(TransportError::InvalidFragment));
        assert_eq!(receiver.on_packet(&[3, 1, 2, 3]).err(), Some(TransportError::Closed));
    }
}

#[test]
fn compressed_unknown_and_reserved_kinds_are_not_interpreted_as_rpc() {
    for header in [0x10, 0x11, 0x30, 0x13] {
        assert_eq!(CallAssembler::default().on_packet(&[header, 0]).err(), Some(TransportError::UnsupportedCompression));
    }
    for header in [5, 6, 7, 15, 0x40, 0x80, 0xff] {
        assert_eq!(CallAssembler::default().on_packet(&[header, 0]).err(), Some(TransportError::UnsupportedKind));
    }
}

#[test]
fn tls_carrier_fixture_matches_ff_ff_data_key_tls_and_preserves_raw_bytes() {
    let tls = &[0x16, 3, 3, 0, 2, 0xff, 0];
    let bytes = encode_tls_carrier(tls).unwrap();
    assert_eq!(bytes.expose_to_transport(), &[0xff, 0xff, 6, 0x16, 3, 3, 0, 2, 0xff, 0]);
    assert_eq!(decode_tls_carrier(bytes.expose_to_transport()).unwrap(), tls);
}

#[test]
fn tls_required_route_rejects_plaintext_legacy_kx_or_escaped_netstream_packets() {
    for packet in [&[0, 0, 6, 1][..], &[0xff, 0xff, 5, 1][..], &[0xff, 0xff, 0, 1][..], &[0xff, 0xff, 0xff, 1][..]] {
        assert_eq!(decode_tls_carrier(packet).err(), Some(TransportError::InvalidPacket));
    }
    for packet in [&[][..], &[0xff][..], &[0xff, 0xff][..], &[0xff, 0xff, 6][..]] {
        assert_eq!(decode_tls_carrier(packet).err(), Some(TransportError::Truncated));
    }
}

#[test]
fn local_packet_limits_and_invalid_fragment_thresholds_are_explicit() {
    for threshold in [0, 1, 4, 7, TLS_FRAGMENT_THRESHOLD + 1, usize::MAX] {
        assert!(encode_call_packets(CallKind::Request, b"data", threshold).is_err());
    }
    let large = vec![0; MAX_STREAM_BYTES + 1];
    assert_eq!(encode_call_packets(CallKind::Request, &large, 8).err(), Some(TransportError::TooLarge));
    assert_eq!(encode_tls_carrier(&large).err(), Some(TransportError::TooLarge));
    assert_eq!(decode_tls_carrier(&vec![0; MAX_CARRIER_BYTES + 1]).err(), Some(TransportError::TooLarge));
    assert!(encode_tls_carrier(&[]).is_err());
}

#[test]
fn retained_buffer_growth_is_geometric_bounded_and_does_not_revoke_caller_copies() {
    let mut bytes=vec![1,2];let caller_copy=bytes.clone();extend_wiping(&mut bytes,&[3,4,5],8).unwrap();
    assert_eq!(bytes,[1,2,3,4,5]);assert!(bytes.capacity()>=5 && bytes.capacity()<=8);assert_eq!(caller_copy,[1,2]);
    assert_eq!(extend_wiping(&mut bytes,&[6,7,8,9],8).err(),Some(TransportError::TooLarge));assert_eq!(bytes,[1,2,3,4,5]);
    assert_eq!(extend_wiping(&mut bytes,&[],4).err(),Some(TransportError::TooLarge));
}

#[test]
fn original_call_fragment_buffer_reserves_announced_bound_without_partial_reallocation() {
    let packets=encode_call_packets(CallKind::Request,b"fixture-private-data",8).unwrap();let mut assembler=CallAssembler::default();
    assembler.on_packet(packets[0].expose_to_transport()).unwrap();let partial=assembler.partial.as_ref().unwrap();
    assert!(partial.bytes.capacity()>=partial.expected);let original=partial.bytes.as_ptr();
    for (index,packet) in packets.iter().enumerate().skip(1) {
        let result=assembler.on_packet(packet.expose_to_transport()).unwrap();
        if index+1<packets.len() { assert_eq!(assembler.partial.as_ref().unwrap().bytes.as_ptr(),original); }
        else { assert_eq!(result.unwrap().payload.expose_to_transport().as_ptr(),original); }
    }
}

#[test]
fn debug_never_contains_carrier_contents_or_partial_plaintext() {
    let packets = encode_call_packets(CallKind::Request, b"fixture-private-data", 8).unwrap();
    let mut receiver = CallAssembler::default();
    receiver.on_packet(packets[0].expose_to_transport()).unwrap();
    assert!(!format!("{receiver:?}").contains("fixture"));
    assert!(!format!("{:?}", packets[0]).contains("fixture"));
    assert!(!format!("{:?}", encode_tls_carrier(b"fixture-private-tls").unwrap()).contains("fixture"));
}
