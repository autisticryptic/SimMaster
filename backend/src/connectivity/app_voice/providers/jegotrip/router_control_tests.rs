use super::*;

fn report_vector() -> Vec<u8> {
    vec![
        4,0, 1,2,3,4, // opcode, StreamData0, four byte levels
        0,0,0,1, // network map count
        0,0,0,42, 0,1,0,2,0xff,0xff,0,4,
        0,0,0,1, // peer map count
        0,0,0,42, 0x11,0x22,0x33,0x44, 0,0,0,12, 0x55,0x66,0x77,0x88,
        0,10,0,11,0,12,0,13,
    ]
}

#[test]
fn native_node_probe_and_alive_are_single_byte_payloads_not_empty_or_rpc_bool_streams() {
    assert_eq!(decode_node_control(&[0]).unwrap(),NodeControl::Probe);
    assert_eq!(decode_node_control(&[1]).unwrap(),NodeControl::Alive);
    assert_eq!(decode_node_control(&[]).err(),Some(CodecError::Truncated));
    for payload in [&[0,0][..],&[1,0][..],&[0,1][..]] {
        assert_eq!(decode_node_control(payload).err(),Some(CodecError::TrailingData));
    }
}

#[test]
fn report_control_uses_independent_byte_short_and_word_vector_without_rpc_footer() {
    let expected = report_vector(); assert_eq!(expected.len(),50);
    let NodeControl::Report(report) = decode_node_control(&expected).unwrap() else { panic!("report required") };
    assert_eq!(report.levels,[1,2,3,4]); assert_eq!(report.networks.len(),1); assert_eq!(report.peers.len(),1);
    assert_eq!(report.networks.get(&42),Some(&[1,2,0xffff,4]));
    assert_eq!(report.peers.get(&PeerWords([42,0x11223344,12,0x55667788])),Some(&[10,11,12,13]));
}

#[test]
fn empty_report_maps_are_valid_but_do_not_create_route_or_port_grants() {
    let bytes = [4,0,0,0,0,0, 0,0,0,0, 0,0,0,0];
    let NodeControl::Report(report) = decode_node_control(&bytes).unwrap() else { panic!("report required") };
    assert_eq!(report.levels,[0;4]); assert!(report.networks.is_empty()); assert!(report.peers.is_empty());
}

#[test]
fn every_incomplete_report_prefix_fails_instead_of_accepting_partial_route_maps() {
    let full = report_vector();
    for end in 1..full.len() { assert!(decode_node_control(&full[..end]).is_err(),"truncated at {end}"); }
}

#[test]
fn unsupported_opcode_stream_type_and_unexpected_trailing_footer_are_rejected() {
    for opcode in [2,3,5,0xff] { assert!(decode_node_control(&[opcode]).is_err()); }
    let mut bytes = report_vector(); bytes[1] = 2;
    assert_eq!(decode_node_control(&bytes).err(),Some(CodecError::UnsupportedStream));
    let mut bytes = report_vector(); bytes.extend_from_slice(&[0,0,0,0]);
    assert_eq!(decode_node_control(&bytes).err(),Some(CodecError::TrailingData));
}

#[test]
fn report_counts_and_total_bytes_are_bounded_before_map_allocation() {
    let mut bytes = report_vector(); bytes[6..10].copy_from_slice(&129u32.to_be_bytes());
    assert_eq!(decode_node_control(&bytes).err(),Some(CodecError::TooLarge));
    let mut bytes = report_vector(); bytes[22..26].copy_from_slice(&129u32.to_be_bytes());
    assert_eq!(decode_node_control(&bytes).err(),Some(CodecError::TooLarge));
    assert_eq!(decode_node_control(&vec![4;MAX_CONTROL_BYTES+1]).err(),Some(CodecError::TooLarge));
}

#[test]
fn duplicate_network_and_peer_map_keys_are_not_silently_overwritten() {
    let original = report_vector();
    let mut network = original[..10].to_vec(); network[6..10].copy_from_slice(&2u32.to_be_bytes());
    network.extend_from_slice(&original[10..22]); network.extend_from_slice(&original[10..22]); network.extend_from_slice(&original[22..]);
    assert_eq!(decode_node_control(&network).err(),Some(CodecError::DuplicateKey));
    let mut peer = original[..26].to_vec(); peer[22..26].copy_from_slice(&2u32.to_be_bytes());
    peer.extend_from_slice(&original[26..]); peer.extend_from_slice(&original[26..]);
    assert_eq!(decode_node_control(&peer).err(),Some(CodecError::DuplicateKey));
}

