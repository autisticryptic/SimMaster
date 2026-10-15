use super::*;

fn field(tlv_type: u8, value: Vec<u8>) -> QmiTlv {
    QmiTlv { tlv_type, value }
}

fn success() -> QmiTlv {
    field(2, vec![0; 4])
}

fn response(service: u8, cid: u8, tx: u16, id: u16, tlvs: Vec<QmiTlv>) -> Vec<u8> {
    let mut frame = encode_qmi_message(&QmiMessage {
        service, client_id: cid, transaction_id: tx, message_id: id, tlvs,
    }).unwrap();
    frame[3] = 0x80;
    frame[6] = if service == 0 { 1 } else { 2 };
    frame
}

fn wds_reply(fields: Vec<QmiTlv>) -> Vec<u8> {
    let mut tlvs = vec![success()];
    tlvs.extend(fields);
    response(WDS, 7, 0x1234, GET_CURRENT_SETTINGS, tlvs)
}

fn domains(names: &[&[u8]]) -> Vec<u8> {
    let mut bytes = vec![names.len() as u8];
    for name in names {
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(name);
    }
    bytes
}

fn decode_settings(frame: &[u8]) -> Result<RetainedWdsSettings, NativeError> {
    let (message, is_response) = decode_inbound(frame)?;
    assert!(is_response);
    settings(&message)
}

#[test]
fn retained_query_requests_only_pinned_pcscf_lists_on_exact_cid() {
    assert_eq!(encode_qmi_message(&request(7, 0x1234).unwrap()).unwrap(), vec![
        1, 19, 0, 0, 1, 7, 0, 0x34, 0x12, 0x2d, 0, 7, 0,
        0x10, 4, 0, 0, 0x18, 0, 0,
    ]);
    for cid in [0, 255] {
        assert!(request(cid, 1).is_err());
    }
    assert!(request(7, 0).is_err());
}

#[test]
fn retained_codec_decodes_little_endian_ipv4_and_counted_u16_fqdns() {
    let frame = wds_reply(vec![
        field(0x23, vec![2, 7, 2, 0, 192, 8, 2, 0, 192]),
        field(0x24, domains(&[b"PCSCF.Example.ORG.", b"backup.example.org"])),
    ]);
    let parsed = decode_settings(&frame).unwrap();
    assert_eq!(parsed.addresses, vec![
        "192.0.2.7".parse::<IpAddr>().unwrap(), "192.0.2.8".parse::<IpAddr>().unwrap(),
    ]);
    assert_eq!(parsed.domains, vec!["pcscf.example.org", "backup.example.org"]);
    assert_eq!(format!("{parsed:?}"), "RetainedWdsSettings { address_count: 2, domain_count: 2 }");
}

#[test]
fn retained_codec_never_promotes_generic_domains_or_interface_ipv6_to_pcscf() {
    let frame = wds_reply(vec![
        field(0x2a, domains(&[b"not-a-pcscf.example.org"])),
        field(0x25, vec![0; 17]),
        field(0x22, vec![1]),
    ]);
    assert_eq!(decode_settings(&frame).unwrap(), RetainedWdsSettings::default());
    assert_eq!(decode_settings(&wds_reply(vec![])).unwrap(), RetainedWdsSettings::default());
    assert_eq!(decode_settings(&wds_reply(vec![field(0x23, vec![0]), field(0x24, vec![0])])).unwrap(),
        RetainedWdsSettings::default());
}

#[test]
fn retained_codec_rejects_unsafe_pcscf_names_instead_of_cleaning_them() {
    for name in [
        "", "pcscf", ".example.org", "pcscf..example.org", "pcscf.example.org..",
        "pcscf.example.org:5060", "sip:pcscf.example.org", "https://pcscf.example.org",
        "user@pcscf.example.org", "pcscf.example.org/path", "*.example.org", "_sip.example.org",
        "-pcscf.example.org", "pcscf-.example.org", " pcscf.example.org", "pcscf.example.org ",
        "pcscf.example.org\0", "pcscf.\nexample.org", "pcscf.例子.org", "192.0.2.7",
        "2001:db8::7", "[2001:db8::7]", "pcscf.123", "127.1", "0x7f.1",
    ] {
        assert!(decode_settings(&wds_reply(vec![field(0x24, domains(&[name.as_bytes()]))])).is_err(), "{name:?}");
    }
    assert!(pcscf_name(&[0xff, b'.', b'x']).is_err());
    assert!(pcscf_name(format!("{}.example.org", "a".repeat(64)).as_bytes()).is_err());
    let maximal = format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61));
    assert_eq!(maximal.len(), 253);
    assert!(pcscf_name(maximal.as_bytes()).is_ok());
    assert!(pcscf_name(format!("{maximal}.").as_bytes()).is_ok());
    assert!(pcscf_name(format!("{maximal}x").as_bytes()).is_err());
}

