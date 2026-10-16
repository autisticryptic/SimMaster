use super::*;

fn row(fields: &str) -> String {
    format!("+QIMSCFG:\"ims_status\",{fields}\r\nOK\r\n")
}

#[test]
fn native_ims_all_enums_are_explicit_and_zero_is_not_negative() {
    for value in 0u8..=255 {
        let sample = parse_status(&row(&format!("0,1,{value},{value},{value},{value}"))).unwrap();
        assert_eq!(sample.registration, match value {
            0 => NativeImsRegistration::UnknownOrUnreported,
            1 => NativeImsRegistration::Registering,
            2 => NativeImsRegistration::Registered,
            3 => NativeImsRegistration::Limited,
            _ => NativeImsRegistration::Unknown,
        });
        let service = match value {
            0 => NativeImsService::UnknownOrUnreported,
            1 => NativeImsService::Limited,
            2 => NativeImsService::Available,
            _ => NativeImsService::Unknown,
        };
        assert_eq!((sample.sms, sample.voice, sample.ut), (service, service, service));
    }
}

#[test]
fn native_ims_flags_are_independent_and_last_field_is_ut() {
    let sample = parse_status(&row("0,0,2,1,2,0")).unwrap();
    assert!(!sample.registration_reporting && !sample.service_reporting);
    assert_eq!(sample.registration, NativeImsRegistration::Registered);
    assert_eq!(sample.sms, NativeImsService::Limited);
    assert_eq!(sample.voice, NativeImsService::Available);
    assert_eq!(sample.ut, NativeImsService::UnknownOrUnreported);
    let sample = parse_status(&row("1,1,0,0,0,2")).unwrap();
    assert!(sample.registration_reporting && sample.service_reporting);
    assert_eq!(sample.registration, NativeImsRegistration::UnknownOrUnreported);
    assert_eq!(sample.ut, NativeImsService::Available);
}

#[test]
fn native_ims_echo_whitespace_and_known_urcs_do_not_become_status() {
    let text = concat!(
        "\r\nAT+QIMSCFG=\"ims_status\"\r\n",
        "+QIMSREGSTAT: 2\r\n+QIMSSRVSTAT: 2,2,2\r\n",
        "+CMTI: \"SM\",1\r\nRING\r\nNO CARRIER\r\n+CEREG: 1\r\n",
        "  +QIMSCFG: \"ims_status\" , 1 , 0 , 2 , 0 , 1 , 2  \r\nOK\r\n"
    );
    assert_eq!(parse_status(text).unwrap(), parse_status(&row("1,0,2,0,1,2")).unwrap());
    assert!(parse_status("+QIMSREGSTAT: 2\r\n+QIMSSRVSTAT: 2,2,2\r\nOK").is_err());
}

#[test]
fn native_ims_rejects_wrong_field_count_flags_numeric_shape_and_overflow() {
    for fields in [
        "0,0,2,2,2", "0,0,2,2,2,2,2", "0,0,2,2,2,", "0,0,2,,2,2",
        "2,0,2,2,2,2", "0,255,2,2,2,2", "0,0,256,2,2,2",
        "0,0,2,99999999999999999999,2,2", "0,0,-1,2,2,2", "0,0,+2,2,2,2",
        "0,0,\"2\",2,2,2", "0,0,2.0,2,2,2", "0,0,0x2,2,2,2", "0,0,2,2,2,NaN",
    ] {
        assert_eq!(parse_status(&row(fields)), Err(NativeImsProbeError::MalformedResponse));
    }
}

#[test]
fn native_ims_rejects_duplicate_missing_malformed_or_unsuccessful_rows() {
    let valid = "+QIMSCFG:\"ims_status\",0,0,2,2,2,2";
    for text in [
        "OK".into(), valid.into(), format!("{valid}\r\n{valid}\r\nOK"),
        format!("{valid}\r\nERROR"), format!("{valid}\r\n+CME ERROR: 42"),
        format!("{valid}\r\n+CMS ERROR: 42\r\nOK"), format!("{valid}\r\nOK\r\nOK"),
        format!("OK\r\n{valid}"), format!("+QIMSCFG: broken\r\n{valid}\r\nOK"),
        "+QIMSCFG:\"other\",0,0,2,2,2,2\r\nOK".into(),
        "+QIMSCFG:\"ims_status\"0,0,2,2,2,2\r\nOK".into(),
        format!("{}\r\n{valid}\r\nOK", "x".repeat(4096)),
    ] {
        assert_eq!(parse_status(&text), Err(NativeImsProbeError::MalformedResponse));
    }
}

#[test]
fn native_ims_qualification_is_exact_not_family_substring_or_attestation() {
    for model in ["EC20", "EC20F", "EC20CEFAG"] {
        assert!(qualify(&format!("AT+CGMM\r\n{model}\r\nOK"), "EC20CEFAGR08A03M4G\r\nOK").is_ok());
    }
    assert!(qualify("EC20\r\nOK", "Revision: EC20CEFAGR08A03M4G\r\nOK").is_ok());
    for model in ["EC25", "EG25-G", "not-EC20", "EC20CEFAG-extra", "EC20\r\nEC20", ""] {
        assert_eq!(qualify(&format!("{model}\r\nOK"), "EC20CEFAGR08A03M4G\r\nOK"), Err(NativeImsProbeError::FirmwareNotQualified));
    }
    for revision in ["EC20CEFAGR08A04M4G", "EC20CEFAGR08A03M4G-extra", "EC20CEFAGR08A03M4G", "OK", "ERROR"] {
        // Bare revision lacks NativeIo's final completion, even if it matches.
        assert!(qualify("EC20\r\nOK", revision).is_err());
    }
    assert!(qualify("EC20\r\nOK", "EC20CEFAGR08A04M4G\r\nOK").is_err());
}

#[test]
fn native_ims_iccid_is_strict_private_and_normalized_before_comparison() {
    assert_eq!(parse_iccid("+QCCID: 8900000000000000001F\r\nOK").unwrap(), "8900000000000000001");
    assert_eq!(normalized_iccid("89000000000000000010"), Some("8900000000000000001".into()));
    for value in ["", "abc8900000000000000001", "8900000000000000001-private", "0001", "8900000000000000001FF"] {
        assert!(normalized_iccid(value).is_none());
    }
    assert!(parse_iccid("+QCCID: 8900000000000000001\r\n+QCCID: 8900000000000000002\r\nOK").is_err());
    assert!(parse_iccid("OK").is_err());
}

#[test]
fn native_ims_serialized_whitelist_and_errors_contain_no_raw_payloads() {
    let sample = parse_status(&row("1,0,2,0,1,2")).unwrap();
    assert_eq!(serde_json::to_value(sample).unwrap(), serde_json::json!({
        "schema": "ec20_ce_fag_v0803", "registration_reporting": true,
        "service_reporting": false, "registration": "registered",
        "sms": "unknown_or_unreported", "voice": "limited", "ut": "available"
    }));
    assert_eq!(NativeImsSample::default().registration, NativeImsRegistration::Unknown);
    for error in [NativeImsProbeError::Unsupported, NativeImsProbeError::Busy,
        NativeImsProbeError::BindingChanged, NativeImsProbeError::Unavailable,
        NativeImsProbeError::MalformedResponse, NativeImsProbeError::FirmwareNotQualified] {
        assert_eq!(error.to_string(), error.code());
        assert!(error.code().bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
    }
}
