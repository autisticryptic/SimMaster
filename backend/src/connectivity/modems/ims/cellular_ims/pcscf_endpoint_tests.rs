//! Pure parsing and mock DNS regressions. No sockets, environment mutation,
//! devices or real resolvers are used by these tests.
use super::*;
use std::{collections::BTreeMap, future::ready};

fn local() -> IpAddr { "192.0.2.2".parse().unwrap() }
fn server() -> IpAddr { "192.0.2.53".parse().unwrap() }
fn settings() -> ImsIpSettings {
    ImsIpSettings { ipv4_dns: vec![server()], ..ImsIpSettings::default() }
}
fn addresses(values: &[&str], ttl: u32) -> DnsRecords {
    let addresses = values.iter().map(|value| value.parse::<IpAddr>().unwrap()).collect::<Vec<_>>();
    let address_ttls = addresses.iter().map(|address| (*address, ttl)).collect::<BTreeMap<_, _>>();
    DnsRecords { addresses, address_ttls, ..DnsRecords::default() }
}
fn service(host: &str, port: u16, priority: u16, weight: u16, ttl: u32) -> SrvTarget {
    SrvTarget { target: host.to_string(), port, priority, weight, ttl }
}
fn explicit(value: &str) -> Selection {
    select_source(Some(value), &settings(), None, &[]).unwrap()
}

#[test]
fn parses_ip_hostname_ports_and_udp_sip_without_loss() {
    for (text, expected_host, expected_port) in [
        ("192.0.2.10", PcscfHost::Address("192.0.2.10".parse().unwrap()), 5060),
        ("192.0.2.10:5078", PcscfHost::Address("192.0.2.10".parse().unwrap()), 5078),
        ("2001:db8::5078", PcscfHost::Address("2001:db8::5078".parse().unwrap()), 5060),
        ("[2001:db8::10]", PcscfHost::Address("2001:db8::10".parse().unwrap()), 5060),
        ("[2001:db8::10]:5078", PcscfHost::Address("2001:db8::10".parse().unwrap()), 5078),
        ("PCSCF.Example.:5078", PcscfHost::Name("pcscf.example".to_string()), 5078),
        ("sip:pcscf.example:5078;transport=udp", PcscfHost::Name("pcscf.example".to_string()), 5078),
        ("SIP:[2001:db8::10]:65535;Transport=UDP", PcscfHost::Address("2001:db8::10".parse().unwrap()), 65535),
        ("sip:192.0.2.10:1", PcscfHost::Address("192.0.2.10".parse().unwrap()), 1),
    ] {
        let parsed = parse_pcscf_endpoint(text).unwrap();
        assert_eq!(parsed.host, expected_host, "{text}");
        assert_eq!(parsed.port, expected_port, "{text}");
        assert_eq!(parsed.transport, PcscfTransport::Udp);
    }
}

#[test]
fn rejects_invalid_ports_brackets_hosts_and_uri_components() {
    for text in [
        "", " ", " pcscf.example", "pcscf.example ", "pcscf.example:",
        "pcscf.example:0", "pcscf.example:65536", "pcscf.example:+5060",
        "pcscf.example:-1", "pcscf.example:5a", "pcscf.example:5060:5070",
        "[2001:db8::10]:", "[2001:db8::10]:0", "[2001:db8::10]:65536",
        "[2001:db8::10", "2001:db8::10]", "[192.0.2.10]:5060", "[::1]tail",
        "[[::1]]:5060", "2001:db8::xyz", "sip:", "sip::5060", "sip:host:",
        "sip:2001:db8::10", "sip:2001:db8::10:5078", "sip:host;transport=udp=other",
        "sip:user@pcscf.example", "sip:host?x=y", "sip:host#fragment",
        "sip://pcscf.example", "https://pcscf.example", "<sip:pcscf.example>",
        "sip:host;lr", "sip:host;transport=udp;lr", "sip:host;foo=bar",
        "sip:host;transport=udp;transport=udp", "sip:host;transport=", "sip:host;",
        "pcscf.example;transport=udp", "pcscf.example/path", "[fe80::1%eth0]:5060",
        "bad..example", "-bad.example", "bad-.example", "_sip.example", ".",
        "192.0.2.999", "192.0.2", "12345", "host..", "sip:ho%73t", "höst.example",
        "sip:host\r\nVia:injected", "host\0", "host\t", "host\\other",
    ] {
        assert_eq!(parse_pcscf_endpoint(text).unwrap_err().code(), code::PCSCF_ENDPOINT_INVALID, "{text:?}");
    }
    assert!(parse_pcscf_endpoint(&format!("{}.example", "x".repeat(64))).is_err());
    assert!(parse_pcscf_endpoint(&format!("{0}.{0}.{0}.{0}", "x".repeat(63))).is_err());
    for byte in 0..=31u8 {
        assert!(parse_pcscf_endpoint(&format!("host{}", char::from(byte))).is_err());
    }
    assert!(parse_pcscf_endpoint("host\u{7f}").is_err());
}

