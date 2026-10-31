use super::*;

#[test]
fn native_slash_identity_and_semicolon_explicit_endpoint_fixture_roundtrips() {
    let input = b"facet/category/#fixture:tcp -h fixture.invalid -p 443;tcp -p 9443 -h 127.0.0.1;";
    let parsed = parse_object_reference(input).unwrap();
    assert_eq!(parsed.identity(), b"facet/category/#fixture"); assert_eq!(parsed.components().len(), 3);
    let endpoints = parsed.direct_raw_tcp().unwrap(); assert_eq!(endpoints.len(), 2);
    assert_eq!(endpoints[0].host(), b"fixture.invalid"); assert_eq!(endpoints[0].port(), 443);
    assert_eq!(endpoints[1].host(), b"127.0.0.1"); assert_eq!(endpoints[1].port(), 9443);
    assert_eq!(parsed.encode_native().unwrap().expose_to_backend(), b"facet/category/#fixture:tcp -h fixture.invalid -p 443;tcp -h 127.0.0.1 -p 9443;");
}

#[test]
fn balanced_identity_groups_preserve_internal_delimiters_and_prefix_components() {
    let parsed = parse_object_reference(b"facet/[uid:fixture@domain]/<literal:{nested}>@fixture-adapter$fixture-locator").unwrap();
    assert_eq!(parsed.identity(), b"facet/[uid:fixture@domain]/<literal:{nested}>");
    match parsed.target() {
        ReferenceTarget::Adapter { adapter, locator } => { assert_eq!(adapter, b"fixture-adapter"); assert_eq!(locator.as_deref(), Some(b"fixture-locator".as_slice())); }
        _ => panic!("adapter target required"),
    }
    assert_eq!(parsed.direct_raw_tcp().err(), Some(ReferenceError::NoDirectEndpoints));
}

#[test]
fn grouped_dollar_is_not_split_into_unbalanced_adapter_and_locator_fields() {
    let reference = parse_object_reference(b"id@[a$b]").unwrap();
    match reference.target() {
        ReferenceTarget::Adapter { adapter, locator } => { assert_eq!(adapter, b"[a$b]"); assert!(locator.is_none()); }
        _ => panic!("adapter target required"),
    }
    let reference = parse_object_reference(b"id@{[a$b]}$<locator:{c$d}>").unwrap();
    match reference.target() {
        ReferenceTarget::Adapter { adapter, locator } => { assert_eq!(adapter, b"{[a$b]}"); assert_eq!(locator.as_deref(), Some(b"<locator:{c$d}>".as_slice())); }
        _ => panic!("adapter target required"),
    }
    assert_eq!(reference.encode_native().unwrap().expose_to_backend(), b"id@{[a$b]}$<locator:{c$d}>");
    assert_eq!(reference.direct_raw_tcp().err(), Some(ReferenceError::NoDirectEndpoints));
}

#[test]
fn adapter_locator_is_not_host_port_and_does_not_synthesize_endpoint_defaults() {
    for input in [b"#fixture".as_slice(), b"#fixture@adapter", b"#fixture@adapter$locator"] {
        let parsed = parse_object_reference(input).unwrap();
        assert_eq!(parsed.encode_native().unwrap().expose_to_backend(), input);
        assert_eq!(parsed.direct_raw_tcp().err(), Some(ReferenceError::NoDirectEndpoints));
    }
}

#[test]
fn ipv6_endpoint_literal_is_a_whitespace_argument_not_an_extra_reference_separator() {
    let parsed = parse_object_reference(b"#fixture:tcp -h 2001:db8::1 -p 443;").unwrap();
    assert_eq!(parsed.direct_raw_tcp().unwrap()[0].host(), b"2001:db8::1");
    assert_eq!(parsed.encode_native().unwrap().expose_to_backend(), b"#fixture:tcp -h 2001:db8::1 -p 443;");
}