#[test]
fn retained_codec_rejects_unusable_addresses_and_duplicate_candidates() {
    for ip in ["0.0.0.0", "0.1.2.3", "127.0.0.1", "127.1.2.3", "169.254.1.2",
        "224.0.0.1", "239.255.255.255", "240.0.0.1", "255.255.255.255"] {
        let ip = ip.parse::<Ipv4Addr>().unwrap();
        let mut bytes = vec![1];
        bytes.extend_from_slice(&u32::from(ip).to_le_bytes());
        assert!(decode_settings(&wds_reply(vec![field(0x23, bytes)])).is_err());
    }
    assert!(usable_address("10.1.2.3".parse().unwrap()));
    assert!(decode_settings(&wds_reply(vec![field(0x23, vec![2, 7, 2, 0, 192, 7, 2, 0, 192])])).is_err());
    assert!(decode_settings(&wds_reply(vec![field(0x24, domains(&[b"PCSCF.example.org.", b"pcscf.example.org"]))])).is_err());
}

#[test]
fn retained_codec_rejects_list_count_length_and_resource_bound_violations() {
    for value in [vec![], vec![1], vec![1, 7, 2, 0], vec![0, 0], vec![2, 7, 2, 0, 192]] {
        assert!(decode_settings(&wds_reply(vec![field(0x23, value)])).is_err());
    }
    for value in [vec![], vec![1], vec![1, 1], vec![1, 0, 0], vec![1, 3, 0, b'a'],
        vec![1, 255, 255], vec![0, 0]] {
        assert!(decode_settings(&wds_reply(vec![field(0x24, value)])).is_err());
    }
    let mut value = domains(&[b"pcscf.example.org"]);
    value[0] = 2;
    assert!(decode_settings(&wds_reply(vec![field(0x24, value)])).is_err());
    let mut value = domains(&[b"pcscf.example.org"]);
    value.push(0);
    assert!(decode_settings(&wds_reply(vec![field(0x24, value)])).is_err());
    let mut addresses = vec![(MAX_LIST_ITEMS + 1) as u8];
    for i in 0..=MAX_LIST_ITEMS { addresses.extend_from_slice(&[i as u8, 2, 0, 192]); }
    assert!(decode_settings(&wds_reply(vec![field(0x23, addresses)])).is_err());
    let names = (0..=MAX_LIST_ITEMS).map(|i| format!("pcscf{i}.example.org")).collect::<Vec<_>>();
    let names = names.iter().map(|s| s.as_bytes()).collect::<Vec<_>>();
    assert!(decode_settings(&wds_reply(vec![field(0x24, domains(&names))])).is_err());
    let fields = (32..32 + MAX_TLVS as u8).map(|kind| field(kind, vec![])).collect();
    assert!(decode_settings(&wds_reply(fields)).is_err());
    assert!(decode_inbound(&vec![0; MAX_FRAME_BYTES + 1]).is_err());
}

#[test]
fn retained_codec_rejects_duplicate_tlvs_missing_result_and_inconsistent_status() {
    for duplicate in [success(), field(0x23, vec![0]), field(0x24, vec![0]), field(0xe0, vec![])] {
        assert!(decode_settings(&wds_reply(vec![duplicate.clone(), duplicate])).is_err());
    }
    assert!(decode_settings(&response(WDS, 7, 0x1234, GET_CURRENT_SETTINGS, vec![])).is_err());
    for value in [vec![], vec![0; 3], vec![0; 5], vec![0, 0, 94, 0], vec![1, 0, 0, 0], vec![2, 0, 94, 0]] {
        assert!(decode_settings(&response(WDS, 7, 0x1234, GET_CURRENT_SETTINGS, vec![field(2, value)])).is_err());
    }
}