#[test]
fn report_maps_keep_full_width_words_and_unknown_auxiliary_values_without_truncation() {
    let mut bytes = report_vector(); bytes[10..14].copy_from_slice(&u32::MAX.to_be_bytes());
    bytes[26..42].fill(0xff); bytes[42..50].fill(0xff);
    let NodeControl::Report(report) = decode_node_control(&bytes).unwrap() else { panic!("report required") };
    assert!(report.networks.contains_key(&u32::MAX));
    assert_eq!(report.peers.get(&PeerWords([u32::MAX;4])),Some(&[u16::MAX;4]));
    // These are opaque reports, not parsed logical endpoints. Narrowing and
    // original peer selection must occur before any outbound routing use.
}

#[test]
fn maximum_network_map_is_accepted_without_raising_the_existing_rpc_resource_cap() {
    let mut bytes = vec![4,0,1,2,3,4]; bytes.extend_from_slice(&(MAX_REPORT_ENTRIES as u32).to_be_bytes());
    for key in 0..MAX_REPORT_ENTRIES as u32 { bytes.extend_from_slice(&key.to_be_bytes()); bytes.extend_from_slice(&[0xff;8]); }
    bytes.extend_from_slice(&[0;4]);
    let NodeControl::Report(report) = decode_node_control(&bytes).unwrap() else { panic!("report required") };
    assert_eq!(report.networks.len(),MAX_REPORT_ENTRIES); assert!(report.peers.is_empty());
}

#[test]
fn report_and_control_debug_redact_router_keys_and_transport_words() {
    let control = decode_node_control(&report_vector()).unwrap(); let text = format!("{control:?}");
    assert!(text.contains("<redacted>")); assert!(!text.contains("11223344")); assert!(!text.contains("42"));
}

#[test]
fn report_delta_keeps_unmentioned_original_network_and_peer_records() {
    let NodeControl::Report(mut report)=decode_node_control(&report_vector()).unwrap() else{panic!("report required")};
    let mut delta=RouterReport {levels:[9;4],..Default::default()}; delta.networks.insert(43,[5;4]);
    report.merge_from(&delta).unwrap(); assert_eq!(report.levels,[9;4]); assert_eq!(report.networks.len(),2);
    assert_eq!(report.networks.get(&42),Some(&[1,2,0xffff,4])); assert_eq!(report.peers.len(),1);
}

#[test]
fn report_delta_overflow_is_atomic_for_levels_and_both_maps() {
    for peer_map in [false,true] {
        let mut report=RouterReport {levels:[1;4],..Default::default()};
        for key in 0..MAX_REPORT_ENTRIES as u32 {
            if peer_map {report.peers.insert(PeerWords([key,0,0,0]),[1;4]);} else {report.networks.insert(key,[1;4]);}
        }
        let original=report.clone(); let mut delta=RouterReport {levels:[9;4],..Default::default()};
        if peer_map {delta.peers.insert(PeerWords([MAX_REPORT_ENTRIES as u32,0,0,0]),[2;4]);}
        else {delta.networks.insert(MAX_REPORT_ENTRIES as u32,[2;4]);}
        assert_eq!(report.merge_from(&delta),Err(CodecError::TooLarge)); assert_eq!(report,original);
    }
}

#[test]
fn report_delta_can_update_an_existing_key_at_capacity_without_evicting_other_routes() {
    let mut report=RouterReport::default(); for key in 0..MAX_REPORT_ENTRIES as u32 {report.networks.insert(key,[1;4]);}
    let mut delta=RouterReport {levels:[2;4],..Default::default()}; delta.networks.insert(0,[7;4]);
    report.merge_from(&delta).unwrap(); assert_eq!(report.networks.len(),MAX_REPORT_ENTRIES);
    assert_eq!(report.networks.get(&0),Some(&[7;4])); assert_eq!(report.networks.get(&1),Some(&[1;4]));
}