#[test]
fn rejects_sips_tcp_tls_and_other_transports_explicitly() {
    for text in [
        "sips:pcscf.example", "SIPS:[2001:db8::10]:5061;transport=udp",
        "sip:pcscf.example;transport=tcp", "sip:192.0.2.10:5061;transport=TLS",
        "sip:host;transport=sctp", "sip:host;transport=ws",
    ] {
        assert_eq!(parse_pcscf_endpoint(text).unwrap_err().code(), code::PCSCF_TRANSPORT_UNSUPPORTED, "{text}");
    }
}

#[test]
fn endpoint_lists_validate_every_entry_before_io_and_are_bounded() {
    let targets = parse_list("192.0.2.10:5078, [2001:db8::10]:5088 sip:host:5098;transport=udp").unwrap();
    assert_eq!(targets.iter().map(|target| target.port).collect::<Vec<_>>(), [5078, 5088, 5098]);
    for text in [
        "", " ", ",192.0.2.10", "192.0.2.10,", "host,,other",
        "192.0.2.10;192.0.2.11", "host, sip:other;lr", "host\nother", "host\tother",
        "192.0.2.10,host:0", "host, sips:other",
    ] {
        assert!(parse_list(text).is_err(), "{text:?}");
    }
    let valid = vec!["host"; MAX_CONFIGURED_TARGETS].join(",");
    assert_eq!(parse_list(&valid).unwrap().len(), MAX_CONFIGURED_TARGETS);
    assert!(parse_list(&format!("{valid},host")).is_err());
    assert!(parse_list(&" ".repeat(MAX_CONFIGURED_TEXT + 1)).is_err());
    // An invalid tail cannot hide beyond the returned-candidate cap.
    let tail = format!("{},host:0", vec!["192.0.2.10"; MAX_PCSCF_ENDPOINTS].join(","));
    assert!(parse_list(&tail).is_err());
}

#[test]
fn source_priority_is_presence_based_including_isim_before_standard_dns() {
    let mut settings = settings();
    settings.pcscf = vec!["192.0.2.11".parse().unwrap()];
    let isim = vec!["192.0.2.13".to_string()];
    assert!(matches!(select_source(Some("192.0.2.10"), &settings, Some("bad:0"), &isim).unwrap(), Selection::Explicit(PcscfSource::Environment, _)));
    assert!(matches!(select_source(None, &settings, Some("bad:0"), &isim).unwrap(), Selection::Explicit(PcscfSource::BearerPco, _)));
    settings.pcscf.clear();
    assert!(matches!(select_source(None, &settings, Some("192.0.2.12"), &isim).unwrap(), Selection::Explicit(PcscfSource::Profile, _)));
    assert!(matches!(select_source(None, &settings, None, &isim).unwrap(), Selection::Explicit(PcscfSource::Isim, _)));
    assert!(matches!(select_source(None, &settings, None, &[]).unwrap(), Selection::StandardDns));
    for invalid in ["", "host:0", "sips:host"] {
        assert!(select_source(Some(invalid), &settings, Some("192.0.2.12"), &isim).is_err());
        assert!(select_source(None, &settings, Some(invalid), &isim).is_err());
        assert!(select_source(None, &settings, None, &[invalid.to_string()]).is_err());
    }
    assert!(select_source(None, &settings, None, &vec!["host".to_string(); MAX_CONFIGURED_TARGETS + 1]).is_err());
}

