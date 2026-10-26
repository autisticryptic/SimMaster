//! Actions-only native Mpath DATA byte/sequence/fragment oracles. No IO/readiness.
use super::*;
fn oracle(part: u8, path: u8, sequence: u16, tick: u16, body: &[u8]) -> Vec<u8> {
    let word = ((part as u32) << 30) | ((path as u32) << 28) | ((sequence as u32) << 14) | (tick as u32 >> 2);
    let mut out = word.to_be_bytes().to_vec(); out.extend_from_slice(body); out
}
fn bytes(input: Option<Input>) -> (u8, Vec<u8>) {
    match input.unwrap() { Input::Payload { channel, payload, .. } => (channel, payload.expose_to_transport().to_vec()),
        Input::Control { .. } => panic!("native control is not audio DATA") }
}
#[test]
fn independent_be32_header_preserves_kind_path_seq14_quantized_clock_and_channel_bytes() {
    let (packets, next) = encode_payload(1, 0x2345, 0xabcd, 0, &[0x80, 0xe0, 0, 0xff]).unwrap();
    assert_eq!(packets[0].expose_to_transport(), &[0x18, 0xd1, 0x6a, 0xf3, 0, 0x80, 0xe0, 0, 0xff]);
    assert_eq!(next, 0x2346);
    let (header, body) = decode_header(packets[0].expose_to_transport()).unwrap();
    assert_eq!(header.part, Part::Whole); assert_eq!(header.path, 1); assert_eq!(header.sequence, 0x2345);
    assert_eq!(header.tick, 0xabcc); assert_eq!(body, &[0, 0x80, 0xe0, 0, 0xff]);
}
#[test]
fn native_single_threshold_is_1459_not_rpc_tls_fragment_or_udp_maximum() {
    let payload = vec![0x80; 1459]; let (packets, next) = encode_payload(0, 7, 100, 0, &payload).unwrap();
    assert_eq!(packets.len(), 1); assert_eq!(packets[0].expose_to_transport().len(), 1464); assert_eq!(next, 8);
    let (packets, next) = encode_payload(0, 7, 100, 0, &vec![0x80; 1460]).unwrap();
    assert_eq!(packets.len(), 2); assert_eq!(next, 9);
    assert_eq!(packets[0].expose_to_transport().len(), 735); assert_eq!(packets[1].expose_to_transport().len(), 734);
}
#[test]
fn independent_balanced_halves_put_channel_only_in_first_and_wrap_at_16384() {
    let payload: Vec<_> = (0..1461).map(|value| (value % 251) as u8).collect();
    let (packets, next) = encode_payload(0, 0x3fff, 0x1234, 7, &payload).unwrap();
    let mut first = vec![0x4f, 0xff, 0xc4, 0x8d, 7]; first.extend_from_slice(&payload[..730]);
    let mut last = vec![0x80, 0, 4, 0x8d]; last.extend_from_slice(&payload[730..]);
    assert_eq!(packets[0].expose_to_transport(), first); assert_eq!(packets[1].expose_to_transport(), last); assert_eq!(next, 1);
}
#[test]
fn native_receiver_joins_both_orders_and_full_width_wrapping_without_losing_binary_bytes() {
    let now = Instant::now(); let payload = vec![0xff; 2918];
    let (packets, _) = encode_payload(2, 0x3fff, 1000, 0, &payload).unwrap();
    for reverse in [false, true] {
        let mut receiver = Receiver::default(); let (first, last) = if reverse { (&packets[1], &packets[0]) } else { (&packets[0], &packets[1]) };
        assert!(receiver.on_packet(first.expose_to_transport(), now).unwrap().is_none());
        assert_eq!(bytes(receiver.on_packet(last.expose_to_transport(), now).unwrap()), (0, payload.clone()));
        assert!(receiver.halves.is_empty());
    }
}
#[test]
fn different_paths_cannot_join_adjacent_half_sequences_or_change_channel_origin() {
    let now = Instant::now(); let mut receiver = Receiver::default();
    let first = oracle(1, 0, 9, 100, &[0, 1, 2]); let other = oracle(2, 1, 10, 100, &[3, 4]);
    assert!(receiver.on_packet(&first, now).unwrap().is_none()); assert!(receiver.on_packet(&other, now).unwrap().is_none());
    assert_eq!(receiver.halves.len(), 2);
    let original = oracle(2, 0, 10, 100, &[3, 4]);
    assert_eq!(bytes(receiver.on_packet(&original, now).unwrap()), (0, vec![1, 2, 3, 4]));
    assert_eq!(receiver.halves.len(), 1);
}
#[test]
fn actual_mpath_dedup_drops_duplicate_whole_packet_unlike_router_statistics_only_bitmap() {
    let now = Instant::now(); let mut receiver = Receiver::default(); let raw = oracle(0, 0, 100, 500, &[0, 0xff]);
    assert_eq!(bytes(receiver.on_packet(&raw, now).unwrap()), (0, vec![0xff]));
    assert!(receiver.on_packet(&raw, now).unwrap().is_none());
    let next = oracle(0, 0, 101, 504, &[0, 7]); assert_eq!(bytes(receiver.on_packet(&next, now).unwrap()), (0, vec![7]));
}
#[test]
fn duplicate_first_half_does_not_destroy_original_pending_fragment() {
    let now = Instant::now(); let mut receiver = Receiver::default(); let first = oracle(1, 0, 12, 50, &[0, 1]);
    assert!(receiver.on_packet(&first, now).unwrap().is_none()); assert!(receiver.on_packet(&first, now).unwrap().is_none());
    assert_eq!(receiver.halves.len(), 1);
    assert_eq!(bytes(receiver.on_packet(&oracle(2, 0, 13, 50, &[2]), now).unwrap()), (0, vec![1, 2]));
}
#[test]
fn source_clock_rollback_poison_close_and_local_half_retention_are_explicit() {
    let now = Instant::now(); let mut receiver = Receiver::default(); let first = oracle(1, 0, 12, 50, &[0, 1]);
    receiver.on_packet(&first, now).unwrap(); receiver.expire(now + HALF_RETENTION).unwrap(); assert!(receiver.halves.is_empty());
    assert!(receiver.expire(now).is_err()); assert!(receiver.closed); assert!(receiver.on_packet(&first, now).is_err());
}
#[test]
fn native_kind_three_control_and_other_media_channels_never_masquerade_as_audio_packets() {
    let now = Instant::now(); let mut receiver = Receiver::default(); let control = oracle(3, 3, 0x3fff, 0xffff, &[0, 1, 2]);
    assert_eq!(&control[..4], &[0xff; 4]);
    assert!(matches!(receiver.on_packet(&control, now).unwrap(), Some(Input::Control { .. })));
    assert_eq!(bytes(receiver.on_packet(&oracle(0, 1, 100, 500, &[1, 0xff]), now).unwrap()), (1, vec![0xff]));
}
#[test]
fn stream_packet_header_or_rpc_bytes_are_not_a_relabelled_mpath_audio_payload() {
    let now = Instant::now(); let mut receiver = Receiver::default();
    assert!(receiver.on_packet(&[0, 0, 1], now).is_err());
    assert!(encode_payload(4, 1, 0, 0, b"x").is_err()); assert!(encode_payload(0, 0x4000, 0, 0, b"x").is_err());
    assert!(encode_payload(0, 1, 0, 0, b"").is_err()); assert!(encode_payload(0, 1, 0, 0, &vec![0; 2919]).is_err());
}
#[test]
fn native_seq14_reset_clears_original_path_halves_but_not_other_path_state() {
    let now = Instant::now(); let mut receiver = Receiver::default();
    receiver.on_packet(&oracle(1, 0, 10, 0, &[0, 1]), now).unwrap();
    receiver.on_packet(&oracle(1, 1, 10, 0, &[0, 2]), now).unwrap();
    receiver.on_packet(&oracle(0, 0, 9000, 0, &[0, 3]), now).unwrap();
    assert_eq!(receiver.halves.len(), 1); assert!(receiver.halves.contains_key(&(1, 10)));
}
#[test]
fn mpath_wrappers_remain_fully_redacted_and_do_not_grant_any_native_owner() {
    let (packets, _) = encode_payload(0, 1, 0, 0, b"private-audio").unwrap();
    assert!(!format!("{:?}", packets[0]).contains("private-audio"));
    let mut receiver = Receiver::default(); let input = receiver.on_packet(packets[0].expose_to_transport(), Instant::now()).unwrap().unwrap();
    assert_eq!(format!("{input:?}"), "MpathInput(<redacted>)"); assert_eq!(format!("{receiver:?}"), "MpathReceiver(<redacted>)");
}