#[test]
fn different_native_transports_are_preserved_but_never_downgraded_to_raw_tcp_tls_carrier() {
    for scheme in ["udp", "tls", "http", "https", "tlsp", "svarc", "tcpp"] {
        let raw = format!("#fixture:{scheme} -h fixture.invalid -p 443;");
        let parsed = parse_object_reference(raw.as_bytes()).unwrap();
        assert_eq!(parsed.direct_raw_tcp().err(), Some(ReferenceError::UnsupportedEndpoint));
        match parsed.target() { ReferenceTarget::Direct(endpoints) => assert_eq!(endpoints[0].scheme(), scheme.as_bytes()), _ => panic!("direct required") }
    }
}

#[test]
fn missing_duplicate_zero_signed_or_overflow_host_ports_do_not_use_defaults() {
    for input in [b"#fixture:tcp -h fixture.invalid;".as_slice(), b"#fixture:tcp -p 443;", b"#fixture:tcp -h a -h b -p 443;",
        b"#fixture:tcp -h a -p 0;", b"#fixture:tcp -h a -p -1;", b"#fixture:tcp -h a -p +443;", b"#fixture:tcp -h a -p 65536;",
        b"#fixture:tcp -h a -p 443 -p 443;", b"#fixture:tcp;"] {
        assert!(parse_object_reference(input).is_err(), "fixture rejected");
    }
}

#[test]
fn unimplemented_options_proxies_ranges_placeholders_and_invalid_hosts_fail_explicitly() {
    for input in [b"#fixture:tcp -h a -p 443 -hb value;".as_slice(), b"#fixture:tcp -h a -lp 100-200;", b"#fixture:tcp -h a -p 443 -z 1;",
        b"#fixture:tcp -h *.invalid -p 443;", b"#fixture:tcp -h a..b -p 443;", b"#fixture:tcp -h -bad -p 443;",
        b"#fixture:tcp -h ${Hosts} -p 443;", b"#fixture:tcp -h [::1] -p 443;"] {
        assert_eq!(parse_object_reference(input).err(), Some(ReferenceError::UnsupportedEndpoint));
    }
}

#[test]
fn malformed_groups_quotes_escapes_empty_paths_and_extra_targets_are_not_reinterpreted() {
    for input in [b"".as_slice(), b"a//id", b"/id", b"id/", b"id@", b"id@a$", b"id@$l", b"id@a$l$x", b"id@a:tcp -h a -p 1",
        b"[unclosed", b"{mismatched]", b"extra]", b"id\\escaped", b"\"quoted\"", b"literal space", b"id\0hidden"] {
        assert!(parse_object_reference(input).is_err());
    }
}

#[test]
fn outer_native_whitespace_and_optional_final_semicolon_do_not_change_identity_values() {
    let parsed = parse_object_reference(b"\t #fixture:tcp -h fixture.invalid -p 443 \r\n").unwrap();
    assert_eq!(parsed.identity(), b"#fixture"); assert_eq!(parsed.direct_raw_tcp().unwrap()[0].port(), 443);
    assert_eq!(parsed.encode_native().unwrap().expose_to_backend(), b"#fixture:tcp -h fixture.invalid -p 443;");
    assert!(parse_object_reference(b"#fixture:tcp -h a -p 1;;").is_err());
}

#[test]
fn reference_path_endpoint_and_group_resource_caps_are_local_not_network_limits() {
    assert_eq!(parse_object_reference(&vec![b'x'; MAX_FIELD_BYTES + 1]).err(), Some(ReferenceError::TooLarge));
    let path = vec!["x"; MAX_COMPONENTS + 1].join("/"); assert_eq!(parse_object_reference(path.as_bytes()).err(), Some(ReferenceError::TooLarge));
    let endpoints = format!("id:{}", "tcp -h a -p 1;".repeat(MAX_ENDPOINTS + 1)); assert_eq!(parse_object_reference(endpoints.as_bytes()).err(), Some(ReferenceError::TooLarge));
    let nested = format!("{}x{}", "[".repeat(MAX_GROUP_DEPTH + 1), "]".repeat(MAX_GROUP_DEPTH + 1));
    assert_eq!(parse_object_reference(nested.as_bytes()).err(), Some(ReferenceError::TooLarge));
}

