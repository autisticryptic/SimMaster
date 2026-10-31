use super::*;

fn address(mask: u8) -> RouteAddress {
    RouteAddress { mask, label: 0xa1, network: 0x1234, network_tag: 0x5678,
        node: 0x90abcdef, node_tag: 0x1357, forward_network: 0x2468,
        forward_node: 0xdeadbeef, port: 0x9abc }
}
// Independent literals from the recovered field widths, NOT encoder output.
fn address_vector(mask: u8) -> &'static [u8] {
    match mask {
        0 => &[0x9a,0xbc],
        1 => &[0x12,0x34,0x56,0x78,0x9a,0xbc],
        2 => &[0x90,0xab,0xcd,0xef,0x13,0x57,0x9a,0xbc],
        3 => &[0x12,0x34,0x56,0x78,0x90,0xab,0xcd,0xef,0x13,0x57,0x9a,0xbc],
        4 => &[0x24,0x68,0xde,0xad,0xbe,0xef,0x9a,0xbc],
        5 => &[0x12,0x34,0x56,0x78,0x24,0x68,0xde,0xad,0xbe,0xef,0x9a,0xbc],
        6 => &[0x90,0xab,0xcd,0xef,0x13,0x57,0x24,0x68,0xde,0xad,0xbe,0xef,0x9a,0xbc],
        7 => &[0x12,0x34,0x56,0x78,0x90,0xab,0xcd,0xef,0x13,0x57,0x24,0x68,0xde,0xad,0xbe,0xef,0x9a,0xbc],
        _ => panic!("fixture mask"),
    }
}
fn extended_vector(extension: &[u8], flags: u8, source_mask: u8) -> Vec<u8> {
    let mut bytes = vec![0,0,0,0,flags,source_mask << 2,0xa1,0];
    bytes.extend_from_slice(address_vector(source_mask)); bytes.extend_from_slice(&[0,2]);
    if source_mask & 2 != 0 && flags & 0x40 == 0 { bytes.extend_from_slice(&[192,0,2,1]); }
    bytes.extend_from_slice(&(extension.len() as u16).to_be_bytes());
    bytes.extend_from_slice(extension); bytes.push(0x43); bytes
}

#[test]
fn all_eight_address_masks_use_independent_mixed_width_wire_vectors() {
    for mask in 0..8 {
        let expected = address_vector(mask);
        let mut output = Vec::new(); address(mask).write_to(&mut output).unwrap();
        assert_eq!(output, expected); assert_eq!(expected.len(), ADDRESS_BYTES[mask as usize]);
        let mut cursor = Cursor { rest: expected }; let decoded = read_address(mask, 0xa1, &mut cursor).unwrap();
        assert!(cursor.rest.is_empty()); assert_eq!(decoded.mask, mask); assert_eq!(decoded.label, 0xa1);
        assert_eq!(decoded.network, if mask & 1 != 0 { 0x1234 } else { 0 });
        assert_eq!(decoded.network_tag, if mask & 1 != 0 { 0x5678 } else { 0xffff });
        assert_eq!(decoded.node, if mask & 2 != 0 { 0x90abcdef } else { 0 });
        assert_eq!(decoded.node_tag, if mask & 2 != 0 { 0x1357 } else { 0xffff });
        assert_eq!(decoded.forward_network, if mask & 4 != 0 { 0x2468 } else { 0 });
        assert_eq!(decoded.forward_node, if mask & 4 != 0 { 0xdeadbeef } else { 0 });
        assert_eq!(decoded.port, 0x9abc);
    }
}