#[test]
fn environment_legacy_alias_is_used_only_when_new_name_is_absent() {
    let mut names = Vec::new();
    let value = environment_override_with(|name| {
        names.push(name.to_string());
        if name == ENV_PCSCF { Err(std::env::VarError::NotPresent) } else { Ok("host:5078".to_string()) }
    }).unwrap();
    assert_eq!(value.as_deref(), Some("host:5078"));
    assert_eq!(names, [ENV_PCSCF, LEGACY_ENV_PCSCF]);
    for value in ["", "sips:host", "host:0"] {
        let result = environment_override_with(|name| {
            assert_eq!(name, ENV_PCSCF, "configured new name must not try legacy");
            Ok(value.to_string())
        }).unwrap();
        assert!(select_source(result.as_deref(), &settings(), None, &[]).is_err());
    }
    assert!(environment_override_with(|name| {
        assert_eq!(name, ENV_PCSCF);
        Err(std::env::VarError::NotUnicode(std::ffi::OsString::from("invalid")))
    }).is_err());
}

#[tokio::test]
async fn family_mismatch_keeps_selected_source_for_existing_family_fallback() {
    let mut settings = settings();
    settings.pcscf = vec!["192.0.2.11".parse().unwrap()];
    for selection in [
        select_source(Some("[2001:db8::10]:5078"), &settings, Some("192.0.2.12"), &[]).unwrap(),
        Selection::Explicit(PcscfSource::BearerPco, parse_list("2001:db8::11").unwrap()),
        select_source(None, &ImsIpSettings::default(), Some("2001:db8::12"), &["192.0.2.13".to_string()]).unwrap(),
    ] {
        let error = discover_with(selection, &settings, "ims.example", local(), |_, _, _| {
            panic!("wrong-family explicit source must never trigger DNS");
            #[allow(unreachable_code)]
            ready(Ok(DnsRecords::default()))
        }, DiscoveryLimits::default()).await.unwrap_err();
        assert_eq!(error.code(), code::PCSCF_FAMILY_MISMATCH);
    }
    let selection = select_source(Some("[2001:db8::10]:5078"), &settings, Some("192.0.2.12"), &[]).unwrap();
    let endpoints = discover_with(selection, &settings, "ims.example", "2001:db8::2".parse().unwrap(), |_, _, _| {
        panic!("IP literal requires no DNS");
        #[allow(unreachable_code)]
        ready(Ok(DnsRecords::default()))
    }, DiscoveryLimits::default()).await.unwrap();
    assert_eq!(endpoints[0].source, PcscfSource::Environment);
    assert_eq!(endpoints[0].socket.to_string(), "[2001:db8::10]:5078");
}

#[tokio::test]
async fn unresolved_explicit_host_never_falls_back_to_other_sources_or_srv() {
    let mut settings = settings();
    settings.pcscf.push("192.0.2.11".parse().unwrap());
    for fail in [false, true] {
        let selection = select_source(Some("missing.example:5078"), &settings, Some("192.0.2.12"), &["192.0.2.13".to_string()]).unwrap();
        let mut calls = 0;
        let result = discover_with(selection, &settings, "ims.example", local(), |resolver, name, kind| {
            calls += 1;
            assert_eq!(resolver, server());
            assert_eq!(name, "missing.example");
            assert_eq!(kind, 1);
            ready(if fail { Err(unavailable()) } else { Ok(DnsRecords::default()) })
        }, DiscoveryLimits::default()).await.unwrap_err();
        assert_eq!(result.code(), code::RUNTIME_ALL_PCSCF_FAILED);
        assert_eq!(calls, 1);
    }
}

