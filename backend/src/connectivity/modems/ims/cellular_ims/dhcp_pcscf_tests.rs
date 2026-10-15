use super::*;

fn transaction() -> Transaction {
    let mut duid = [1u8; 18];
    duid[..2].copy_from_slice(&4u16.to_be_bytes());
    duid[8] = 0x41;
    duid[10] = 0x81;
    let mut client_v4 = vec![255, 1, 2, 3, 4];
    client_v4.extend_from_slice(&duid);
    Transaction { xid: [1, 2, 3, 4], duid, client_v4, jitter: [0; 3] }
}

fn local4() -> Ipv4Addr { Ipv4Addr::new(192, 0, 2, 2) }
fn server4() -> SocketAddr { "192.0.2.1:67".parse().unwrap() }
fn server6() -> SocketAddr { "[2001:db8::1]:547".parse().unwrap() }
fn multicast6() -> SocketAddr { "[ff02::1:2%7]:547".parse().unwrap() }

fn ack4_base(tx: &Transaction) -> Vec<u8> {
    let mut packet = inform_v4(local4(), tx, Duration::ZERO);
    packet.truncate(240);
    packet[0] = 2;
    push_v4(&mut packet, 53, &[5]);
    push_v4(&mut packet, 54, &[192, 0, 2, 1]);
    push_v4(&mut packet, 61, &tx.client_v4);
    packet
}

fn ack4(tx: &Transaction, sip: &[u8]) -> Vec<u8> {
    let mut packet = ack4_base(tx);
    push_v4(&mut packet, 120, sip);
    packet.push(255);
    packet
}

fn reply6_base(tx: &Transaction) -> Vec<u8> {
    let mut packet = vec![7];
    packet.extend_from_slice(&tx.xid[..3]);
    push_v6(&mut packet, 1, &tx.duid);
    let mut server = [5u8; 18];
    server[..2].copy_from_slice(&4u16.to_be_bytes());
    server[8] = 0x45;
    server[10] = 0x85;
    push_v6(&mut packet, 2, &server);
    packet
}

fn reply6(tx: &Transaction) -> Vec<u8> {
    let mut packet = reply6_base(tx);
    push_v6(&mut packet, 21, b"\x03sip\x07example\x03com\0");
    push_v6(&mut packet, 22, &"2001:db8::10".parse::<Ipv6Addr>().unwrap().octets());
    packet
}

#[test]
fn valid_v4_address_and_domain_vectors_preserve_order_and_source() {
    let tx = transaction();
    let numeric = decode_v4(&ack4(&tx, &[1, 192, 0, 2, 10, 10, 0, 0, 1]), &tx, local4(), server4()).unwrap();
    assert_eq!(numeric.candidates, vec!["192.0.2.10".parse::<IpAddr>().unwrap(), "10.0.0.1".parse().unwrap()]);
    assert!(numeric.domains.is_empty());
    assert_eq!(numeric.context_id, None);
    assert_eq!(numeric.source, "dhcp_information");
    let names = decode_v4(&ack4(&tx, b"\0\x03SIP\x07example\x03com\0\x05proxy\xc0\x04"), &tx, local4(), server4()).unwrap();
    assert_eq!(names.domains, ["sip.example.com", "proxy.example.com"]);
    assert!(names.candidates.is_empty());
    assert_eq!(domain_list(b"\x03sip\x07example\x03com\0\x05proxy\xc0\x04\x05pcscf\xc0\x17", true).unwrap(),
        ["sip.example.com", "proxy.example.com", "pcscf.example.com"]);
}

#[test]
fn v4_ack_may_leave_ciaddr_zero_without_weakening_transaction_or_client_binding() {
    let tx = transaction();
    let mut packet = ack4(&tx, &[1, 192, 0, 2, 10]);
    // RFC 2131 ACK permits zero ciaddr. The receive socket still belongs to
    // the granted local address; xid/client-id/server-id remain mandatory.
    packet[12..16].fill(0);
    assert_eq!(decode_v4(&packet, &tx, local4(), server4()).unwrap().candidates.len(), 1);
    packet[4] ^= 1;
    assert!(decode_v4(&packet, &tx, local4(), server4()).is_err());
}

