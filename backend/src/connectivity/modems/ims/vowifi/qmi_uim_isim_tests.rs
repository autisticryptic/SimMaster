use super::*;

fn object(value: &[u8]) -> Vec<u8> {
    let mut bytes = if value.len() < 128 { vec![0x80, value.len() as u8] }
        else { vec![0x80, 0x82, (value.len() >> 8) as u8, value.len() as u8] };
    bytes.extend_from_slice(value);
    bytes
}
fn fcp(file: u16, size: usize, record: Option<(usize, usize)>) -> Vec<u8> {
    let mut body = match record {
        None => vec![0x82, 2, 0x41, 0x21],
        Some((length, count)) => vec![0x82, 5, 0x42, 0x21, (length >> 8) as u8, length as u8, count as u8],
    };
    body.extend([0x83, 2, (file >> 8) as u8, file as u8, 0x80, 2, (size >> 8) as u8, size as u8]);
    let mut result = vec![0x62, body.len() as u8];
    result.extend(body);
    result
}
fn response(data: Vec<u8>) -> UimApduResponse { UimApduResponse { data, sw1: 0x90, sw2: 0 } }

#[test]
fn isim_valid_identity_domain_and_multiple_uri_forms() {
    assert_eq!(parse_ef_impi(&object(b"alice@ims.example")).unwrap().as_deref(), Some("alice@ims.example"));
    assert_eq!(parse_ef_domain(&object(b"ims.example")).unwrap().as_deref(), Some("ims.example"));
    for value in ["sip:alice@ims.example", "sips:alice@[2001:db8::1]:5070", "tel:+12025550123"] {
        assert_eq!(parse_ef_impu_record(&object(value.as_bytes())).unwrap().as_deref(), Some(value));
    }
}

#[test]
fn isim_pcscf_address_types_are_not_uri_guesses() {
    let mut name = vec![0]; name.extend(b"proxy.ims.example");
    assert_eq!(parse_ef_pcscf_record(&object(&name)).unwrap().as_deref(), Some("proxy.ims.example"));
    assert_eq!(parse_ef_pcscf_record(&object(&[1, 192, 0, 2, 1])).unwrap().as_deref(), Some("192.0.2.1"));
    let mut ipv6 = vec![2]; ipv6.extend("2001:db8::1".parse::<Ipv6Addr>().unwrap().octets());
    assert_eq!(parse_ef_pcscf_record(&object(&ipv6)).unwrap().as_deref(), Some("2001:db8::1"));
    for value in [&[3, 1, 2, 3, 4][..], &[1, 1, 2, 3][..], b"\0sip:proxy.ims.example", b"\0proxy.ims.example:5060"] {
        assert_eq!(parse_ef_pcscf_record(&object(value)), Err("isim_pcscf_invalid"));
    }
}

#[test]
fn isim_long_ber_and_ff_padding_are_bounded() {
    let identity = format!("{}@ims.example", "a".repeat(130));
    let mut data = object(identity.as_bytes()); data.extend([0xff; 9]);
    assert_eq!(parse_ef_impi(&data).unwrap().as_deref(), Some(identity.as_str()));
    for bad in [vec![0x80, 0x80, 0, 0], vec![0x80, 0x82, 1], vec![0x80, 0x83, 0, 0, 1, 1], vec![0x80, 8, 1], vec![0x80, 0]] {
        assert!(parse_ef_impi(&bad).is_err());
    }
    let mut bad = object(b"alice@ims.example"); bad.push(0);
    assert_eq!(parse_ef_impi(&bad), Err("isim_tlv_invalid"));
    assert_eq!(parse_ef_impi(&vec![0xff; MAX_EF_BYTES + 1]), Err("isim_file_too_large"));
}

#[test]
fn isim_rejects_identity_header_injection_and_unsupported_uris() {
    for value in ["sip:a@ims.example\r\nVia:x", "sip:a@ims.example?Route=x", "sip:a@ims.example;unknown=x", "sip:a%0d@ims.example", "sip:a:b@ims.example", "sip:a@ims.example:0", "sip:a@ims.example:65536", "tel:123", "tel:+123;ext=4"] {
        assert!(parse_ef_impu_record(&object(value.as_bytes())).is_err(), "{value:?}");
    }
    for value in ["a@ims.example\n", "a b@ims.example", "a@@ims.example", "a@-bad.example"] {
        assert!(parse_ef_impi(&object(value.as_bytes())).is_err());
    }
}