#[tokio::test]
async fn configured_dns_collects_deduplicated_same_family_addresses_and_ports() {
    let settings = settings();
    let mut calls = Vec::new();
    let endpoints = discover_with(explicit("sip:one.example:5078;transport=udp,two.example:5078,one.example:5088"),
        &settings, "ims.example", local(), |resolver, name, kind| {
            calls.push((resolver, name.clone(), kind));
            ready(Ok(if name == "one.example" {
                addresses(&["192.0.2.10", "192.0.2.10", "2001:db8::10", "192.0.2.11"], 60)
            } else { addresses(&["192.0.2.10", "192.0.2.12"], 30) }))
        }, DiscoveryLimits::default()).await.unwrap();
    assert_eq!(calls.len(), 2, "same-host second port reuses the bounded query result");
    assert_eq!(endpoints.iter().map(|e| e.socket.to_string()).collect::<Vec<_>>(), [
        "192.0.2.10:5078", "192.0.2.11:5078", "192.0.2.12:5078", "192.0.2.10:5088", "192.0.2.11:5088",
    ]);
    assert_eq!(endpoints[0].ttl, Some(30));
    assert_eq!(endpoints[0].host.as_deref(), Some("one.example"));
    assert!(endpoints.iter().all(|e| e.source == PcscfSource::Environment && e.srv_priority.is_none()));
}

#[tokio::test]
async fn isim_dns_and_ipv6_candidates_keep_provenance_and_explicit_ports() {
    let settings = ImsIpSettings { ipv6_dns: vec!["2001:db8::53".parse().unwrap()], ..ImsIpSettings::default() };
    let selection = select_source(None, &settings, None, &["host.example:5098".to_string()]).unwrap();
    let endpoints = discover_with(selection, &settings, "ims.example", "2001:db8::2".parse().unwrap(), |server, name, kind| {
        assert!(server.is_ipv6());
        assert_eq!(name, "host.example");
        assert_eq!(kind, 28);
        ready(Ok(addresses(&["2001:db8::10", "192.0.2.10", "2001:db8::11"], 45)))
    }, DiscoveryLimits::default()).await.unwrap();
    assert_eq!(endpoints.len(), 2);
    assert!(endpoints.iter().all(|e| e.source == PcscfSource::Isim && e.socket.is_ipv6() && e.socket.port() == 5098 && e.ttl == Some(45)));
}

#[tokio::test]
async fn standard_dns_returns_whole_direct_rrset_before_udp_srv_fallback() {
    let mut calls = 0;
    let endpoints = discover_with(Selection::StandardDns, &settings(), "ims.example", local(), |_, name, kind| {
        calls += 1;
        assert_eq!(name, "pcscf.ims.example");
        assert_eq!(kind, 1);
        ready(Ok(addresses(&["192.0.2.10", "192.0.2.11", "192.0.2.10"], 60)))
    }, DiscoveryLimits::default()).await.unwrap();
    assert_eq!(calls, 1);
    assert_eq!(endpoints.len(), 2);
    assert!(endpoints.iter().all(|e| e.source == PcscfSource::StandardDns && e.socket.port() == 5060));
}