#[test]
fn all_source_destination_mask_pairs_keep_independent_boundaries_and_ipv4_decoration() {
    for source in 0..8 { for destination in 0..8 {
        let mut bytes = vec![1,2,3,4,0,source << 2 | destination << 5,0xa1,0xa1];
        bytes.extend_from_slice(address_vector(source)); bytes.extend_from_slice(address_vector(destination));
        if source & 2 != 0 { bytes.extend_from_slice(&[0; 4]); }
        bytes.extend_from_slice(b"channel-not-yet-accepted");
        let route = Routing { tag: 0x01020304, source: address(source), destination: address(destination), ..Routing::default() };
        assert_eq!(encode_routing_body(&route, b"channel-not-yet-accepted").unwrap().expose_to_transport(), bytes);
        let decoded = decode_routing_body(&bytes).unwrap();
        assert_eq!(decoded.payload, b"channel-not-yet-accepted"); assert_eq!(decoded.routing.source.port, 0x9abc);
        assert_eq!(decoded.routing.destination.mask, destination);
        assert_eq!(decoded.routing.source_ip, (source & 2 != 0).then_some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
    }}
}

#[test]
fn missing_segments_keep_native_ffff_auxiliary_defaults_not_zero_or_port_values() {
    let decoded = decode_routing_body(&[0,0,0,0,0,0,0,0,0x27,0x10,0,98]).unwrap();
    assert_eq!(decoded.routing.source.network, 0); assert_eq!(decoded.routing.source.node, 0);
    assert_eq!(decoded.routing.source.network_tag, NO_ADDRESS_TAG); assert_eq!(decoded.routing.source.node_tag, NO_ADDRESS_TAG);
    assert_eq!(decoded.routing.source.port, 10000); assert_eq!(decoded.routing.destination.port, 98);
    assert!(decoded.payload.is_empty());
}

#[test]
fn full_routing_prefix_paths_ipv4_port_and_relay_tlvs_match_independent_vector() {
    let mut bytes = vec![1,0x23,0x45,0x67,0x83,0x7e,0xa1,0xb2,0x11,0x22,0x33,0x44];
    bytes.extend_from_slice(address_vector(7)); bytes.extend_from_slice(address_vector(3));
    bytes.extend_from_slice(&[192,0,2,9,0,8,0x21,0x1f,0x90,0x33,0x55,0x66,0x77,0x88,0xaa,0xbb,0xcc]);
    let route = Routing { tag: 0x01234567, priority: 3, path_words: vec![0x1122,0x3344],
        source: address(7), destination: RouteAddress { label: 0xb2, forward_network: 0, forward_node: 0, ..address(3) },
        source_ip: Some(IpAddr::V4(Ipv4Addr::new(192,0,2,9))), source_port: 8080, relay_paths: Some(vec![0x5566,0x7788]) };
    assert_eq!(encode_routing_body(&route, &[0xaa,0xbb,0xcc]).unwrap().expose_to_transport(), bytes);
    let decoded = decode_routing_body(&bytes).unwrap(); assert_eq!(decoded.routing, route); assert_eq!(decoded.payload, [0xaa,0xbb,0xcc]);
    for end in 0..56 { assert!(decode_routing_body(&bytes[..end]).is_err(), "truncated prefix at {end}"); }
    assert!(decode_routing_body(&bytes[..56]).unwrap().payload.is_empty());
}

#[test]
fn ipv6_extension_replaces_four_byte_decoration_without_shifting_payload() {
    let ip = [0x20,1,0x0d,0xb8,0,0,0,0,0,0,0,0,0,0,0,9];
    let mut extension = vec![0x1f]; extension.extend_from_slice(&ip);
    let bytes = extended_vector(&extension, 0xc0, 2);
    let decoded = decode_routing_body(&bytes).unwrap();
    assert_eq!(decoded.routing.source_ip, Some(IpAddr::V6(Ipv6Addr::from(ip)))); assert_eq!(decoded.payload, [0x43]);
    assert_eq!(encode_routing_body(&decoded.routing, decoded.payload).unwrap().expose_to_transport(), bytes);
}

#[test]
fn combined_ipv6_port_and_eight_relay_words_follow_native_prepend_order() {
    let ip = [1u8; 16]; let mut extension = vec![0x1f]; extension.extend_from_slice(&ip);
    extension.extend_from_slice(&[0x21,0x12,0x34,0x3f]);
    for word in 1u16..=8 { extension.extend_from_slice(&word.to_be_bytes()); }
    assert_eq!(extension.len(), 37);
    let bytes = extended_vector(&extension, 0xc2, 3);
    let decoded = decode_routing_body(&bytes).unwrap(); assert_eq!(decoded.routing.priority, 2);
    assert_eq!(decoded.routing.source_port, 0x1234); assert_eq!(decoded.routing.relay_paths, Some((1u16..=8).collect::<Vec<_>>()));
    assert_eq!(encode_routing_body(&decoded.routing, decoded.payload).unwrap().expose_to_transport(), bytes);
}

#[test]
fn explicit_empty_relay_paths_are_distinct_from_unspecified_paths() {
    let source = RouteAddress { port: 1, mask: 1, network: 7, network_tag: 8, ..RouteAddress::default() };
    let route = Routing { source, relay_paths: Some(Vec::new()), ..Routing::default() };
    let bytes = encode_routing_body(&route, &[]).unwrap(); assert_eq!(bytes.expose_to_transport()[4], 0x20);
    assert_eq!(decode_routing_body(bytes.expose_to_transport()).unwrap().routing.relay_paths, Some(Vec::new()));
    let mut unspecified = bytes.expose_to_transport().to_vec(); unspecified[4] = 0;
    assert_eq!(decode_routing_body(&unspecified).unwrap().routing.relay_paths, None);
}

#[test]
fn empty_zero_route_is_syntax_only_and_never_invents_an_application_payload() {
    let body = encode_routing_body(&Routing::default(), &[]).unwrap(); assert_eq!(body.expose_to_transport(), [0; 12]);
    let decoded = decode_routing_body(body.expose_to_transport()).unwrap(); assert!(decoded.payload.is_empty());
    assert_eq!(decoded.routing.source.port, 0); assert_eq!(decoded.routing.destination.port, 0);
    // Preserve the older strict subset's explicit rejection of empty payloads.
    assert!(super::super::router_packets::decode_routed_body(body.expose_to_transport()).is_err());
}

#[test]
fn legacy_ipv4_decoration_is_required_even_when_all_four_bytes_are_zero() {
    let mut bytes = vec![0,0,0,0,0,8,0,0]; bytes.extend_from_slice(address_vector(2)); bytes.extend_from_slice(&[0,2]);
    for count in 0..4 { let mut truncated = bytes.clone(); truncated.extend_from_slice(&[0; 4][..count]); assert!(decode_routing_body(&truncated).is_err()); }
    bytes.extend_from_slice(&[0; 4]); assert!(decode_routing_body(&bytes).unwrap().payload.is_empty());
    assert!(super::super::router_packets::decode_routed_body(&bytes).is_err());
}

#[test]
fn every_truncated_address_segment_is_rejected_without_reading_the_next_field() {
    for mask in 0..8 { let bytes = address_vector(mask); for end in 0..bytes.len() {
        assert!(read_address(mask, 0, &mut Cursor { rest: &bytes[..end] }).is_err());
    }}
    assert!(address(8).write_to(&mut Vec::new()).is_err());
}

#[test]
fn reserved_route_flags_are_not_stripped_into_apparent_channel_content() {
    for bit in [4,8,16] { let mut bytes = vec![0; 12]; bytes[4] = bit; bytes.extend_from_slice(b"not-channel");
        assert_eq!(decode_routing_body(&bytes).err(), Some(TransportError::UnsupportedRoute)); }
}

#[test]
fn extension_length_and_each_tlv_length_are_checked_inside_the_declared_envelope() {
    for extension in [vec![0x21],vec![0x21,0],vec![0x1f,0,0],vec![0x33,0,0]] {
        assert!(decode_routing_body(&extended_vector(&extension, 0x80, 3)).is_err());
    }
    let mut too_long = extended_vector(&[], 0x80, 0); let offset = 8 + 2 + 2;
    too_long[offset..offset+2].copy_from_slice(&64u16.to_be_bytes());
    assert_eq!(decode_routing_body(&too_long).err(), Some(TransportError::Truncated));
    too_long[offset..offset+2].copy_from_slice(&65u16.to_be_bytes());
    assert_eq!(decode_routing_body(&too_long).err(), Some(TransportError::TooLarge));
}

#[test]
fn duplicate_unknown_and_noncanonical_known_extensions_fail_closed() {
    for extension in [vec![0x21,0,1,0x21,0,1],vec![0x40,0],vec![0x20,1],vec![0x30,0],vec![0x32,0,1,0]] {
        assert!(decode_routing_body(&extended_vector(&extension, 0x80, 3)).is_err());
    }
    // Explicit clear plus a path replacement is ambiguous, not last-write-wins.
    assert!(decode_routing_body(&extended_vector(&[0x31,0,1], 0xa0, 3)).is_err());
}

#[test]
fn ipv6_flags_and_extensions_must_agree_and_cannot_override_legacy_ipv4() {
    let mut ip = vec![0x1f]; ip.extend_from_slice(&[0; 16]);
    for (extension,flags,mask) in [(&ip[..],0x80,3),(&ip[..],0xc0,0),(&[][..],0xc0,3)] {
        assert!(decode_routing_body(&extended_vector(extension,flags,mask)).is_err());
    }
    let mut missing = vec![0; 12]; missing[4] = 0x40; assert!(decode_routing_body(&missing).is_err());
    assert!(decode_routing_body(&extended_vector(&[0x21,0,1],0x80,0)).is_err());
}

#[test]
fn no_encoder_silently_drops_ip_port_or_relay_metadata_for_an_incompatible_mask() {
    for route in [Routing { source_ip: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), ..Routing::default() },
        Routing { source_port: 7, ..Routing::default() }, Routing { relay_paths: Some(vec![1]), ..Routing::default() }] {
        assert!(encode_routing_body(&route, b"x").is_err());
    }
    let mut route = Routing { source: address(7), ..Routing::default() };
    route.path_words = vec![1; 4]; assert!(encode_routing_body(&route,b"x").is_err());
    route.path_words.clear(); route.priority = 4; assert!(encode_routing_body(&route,b"x").is_err());
    route.priority = 0; route.relay_paths = Some(vec![1; 9]); assert!(encode_routing_body(&route,b"x").is_err());
}

