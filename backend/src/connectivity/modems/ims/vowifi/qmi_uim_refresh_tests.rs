//! Pure framing/policy tests; no socket or modem is used. Actions-only.
use super::*;

#[test]
fn register_and_complete_tlvs_match_the_published_schema() {
    assert_eq!(session_tlv(0, &[0xa0, 0x00]), vec![0, 2, 0xa0, 0x00]);
    assert_eq!(register_info_tlv(), vec![1, 1, 0, 0]);
    assert_eq!(complete_info_tlv(true), vec![1]);
    assert_eq!(complete_info_tlv(false), vec![0]);
}

#[test]
fn refresh_request_round_trips_through_the_qmux_framing() {
    let request = QmiMessage {
        service: QMUX_UIM_SERVICE,
        client_id: 7,
        transaction_id: 3,
        message_id: QMI_UIM_REFRESH_REGISTER,
        tlvs: vec![
            QmiTlv { tlv_type: SESSION_TLV, value: session_tlv(SESSION_PRIMARY_GW, &[0xa0, 0x00, 0x00]) },
            QmiTlv { tlv_type: INFO_TLV, value: register_info_tlv() },
        ],
    };
    let frame = encode_qmi_message(&request).unwrap();
    let decoded = decode_qmi_frame(&frame).unwrap();
    assert_eq!(decoded.message_id, QMI_UIM_REFRESH_REGISTER);
    assert_eq!(decoded.client_id, 7);
    assert_eq!(decoded.service, QMUX_UIM_SERVICE);
    assert_eq!(find_tlv(&decoded, INFO_TLV), Some(&[1u8, 1, 0, 0][..]));
    assert_eq!(find_tlv(&decoded, SESSION_TLV), Some(&[0u8, 3, 0xa0, 0x00, 0x00][..]));
}

#[test]
fn event_parsing_reads_stage_session_and_aid() {
    let value = [STAGE_END_WITH_SUCCESS, 3, SESSION_PRIMARY_GW, 2, 0xa0, 0x00];
    assert_eq!(parse_event(&value), Some((STAGE_END_WITH_SUCCESS, SESSION_PRIMARY_GW, vec![0xa0, 0x00])));
    assert_eq!(parse_event(&[]), None);
    assert_eq!(parse_event(&[STAGE_WAIT_FOR_OK]), None);
    // Truncated AID must be rejected, not padded.
    assert_eq!(parse_event(&[STAGE_WAIT_FOR_OK, 3, SESSION_PRIMARY_GW, 4, 0xa0]), None);
}

#[test]
fn only_matching_sessions_and_aids_are_acted_on() {
    let ours = [0xa0u8, 0x00, 0x00, 0x00, 0x87, 0x10, 0x04];
    let event = (STAGE_WAIT_FOR_OK, SESSION_PRIMARY_GW, ours.to_vec());
    assert!(event_matches(SESSION_PRIMARY_GW, &ours, &event));
    // Modem may omit the AID; the session still identifies the card.
    assert!(event_matches(SESSION_PRIMARY_GW, &ours, &(STAGE_WAIT_FOR_OK, SESSION_PRIMARY_GW, vec![])));
    // Another slot/session must never receive our vote.
    assert!(!event_matches(5, &ours, &event));
    // Another application on the same slot is not our provisioning change.
    assert!(!event_matches(SESSION_PRIMARY_GW, &ours, &(STAGE_WAIT_FOR_OK, SESSION_PRIMARY_GW, vec![0xa0, 0x00, 0x00, 0x00, 0x87, 0x10, 0x03])));
}

#[test]
fn vote_denies_only_while_busy_and_never_beyond_the_bound() {
    let busy = Arc::new(AtomicBool::new(true));
    let mut voter = Voter::new(busy.clone());
    assert!(!voter.allow(), "an in-flight REGISTER/AKA must defer provisioning");
    busy.store(false, Ordering::SeqCst);
    assert!(voter.allow(), "idle line accepts the refresh immediately");
    // Bounded deny: a stuck exchange cannot block the card forever.
    let busy = Arc::new(AtomicBool::new(true));
    let mut voter = Voter::new(busy);
    voter.force_denied_since(std::time::Instant::now() - MAX_DENY - Duration::from_millis(1));
    assert!(voter.allow());
}

#[test]
fn socket_timeouts_are_not_treated_as_fatal() {
    assert!(is_timeout(&io::Error::new(io::ErrorKind::WouldBlock, "x")));
    assert!(is_timeout(&io::Error::new(io::ErrorKind::TimedOut, "x")));
    assert!(!is_timeout(&io::Error::new(io::ErrorKind::ConnectionReset, "x")));
}

#[test]
fn subscription_start_rejects_invalid_identity_inputs() {
    assert!(start("@qmi-proxy", "", 1, vec![]).is_none());
    assert!(start("@qmi-proxy", "/dev/cdc-wdm0", 0, vec![]).is_none());
}