#[tokio::test]
async fn srv_discovery_preserves_all_targets_ports_priorities_weights_and_min_ttls() {
    let mut calls = Vec::new();
    let endpoints = discover_with(Selection::StandardDns, &settings(), "ims.example", local(), |_, name, kind| {
        calls.push((name.clone(), kind));
        ready(Ok(match (name.as_str(), kind) {
            ("pcscf.ims.example", 1) => DnsRecords::default(),
            ("_sip._udp.pcscf.ims.example", 33) => DnsRecords {
                srv_targets: vec![service("same.example", 5088, 20, 7, 90), service("same.example", 5078, 10, 3, 30)],
                ..DnsRecords::default()
            },
            ("same.example", 1) => addresses(&["192.0.2.10", "192.0.2.11"], 60),
            _ => panic!("unexpected DNS query {name}/{kind}"),
        }))
    }, DiscoveryLimits::default()).await.unwrap();
    assert_eq!(calls.len(), 3, "target addresses shared across SRV ports");
    assert_eq!(endpoints.iter().map(|e| (e.socket.port(), e.srv_priority, e.srv_weight, e.ttl)).collect::<Vec<_>>(), [
        (5078, Some(10), Some(3), Some(30)), (5078, Some(10), Some(3), Some(30)),
        (5088, Some(20), Some(7), Some(60)), (5088, Some(20), Some(7), Some(60)),
    ]);
}

#[test]
fn srv_weighted_order_is_deterministic_with_injected_draw_and_never_crosses_priority() {
    let targets = vec![
        service("late.example", 5060, 20, 100, 60),
        service("zero.example", 5060, 10, 0, 60),
        service("light.example", 5060, 10, 1, 60),
        service("heavy.example", 5060, 10, 9, 60),
    ];
    let high = order_srv_targets_with(targets.clone(), |max| Ok(max)).unwrap();
    assert_eq!(high.iter().map(|s| s.target.as_str()).collect::<Vec<_>>(), ["heavy.example", "light.example", "zero.example", "late.example"]);
    let zero = order_srv_targets_with(targets, |_| Ok(0)).unwrap();
    assert_eq!(zero[0].target, "zero.example");
    assert_eq!(zero.last().unwrap().priority, 20);
    let all_zero = vec![service("a.example", 5060, 0, 0, 60), service("b.example", 5060, 0, 0, 60)];
    assert_eq!(order_srv_targets_with(all_zero, |_| Ok(0)).unwrap().len(), 2);
    assert!(order_srv_targets_with(vec![service("a.example", 5060, 0, 1, 60)], |_| Err(unavailable())).is_err());
}

#[tokio::test]
async fn candidate_and_query_caps_apply_across_the_entire_discovery() {
    let many = (0..MAX_CONFIGURED_TARGETS).map(|i| format!("p{i}.example")).collect::<Vec<_>>().join(",");
    let mut calls = 0;
    let endpoints = discover_with(explicit(&many), &settings(), "ims.example", local(), |_, _, _| {
        calls += 1;
        ready(Ok(addresses(&[&format!("192.0.2.{}", calls + 10)], 60)))
    }, DiscoveryLimits::default()).await.unwrap();
    assert_eq!(endpoints.len(), MAX_PCSCF_ENDPOINTS);
    assert_eq!(calls, MAX_PCSCF_ENDPOINTS);
    calls = 0;
    let error = discover_with(explicit(&many), &settings(), "ims.example", local(), |_, _, _| {
        calls += 1;
        ready(Err(unavailable()))
    }, DiscoveryLimits::default()).await.unwrap_err();
    assert_eq!(calls, MAX_DNS_QUERIES);
    assert_eq!(error.code(), code::RUNTIME_ALL_PCSCF_FAILED);
}

#[tokio::test]
async fn resolver_servers_are_bounded_deduplicated_and_same_family_only() {
    let mut settings = settings();
    settings.ipv4_dns = vec![server(), server(), "2001:db8::53".parse().unwrap(),
        "192.0.2.54".parse().unwrap(), "192.0.2.55".parse().unwrap(), "192.0.2.56".parse().unwrap()];
    let mut servers = Vec::new();
    let _ = discover_with(explicit("missing.example"), &settings, "ims.example", local(), |server, _, _| {
        servers.push(server);
        ready(Err(unavailable()))
    }, DiscoveryLimits::default()).await;
    assert_eq!(servers.len(), MAX_DNS_SERVERS);
    assert_eq!(servers, ["192.0.2.53", "192.0.2.54", "192.0.2.55"].map(|s| s.parse::<IpAddr>().unwrap()));
}