#[test]
fn v4_rfc3396_split_and_overload_options_file_sname_order() {
    let tx = transaction();
    let sip = b"\0\x03sip\x07example\x03com\0\x05proxy\xc0\x04";
    let mut packet = ack4_base(&tx);
    push_v4(&mut packet, 52, &[3]);
    push_v4(&mut packet, 120, &sip[..3]);
    packet.push(255);
    let mut file = Vec::new();
    push_v4(&mut file, 120, &sip[3..20]);
    file.push(255);
    packet[108..108 + file.len()].copy_from_slice(&file);
    let mut sname = Vec::new();
    push_v4(&mut sname, 120, &sip[20..]);
    sname.push(255);
    packet[44..44 + sname.len()].copy_from_slice(&sname);
    assert_eq!(decode_v4(&packet, &tx, local4(), server4()).unwrap().domains,
        ["sip.example.com", "proxy.example.com"]);
    let mut numeric = ack4_base(&tx);
    push_v4(&mut numeric, 120, &[1, 192]);
    push_v4(&mut numeric, 120, &[0, 2, 10]);
    numeric.push(255);
    assert_eq!(decode_v4(&numeric, &tx, local4(), server4()).unwrap().candidates.len(), 1);
}

#[test]
fn v4_rfc3396_option_over_255_bytes_is_concatenated_before_decode() {
    let tx = transaction();
    let mut sip = vec![0];
    for first in b'a'..=b'd' {
        sip.push(63);
        sip.extend_from_slice(&vec![first; 63]);
        sip.extend_from_slice(b"\x03com\0");
    }
    assert!(sip.len() > 255);
    let mut packet = ack4_base(&tx);
    for fragment in sip.chunks(255) { push_v4(&mut packet, 120, fragment); }
    packet.push(255);
    assert_eq!(decode_v4(&packet, &tx, local4(), server4()).unwrap().domains.len(), 4);
}

#[test]
fn v4_rejects_duplicate_mandatory_identity_and_overload_fields() {
    let tx = transaction();
    for (code, value) in [(53, vec![5]), (54, vec![192, 0, 2, 1]), (61, tx.client_v4.clone())] {
        let mut packet = ack4(&tx, &[1, 192, 0, 2, 10]);
        packet.pop();
        push_v4(&mut packet, code, &value);
        packet.push(255);
        assert!(decode_v4(&packet, &tx, local4(), server4()).is_err());
    }
    let mut packet = ack4(&tx, &[1, 192, 0, 2, 10]);
    packet.pop();
    push_v4(&mut packet, 52, &[1]);
    push_v4(&mut packet, 52, &[1]);
    packet.push(255);
    assert!(decode_v4(&packet, &tx, local4(), server4()).is_err());
}

#[test]
fn v4_rejects_wrong_transaction_client_server_local_and_relay() {
    let tx = transaction();
    let valid = ack4(&tx, &[1, 192, 0, 2, 10]);
    for offset in [0, 1, 2, 3, 4, 10, 12, 16, 24, 28, 236, 245, 252] {
        let mut bad = valid.clone();
        bad[offset] ^= 1;
        assert!(decode_v4(&bad, &tx, local4(), server4()).is_err(), "offset {offset}");
    }
    for peer in ["192.0.2.2:67", "192.0.2.1:68", "127.0.0.1:67", "224.0.0.1:67", "[2001:db8::1]:67"] {
        assert!(decode_v4(&valid, &tx, local4(), peer.parse().unwrap()).is_err());
    }
    let mut no_echo = ack4_base(&tx);
    // Keep only message type and server identifier; RFC 6842 echo is mandatory.
    no_echo.truncate(249);
    push_v4(&mut no_echo, 120, &[1, 192, 0, 2, 10]);
    no_echo.push(255);
    assert!(decode_v4(&no_echo, &tx, local4(), server4()).is_err());
}

#[test]
fn v4_rejects_truncation_overflow_bad_end_and_assignment() {
    let tx = transaction();
    let valid = ack4(&tx, &[1, 192, 0, 2, 10]);
    for end in 0..valid.len() { assert!(decode_v4(&valid[..end], &tx, local4(), server4()).is_err()); }
    let mut oversized = valid.clone();
    oversized.resize(MAX_WIRE + 1, 0);
    assert!(decode_v4(&oversized, &tx, local4(), server4()).is_err());
    let mut trailing = valid.clone();
    trailing.push(1);
    assert!(decode_v4(&trailing, &tx, local4(), server4()).is_err());
    for code in [50, 51, 58, 59, 82] {
        let mut packet = valid.clone();
        packet.pop();
        push_v4(&mut packet, code, &[0, 0, 0, 1]);
        packet.push(255);
        assert!(decode_v4(&packet, &tx, local4(), server4()).is_err());
    }
}

