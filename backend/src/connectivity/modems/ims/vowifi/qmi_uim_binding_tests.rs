use super::*;
fn card(aids: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![1, 0, 0, 0, 0, aids.len() as u8];
    for aid in aids {
        out.extend([2, 7, 2, 11, 0, 0, aid.len() as u8]);
        out.extend_from_slice(aid);
        out.extend([0, 3, 3, 10, 1, 3, 10]);
    }
    out
}
fn message(data: Vec<u8>) -> QmiMessage {
    QmiMessage { service: QMUX_UIM_SERVICE, client_id: 2, transaction_id: 1,
        message_id: QMI_UIM_GET_CARD_STATUS,
        tlvs: vec![tlv(TLV_RESULT, vec![0, 0, 0, 0]), tlv(TLV_UIM_CARD_STATUS, data)] }
}
#[test]
fn isim_application_discovery_is_slot_scoped_and_length_checked() {
    let mut status = vec![0; 8]; status.push(2);
    let mut first = USIM_AID_PREFIX.to_vec(); first.push(1);
    let mut second = isim::ISIM_AID_PREFIX.to_vec(); second.push(2);
    status.extend(card(&[&first])); status.extend(card(&[&second]));
    let reply = message(status.clone());
    assert_eq!(parse_application_ids_for_slot(&reply, 1).unwrap(), vec![first]);
    assert_eq!(parse_application_ids_for_slot(&reply, 2).unwrap(), vec![second]);
    assert!(parse_application_ids_for_slot(&reply, 0).is_err());
    assert!(parse_application_ids_for_slot(&reply, 3).is_err());
    status.pop();
    assert!(parse_application_ids_for_slot(&message(status), 1).is_err());
}
#[test]
fn isim_application_discovery_rejects_absent_card_and_duplicate_aid() {
    let mut absent = vec![0; 8]; absent.push(1); absent.extend([0, 0, 0, 0, 0, 0]);
    assert!(parse_application_ids_for_slot(&message(absent), 1).is_err());
    let mut duplicate = vec![0; 8]; duplicate.push(1);
    duplicate.extend(card(&[isim::ISIM_AID_PREFIX, isim::ISIM_AID_PREFIX]));
    assert!(parse_application_ids_for_slot(&message(duplicate), 1).is_err());
    use crate::connectivity::modems::ims::cellular_ims::identity::UiccApplications;
    let mut second = isim::ISIM_AID_PREFIX.to_vec(); second.push(2);
    assert_eq!(UiccApplications::from_aids(vec![isim::ISIM_AID_PREFIX.to_vec(), second]), Err("isim_application_ambiguous"));
}
#[test]
fn isim_cleanup_closes_then_releases_exactly_once_even_on_failure() {
    for read_failed in [false, true] {
        for close_failed in [false, true] {
            let mut calls = Vec::new();
            let result = finish_isim_channel_read(
                if read_failed { Err("mock_read_error") } else { Ok(42) },
                |close| { calls.push(close); if close && close_failed { Err("mock_close_error") } else { Ok(()) } },
            );
            assert_eq!(calls, [true, false]);
            assert_eq!(result, if read_failed { Err("mock_read_error") } else if close_failed { Err("mock_close_error") } else { Ok(42) });
        }
    }
    assert_eq!(finish_isim_channel_read(Ok(42), |close| if close { Ok(()) } else { Err("mock_release_error") }), Err("mock_release_error"));
}
#[test]
fn isim_apdu_channel_is_kept_on_every_file_operation() {
    for apdu in [&[0, 0xa4, 0, 4, 2, 0x6f, 2, 0][..], &[0, 0xb0, 0, 0, 20][..], &[0, 0xb2, 1, 4, 20][..]] {
        let encoded = build_send_apdu_frame(3, 1, 2, 7, apdu).unwrap();
        let frame = decode_qmi_frame(&encoded).unwrap();
        assert_eq!(find_tlv(&frame, TLV_UIM_APDU_CHANNEL_ID), Some(&[7][..]));
        assert_eq!(find_tlv(&frame, TLV_UIM_SLOT), Some(&[2][..]));
    }
}