#[test]
fn retained_codec_preserves_explicit_unsupported_and_protocol_failure() {
    for code in [25u16, 31, 71, 94, 7, 14] {
        let [low, high] = code.to_le_bytes();
        let error = decode_settings(&response(WDS, 7, 0x1234, GET_CURRENT_SETTINGS,
            vec![field(2, vec![1, 0, low, high])])).unwrap_err();
        if matches!(code, 25 | 71 | 94) {
            assert_eq!(error, NativeError::Unsupported("native_wds_pcscf_unsupported"));
        } else {
            assert_eq!(error, NativeError::ProtocolRejected(code));
        }
    }
}

#[test]
fn retained_codec_rejects_truncation_length_smuggling_and_wrong_flags() {
    let valid = wds_reply(vec![field(0x24, domains(&[b"pcscf.example.org"]))]);
    for length in 0..valid.len() {
        assert!(decode_inbound(&valid[..length]).is_err());
    }
    for (offset, value) in [(0, 2), (1, 0), (2, 1), (3, 0), (11, 0), (12, 1), (14, 3)] {
        let mut bad = valid.clone();
        bad[offset] = value;
        assert!(decode_inbound(&bad).is_err());
    }
    for flag in [0, 1, 3, 5, 6, 0xff] {
        let mut bad = valid.clone();
        bad[6] = flag;
        assert!(decode_inbound(&bad).is_err());
    }
    let mut extra = valid;
    extra.push(0);
    assert!(decode_inbound(&extra).is_err());
}