#[test]
fn explicit_tls_router_ids_roundtrip_without_stripping_or_tcp_downgrade() {
    let input = b"Router:tls -h fixture-one.invalid -p 443 -ri 101;tls -ri 202 -p 9443 -h fixture-two.invalid;";
    let reference = parse_object_reference(input).unwrap();
    assert_eq!(reference.direct_raw_tcp().err(), Some(ReferenceError::UnsupportedEndpoint));
    let groups = reference.explicit_tls_router_groups().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].router_id(), 101); assert_eq!(groups[0].endpoints()[0].host(), b"fixture-one.invalid");
    assert_eq!(groups[1].router_id(), 202); assert_eq!(groups[1].endpoints()[0].port(), 9443);
    assert_eq!(reference.encode_native().unwrap().expose_to_backend(),
        b"Router:tls -h fixture-one.invalid -p 443 -ri 101;tls -h fixture-two.invalid -p 9443 -ri 202;");
}

#[test]
fn router_grouping_preserves_associations_even_when_host_ports_are_identical() {
    let reference = parse_object_reference(b"Router:tls -h fixture.invalid -p 443 -ri 202;tls -h fixture.invalid -p 443 -ri 101;tls -h fixture-alt.invalid -p 9443 -ri 202;").unwrap();
    let groups = reference.explicit_tls_router_groups().unwrap();
    assert_eq!(groups.iter().map(RouterEndpointGroup::router_id).collect::<Vec<_>>(), vec![101, 202]);
    assert_eq!(groups[1].endpoints().len(), 2);
    assert_eq!(groups[1].endpoints()[0].host(), b"fixture.invalid");
    assert_eq!(groups[1].endpoints()[1].host(), b"fixture-alt.invalid");
}

#[test]
fn native_router_id_base_zero_conversion_preserves_original_literal() {
    for (literal, expected) in [("0x65", 101), ("0XCA", 202), ("0145", 101), ("+101", 101), ("-077", -63), ("-999", -999), ("2147483647", i32::MAX)] {
        let input = format!("Router:tls -h fixture.invalid -p 443 -ri {literal};");
        let reference = parse_object_reference(input.as_bytes()).unwrap();
        assert_eq!(reference.explicit_tls_router_groups().unwrap()[0].router_id(), expected);
        assert_eq!(reference.encode_native().unwrap().expose_to_backend(), input.as_bytes());
    }
}

#[test]
fn zero_malformed_or_wrapping_router_ids_fail_closed() {
    for literal in ["0", "-0", "+0", "0x0", "00", "0x", "08", "1junk", "1.0", "--1", "+", "-", "2147483648", "4294967297", "-2147483649", "0x100000001", "99999999999999999999999999999999999"] {
        let input = format!("Router:tls -h fixture.invalid -p 443 -ri {literal};");
        assert!(parse_object_reference(input.as_bytes()).is_err(), "invalid Router fixture");
    }
    assert!(parse_object_reference(b"Router:tls -h fixture.invalid -p 443 -ri;").is_err());
    assert!(parse_object_reference(b"Router:tls -h fixture.invalid -p 443 -ri 1 -ri 2;").is_err());
}

#[test]
fn parser_preserves_negative_router_ids_but_does_not_synthesize_discovery_groups() {
    for literal in ["-1000", "-2147483648"] {
        let input = format!("Router:tls -h fixture.invalid -p 443 -ri {literal};");
        let reference = parse_object_reference(input.as_bytes()).unwrap();
        assert_eq!(reference.encode_native().unwrap().expose_to_backend(), input.as_bytes());
        assert_eq!(reference.explicit_tls_router_groups().err(), Some(ReferenceError::UnsupportedEndpoint));
        assert_eq!(reference.direct_raw_tcp().err(), Some(ReferenceError::UnsupportedEndpoint));
    }
}