#[test]
fn isim_fcp_layout_uses_file_descriptor_not_guessed_record_lengths() {
    assert_eq!(parse_fcp(&fcp(EF_IMPI, 30, None), EF_IMPI, false).unwrap(), Layout::Transparent { size: 30 });
    assert_eq!(parse_fcp(&fcp(EF_IMPU, 90, Some((30, 3))), EF_IMPU, true).unwrap(), Layout::Records { size: 90, length: 30, count: 3 });
    assert_eq!(parse_fcp(&fcp(EF_IMPU, 90, Some((31, 3))), EF_IMPU, true), Err("isim_fcp_invalid"));
    assert_eq!(parse_fcp(&fcp(EF_IMPU, 330, Some((10, 33))), EF_IMPU, true), Err("isim_record_limit"));
    assert_eq!(parse_fcp(&fcp(EF_IMPU, 257, Some((257, 1))), EF_IMPU, true), Err("isim_record_size_unsupported"));
    assert_eq!(parse_fcp(&fcp(EF_IMPI, 4097, None), EF_IMPI, false), Err("isim_file_too_large"));
    assert!(parse_fcp(&fcp(EF_IMPI, 30, None), EF_DOMAIN, false).is_err());
    assert!(parse_fcp(&fcp(EF_IMPI, 30, None), EF_IMPI, true).is_err());
}

#[test]
fn isim_fcp_rejects_duplicates_trailing_bytes_and_malformed_constructed_data() {
    let mut duplicate = fcp(EF_IMPI, 30, None);
    duplicate.extend([0x80, 2, 0, 30]); duplicate[1] += 4;
    assert!(parse_fcp(&duplicate, EF_IMPI, false).is_err());
    let mut trailing = fcp(EF_IMPI, 30, None); trailing.push(0xff);
    assert!(parse_fcp(&trailing, EF_IMPI, false).is_err());
    let mut nested = fcp(EF_IMPI, 30, None); nested.extend([0xa5, 2, 0x80, 1]); nested[1] += 4;
    assert!(parse_fcp(&nested, EF_IMPI, false).is_err());
    let mut long = fcp(EF_IMPI, 30, None); let length = long.remove(1); long.splice(1..1, [0x82, 0, length]);
    assert_eq!(parse_fcp(&long, EF_IMPI, false).unwrap(), Layout::Transparent { size: 30 });
    assert!(parse_fcp(&vec![0; MAX_FCP_BYTES + 1], EF_IMPI, false).is_err());
}

#[test]
fn isim_mock_reads_transparent_and_records_on_one_selected_application() {
    let impi = object(b"alice@ims.example");
    let domain = object(b"ims.example");
    let mut first = object(b"sip:alice@ims.example"); first.resize(40, 0xff);
    let mut second = object(b"tel:+12025550123"); second.resize(40, 0xff);
    let mut pcscf = object(&[1, 192, 0, 2, 1]); pcscf.resize(20, 0xff);
    let mut script = std::collections::VecDeque::from([
        (vec![0, 0xa4, 0, 4, 2, 0x6f, 2, 0], response(fcp(EF_IMPI, impi.len(), None))),
        (vec![0, 0xb0, 0, 0, impi.len() as u8], response(impi)),
        (vec![0, 0xa4, 0, 4, 2, 0x6f, 3, 0], response(fcp(EF_DOMAIN, domain.len(), None))),
        (vec![0, 0xb0, 0, 0, domain.len() as u8], response(domain)),
        (vec![0, 0xa4, 0, 4, 2, 0x6f, 4, 0], response(fcp(EF_IMPU, 80, Some((40, 2))))),
        (vec![0, 0xb2, 1, 4, 40], response(first)),
        (vec![0, 0xb2, 2, 4, 40], response(second)),
        (vec![0, 0xa4, 0, 4, 2, 0x6f, 9, 0], response(fcp(EF_PCSCF, 20, Some((20, 1))))),
        (vec![0, 0xb2, 1, 4, 20], response(pcscf)),
    ]);
    let material = read_isim_material_with(|apdu| {
        let (expected, reply) = script.pop_front().expect("bounded exchange");
        assert_eq!(apdu, expected);
        Ok(reply)
    }).unwrap();
    assert!(script.is_empty());
    assert_eq!(material.impus, ["sip:alice@ims.example", "tel:+12025550123"]);
    assert_eq!(material.pcscf, ["192.0.2.1"]);
    assert!(!format!("{material:?}").contains("alice"));
}

#[test]
fn isim_missing_or_ff_is_unconfigured_but_select_denial_is_not() {
    let material = read_isim_material_with(|apdu| {
        assert_eq!(apdu[1], 0xa4);
        Ok(UimApduResponse { data: vec![], sw1: 0x6a, sw2: 0x82 })
    }).unwrap();
    assert_eq!(material, IsimImsMaterial::default());
    assert_eq!(parse_ef_impi(&[0xff; 32]), Ok(None));
    assert_eq!(parse_ef_domain(&[]), Ok(None));
    assert_eq!(read_isim_material_with(|_| Ok(UimApduResponse { data: vec![], sw1: 0x69, sw2: 0x82 })), Err("isim_select_failed"));
}

