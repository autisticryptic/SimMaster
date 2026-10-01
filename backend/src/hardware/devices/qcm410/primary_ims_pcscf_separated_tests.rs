use super::*;
use std::{
    cell::{Cell, RefCell},
    future::ready,
};

const ACT: &str = "+CGACT: 1,1\n+CGACT: 2,0\n+CGACT: 3,1";
const DEF: &str = "+CGDCONT: 1,\"IPV4V6\",\"\",\"0.0.0.0\",0,0\n+CGDCONT: 2,\"IPV4V6\",\"\"\n+CGDCONT: 3,\"IPV6\",\"ims\",\"0.0.0.0\",0,0";
const TARGET: &str = "+CGCONTRDP: 3,6,ims,2001:db8:1::a,fe80::1,,,2001:db8:10::1,2001:db8:10::2";
const ADDRESS: &str = "+CGPADDR: 3,2001:db8:1::a";
const OTHER: &str =
    "+CGCONTRDP: 1,5,INTERNET,2001:db8:9::b,fe80::1,2001:db8::53,,2001:db8:ffff::99";

fn settings() -> CgcontrdpSettings {
    CgcontrdpSettings {
        ipv6_address: Some("2001:db8:1::2".parse().unwrap()),
        ipv6_prefix: Some(64),
        ipv6_gateway: Some("fe80::1".parse().unwrap()),
        ..Default::default()
    }
}
fn reply(command: &str) -> String {
    match command {
        "AT+CGACT?" => ACT,
        "AT+CGDCONT?" => DEF,
        "AT+CGPADDR=3" => ADDRESS,
        "AT+CGCONTRDP=3" => TARGET,
        "AT+CGCONTRDP=1" => OTHER,
        other => panic!("unexpected/non-read-only command: {other}"),
    }
    .into()
}
fn decimal(ip: &str) -> String {
    ip.parse::<std::net::Ipv6Addr>()
        .unwrap()
        .octets()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

#[tokio::test]
async fn owned_ipv6_disjoint_context_proof_accepts_iid_split_without_borrowing_other_pco() {
    let expected = settings();
    let calls = RefCell::new(Vec::new());
    let result = super::super::discover_with_policy(
        &expected,
        Some(3),
        "ims",
        Duration::ZERO,
        true,
        || ready(Ok(expected.clone())),
        |command| {
            calls.borrow_mut().push(command.clone());
            let text = reply(&command)
                .replace("2001:db8:1::a", &decimal("2001:db8:1::a"))
                .replace("2001:db8:9::b", &decimal("2001:db8:9::b"));
            ready(Ok(text))
        },
    )
    .await
    .unwrap();
    assert_eq!(result.source, "mm_owned_at_disjoint_context_ipv6_prefix");
    assert_eq!(result.context_id, Some(3));
    assert_eq!(
        result.candidates,
        [
            "2001:db8:10::1".parse::<IpAddr>().unwrap(),
            "2001:db8:10::2".parse().unwrap()
        ]
    );
    assert!(!result
        .candidates
        .contains(&"2001:db8:ffff::99".parse().unwrap()));
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|c| c.as_str() == "AT+CGPADDR=3")
            .count(),
        2
    );
    assert_eq!(
        calls
            .borrow()
            .iter()
            .filter(|c| c.as_str() == "AT+CGCONTRDP=1")
            .count(),
        2
    );
    assert_eq!(
        expected,
        settings(),
        "never take AT addressing/gateway/DNS for the host"
    );
}

#[tokio::test]
async fn disjoint_context_proof_never_changes_ordinary_pin_or_exact_match_behavior() {
    for (owned, pin, exact) in [
        (false, Some(3), false),
        (true, None, false),
        (false, None, true),
    ] {
        let calls = RefCell::new(Vec::new());
        let expected = settings();
        let result = super::super::discover_with_policy(
            &expected,
            pin,
            "ims",
            Duration::ZERO,
            owned,
            || ready(Ok(expected.clone())),
            |command| {
                assert!(!command.starts_with("AT+CGPADDR="));
                assert_ne!(command, "AT+CGCONTRDP=1");
                calls.borrow_mut().push(command.clone());
                let text = reply(&command);
                ready(Ok(if exact {
                    text.replace("2001:db8:1::a", "2001:db8:1::2")
                } else {
                    text
                }))
            },
        )
        .await;
        assert_eq!(result.is_ok(), exact);
        if exact {
            assert_eq!(result.unwrap().source, "mm_owned_at_exact_address");
        }
    }
    // A dual-family definition is outside this narrowly validated exception.
    let expected = settings();
    assert!(super::super::discover_with_policy(
        &expected,
        Some(3),
        "ims",
        Duration::ZERO,
        true,
        || ready(Ok(expected.clone())),
        |command| {
            assert!(!command.starts_with("AT+CGPADDR="));
            ready(Ok(reply(&command).replace("3,\"IPV6\"", "3,\"IPV4V6\"")))
        }
    )
    .await
    .is_err());
}