#[test]
fn complete_body_limit_is_checked_before_allocating_or_returning_a_payload() {
    assert_eq!(decode_routing_body(&vec![0; MAX_ROUTED_BODY_BYTES+1]).err(), Some(TransportError::TooLarge));
    let route = Routing::default(); let payload = vec![0; MAX_ROUTED_BODY_BYTES-12];
    assert_eq!(encode_routing_body(&route,&payload).unwrap().expose_to_transport().len(), MAX_ROUTED_BODY_BYTES);
    assert_eq!(encode_routing_body(&route,&vec![0; MAX_ROUTED_BODY_BYTES-11]).err(), Some(TransportError::TooLarge));
}

#[test]
fn three_header_path_words_and_labels_remain_transport_fields_not_rpc_identity() {
    let bytes = [0xde,0xad,0xbe,0xef,2,3,0x80,0xff,0,1,0x80,2,0xff,0xff,0,1,0,2,0x99];
    let decoded = decode_routing_body(&bytes).unwrap(); assert_eq!(decoded.routing.path_words, [1,0x8002,0xffff]);
    assert_eq!(decoded.routing.tag, 0xdeadbeef); assert_eq!(decoded.routing.source.label, 0x80);
    assert_eq!(decoded.routing.destination.label, 0xff); assert_eq!(decoded.payload, [0x99]);
}

#[test]
fn routing_debug_never_exposes_ip_addresses_wire_tags_or_payload_text() {
    let bytes = extended_vector(&[0x21,0x12,0x34],0x80,3); let decoded = decode_routing_body(&bytes).unwrap();
    let text = format!("{:?} {:?} {:?}",decoded,decoded.routing,decoded.routing.source);
    for secret in ["192.0.2.1","90abcdef","4660","1234"] { assert!(!text.contains(secret)); }
    assert!(text.contains("<redacted>"));
}