#[test]
fn entire_tls_router_reference_rejects_mixed_or_missing_route_associations() {
    for input in [b"Router:tls -h a.invalid -p 443;".as_slice(),
        b"Router:tls -h a.invalid -p 443 -ri 1;tls -h b.invalid -p 443;",
        b"Router:tls -h a.invalid -p 443 -ri 1;tcp -h b.invalid -p 443 -ri 2;",
        b"Other:tls -h a.invalid -p 443 -ri 1;", b"Router@fixture-adapter"] {
        let reference = parse_object_reference(input).unwrap();
        assert!(reference.explicit_tls_router_groups().is_err());
        assert!(reference.direct_raw_tcp().is_err());
    }
    // tcp/-ri is valid reference data but must not enter the old direct actor.
    let reference = parse_object_reference(b"Router:tcp -h a.invalid -p 443 -ri 1;").unwrap();
    assert_eq!(reference.direct_raw_tcp().err(), Some(ReferenceError::UnsupportedEndpoint));
}

#[test]
fn arc_scheme_router_option_is_not_reinterpreted_as_an_endpoint_route() {
    for scheme in ["arc", "varc", "sarc", "svarc", "udp", "https"] {
        let input = format!("Router:{scheme} -h fixture.invalid -p 443 -ri 101;");
        assert_eq!(parse_object_reference(input.as_bytes()).err(), Some(ReferenceError::UnsupportedEndpoint));
    }
}

#[test]
fn router_group_debug_hides_hosts_and_private_numeric_associations() {
    let reference = parse_object_reference(b"Router:tls -h fixture-private.invalid -p 443 -ri 123456789;").unwrap();
    let groups = reference.explicit_tls_router_groups().unwrap();
    for text in [format!("{reference:?}"), format!("{:?}", reference.target()), format!("{:?}", groups[0]), format!("{:?}", groups[0].endpoints()[0]), format!("{:?}", reference.encode_native().unwrap())] {
        assert!(!text.contains("fixture")); assert!(!text.contains("123456789"));
    }
}

#[test]
fn debug_does_not_dump_account_id_adapter_host_or_native_reference() {
    let parsed = parse_object_reference(b"#fixture-private-account:tcp -h fixture.invalid -p 443;").unwrap();
    for output in [format!("{parsed:?}"), format!("{:?}", parsed.target()), format!("{:?}", parsed.direct_raw_tcp().unwrap()[0]), format!("{:?}", parsed.encode_native().unwrap())] {
        assert!(!output.contains("fixture"));
    }
}

#[test]
fn original_arc_query_is_scheme_specific_and_strips_exactly_one_prefix() {
    for scheme in ["arc", "sarc", "varc", "svarc"] {
        let input = format!("AccountEntry:{scheme} -h arc@AccountEntry -p 98;");
        let reference = parse_object_reference(input.as_bytes()).unwrap();
        let ReferenceTarget::Direct(endpoints) = reference.target() else { panic!("explicit endpoint required") };
        assert!(matches!(endpoints[0].router_host().unwrap(), RouterHost::Query(b"AccountEntry")));
        assert_eq!(endpoints[0].host(), b"arc@AccountEntry");
        assert_eq!(endpoints[0].port(), 98);
        assert_eq!(reference.encode_native().unwrap().expose_to_backend(), input.as_bytes());
        assert_eq!(reference.direct_raw_tcp().err(), Some(ReferenceError::UnsupportedEndpoint));
        assert!(reference.explicit_tls_router_groups().is_err());
    }
}

#[test]
fn arc_query_does_not_relax_tcp_tls_hosts_or_accept_nested_and_empty_queries() {
    for scheme in ["tcp", "tls", "https", "udp", "sArc"] {
        let input = format!("id:{scheme} -h arc@AccountEntry -p 98;");
        assert_eq!(parse_object_reference(input.as_bytes()).err(), Some(ReferenceError::UnsupportedEndpoint));
    }
    for host in ["arc@", "arc@arc@AccountEntry", "ARC@AccountEntry", "arc@a/b", "arc@a?b", "arc@a:b", "arc@${host}", "arc@ENC(value)"] {
        let input = format!("id:sarc -h {host} -p 98;");
        assert!(parse_object_reference(input.as_bytes()).is_err());
    }
    let input = format!("id:sarc -h arc@{} -p 98;", "x".repeat(250));
    assert!(parse_object_reference(input.as_bytes()).is_err());
}