#[test]
fn disjoint_context_requires_strict_cgpaddr_full_address_cid_and_complete_response() {
    let expected = "2001:db8:1::a".parse().unwrap();
    assert_eq!(assigned_address(ADDRESS, 3, expected).unwrap(), expected);
    for invalid in [
        "+CGPADDR: 2,2001:db8:1::a",
        "+CGPADDR: 3,2001:db8:1::b",
        "+CGPADDR: 3,2001:db8:1::a,2001:db8:1::a",
        "+CGPADDR: 3,2001:db8:1::a junk",
        "+CGPADDR: 3,2001:db8:1::a\nERROR",
        "+CGPADDR: 3,2001:db8:1::a\n+CGPADDR: 3,2001:db8:1::a",
        "+CGPADDR: 3,::",
        "+CGPADDR: 3,fe80::a",
        "+CGPADDR: 3,2001:db8:1::a/64",
        "+CGPADDR: 3,32.1.13.184.0.1.0.0.0.0.0.0.0.0.0.+10",
    ] {
        assert!(assigned_address(invalid, 3, expected).is_err(), "{invalid}");
    }
}

#[tokio::test]
async fn disjoint_context_rejects_overlap_unknown_or_same_ims_context() {
    for value in [
        OTHER.replace("2001:db8:9::b", "2001:db8:1::b"),
        OTHER.replace("2001:db8:9::b", "2001:db8:9::"),
        OTHER.replace("2001:db8:9::b", "2001:db8:9::b/48"),
        OTHER.replace("2001:db8:9::b", "fe80::b"),
        OTHER.replace("1,5,INTERNET", "1,6,INTERNET"),
        OTHER.replace("1,5,INTERNET", "1,5,ims"),
        OTHER.replace("1,5,INTERNET", "2,5,INTERNET"),
        OTHER.replace("1,5,INTERNET", "1,5,"),
        "+CGCONTRDP: 1,5,INTERNET,192.0.2.3,192.0.2.1,,".into(),
        format!("{OTHER}\n{OTHER}"),
        format!("{OTHER}\nERROR"),
        String::new(),
    ] {
        let expected = settings();
        let result = super::super::discover_with_policy(
            &expected,
            Some(3),
            "ims",
            Duration::ZERO,
            true,
            || ready(Ok(expected.clone())),
            |command| {
                ready(Ok(if command == "AT+CGCONTRDP=1" {
                    value.clone()
                } else {
                    reply(&command)
                }))
            },
        )
        .await;
        assert!(result.is_err(), "{value}");
    }
}

#[tokio::test]
async fn disjoint_context_checks_every_observation_again_before_publication() {
    for (command, nth, changed) in [
        ("AT+CGACT?", 2, ACT.replace("1,1", "1,0")),
        ("AT+CGACT?", 3, ACT.replace("3,1", "3,0")),
        ("AT+CGDCONT?", 2, DEF.replace("0,0", "1,0")),
        (
            "AT+CGCONTRDP=3",
            2,
            TARGET.replace("2001:db8:10::1", "2001:db8:10::3"),
        ),
        ("AT+CGCONTRDP=3", 2, TARGET.replace("3,6,", "3,7,")),
        ("AT+CGPADDR=3", 2, ADDRESS.replace("::a", "::b")),
        ("AT+CGCONTRDP=1", 2, OTHER.replace("INTERNET", "OTHER")),
        (
            "AT+CGCONTRDP=1",
            2,
            OTHER.replace("2001:db8:9::b", "2001:db8:8::b"),
        ),
        ("AT+CGCONTRDP=1", 2, OTHER.replace("1,5,", "1,7,")),
        (
            "AT+CGCONTRDP=1",
            2,
            OTHER.replace("2001:db8::53", "2001:db8::54"),
        ),
    ] {
        let expected = settings();
        let counts = RefCell::new(BTreeMap::<String, usize>::new());
        let result = super::super::discover_with_policy(
            &expected,
            Some(3),
            "ims",
            Duration::ZERO,
            true,
            || ready(Ok(expected.clone())),
            |request| {
                let mut counts = counts.borrow_mut();
                let count = counts.entry(request.clone()).or_default();
                *count += 1;
                ready(Ok(if request == command && *count == nth {
                    changed.clone()
                } else {
                    reply(&request)
                }))
            },
        )
        .await;
        assert!(result.is_err(), "{command}/{nth}");
    }
    let expected = settings();
    let reads = Cell::new(0);
    let result = super::super::discover_with_policy(
        &expected,
        Some(3),
        "ims",
        Duration::ZERO,
        true,
        || {
            reads.set(reads.get() + 1);
            let mut snapshot = expected.clone();
            if reads.get() > 1 {
                snapshot.ipv6_address = Some("2001:db8:1::3".parse().unwrap());
            }
            ready(Ok(snapshot))
        },
        |command| ready(Ok(reply(&command))),
    )
    .await
    .unwrap_err();
    assert_eq!(result.kind, ImsBearerErrorKind::SessionLost);
}
