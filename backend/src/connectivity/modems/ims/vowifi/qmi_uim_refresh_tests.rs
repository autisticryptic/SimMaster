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