#[tokio::test]
async fn srv_targets_share_query_budget_and_stop_at_candidate_cap() {
    let limits = DiscoveryLimits { candidates: 3, ..DiscoveryLimits::default() };
    let mut queries = 0;
    let endpoints = discover_with(Selection::StandardDns, &settings(), "ims.example", local(), |_, name, kind| {
        queries += 1;
        ready(Ok(if kind == 33 {
            DnsRecords { srv_targets: (0..16).map(|n| service(&format!("p{n}.example"), 5078, n, 0, 60)).collect(), ..DnsRecords::default() }
        } else if name == "pcscf.ims.example" { DnsRecords::default() }
        else {
            let index = name.strip_prefix('p').unwrap().split('.').next().unwrap().parse::<u8>().unwrap();
            addresses(&[&format!("192.0.2.{}", index + 10)], 60)
        }))
    }, limits).await.unwrap();
    assert_eq!(queries, 5);
    assert_eq!(endpoints.len(), 3);
    assert_eq!(endpoints.iter().map(|e| e.srv_priority).collect::<Vec<_>>(), [Some(0), Some(1), Some(2)]);
}

#[tokio::test]
async fn total_deadline_cancels_stalled_query_and_retains_collected_literals() {
    let limits = DiscoveryLimits { total: Duration::from_millis(5), per_query: Duration::from_secs(1), ..DiscoveryLimits::default() };
    let mut queries = 0;
    let endpoints = discover_with(explicit("192.0.2.10:5078,stalled.example,next.example,192.0.2.11:5088"),
        &settings(), "ims.example", local(), |_, _, _| {
            queries += 1;
            std::future::pending::<Result<DnsRecords, CellularImsError>>()
        }, limits).await.unwrap();
    assert_eq!(queries, 1, "total deadline is shared, not restarted per host");
    assert_eq!(endpoints.iter().map(|e| e.socket.to_string()).collect::<Vec<_>>(), ["192.0.2.10:5078", "192.0.2.11:5088"]);
}

#[tokio::test]
async fn per_query_timeout_can_try_next_bearer_resolver_without_changing_the_host() {
    let mut settings = settings();
    settings.ipv4_dns.push("192.0.2.54".parse().unwrap());
    let limits = DiscoveryLimits {
        per_query: Duration::from_millis(5),
        ..DiscoveryLimits::default()
    };
    let mut queries = 0;
    let endpoints = discover_with(explicit("host.example:5078"), &settings, "ims.example", local(), |resolver, name, kind| {
        queries += 1;
        assert_eq!(name, "host.example");
        assert_eq!(kind, 1);
        async move {
            if resolver == server() {
                std::future::pending::<Result<DnsRecords, CellularImsError>>().await
            } else {
                Ok(addresses(&["192.0.2.10", "192.0.2.11"], 60))
            }
        }
    }, limits).await.unwrap();
    assert_eq!(queries, 2);
    assert_eq!(endpoints.len(), 2);
    assert!(endpoints.iter().all(|e| e.socket.port() == 5078));
}

#[tokio::test]
async fn zero_time_or_query_budget_performs_no_dns_io() {
    for limits in [DiscoveryLimits { total: Duration::ZERO, ..DiscoveryLimits::default() }, DiscoveryLimits { queries: 0, ..DiscoveryLimits::default() }] {
        let result = discover_with(explicit("missing.example"), &settings(), "ims.example", local(), |_, _, _| {
            panic!("exhausted budget must not create a socket");
            #[allow(unreachable_code)]
            ready(Ok(DnsRecords::default()))
        }, limits).await;
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn duplicate_failed_queries_are_not_repeated_for_another_port() {
    let mut queries = 0;
    let result = discover_with(explicit("host.example:5078,host.example:5088"), &settings(), "ims.example", local(), |_, _, _| {
        queries += 1;
        ready(Err(unavailable()))
    }, DiscoveryLimits::default()).await;
    assert!(result.is_err());
    assert_eq!(queries, 1);
}