#[test]
fn v4_unsolicited_dns_and_routes_are_not_returned_or_requested() {
    let tx = transaction();
    let mut packet = ack4_base(&tx);
    push_v4(&mut packet, 3, &[10, 0, 0, 1]);
    push_v4(&mut packet, 6, &[10, 0, 0, 53]);
    push_v4(&mut packet, 120, &[1, 192, 0, 2, 10]);
    packet.push(255);
    let result = decode_v4(&packet, &tx, local4(), server4()).unwrap();
    assert_eq!(result.candidates, vec!["192.0.2.10".parse::<IpAddr>().unwrap()]);
    let request = inform_v4(local4(), &tx, Duration::from_secs(3));
    let options = options_v4(&request).unwrap();
    assert_eq!(options[53].as_deref(), Some(&[8][..]));
    assert_eq!(options[55].as_deref(), Some(&[120][..]));
    assert_eq!(&request[12..16], &local4().octets());
    assert_eq!(&request[16..236], &[0u8; 220]);
    for code in [3, 6, 50, 51, 54, 58, 59] { assert!(options[code].is_none()); }
}

#[test]
fn valid_v6_domain_and_address_vectors_and_scoped_peer() {
    let tx = transaction();
    let packet = reply6(&tx);
    let result = decode_v6(&packet, &tx, server6(), multicast6()).unwrap();
    assert_eq!(result.domains, ["sip.example.com"]);
    assert_eq!(result.candidates, vec!["2001:db8::10".parse::<IpAddr>().unwrap()]);
    assert_eq!(result.source, "dhcp_information");
    assert_eq!(result.context_id, None);
    assert!(decode_v6(&packet, &tx, "[fe80::1%7]:547".parse().unwrap(), multicast6()).is_ok());
    for source in ["[fe80::1%8]:547", "[fe80::1]:547", "[::1]:547", "[ff02::1]:547", "[2001:db8::1]:546"] {
        assert!(decode_v6(&packet, &tx, source.parse().unwrap(), multicast6()).is_err());
    }
}

#[test]
fn v6_rejects_duplicate_ids_options_wrong_xid_client_and_server() {
    let tx = transaction();
    let valid = reply6(&tx);
    for code in [1, 2, 21, 22] {
        let mut packet = valid.clone();
        push_v6(&mut packet, code, &tx.duid);
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err());
    }
    for offset in [0, 1, 8, 30] { // type, xid, client DUID, server DUID type
        let mut packet = valid.clone();
        packet[offset] ^= 128;
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err(), "offset {offset}");
    }
    let mut client_as_server = vec![7, 1, 2, 3];
    push_v6(&mut client_as_server, 1, &tx.duid);
    push_v6(&mut client_as_server, 2, &tx.duid);
    push_v6(&mut client_as_server, 21, b"\x03sip\x07example\x03com\0");
    assert!(decode_v6(&client_as_server, &tx, server6(), multicast6()).is_err());
    for missing in [1, 2] {
        let mut packet = vec![7, 1, 2, 3];
        push_v6(&mut packet, missing, &tx.duid);
        push_v6(&mut packet, 21, b"\x03sip\x07example\x03com\0");
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err());
    }
}

#[test]
fn v6_rejects_truncation_oversize_relay_envelopes_assignments_and_failure_status() {
    let tx = transaction();
    let mut address_only = reply6_base(&tx);
    push_v6(&mut address_only, 22, &"2001:db8::10".parse::<Ipv6Addr>().unwrap().octets());
    for end in 0..address_only.len() {
        assert!(decode_v6(&address_only[..end], &tx, server6(), multicast6()).is_err(), "prefix {end}");
    }
    let mut oversized = address_only.clone();
    oversized.resize(MAX_WIRE + 1, 0);
    assert!(decode_v6(&oversized, &tx, server6(), multicast6()).is_err());
    for msg in [2, 10, 12, 13] {
        let mut packet = address_only.clone(); packet[0] = msg;
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err());
    }
    for code in [3, 4, 5, 9, 18, 25, 26, 13] {
        let mut packet = address_only.clone();
        push_v6(&mut packet, code, &[0, 1]);
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err());
    }
}

#[test]
fn v6_request_is_stateless_and_has_no_server_or_identity_associations() {
    let tx = transaction();
    let packet = information_v6(&tx, Duration::from_millis(1230));
    assert_eq!(&packet[..4], &[11, 1, 2, 3]);
    let mut expected = vec![11, 1, 2, 3];
    push_v6(&mut expected, 1, &tx.duid);
    push_v6(&mut expected, 6, &[0, 21, 0, 22, 0, 83, 0, 32]);
    push_v6(&mut expected, 8, &[0, 123]);
    assert_eq!(packet, expected);
}