/// End-to-end over a fake QMI proxy (abstract Unix socket). This exercises the
/// real listener loop: proxy open -> allocate CID -> Refresh Register ->
/// WAIT_FOR_OK vote -> END_WITH_SUCCESS event -> Release CID.
#[cfg(unix)]
mod end_to_end {
    use super::super::super::{decode_qmi_frame, encode_qmi_message, QmiMessage, QmiTlv};
    use super::*;
    use std::io::{Read, Write};
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
    use std::sync::atomic::AtomicUsize;

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn unique_name() -> String {
        format!("simadmin-refresh-e2e-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst))
    }

    fn bind(name: &str) -> UnixListener {
        let address = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        UnixListener::bind_addr(&address).unwrap()
    }

    fn read_frame(stream: &mut UnixStream) -> QmiMessage {
        let mut header = [0u8; 3];
        stream.read_exact(&mut header).unwrap();
        // The QMUX length counts everything after the 3-byte prefix, so the
        // remaining bytes are length-2 (same arithmetic as read_qmi_message).
        let qmux_len = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut rest = vec![0u8; qmux_len.saturating_sub(2)];
        stream.read_exact(&mut rest).unwrap();
        let mut frame = header.to_vec();
        frame.extend_from_slice(&rest);
        decode_qmi_frame(&frame).unwrap()
    }

    fn send(stream: &mut UnixStream, message: QmiMessage) {
        stream.write_all(&encode_qmi_message(&message).unwrap()).unwrap();
        stream.flush().unwrap();
    }

    fn ok() -> QmiTlv {
        QmiTlv { tlv_type: 2, value: vec![0, 0, 0, 0] }
    }

    fn indication(stage: u8, client: u8) -> QmiMessage {
        QmiMessage {
            service: QMUX_UIM_SERVICE,
            client_id: client,
            transaction_id: 0,
            message_id: QMI_UIM_REFRESH_INDICATION,
            tlvs: vec![QmiTlv {
                tlv_type: EVENT_TLV,
                value: vec![stage, 0, SESSION_PRIMARY_GW, 2, 0xa0, 0x00],
            }],
        }
    }

    /// Returns the vote bytes we received for WAIT_FOR_OK.
    fn fake_modem(listener: UnixListener, delay: Duration) -> Vec<u8> {
        let (mut stream, _) = listener.accept().unwrap();
        let open = read_frame(&mut stream);
        assert_eq!(open.message_id, 0xff00, "first request must be proxy open");
        send(&mut stream, QmiMessage { service: 0, client_id: 0, transaction_id: open.transaction_id,
            message_id: open.message_id, tlvs: vec![ok()] });

        let allocate = read_frame(&mut stream);
        assert_eq!(allocate.message_id, 0x0022, "second request must allocate a UIM CID");
        send(&mut stream, QmiMessage { service: 0, client_id: 0, transaction_id: allocate.transaction_id,
            message_id: allocate.message_id,
            tlvs: vec![QmiTlv { tlv_type: 1, value: vec![QMUX_UIM_SERVICE, 7] }, ok()] });

        let register = read_frame(&mut stream);
        assert_eq!(register.message_id, QMI_UIM_REFRESH_REGISTER);
        assert_eq!(find_tlv(&register, SESSION_TLV), Some(&[SESSION_PRIMARY_GW, 2, 0xa0, 0x00][..]));
        assert_eq!(find_tlv(&register, INFO_TLV), Some(&[1u8, 1, 0, 0][..]));
        send(&mut stream, QmiMessage { service: QMUX_UIM_SERVICE, client_id: 7,
            transaction_id: register.transaction_id, message_id: register.message_id, tlvs: vec![ok()] });

        std::thread::sleep(delay);
        send(&mut stream, indication(STAGE_WAIT_FOR_OK, 7));
        let vote = read_frame(&mut stream);
        assert_eq!(vote.message_id, QMI_UIM_REFRESH_COMPLETE);
        let votes = find_tlv(&vote, INFO_TLV).expect("vote TLV").to_vec();
        assert_eq!(find_tlv(&vote, SESSION_TLV), Some(&[SESSION_PRIMARY_GW, 2, 0xa0, 0x00][..]));

        send(&mut stream, indication(STAGE_END_WITH_SUCCESS, 7));
        let release = read_frame(&mut stream);
        assert_eq!(release.message_id, 0x0023, "client must release its CID");
        send(&mut stream, QmiMessage { service: 0, client_id: 0, transaction_id: release.transaction_id,
            message_id: release.message_id, tlvs: vec![ok()] });
        votes
    }

    fn wait_event(subscription: &mut RefreshSubscription, timeout: Duration) -> Option<RefreshEvent> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Some(event) = subscription.poll_event() { return Some(event); }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    #[test]
    fn idle_line_votes_ok_and_reports_completion_then_releases_the_client() {
        let name = unique_name();
        let listener = bind(&name);
        let server = std::thread::spawn(move || fake_modem(listener, Duration::from_millis(50)));
        let mut subscription = start(&format!("@{name}"), "/dev/fake-cdc-wdm0", 1, vec![0xa0, 0x00])
            .expect("subscription starts");
        assert_eq!(wait_event(&mut subscription, Duration::from_secs(5)), Some(RefreshEvent::Completed),
            "listener exit: {:?}", super::last_exit());
        subscription.stop();
        assert_eq!(server.join().unwrap(), vec![1u8], "idle line allows the refresh");
    }

    #[test]
    fn in_flight_exchange_defers_the_refresh_vote() {
        let name = unique_name();
        let listener = bind(&name);
        let server = std::thread::spawn(move || fake_modem(listener, Duration::from_millis(400)));
        let mut subscription = start(&format!("@{name}"), "/dev/fake-cdc-wdm0", 1, vec![0xa0, 0x00])
            .expect("subscription starts");
        subscription.set_busy(true);
        assert_eq!(wait_event(&mut subscription, Duration::from_secs(5)), Some(RefreshEvent::Completed),
            "listener exit: {:?}", super::last_exit());
        subscription.set_busy(false);
        subscription.stop();
        assert_eq!(server.join().unwrap(), vec![0u8], "an in-flight REGISTER/AKA must defer");
    }
}
