use super::*;

fn report(status: u8) -> Vec<u8> {
    let mut pdu = vec![0, 2, 7, 4, 0x81, 0x21, 0x43];
    pdu.extend_from_slice(&[0x62, 0x10, 0x10, 0, 0, 0, 0]);
    pdu.extend_from_slice(&[0x62, 0x10, 0x10, 0, 1, 0, 0]);
    pdu.push(status);
    pdu
}

#[test]
fn status_report_decodes_reference_recipient_and_both_timestamps() {
    let parsed = parse_modem_status_report(&report(0)).unwrap();
    assert_eq!(parsed.reference, 7);
    assert_eq!(parsed.recipient, "1234");
    assert_eq!(parsed.service_center_timestamp, "2026-01-01T00:00:00Z");
    assert_eq!(parsed.discharge_timestamp, "2026-01-01T00:10:00Z");
    assert_eq!(parsed.status, 0);
    assert_eq!(parse_modem_status_report(&report(1)).unwrap().status, 1);
    assert_eq!(parse_modem_status_report(&report(64)).unwrap().status, 64);
}

#[test]
fn truncated_or_command_reports_and_bad_numeric_addresses_are_rejected() {
    let pdu = report(0);
    for length in 0..pdu.len() {
        assert!(parse_modem_status_report(&pdu[..length]).is_err());
    }
    let mut command = pdu.clone();
    command[1] |= 0x20;
    assert!(parse_modem_status_report(&command).is_err());
    let mut bad = pdu.clone();
    bad[5] = 0xff;
    assert!(parse_modem_status_report(&bad).is_err());
    let mut alpha = pdu.clone();
    alpha[4] = 0xd0;
    assert!(parse_modem_status_report(&alpha).is_err());
    let mut incomplete_pi = pdu.clone();
    incomplete_pi.push(7);
    assert!(parse_modem_status_report(&incomplete_pi).is_err());
    let mut empty_pi = pdu;
    empty_pi.push(0);
    assert!(parse_modem_status_report(&empty_pi).is_ok());
}

#[test]
fn only_native_wrapper_requests_reports_for_single_and_multipart_sms() {
    assert_eq!(
        build_sms_submit_tpdu("+1234", "fixture", 7).unwrap()[0] & 0x20,
        0
    );
    for text in ["fixture".to_string(), "测试".repeat(100)] {
        for pdu in build_modem_submit_pdus("+1234", &text, "+12345").unwrap() {
            let first = pdu[1 + usize::from(pdu[0])];
            assert_eq!(first & 3, 1);
            assert_ne!(first & 0x20, 0);
        }
    }
}

#[test]
fn strict_modem_deliver_does_not_turn_truncation_into_shorter_success() {
    let mut pdu = vec![0, 0, 4, 0x81, 0x21, 0x43, 0, 8];
    pdu.extend_from_slice(&[0x62, 0x10, 0x10, 0, 0, 0, 0]);
    pdu.extend_from_slice(&[2, 0, b'A']);
    assert_eq!(parse_modem_deliver_pdu(&pdu).unwrap().text, "A");
    for length in 0..pdu.len() {
        assert!(parse_modem_deliver_pdu(&pdu[..length]).is_err());
    }
    pdu.push(0);
    assert!(parse_modem_deliver_pdu(&pdu).is_err());
}