#[test]
fn rejects_unsuitable_and_duplicate_numeric_servers_both_families() {
    let tx = transaction();
    for value in ["0.1.2.3", "127.0.0.1", "169.254.1.1", "224.0.0.1", "255.255.255.255", "240.0.0.1"] {
        let mut sip = vec![1];
        sip.extend_from_slice(&value.parse::<Ipv4Addr>().unwrap().octets());
        assert!(decode_v4(&ack4(&tx, &sip), &tx, local4(), server4()).is_err());
    }
    assert!(decode_v4(&ack4(&tx, &[1, 192, 0, 2, 1, 192, 0, 2, 1]), &tx, local4(), server4()).is_err());
    for value in ["::", "::1", "fe80::1", "fec0::1", "ff02::1", "::ffff:192.0.2.1", "::192.0.2.1"] {
        let mut packet = reply6_base(&tx);
        push_v6(&mut packet, 22, &value.parse::<Ipv6Addr>().unwrap().octets());
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err());
    }
    let mut duplicate = reply6_base(&tx);
    push_v6(&mut duplicate, 22, &["2001:db8::1".parse::<Ipv6Addr>().unwrap().octets(); 2].concat());
    assert!(decode_v6(&duplicate, &tx, server6(), multicast6()).is_err());
}

#[test]
fn domains_reject_cycles_bad_pointers_binary_labels_bad_names_and_duplicates() {
    for bytes in [
        &b"\0"[..], &b"\xc0\0"[..], &b"\xc0\x02\0"[..], &b"\x40a\0"[..],
        &b"\x03sip\xc0\0"[..], &b"\x03sip\x07example\x03com\0\xc0\x01"[..],
        &b"\x03sip\x07example\x03com\0\xc0\0"[..], &b"\x03sip\0"[..],
        &b"\x04_sip\x03com\0"[..], &b"\x04-sip\x03com\0"[..],
        &b"\x04sip-\x03com\0"[..], &b"\x03s\0p\x03com\0"[..],
        &b"\x03sip\x03com"[..], &b"\x03sip\x03c\xffm\0"[..],
        &b"\x03192\x010\x012\x011\0"[..], &b"\x03sip\x05local\0"[..],
    ] { assert!(domain_list(bytes, true).is_err(), "{bytes:?}"); }
    let compressed = b"\x03sip\x07example\x03com\0\x05proxy\xc0\x04";
    assert!(domain_list(compressed, false).is_err());
    let mut long = Vec::new();
    for _ in 0..4 { long.push(63); long.extend_from_slice(&[b'a'; 63]); }
    long.push(0);
    assert!(domain_list(&long, true).is_err());
    assert!(domain_list(&vec![0; MAX_WIRE + 1], true).is_err());
}

#[test]
fn malformed_domains_fail_at_both_full_message_boundaries() {
    let tx = transaction();
    for names in [&b"\0"[..], &b"\xc0\0"[..], &b"\x04_sip\x03com\0"[..],
        &b"\x03sip\x07example\x03com\0\x03SIP\x07example\x03com\0"[..]]
    {
        let mut sip = vec![0]; sip.extend_from_slice(names);
        assert!(decode_v4(&ack4(&tx, &sip), &tx, local4(), server4()).is_err());
        let mut packet = reply6_base(&tx); push_v6(&mut packet, 21, names);
        assert!(decode_v6(&packet, &tx, server6(), multicast6()).is_err());
    }
}

#[test]
fn option_and_result_counts_are_bounded() {
    let tx = transaction();
    let mut packet = ack4_base(&tx);
    for _ in 0..MAX_OPTIONS { push_v4(&mut packet, 100, &[1]); }
    push_v4(&mut packet, 120, &[1, 192, 0, 2, 10]);
    packet.push(255);
    assert!(decode_v4(&packet, &tx, local4(), server4()).is_err());
    let mut sip = vec![1];
    for n in 1..=17 { sip.extend_from_slice(&[192, 0, 2, n]); }
    assert!(decode_v4(&ack4(&tx, &sip), &tx, local4(), server4()).is_err());
    for sip in [&[1][..], &[2, 192, 0, 2, 1][..], &[1, 192, 0][..], &[0][..]] {
        assert!(decode_v4(&ack4(&tx, sip), &tx, local4(), server4()).is_err());
    }
}

