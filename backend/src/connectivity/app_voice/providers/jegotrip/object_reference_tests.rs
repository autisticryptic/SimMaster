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
fn debug_does_not_dump_account_id_adapter_host_or_native_reference() {
    let parsed = parse_object_reference(b"#fixture-private-account:tcp -h fixture.invalid -p 443;").unwrap();
    for output in [format!("{parsed:?}"), format!("{:?}", parsed.target()), format!("{:?}", parsed.direct_raw_tcp().unwrap()[0]), format!("{:?}", parsed.encode_native().unwrap())] {
        assert!(!output.contains("fixture"));
    }
}
