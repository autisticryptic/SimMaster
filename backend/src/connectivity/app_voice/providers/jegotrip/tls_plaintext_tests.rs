use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    rpc_envelope::{encode_reply, encode_request}, rpc_stream::RpcProperty,
    transport_packets::encode_call_packets,
};

fn request() -> Vec<u8> {
    let bytes = encode_request(7, &[RpcProperty { key: &[0, 0xff], value: &[3, 4, 14, 0x20, 0xff] }],
        b"fixture.method", &[0, 0, 1, 0, 0, 3, 14, 0xff]).unwrap();
    encode_call_packets(CallKind::Request, bytes.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap()[0].expose_to_transport().to_vec()
}
fn reply(body: &[u8]) -> Vec<u8> {
    let bytes = encode_reply(9, 0, body).unwrap();
    encode_call_packets(CallKind::Reply, bytes.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap().into_iter()
        .flat_map(|packet| packet.expose_to_transport().to_vec()).collect()
}
fn feed(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut decoder = PlaintextPacketDecoder::default(); let mut packets = vec![];
    for chunk in chunks { packets.extend(decoder.on_bytes(chunk).unwrap().iter().map(|p| p.expose_to_transport().to_vec())); }
    assert!(decoder.finish_eof().is_ok()); packets
}

#[test]
fn ordinary_native_request_reply_and_controls_are_self_delimiting_without_inner_prefix() {
    let request = request(); let reply = reply(&[0, 1, 0, 0, 0, 0]);
    let all = [vec![2], request.clone(), reply.clone(), vec![14], vec![4]].concat();
    assert_eq!(feed(&[&all]), vec![vec![2], request, reply, vec![14], vec![4]]);
}

#[test]
fn every_plaintext_split_preserves_header_like_binary_metadata_and_nested_fields() {
    let request = request(); let reply = reply(&[0, 3, 14, 0x21, 0xff, 0]);
    let all = [request.clone(), reply.clone()].concat();
    for split in 0..=all.len() {
        assert_eq!(feed(&[&all[..split], &all[split..]]), vec![request.clone(), reply.clone()], "split {split}");
    }
    let chunks = all.chunks(1).collect::<Vec<_>>(); assert_eq!(feed(&chunks), vec![request, reply]);
}

#[test]
fn canonical_mode_three_fragments_survive_tls_record_splits_and_coalescing() {
    for body_length in [TLS_FRAGMENT_THRESHOLD - 13, TLS_FRAGMENT_THRESHOLD - 12, TLS_FRAGMENT_THRESHOLD + 100, MAX_FIELD_BYTES] {
        let payload = encode_reply(1, 0, &vec![0xff; body_length]).unwrap();
        let expected = encode_call_packets(CallKind::Reply, payload.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
        let all = expected.iter().flat_map(|p| p.expose_to_transport().to_vec()).collect::<Vec<_>>();
        let decoded = feed(&all.chunks(4096).collect::<Vec<_>>());
        assert_eq!(decoded, expected.iter().map(|p| p.expose_to_transport().to_vec()).collect::<Vec<_>>());
        assert_eq!(feed(&[&all]), decoded);
    }
}

#[test]
fn independent_fragment_lengths_use_original_payload_plus_four_balanced_plan() {
    // The reply envelope has 13 bytes before its length-prefixed result; with
    // result16400 its original length is16413, split length16417 ->8208+8209.
    let payload = encode_reply(1, 0, &vec![0xff; 16_400]).unwrap();
    assert_eq!(payload.expose_to_transport().len(), 16_413);
    let packets = encode_call_packets(CallKind::Reply, payload.expose_to_transport(), TLS_FRAGMENT_THRESHOLD).unwrap();
    assert_eq!(packets.iter().map(|p| p.expose_to_transport().len()).collect::<Vec<_>>(), vec![8209, 8210]);
    assert_eq!(&packets[0].expose_to_transport()[..5], &[0x21, 0, 0, 0x40, 0x1d]);
    let all = [packets[0].expose_to_transport(), packets[1].expose_to_transport()].concat();
    assert_eq!(feed(&[&all])[1][0], 3);
}

#[test]
fn malformed_counts_lengths_streams_ids_and_duplicate_metadata_fail_closed() {
    let request = request();
    for malformed in [vec![0, 1], vec![1, 0, 0, 0, 0, 0], vec![1, 0, 0xff, 0xff, 0xff, 0xff],
        vec![0, 0, 0, 0, 0, 1, 0, 0, 0xff, 0xff], vec![1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]] {
        let mut decoder = PlaintextPacketDecoder::default(); assert!(decoder.on_bytes(&malformed).is_err());
        assert_eq!(decoder.on_bytes(&request).err(), Some(TransportError::Closed));
    }
    // Two empty metadata keys are duplicates even though their boundary lengths
    // would be structurally valid; full envelope validation runs before output.
    let duplicate = [0, 0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, b'm', 0, 0, 0, 1, 0];
    assert_eq!(PlaintextPacketDecoder::default().on_bytes(&duplicate).err(), Some(TransportError::InvalidPacket));
}

#[test]
fn unsupported_compression_kinds_noncanonical_threshold_and_orphan_continuation_are_not_guessed() {
    for value in [0x10, 0x11, 0x30, 0x40, 0x80, 0xff, 5, 6, 15] {
        assert!(PlaintextPacketDecoder::default().on_bytes(&[value]).is_err());
    }
    for value in [0, 1, TLS_FRAGMENT_THRESHOLD as u32] {
        let mut bytes = vec![0x21]; bytes.extend_from_slice(&value.to_be_bytes());
        assert_eq!(PlaintextPacketDecoder::default().on_bytes(&bytes).err(), Some(TransportError::InvalidFragment));
    }
    assert_eq!(PlaintextPacketDecoder::default().on_bytes(&[3, 1]).err(), Some(TransportError::InvalidFragment));
    let mut huge = vec![0x21]; huge.extend_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(PlaintextPacketDecoder::default().on_bytes(&huge).err(), Some(TransportError::TooLarge));
}

#[test]
fn incomplete_prefix_envelope_fragment_and_eof_have_no_read_boundary_success() {
    for bytes in [&[0][..], &[1, 0][..], &[1, 0, 0, 0, 0, 1][..], &[0x21, 0, 0, 0x40, 0x1d, 0][..]] {
        let mut decoder = PlaintextPacketDecoder::default(); assert!(decoder.on_bytes(bytes).unwrap().is_empty());
        assert!(decoder.has_partial()); assert!(decoder.on_bytes(&[]).unwrap().is_empty());
        assert_eq!(decoder.finish_eof(), Err(TransportError::Truncated));
        assert_eq!(decoder.on_bytes(&[2]).err(), Some(TransportError::Closed));
    }
}

#[test]
fn later_invalid_packet_suppresses_same_input_outputs_and_cannot_reauthenticate() {
    let all = [request(), vec![0xff]].concat(); let mut decoder = PlaintextPacketDecoder::default();
    assert_eq!(decoder.on_bytes(&all).err(), Some(TransportError::UnsupportedKind));
    assert_eq!(decoder.on_bytes(&reply(&[0])).err(), Some(TransportError::Closed));
}

#[test]
fn input_and_packet_count_limits_are_separate_from_tls_record_sizes() {
    assert_eq!(PlaintextPacketDecoder::default().on_bytes(&vec![2; MAX_PLAINTEXT_INPUT_BYTES + 1]).err(), Some(TransportError::TooLarge));
    assert_eq!(PlaintextPacketDecoder::default().on_bytes(&vec![2; MAX_PACKETS_PER_INPUT]).unwrap().len(), MAX_PACKETS_PER_INPUT);
    assert_eq!(PlaintextPacketDecoder::default().on_bytes(&vec![2; MAX_PACKETS_PER_INPUT + 1]).err(), Some(TransportError::TooLarge));
}

#[test]
fn debug_redacts_partial_tokens_and_fragment_contents() {
    let bytes = request(); let mut decoder = PlaintextPacketDecoder::default();
    decoder.on_bytes(&bytes[..bytes.len() - 1]).unwrap();
    assert!(decoder.has_partial()); assert!(!format!("{decoder:?}").contains("fixture")); decoder.close();
    assert!(!decoder.has_partial());
}