#[cfg(unix)]
mod stream {
    use super::*;
    use std::time::Duration;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixStream, time::Instant};

    fn open_reply() -> Vec<u8> {
        response(0, 0, 1, PROXY_OPEN, vec![success()])
    }

    fn assert_open(message: &QmiMessage) {
        assert_eq!((message.service, message.client_id, message.transaction_id, message.message_id), (0, 0, 1, PROXY_OPEN));
        assert_eq!(message.tlvs.len(), 1);
        assert_eq!(message.tlvs[0].tlv_type, 1);
        assert_eq!(message.tlvs[0].value, b"/dev/fixture");
    }

    fn assert_query(message: &QmiMessage) {
        assert_eq!((message.service, message.client_id, message.transaction_id, message.message_id), (WDS, 7, 0x1234, GET_CURRENT_SETTINGS));
        assert_eq!(message.tlvs.len(), 1);
        assert_eq!(message.tlvs[0].tlv_type, 0x10);
        assert_eq!(message.tlvs[0].value, [0, 0x18, 0, 0]);
    }

    async fn received(proxy: &mut UnixStream) -> QmiMessage {
        decode_qmi_frame(&read_frame(proxy).await.unwrap()).unwrap()
    }

    async fn assert_no_more_requests(proxy: &mut UnixStream) {
        let mut unexpected = Vec::new();
        let result = proxy.read_to_end(&mut unexpected).await;
        assert!(unexpected.is_empty(), "must not allocate/release/rebind/activate during cleanup");
        // Closing with an unread oversized/unrelated reply may reset the
        // local socket rather than produce EOF. Neither permits another command.
        assert!(result.is_ok() || matches!(result, Err(ref error)
            if error.kind() == std::io::ErrorKind::ConnectionReset));
    }

    /// Drives the production exchange, not a stand-in decoder or NativeIo mock.
    /// EOF proves dropping the socket emits no CTL release or other cleanup.
    async fn scripted(open_frames: Vec<Vec<u8>>, query_frames: Vec<Vec<u8>>) -> Result<RetainedWdsSettings, NativeError> {
        let (client, mut proxy) = UnixStream::pair().unwrap();
        let peer = async move {
            assert_open(&received(&mut proxy).await);
            for frame in open_frames { proxy.write_all(&frame).await.unwrap(); }
            assert_query(&received(&mut proxy).await);
            for frame in query_frames {
                if proxy.write_all(&frame).await.is_err() { break; }
            }
            let _ = proxy.shutdown().await;
            assert_no_more_requests(&mut proxy).await;
        };
        let (result, ()) = tokio::join!(
            exchange(client, "/dev/fixture", 7, 0x1234, Instant::now() + Duration::from_secs(2)), peer,
        );
        result
    }

    #[tokio::test]
    async fn retained_stream_uses_only_logical_open_and_original_cid_read() {
        let result = scripted(vec![open_reply()], vec![wds_reply(vec![
            field(0x23, vec![1, 7, 2, 0, 192]),
            field(0x24, domains(&[b"pcscf.example.org"])),
        ])]).await.unwrap();
        assert_eq!(result.addresses, vec!["192.0.2.7".parse::<IpAddr>().unwrap()]);
        assert_eq!(result.domains, vec!["pcscf.example.org"]);
    }

    #[tokio::test]
    async fn retained_stream_correlates_qmux_scope_flags_transaction_and_message() {
        let mut unrelated = vec![
            response(2, 7, 0x1234, GET_CURRENT_SETTINGS, vec![success()]),
            response(WDS, 8, 0x1234, GET_CURRENT_SETTINGS, vec![success()]),
            response(WDS, 7, 0x1235, GET_CURRENT_SETTINGS, vec![success()]),
            response(WDS, 7, 0x1234, GET_CURRENT_SETTINGS + 1, vec![success()]),
        ];
        let mut indication = wds_reply(vec![field(0x24, domains(&[b"wrong.example.org"]))]);
        indication[6] = 4;
        unrelated.push(indication);
        for frame in &unrelated {
            assert!(scripted(vec![open_reply()], vec![frame.clone()]).await.is_err());
        }
        unrelated.push(wds_reply(vec![field(0x24, domains(&[b"right.example.org"]))]));
        assert_eq!(scripted(vec![open_reply()], unrelated).await.unwrap().domains, vec!["right.example.org"]);
        let wrong_open = response(0, 0, 2, PROXY_OPEN, vec![success()]);
        assert!(scripted(vec![wrong_open, open_reply()], vec![wds_reply(vec![])]).await.is_ok());
    }

    #[tokio::test]
    async fn retained_stream_rejects_malformed_duplicate_truncated_and_oversize_replies() {
        let valid = wds_reply(vec![field(0x24, domains(&[b"pcscf.example.org"]))]);
        let mut wrong_flags = valid.clone();
        wrong_flags[6] = 0;
        let mut wrong_direction = valid.clone();
        wrong_direction[3] = 0;
        for frame in [
            wds_reply(vec![success()]),
            wds_reply(vec![field(0x24, vec![0]), field(0x24, vec![0])]),
            wds_reply(vec![field(0x24, vec![1, 255, 255])]),
            valid[..valid.len() - 1].to_vec(),
            vec![1, 0xff, 0x7f],
            wrong_flags, wrong_direction,
        ] {
            assert!(scripted(vec![open_reply()], vec![frame]).await.is_err());
        }
    }

    #[tokio::test]
    async fn retained_stream_reports_unsupported_without_allocating_a_fallback_client() {
        let frame = response(WDS, 7, 0x1234, GET_CURRENT_SETTINGS, vec![field(2, vec![1, 0, 94, 0])]);
        assert_eq!(scripted(vec![open_reply()], vec![frame]).await.unwrap_err(),
            NativeError::Unsupported("native_wds_pcscf_unsupported"));
    }

    #[tokio::test]
    async fn retained_stream_requires_open_ack_before_sending_wds_and_never_releases() {
        for frame in [
            response(0, 0, 1, PROXY_OPEN, vec![field(2, vec![1, 0, 7, 0])]),
            response(0, 1, 1, PROXY_OPEN, vec![success()]),
            response(0, 0, 2, PROXY_OPEN, vec![success()]),
            response(0, 0, 1, PROXY_OPEN + 1, vec![success()]),
            response(0, 0, 1, PROXY_OPEN, vec![success(), success()]),
        ] {
            let (client, mut proxy) = UnixStream::pair().unwrap();
            let peer = async move {
                assert_open(&received(&mut proxy).await);
                proxy.write_all(&frame).await.unwrap();
                proxy.shutdown().await.unwrap();
                let mut unexpected = Vec::new();
                proxy.read_to_end(&mut unexpected).await.unwrap();
                assert!(unexpected.is_empty());
            };
            let (result, ()) = tokio::join!(
                exchange(client, "/dev/fixture", 7, 0x1234, Instant::now() + Duration::from_secs(2)), peer,
            );
            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn retained_stream_bounds_total_messages_across_open_and_query() {
        let wrong_open = response(0, 0, 2, PROXY_OPEN, vec![success()]);
        let mut before = vec![wrong_open; 16];
        before.push(open_reply());
        let wrong_query = response(WDS, 8, 0x1234, GET_CURRENT_SETTINGS, vec![success()]);
        let mut after = vec![wrong_query; 16];
        after.push(wds_reply(vec![]));
        assert_eq!(scripted(before, after).await.unwrap_err(),
            NativeError::CommandFailed("native_wds_pcscf_message_limit"));
    }

    #[tokio::test]
    async fn retained_stream_deadline_expires_on_silent_open_or_silent_wds() {
        for acknowledge_open in [false, true] {
            let (client, mut proxy) = UnixStream::pair().unwrap();
            let peer = async move {
                assert_open(&received(&mut proxy).await);
                if acknowledge_open {
                    proxy.write_all(&open_reply()).await.unwrap();
                    assert_query(&received(&mut proxy).await);
                }
                let mut unexpected = Vec::new();
                proxy.read_to_end(&mut unexpected).await.unwrap();
                assert!(unexpected.is_empty());
            };
            let (result, ()) = tokio::join!(
                exchange(client, "/dev/fixture", 7, 0x1234, Instant::now() + Duration::from_millis(100)), peer,
            );
            assert_eq!(result.unwrap_err(), timeout_error());
        }
    }

    #[tokio::test]
    async fn retained_stream_absolute_deadline_is_not_reset_by_partial_reads() {
        for slow_open in [true, false] {
            let (client, mut proxy) = UnixStream::pair().unwrap();
            let peer = async move {
                assert_open(&received(&mut proxy).await);
                if !slow_open {
                    proxy.write_all(&open_reply()).await.unwrap();
                    assert_query(&received(&mut proxy).await);
                }
                // Every byte makes progress, but completing either reply
                // would require far longer than the whole deadline.
                let frame = if slow_open { open_reply() } else { wds_reply(vec![]) };
                for byte in frame {
                    if proxy.write_all(&[byte]).await.is_err() { break; }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                assert_no_more_requests(&mut proxy).await;
            };
            let (result, ()) = tokio::join!(
                exchange(client, "/dev/fixture", 7, 0x1234, Instant::now() + Duration::from_millis(100)), peer,
            );
            assert_eq!(result.unwrap_err(), timeout_error());
        }
    }

    #[tokio::test]
    async fn retained_stream_open_and_wds_share_one_absolute_deadline() {
        let (client, mut proxy) = UnixStream::pair().unwrap();
        let deadline = Instant::now() + Duration::from_millis(200);
        let peer = async move {
            assert_open(&received(&mut proxy).await);
            tokio::time::sleep_until(deadline - Duration::from_millis(75)).await;
            if proxy.write_all(&open_reply()).await.is_ok() {
                if let Ok(frame) = read_frame(&mut proxy).await {
                    assert_query(&decode_qmi_frame(&frame).unwrap());
                    tokio::time::sleep_until(deadline + Duration::from_millis(25)).await;
                    let _ = proxy.write_all(&wds_reply(vec![])).await;
                }
            }
            assert_no_more_requests(&mut proxy).await;
        };
        let (result, ()) = tokio::join!(exchange(client, "/dev/fixture", 7, 0x1234, deadline), peer);
        assert_eq!(result.unwrap_err(), timeout_error());
    }

    #[tokio::test]
    async fn retained_stream_cancellation_drops_only_its_local_socket() {
        let (client, mut proxy) = UnixStream::pair().unwrap();
        let task = tokio::spawn(exchange(client, "/dev/fixture", 7, 0x1234,
            Instant::now() + Duration::from_secs(2)));
        assert_open(&received(&mut proxy).await);
        proxy.write_all(&open_reply()).await.unwrap();
        assert_query(&received(&mut proxy).await);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_no_more_requests(&mut proxy).await;
    }

    #[tokio::test]
    async fn retained_stream_invalid_cid_device_or_expired_budget_emits_no_request() {
        for (device, cid, expired) in [("/dev/fixture", 0, false), ("/dev/fixture", 255, false),
            ("/tmp/not-device", 7, false), ("/dev/fixture", 7, true)] {
            let (client, mut proxy) = UnixStream::pair().unwrap();
            let deadline = if expired { Instant::now() - Duration::from_secs(1) }
                else { Instant::now() + Duration::from_secs(1) };
            assert!(exchange(client, device, cid, 0x1234, deadline).await.is_err());
            let mut unexpected = Vec::new();
            proxy.read_to_end(&mut unexpected).await.unwrap();
            assert!(unexpected.is_empty());
        }
    }
}