#[test]
fn isim_transient_read_error_and_short_eof_are_not_absence() {
    let mut count = 0;
    let result = read_isim_material_with(|_| {
        count += 1;
        if count == 1 { Ok(response(fcp(EF_IMPI, 20, None))) } else { Err("mock_transport_timeout") }
    });
    assert_eq!(result, Err("mock_transport_timeout"));
    assert_eq!(count, 2);
    let eof = UimApduResponse { data: vec![0xff; 20], sw1: 0x62, sw2: 0x82 };
    assert_eq!(validate_read(&eof, 20, true), Ok(()));
    assert_eq!(validate_read(&eof, 20, false), Err("isim_read_failed"));
    assert_eq!(validate_read(&eof, 21, true), Err("isim_read_length_mismatch"));
}

#[test]
fn isim_transparent_chunks_and_record_bounds_follow_fcp() {
    let mut expected = object(format!("{}@ims.example", "a".repeat(290)).as_bytes());
    expected.resize(600, 0xff);
    let mut offset = 0;
    let mut calls = 0;
    let data = read_ef(&mut |apdu| {
        calls += 1;
        if apdu[1] == 0xa4 { return Ok(response(fcp(EF_IMPI, 600, None))); }
        assert_eq!(apdu[1], 0xb0);
        assert_eq!((usize::from(apdu[2]) << 8) | usize::from(apdu[3]), offset);
        let length = (600 - offset).min(256);
        assert_eq!(apdu[4], length as u8);
        let data = expected[offset..offset + length].to_vec();
        offset += length;
        Ok(UimApduResponse { data, sw1: if offset == 600 { 0x62 } else { 0x90 }, sw2: if offset == 600 { 0x82 } else { 0 } })
    }, EF_IMPI, false).unwrap();
    assert_eq!(calls, 4);
    assert_eq!(data, vec![expected]);
    assert!(parse_ef_impi(&data[0]).unwrap().is_some());
    let mut calls = 0;
    assert_eq!(read_ef(&mut |_| {
        calls += 1;
        Ok(response(fcp(EF_IMPU, 330, Some((10, 33)))))
    }, EF_IMPU, true), Err("isim_record_limit"));
    assert_eq!(calls, 1);
}

#[test]
fn isim_all_truncated_tlv_and_fcp_prefixes_fail_without_panicking() {
    let object = object(b"alice@ims.example");
    for length in 1..object.len() { assert!(parse_ef_impi(&object[..length]).is_err()); }
    let fcp = fcp(EF_IMPI, 20, None);
    for length in 0..fcp.len() { assert!(parse_fcp(&fcp[..length], EF_IMPI, false).is_err()); }
}

#[test]
fn isim_directory_discovers_full_aids_and_rejects_malformed_records() {
    let usim = [super::super::USIM_AID_PREFIX, &[0xaa]].concat();
    let isim = [ISIM_AID_PREFIX, &[0xbb]].concat();
    let record = |aid: &[u8]| {
        let mut record = vec![0x61, (aid.len() + 2) as u8, 0x4f, aid.len() as u8];
        record.extend_from_slice(aid);
        record.resize(32, 0xff);
        record
    };
    assert_eq!(parse_application_directory(&[record(&usim), record(&isim)]).unwrap(), vec![usim.clone(), isim]);
    assert!(parse_application_directory(&[vec![0x61, 3, 0x4f, 8, 0]]).is_err());
    assert!(parse_application_directory(&[vec![0x61, 2, 0x50, 0]]).is_err());
    let mut calls = Vec::new();
    let records = read_application_aids_with(|apdu| {
        calls.push(apdu.to_vec());
        Ok(response(if apdu[1] == 0xb2 { record(&usim) }
            else if apdu.get(5) == Some(&0x2f) { fcp(0x2f00, 32, Some((32, 1))) }
            else { vec![] }))
    }).unwrap();
    assert_eq!(records, vec![usim]);
    assert_eq!(calls[0], [0, 0xa4, 0, 4, 2, 0x3f, 0, 0]);
    assert_eq!(calls[2], [0, 0xb2, 1, 4, 32]);
}

#[test]
fn isim_rejects_partial_or_wrong_application_identifiers() {
    assert!(validate_isim_aid(ISIM_AID_PREFIX).is_ok());
    for aid in [&ISIM_AID_PREFIX[..6], super::super::USIM_AID_PREFIX, &[0; 17][..]] {
        assert_eq!(validate_isim_aid(aid), Err("isim_invalid_aid"));
    }
}