#[cfg(unix)]
mod exchange_tests {
    use super::*;
    use crate::platform::netns::NetnsName;
    use crate::services::ue_worker::{UeSocketKind, UeWorkerHandle};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    async fn fixture(ipv6: bool) -> (UeWorkerHandle, ExchangeSettings, UdpSocket, Arc<AtomicUsize>) {
        let host = if ipv6 { "[::1]:0" } else { "127.0.0.1:0" };
        let peer = UdpSocket::bind(host).await.unwrap();
        let local = std::net::UdpSocket::bind(host).unwrap();
        let local_port = local.local_addr().unwrap().port();
        let peer_addr = peer.local_addr().unwrap();
        let worker = UeWorkerHandle::for_line("dhcp-test", NetnsName::for_line("sa-ue", "dhcp-test"));
        let count = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::clone(&count);
        let mut local = Some(local);
        worker.enable_test_socket_factory(move |spec| {
            recorded.fetch_add(1, Ordering::SeqCst);
            assert_eq!(spec.kind, UeSocketKind::Udp);
            assert_eq!(spec.bind_to_device.as_deref(), Some("wwan0"));
            assert_eq!(spec.bind.unwrap().port(), local_port);
            assert_eq!(spec.bind.unwrap().ip(), if ipv6 { "2001:db8::2".parse::<IpAddr>().unwrap() } else { local4().into() });
            assert!(spec.dhcp_information);
            assert!(!spec.reuse_address);
            assert!(spec.connect.is_none());
            Ok(local.take().expect("one socket per exchange").into())
        }).await;
        (worker, ExchangeSettings { wire: Some(TestWire { local_port, peer: peer_addr, budget: Duration::from_secs(1) }) }, peer, count)
    }

    fn transaction_from_request(request: &[u8], ipv6: bool) -> Transaction {
        let mut tx = transaction();
        if ipv6 {
            assert_eq!(request[0], 11);
            tx.xid[..3].copy_from_slice(&request[1..4]);
            assert_eq!(&request[4..8], &[0, 1, 0, 18]);
            tx.duid.copy_from_slice(&request[8..26]);
            let elapsed = u16::from_be_bytes(request[request.len() - 2..].try_into().unwrap());
            assert_eq!(request, information_v6(&tx, Duration::from_millis(u64::from(elapsed) * 10)));
        } else {
            assert_eq!(request[0], 1);
            tx.xid.copy_from_slice(&request[4..8]);
            let options = options_v4(request).unwrap();
            assert_eq!(options[53].as_deref(), Some(&[8][..]));
            assert_eq!(options[55].as_deref(), Some(&[120][..]));
            tx.client_v4 = options[61].clone().unwrap();
            assert_eq!(tx.client_v4[0], 255);
            assert!(options[50].is_none());
            assert!(options[51].is_none());
            assert_eq!(&request[12..16], &local4().octets());
        }
        tx
    }

    async fn receive(peer: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buffer = [0u8; MAX_WIRE + 1];
        let (len, source) = timeout_at(Instant::now() + Duration::from_secs(2), peer.recv_from(&mut buffer)).await.unwrap().unwrap();
        (buffer[..len].to_vec(), source)
    }

