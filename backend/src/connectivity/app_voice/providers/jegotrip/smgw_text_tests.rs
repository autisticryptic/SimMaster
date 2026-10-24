//! Actions-only strict Smgw text/data tests, not real SMS or carrier execution.
use super::*;

fn message(body: &[u8], sender: Option<&[u8]>, time: Option<&[u8]>) -> IncomingMessage {
    IncomingMessage { body: body.to_vec(), from: sender.map(<[u8]>::to_vec), time: time.map(<[u8]>::to_vec) }
}

#[test]
fn only_exact_recovered_receiver_kind_may_classify_text() {
    let input=message(b"fixture text",Some(b"fixture-sender"),None);
    for wrong in [b"".as_slice(),b"Sip.request",b"Sip.Request*",b"Sip.Request\0",b"fixture.kind",b"MessageReceiver"] {
        assert!(classify(wrong,&input).is_none());
    }
    assert!(classify(SMGW_RECEIVER,&input).is_some());
}
#[test]
fn unicode_lines_and_literal_encoded_looking_content_remain_exact_text() {
    for text in ["你好 — fixture 🥘", "a\r\nb\nc\td", "00FF030102", "ZmFrZS1wYXlsb2Fk", "{\"message\":\"fixture\"}", "\\u4f60", ""] {
        let input=message(text.as_bytes(),Some(b"fixture-sender"),None);
        let result=classify(SMGW_RECEIVER,&input).unwrap();assert_eq!(result.text,text);
        assert_eq!(result.sender,"fixture-sender");assert_eq!(result.sent_at_millis,None);
    }
}
#[test]
fn malformed_utf8_or_unsupported_controls_are_not_lossily_promoted() {
    for bytes in [vec![0xff],vec![0xc0,0x80],vec![0xed,0xa0,0x80],vec![0xf4,0x90,0x80,0x80],b"a\0b".to_vec(),vec![0x1b],vec![0x7f],"a\u{0085}b".as_bytes().to_vec()] {
        assert!(classify(SMGW_RECEIVER,&message(&bytes,Some(b"fixture"),None)).is_none());
    }
}
#[test]
fn sender_must_be_explicit_and_valid_without_inventing_number_or_ims_uri() {
    for sender in [None,Some(b"".as_slice()),Some(b" \t"),Some(&[0xff]),Some(b"a\nb")] {
        assert!(classify(SMGW_RECEIVER,&message(b"fixture",sender,None)).is_none());
    }
    for sender in ["+123456", "ServiceName", "  preserved sender  ", "用户"] {
        let input=message(b"fixture",Some(sender.as_bytes()),None);
        assert_eq!(classify(SMGW_RECEIVER,&input).unwrap().sender,sender);
    }
}
#[test]
fn optional_decimal_milliseconds_are_preserved_and_unknown_uses_explicit_receive_source() {
    let input=message(b"fixture",Some(b"sender"),Some(b"1700000000123"));let result=classify(SMGW_RECEIVER,&input).unwrap();
    assert_eq!(result.sent_at_millis,Some(1_700_000_000_123));assert_eq!(result.timestamp_source(),"sender");
    assert_eq!(result.timestamp("2020-01-01T00:00:00Z").as_deref(),Some("2023-11-14T22:13:20.123Z"));
    let absent=message(b"fixture",Some(b"sender"),None);let result=classify(SMGW_RECEIVER,&absent).unwrap();
    assert_eq!(result.sent_at_millis,None);assert_eq!(result.timestamp_source(),"received");
    assert_eq!(result.timestamp("2020-01-02T08:00:00+08:00").as_deref(),Some("2020-01-02T00:00:00.000Z"));
    assert!(result.timestamp("fixture-unknown").is_none());
}
#[test]
fn invalid_or_ambiguous_time_does_not_turn_into_current_send_time() {
    for time in [b"".as_slice(),b"-1",b"+1",b"1.0",b"1e3",b" 123",b"123 ",b"1700000000s",b"2023-11-14",b"9999999999999999999",b"9223372036854775807",b"253402300800000",b"12345678901234567890"] {
        assert!(classify(SMGW_RECEIVER,&message(b"fixture",Some(b"sender"),Some(time))).is_none());
    }
    // Do not copy Android's historical cutoff/currentTimeMillis replacement.
    let input=message(b"fixture",Some(b"sender"),Some(b"0"));let result=classify(SMGW_RECEIVER,&input).unwrap();
    assert_eq!(result.sent_at_millis,Some(0));assert_eq!(result.timestamp("ignored").as_deref(),Some("1970-01-01T00:00:00.000Z"));
    let input=message(b"fixture",Some(b"sender"),Some(b"253402300799999"));
    assert_eq!(classify(SMGW_RECEIVER,&input).unwrap().timestamp("ignored").as_deref(),Some("9999-12-31T23:59:59.999Z"));
}
#[test]
fn text_and_sender_caps_fail_at_the_boundary_without_truncation() {
    let max=vec![b'x';MAX_TEXT_BYTES];assert!(classify(SMGW_RECEIVER,&message(&max,Some(b"sender"),None)).is_some());
    assert!(classify(SMGW_RECEIVER,&message(&vec![b'x';MAX_TEXT_BYTES+1],Some(b"sender"),None)).is_none());
    let max=vec![b'x';MAX_SENDER_BYTES];assert!(classify(SMGW_RECEIVER,&message(b"text",Some(&max),None)).is_some());
    assert!(classify(SMGW_RECEIVER,&message(b"text",Some(&vec![b'x';MAX_SENDER_BYTES+1]),None)).is_none());
}
#[test]
fn debug_output_does_not_expose_sms_body_sender_or_time() {
    let input=message(b"fixture-private-body",Some(b"fixture-private-sender"),Some(b"1700000000123"));
    assert_eq!(format!("{:?}",classify(SMGW_RECEIVER,&input).unwrap()),"SmgwText(<redacted>)");
}