#[test]
fn native_numeric_router_host_uses_first_dot_base_zero_and_mixed_field_ranges() {
    for (host, network, node) in [("1", 1, 0), ("1.2", 1, 2), ("0x12.010", 18, 8),
        ("+7.+9", 7, 9), ("0177.0xfffffffe", 127, u32::MAX - 1), ("65535.0", 65535, 0), ("0.0", 0, 0)] {
        let parsed = RouterLogicalHost::parse(host.as_bytes()).unwrap();
        assert_eq!(parsed.network(), network); assert_eq!(parsed.node(), node);
        let input = format!("id:sarc -h {host} -p 98;");
        let reference = parse_object_reference(input.as_bytes()).unwrap();
        let ReferenceTarget::Direct(endpoints) = reference.target() else { panic!("endpoint required") };
        assert!(matches!(endpoints[0].router_host().unwrap(), RouterHost::Address(value) if value == parsed));
        assert_eq!(reference.encode_native().unwrap().expose_to_backend(), input.as_bytes());
    }
}

#[test]
fn numeric_router_host_rejects_truncation_negative_sentinel_and_extra_dot() {
    for host in ["", " ", ".1", "1.", "1.2.3", "-1", "1.-2", "65536.1", "0x10000.1",
        "1.4294967295", "1.4294967296", "1.0xffffffff", "1.0x100000001", "08.1", "1.08", "0x.1",
        "1.2junk", "1.++2", "1.+ 2", "1.2\0", "1.2\u{000b}"] {
        assert!(RouterLogicalHost::parse(host.as_bytes()).is_err(), "invalid numeric host");
    }
    assert!(RouterLogicalHost::parse(&[b'1'; 254]).is_err());
    let spaced = RouterLogicalHost::parse(b"\t0x12 . 010\r\n").unwrap();
    assert_eq!((spaced.network(), spaced.node()), (18, 8));
}

#[test]
fn dns_reference_data_does_not_implicitly_become_a_logical_router_destination() {
    let reference = parse_object_reference(b"id:sarc -h fixture.invalid -p 98;").unwrap();
    let ReferenceTarget::Direct(endpoints) = reference.target() else { panic!("endpoint required") };
    assert_eq!(endpoints[0].router_host().err(), Some(ReferenceError::UnsupportedEndpoint));
    let tcp = parse_object_reference(b"id:tcp -h 1.2 -p 443;").unwrap();
    assert_eq!(tcp.direct_raw_tcp().unwrap()[0].host(), b"1.2");
    assert_eq!(tcp.direct_raw_tcp().unwrap()[0].router_host().err(), Some(ReferenceError::UnsupportedEndpoint));
}

#[test]
fn arc_security_overrides_are_rejected_not_silently_discarded_by_logical_host_support() {
    for options in ["-S 0", "-S 3", "-kx 1", "-ri 101", "-lp 10000"] {
        let input = format!("id:sarc -h arc@AccountEntry -p 98 {options};");
        assert_eq!(parse_object_reference(input.as_bytes()).err(), Some(ReferenceError::UnsupportedEndpoint));
    }
}

#[test]
fn logical_host_debug_does_not_disclose_query_or_numeric_route_identity() {
    let host = RouterLogicalHost::parse(b"4660.305419896").unwrap();
    for value in [format!("{host:?}"), format!("{:?}", RouterHost::Address(host)),
        format!("{:?}", RouterHost::Query(b"private-router-query"))] {
        assert!(!value.contains("4660")); assert!(!value.contains("305419896")); assert!(!value.contains("private-router-query"));
    }
}