    #[tokio::test]
    async fn v4_fake_peer_uses_production_exchange_and_captured_worker_factory() {
        let (worker, settings, peer, count) = fixture(false).await;
        let binding = worker.bind();
        let responder = async {
            let (request, source) = receive(&peer).await;
            let tx = transaction_from_request(&request, false);
            let mut wrong = ack4(&tx, &[1, 192, 0, 2, 10]); wrong[4] ^= 1;
            peer.send_to(&wrong, source).await.unwrap();
            peer.send_to(&ack4(&tx, b"\0\x03sip\x07example\x03com\0"), source).await.unwrap();
        };
        let locals = [local4().into()];
        let (result, ()) = tokio::join!(discover_with(&binding, "wwan0", &locals, &settings), responder);
        assert_eq!(result.unwrap().domains, ["sip.example.com"]);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn v6_fake_peer_uses_production_exchange_and_ignores_malformed_responses() {
        let (worker, settings, peer, count) = fixture(true).await;
        let binding = worker.bind();
        let responder = async {
            let (request, source) = receive(&peer).await;
            let tx = transaction_from_request(&request, true);
            let mut wrong = reply6(&tx); wrong[8] ^= 1;
            peer.send_to(&wrong, source).await.unwrap();
            peer.send_to(&reply6(&tx), source).await.unwrap();
        };
        let locals = ["2001:db8::2".parse().unwrap()];
        let (result, ()) = tokio::join!(discover_with(&binding, "wwan0", &locals, &settings), responder);
        let result = result.unwrap();
        assert_eq!(result.domains, ["sip.example.com"]);
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn oversized_datagram_and_wrong_udp_peer_cannot_supply_results() {
        let (worker, settings, peer, _) = fixture(false).await;
        let binding = worker.bind();
        let responder = async {
            let (request, source) = receive(&peer).await;
            let tx = transaction_from_request(&request, false);
            let mut oversized = ack4(&tx, &[1, 192, 0, 2, 10]);
            oversized.resize(MAX_WIRE + 500, 0);
            peer.send_to(&oversized, source).await.unwrap();
            let wrong_peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            wrong_peer.send_to(&ack4(&tx, &[1, 192, 0, 2, 10]), source).await.unwrap();
        };
        let locals = [local4().into()];
        let (result, ()) = tokio::join!(discover_with(&binding, "wwan0", &locals, &settings), responder);
        assert_eq!(result.unwrap_err().code(), code::DHCP_PCSCF_FAILED);
    }

    #[tokio::test]
    async fn retries_are_bounded_and_keep_the_same_random_transaction() {
        let (worker, settings, peer, _) = fixture(false).await;
        let binding = worker.bind();
        let responder = async {
            let (first, source) = receive(&peer).await;
            let (second, second_source) = receive(&peer).await;
            assert_eq!(source, second_source);
            assert_eq!(&first[..8], &second[..8]);
            assert_eq!(&first[10..], &second[10..]);
            let mut buffer = [0u8; MAX_WIRE];
            assert!(tokio::time::timeout(Duration::from_millis(200), peer.recv_from(&mut buffer)).await.is_err());
        };
        let locals = [local4().into()];
        let (result, ()) = tokio::join!(discover_with(&binding, "wwan0", &locals, &settings), responder);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invalid_reply_flood_exhausts_message_budget_without_accepting_late_data() {
        let (worker, settings, peer, _) = fixture(false).await;
        let binding = worker.bind();
        let responder = async {
            let (request, source) = receive(&peer).await;
            let tx = transaction_from_request(&request, false);
            let mut wrong = ack4(&tx, &[1, 192, 0, 2, 10]); wrong[4] ^= 1;
            for _ in 0..MAX_MESSAGES { peer.send_to(&wrong, source).await.unwrap(); }
            // The receiver may already have closed after reaching its cap.
            let _ = peer.send_to(&ack4(&tx, &[1, 192, 0, 2, 10]), source).await;
        };
        let locals = [local4().into()];
        let (result, ()) = tokio::join!(discover_with(&binding, "wwan0", &locals, &settings), responder);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn worker_replacement_during_exchange_rejects_late_reply() {
        let (worker, settings, peer, count) = fixture(false).await;
        let binding = worker.bind();
        let responder = async {
            let (request, source) = receive(&peer).await;
            let tx = transaction_from_request(&request, false);
            worker.advance_test_generation().await;
            peer.send_to(&ack4(&tx, &[1, 192, 0, 2, 10]), source).await.unwrap();
        };
        let locals = [local4().into()];
        let (result, ()) = tokio::join!(discover_with(&binding, "wwan0", &locals, &settings), responder);
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(discover_with(&binding, "wwan0", &[local4().into()], &settings).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn missing_namespace_multicast_scope_is_explicit_failure() {
        let socket = UdpSocket::bind("[::1]:0").await.unwrap();
        let settings = ExchangeSettings::default();
        assert!(settings.destination("2001:db8::2".parse().unwrap(), &socket).is_err());
    }

    #[tokio::test]
    async fn no_eligible_family_or_worker_boundary_fails_without_host_socket() {
        let worker = UeWorkerHandle::for_line("missing", NetnsName::for_line("sa-ue", "missing"));
        let binding = worker.bind();
        for locals in [vec![], vec!["0.0.0.0".parse().unwrap(), "::1".parse().unwrap()]] {
            assert_eq!(discover(&binding, "wwan0", &locals).await.unwrap_err().code(), code::DHCP_PCSCF_FAILED);
        }
        assert!(discover(&binding, "wwan0", &[local4().into()]).await.is_err());
        assert!(discover(&binding, "bad/name", &[local4().into()]).await.is_err());
    }
}
