use super::*;
use super::super::{
    aka_digest_password, build_authorization_header, build_initial_authorization_header,
    build_resync_authorization_header_with_digest, AkaMaterial,
};

const RFC_RSPAUTH_NO_QOP: &str = "2a38c66e35e2b1f6763297add4c6c66f";
const RFC_RSPAUTH_AUTH: &str = "376602cfd2f4e8e5e78b948a85263e85";
const RFC_TUPLE: &str = "qop=auth,nc=00000001,cnonce=\"0a4f113b\"";
const AKA_V2_SHA256_RSPAUTH: &str =
    "e71026547edf9953139e3e01a32c66f3167b29423d90a32feda842f3e70eb226";

fn challenge(qop: bool, proxy: bool) -> DigestChallenge {
    DigestChallenge {
        realm: "testrealm@host.com".to_owned(),
        nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_owned(),
        algorithm: "MD5".to_owned(),
        qop: qop.then(|| "auth".to_owned()),
        opaque: None,
        proxy,
        stale: false,
    }
}

fn proof(qop: bool, proxy: bool) -> DigestRequestProof {
    DigestRequestProof::new(&challenge(qop, proxy), "Mufasa", b"Circle Of Life",
        "/dir/index.html", "0a4f113b", "00000001").unwrap()
}

fn response(fields: &str) -> Vec<u8> {
    format!("SIP/2.0 200 OK\r\n{fields}\r\n").into_bytes()
}

fn info(value: &str) -> Vec<u8> {
    response(&format!("Authentication-Info: {value}\r\n"))
}

fn request(auth: &str) -> Vec<u8> {
    format!("REGISTER /dir/index.html SIP/2.0\r\n{auth}\r\nContent-Length: 0\r\n\r\n")
        .into_bytes()
}

fn valid_request(qop: bool, proxy: bool) -> Vec<u8> {
    let c = challenge(qop, proxy);
    let digest = compute_digest_response("Mufasa", &c.realm, b"Circle Of Life", &c.algorithm,
        "REGISTER", "/dir/index.html", &c.nonce, c.qop.as_deref(), "0a4f113b", "00000001")
        .unwrap();
    request(&build_authorization_header(&c, "Mufasa", "/dir/index.html", &digest,
        "0a4f113b", "00000001"))
}

fn assert_response_error(proof: &DigestRequestProof, frame: &[u8], code: &str) {
    assert_eq!(proof.verify_response(frame).unwrap_err().code(), code);
}

#[test]
fn fixed_independent_md5_rspauth_vectors_auth_and_legacy() {
    // Independently generated using Python hashlib, not this implementation:
    // HA1 = MD5(Mufasa:testrealm@host.com:Circle Of Life), HA2 = MD5(:/dir/index.html).
    for (qop, digest) in [(false, RFC_RSPAUTH_NO_QOP), (true, RFC_RSPAUTH_AUTH)] {
        let p = proof(qop, false);
        let tuple = if qop { format!(",{RFC_TUPLE}") } else { String::new() };
        let verified = p.verify_response(&info(&format!("rspauth=\"{digest}\"{tuple}"))).unwrap();
        assert!(verified.rspauth_verified());
        assert_eq!(verified.nextnonce(), None);
        p.validate_request(&valid_request(qop, false)).unwrap();
    }
}

#[test]
fn fixed_independent_akav1_akav2_md5_sha256_rspauth_vectors() {
    // Python hashlib/hmac/base64 vectors, synthetic RES/IK/CK; no SIM material.
    let aka = AkaMaterial { res: &[0x11; 8], ck: &[0x22; 16], ik: &[0x33; 16] };
    for (algorithm, expected, expected_request) in [
        ("AKAv1-MD5", "27603ddb703e643b31289fb5f6ae8ccb", "286e899023f8132dbc19338dce1e6f3b"),
        ("AKAv2-MD5", "190526262ea700b2dbd4bf166e37efbf", "c3141b5a21ea58b0766a41734cd19971"),
        ("AKAv2-SHA-256", AKA_V2_SHA256_RSPAUTH,
            "7b1e4b6f0343c2970757c768727ff7d3ad8e0f8d4c815b2d68b38f9784cb11c5"),
    ] {
        let c = DigestChallenge { realm: "ims.example".to_owned(), nonce: "nonce".to_owned(),
            algorithm: algorithm.to_owned(), ..challenge(true, false) };
        let password = aka_digest_password(algorithm, &aka).unwrap();
        let p = DigestRequestProof::new(&c, "user@example.com", &password,
            "sip:ims.example", "cafebabe", "00000001").unwrap();
        let verified = p.verify_response(&info(&format!(
            "rspauth=\"{expected}\",qop=auth,nc=00000001,cnonce=\"cafebabe\""))).unwrap();
        assert!(verified.rspauth_verified());
        assert_eq!(p.response_digest.len(), if algorithm == "AKAv2-SHA-256" { 32 } else { 16 });
        let auth = build_authorization_header(&c, "user@example.com", "sip:ims.example",
            expected_request, "cafebabe", "00000001");
        p.validate_request(format!("REGISTER sip:ims.example SIP/2.0\r\n{auth}\r\n\r\n").as_bytes())
            .unwrap();
    }
}

#[test]
fn policy_approved_plain_md5_empty_password_is_not_an_empty_aka_authorization() {
    // Existing caller-gated plain MD5 uses an empty password, but sends a real
    // digest. Independent hashlib vector: do not confuse it with response="".
    let c = challenge(true, false);
    let p = DigestRequestProof::new(&c, "Mufasa", b"", "/dir/index.html", "0a4f113b", "00000001")
        .unwrap();
    let auth = build_authorization_header(&c, "Mufasa", "/dir/index.html",
        "243734eb19f93aef0f9c3ea799cb4e21", "0a4f113b", "00000001");
    p.validate_request(&request(&auth)).unwrap();
    assert!(p.verify_response(&info(&format!("rspauth=00756b7e14f81805df1d376051d1ba9f,{RFC_TUPLE}")))
        .unwrap().rspauth_verified());
    assert_eq!(p.validate_request(&request(&auth.replace("243734eb19f93aef0f9c3ea799cb4e21", "")))
        .unwrap_err().code(), "digest_request_auth_mismatch");
}

#[test]
fn hexadecimal_rspauth_is_case_insensitive_for_both_lengths() {
    let p = proof(true, false);
    p.verify_response(&info(&format!("rspauth={},{}", RFC_RSPAUTH_AUTH.to_uppercase(), RFC_TUPLE)))
        .unwrap();
    let c = DigestChallenge { realm: "ims.example".into(), nonce: "nonce".into(),
        algorithm: "AKAv2-SHA-256".into(), ..challenge(true, false) };
    let p = DigestRequestProof::new(&c, "user@example.com",
        b"y1XGaxmHAuuo8s2MPYfXz/CZeEa1RBiLycGrpY293pQ=", "sip:ims.example", "cafebabe",
        "00000001").unwrap();
    p.verify_response(&info(&format!("rspauth={},qop=auth,nc=00000001,cnonce=cafebabe",
        AKA_V2_SHA256_RSPAUTH.to_uppercase()))).unwrap();
}

#[test]
fn request_auth_digest_cannot_substitute_for_rspauth() {
    // MD5 REGISTER proof has a method in HA2; rspauth must use the empty method.
    let p = proof(true, false);
    let sent = String::from_utf8(valid_request(true, false)).unwrap();
    let expected_request_hex = super::super::hex_lower(&p.request_digest);
    assert!(sent.contains(&expected_request_hex));
    assert_response_error(&p, &info(&format!("rspauth={expected_request_hex},{RFC_TUPLE}")),
        "digest_response_auth_mismatch");
    let c = DigestChallenge { realm: "ims.example".into(), nonce: "nonce".into(),
        algorithm: "AKAv2-SHA-256".into(), ..challenge(true, false) };
    let p = DigestRequestProof::new(&c, "user@example.com",
        b"y1XGaxmHAuuo8s2MPYfXz/CZeEa1RBiLycGrpY293pQ=", "sip:ims.example", "cafebabe",
        "00000001").unwrap();
    assert_response_error(&p, &info(concat!(
        "rspauth=7b1e4b6f0343c2970757c768727ff7d3ad8e0f8d4c815b2d68b38f9784cb11c5,",
        "qop=auth,nc=00000001,cnonce=cafebabe")), "digest_response_auth_mismatch");
}

#[test]
fn optional_missing_info_rspauth_and_nextnonce_only_remain_compatible() {
    for qop in [false, true] {
        let p = proof(qop, false);
        let absent = p.verify_response(&response("")).unwrap();
        assert!(!absent.rspauth_verified());
        assert_eq!(absent.nextnonce(), None);
        let next = p.verify_response(&info("nextnonce=\"next,nonce\"")).unwrap();
        assert!(!next.rspauth_verified());
        assert_eq!(next.nextnonce(), Some("next,nonce"));
        let extension = p.verify_response(&info("server-extension=\"a,b\"")).unwrap();
        assert!(!extension.rspauth_verified());
        assert_eq!(extension.nextnonce(), None);
    }
    let absent = validate_response_authentication(None, &response("")).unwrap();
    assert!(!absent.rspauth_verified());
}

#[test]
fn present_tuple_members_always_bind_even_without_rspauth() {
    let p = proof(true, false);
    for partial in ["qop=auth", "nc=00000001", "cnonce=0a4f113b", RFC_TUPLE] {
        assert!(!p.verify_response(&info(partial)).unwrap().rspauth_verified());
    }
    for wrong in ["qop=auth-int", "qop=AUTH", "nc=00000002", "cnonce=other"] {
        assert_response_error(&p, &info(&format!("{wrong},nextnonce=next")),
            "digest_response_auth_mismatch");
    }
}

#[test]
fn copying_old_rspauth_with_rewritten_request_tuple_cannot_validate_new_request() {
    let c = challenge(true, false);
    for (cnonce, nc) in [("0a4f113b", "00000002"), ("fresh-client-nonce", "00000001")] {
        let p = DigestRequestProof::new(&c, "Mufasa", b"Circle Of Life", "/dir/index.html",
            cnonce, nc).unwrap();
        assert_response_error(&p, &info(&format!(
            "rspauth={RFC_RSPAUTH_AUTH},qop=auth,nc={nc},cnonce=\"{cnonce}\"")),
            "digest_response_auth_mismatch");
    }
    assert_response_error(&proof(false, false), &info(&format!("rspauth={RFC_RSPAUTH_AUTH}")),
        "digest_response_auth_mismatch");
}

#[test]
fn rspauth_under_qop_requires_all_three_correlated_fields() {
    let p = proof(true, false);
    for tuple in ["", ",qop=auth", ",nc=00000001", ",cnonce=0a4f113b",
        ",qop=auth,nc=00000001", ",qop=auth,cnonce=0a4f113b", ",nc=00000001,cnonce=0a4f113b"]
    {
        assert_response_error(&p, &info(&format!("rspauth={RFC_RSPAUTH_AUTH}{tuple}")),
            "digest_response_auth_invalid");
    }
}

#[test]
fn no_qop_context_rejects_unsolicited_tuple() {
    let p = proof(false, false);
    for tuple in ["qop=auth", "nc=00000001", "cnonce=0a4f113b", RFC_TUPLE] {
        assert_response_error(&p, &info(tuple), "digest_response_auth_mismatch");
        assert_response_error(&p, &info(&format!("rspauth={RFC_RSPAUTH_NO_QOP},{tuple}")),
            "digest_response_auth_mismatch");
    }
}

#[test]
fn response_digest_rejects_bad_hex_wrong_lengths_and_mismatch() {
    let p = proof(false, false);
    for malformed in [String::new(), "g".repeat(32), "0".repeat(31), "0".repeat(33),
        "0".repeat(64), "é".repeat(16), "00 00".to_owned()]
    {
        assert_response_error(&p, &info(&format!("rspauth=\"{malformed}\"")),
            "digest_response_auth_invalid");
    }
    assert_response_error(&p, &info(&format!("rspauth={}", "0".repeat(32))),
        "digest_response_auth_mismatch");
    let c = DigestChallenge { algorithm: "AKAv2-SHA-256".into(), ..challenge(false, false) };
    let sha = DigestRequestProof::new(&c, "Mufasa", b"password", "/dir/index.html", "", "")
        .unwrap();
    assert_response_error(&sha, &info(&format!("rspauth={RFC_RSPAUTH_NO_QOP}")),
        "digest_response_auth_invalid");
}

#[test]
fn nonce_count_has_exact_eight_hex_digits_and_cannot_overflow_or_be_zero() {
    let p = proof(true, false);
    for nc in ["", "1", "000000001", "100000000", "fffffffff", "-0000001", "0000000g", "00000000"] {
        assert_response_error(&p, &info(&format!("nc=\"{nc}\"")), "digest_response_auth_invalid");
        assert_eq!(DigestRequestProof::new(&challenge(true, false), "Mufasa", b"pw", "sip:r",
            "cnonce", nc).unwrap_err().code(), "digest_request_auth_mismatch");
    }
    let c = challenge(true, false);
    let max = DigestRequestProof::new(&c, "Mufasa", b"pw", "sip:r", "cnonce", "ffffffff")
        .unwrap();
    assert!(!max.verify_response(&info("nc=ffffffff")).unwrap().rspauth_verified());
    assert_response_error(&max, &info("nc=FFFFFFFE"), "digest_response_auth_mismatch");
}

#[test]
fn repeated_folded_fields_combine_once_and_quoted_commas_are_not_separators() {
    let p = proof(true, false);
    let frame = response(&format!(concat!(
        "aUtHeNtIcAtIoN-InFo: rspauth=\"{}\",\r\n\tqop=auth\r\n",
        "Authentication-Info: nc=00000001, cnonce=\"0a4f113b\",\r\n",
        " nextnonce=\"a,b\\\"c\\\\d\", x-note=\"folded\r\n\tvalue\"\r\n"), RFC_RSPAUTH_AUTH));
    let verified = p.verify_response(&frame).unwrap();
    assert!(verified.rspauth_verified());
    assert_eq!(verified.nextnonce(), Some("a,b\"c\\d"));
}

#[test]
fn duplicate_params_are_rejected_case_insensitively_across_all_field_forms() {
    let p = proof(true, false);
    for fields in [
        "Authentication-Info: nextnonce=a,NextNonce=b\r\n",
        "Authentication-Info: nextnonce=\"a,b\",NEXTNONCE=\"a,b\"\r\n",
        "Authentication-Info: nextnonce=a,\r\n NEXTNONCE=b\r\n",
        "Authentication-Info: nextnonce=a\r\nAuthentication-Info: NextNonce=b\r\n",
        "Authentication-Info: x=a\r\nAuthentication-Info: X=b\r\n",
        "Authentication-Info: qop=auth,QOP=auth\r\n",
        "Authentication-Info: nc=00000001,NC=00000001\r\n",
        "Authentication-Info: cnonce=0a4f113b,CNonce=0a4f113b\r\n",
        "Authentication-Info: nextnonce=\"open\r\nAuthentication-Info: rspauth=wrong\"\r\n",
        "Authentication-Info: x=\"open\r\nAuthentication-Info: nextnonce=hidden\"\r\n",
    ] {
        assert_response_error(&p, &response(fields), "digest_response_auth_invalid");
    }
    assert_response_error(&p, &info(&format!("rspauth={0},RSPAUTH={0},{RFC_TUPLE}", RFC_RSPAUTH_AUTH)),
        "digest_response_auth_invalid");
}

#[test]
fn proxy_and_origin_families_cannot_substitute_or_update_each_other() {
    for proxy in [false, true] {
        let p = proof(false, proxy);
        p.validate_request(&valid_request(false, proxy)).unwrap();
        let expected = if proxy { "Proxy-Authentication-Info" } else { "Authentication-Info" };
        let other = if proxy { "Authentication-Info" } else { "Proxy-Authentication-Info" };
        let correct = format!("{expected}: rspauth={RFC_RSPAUTH_NO_QOP},nextnonce=correct\r\n");
        assert!(p.verify_response(&response(&correct)).unwrap().rspauth_verified());
        for bad in [format!("{other}: nextnonce=wrong\r\n"),
            format!("{correct}{other}: nextnonce=wrong\r\n"),
            format!("{other}: x=extension\r\n{correct}")]
        {
            assert_response_error(&p, &response(&bad), "digest_response_auth_scope_mismatch");
        }
    }
}

#[test]
fn unsolicited_info_without_credentials_including_empty_aka_auts_is_rejected() {
    for header in ["Authentication-Info", "Proxy-Authentication-Info"] {
        for value in ["nextnonce=next", "x=extension", "rspauth=00", ""] {
            assert_eq!(validate_response_authentication(None,
                &response(&format!("{header}: {value}\r\n"))).unwrap_err().code(),
                "digest_response_auth_context_missing");
        }
    }
    let p = proof(true, false);
    let initial = build_initial_authorization_header("Mufasa", "testrealm@host.com", "/dir/index.html");
    let auts = build_resync_authorization_header_with_digest(&challenge(true, false),
        "Mufasa", "/dir/index.html", &[0x55; 14], Some("0a4f113b"), Some("00000001"));
    for auth in [initial, auts] {
        assert_eq!(p.validate_request(&request(&auth)).unwrap_err().code(), "digest_request_auth_mismatch");
    }
    for algorithm in ["AKAv1-MD5", "AKAv2-MD5", "AKAv2-SHA-256"] {
        let c = DigestChallenge { algorithm: algorithm.into(), ..challenge(true, false) };
        assert_eq!(DigestRequestProof::new(&c, "Mufasa", b"", "/dir/index.html",
            "0a4f113b", "00000001").unwrap_err().code(), "digest_request_auth_mismatch");
    }
}

#[test]
fn immutable_context_isolated_from_later_nextnonce_uri_and_credentials() {
    let mut c = challenge(true, false);
    let mut password = b"Circle Of Life".to_vec();
    let p = DigestRequestProof::new(&c, "Mufasa", &password, "/dir/index.html", "0a4f113b", "00000001")
        .unwrap();
    let cloned = p.clone();
    let frame = info(&format!("rspauth={RFC_RSPAUTH_AUTH},{RFC_TUPLE},nextnonce=new"));
    assert_eq!(p.verify_response(&frame).unwrap().nextnonce(), Some("new"));
    c.nonce = "new".into();
    password.fill(0);
    assert!(cloned.verify_response(&frame).unwrap().rspauth_verified());
    for (c, user, pw, uri) in [
        (c, "Mufasa", b"Circle Of Life".as_slice(), "/dir/index.html"),
        (challenge(true, false), "Mufasa", b"Circle Of Life".as_slice(), "/other"),
        (challenge(true, false), "other", b"Circle Of Life".as_slice(), "/dir/index.html"),
        (challenge(true, false), "Mufasa", password.as_slice(), "/dir/index.html"),
        (DigestChallenge { realm: "other".into(), ..challenge(true, false) }, "Mufasa",
            b"Circle Of Life".as_slice(), "/dir/index.html"),
    ] {
        let different = DigestRequestProof::new(&c, user, pw, uri, "0a4f113b", "00000001").unwrap();
        assert_response_error(&different, &frame, "digest_response_auth_mismatch");
    }
}

#[test]
fn final_request_contract_checks_uri_method_and_each_authorization_parameter() {
    let p = proof(true, false);
    let original = String::from_utf8(valid_request(true, false)).unwrap();
    for (from, to) in [
        ("REGISTER /dir/index.html", "REGISTER /other"),
        ("REGISTER /dir/index.html", "OPTIONS /dir/index.html"),
        ("uri=\"/dir/index.html\"", "uri=\"/other\""),
        ("username=\"Mufasa\"", "username=\"other\""),
        ("realm=\"testrealm@host.com\"", "realm=\"other\""),
        ("nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\"", "nonce=\"other\""),
        ("algorithm=MD5", "algorithm=AKAv1-MD5"),
        ("qop=auth", "qop=auth-int"),
        ("nc=00000001", "nc=00000002"),
        ("cnonce=\"0a4f113b\"", "cnonce=\"other\""),
        ("Authorization:", "Proxy-Authorization:"),
        ("Digest username", "Digest\u{a0}username"),
    ] {
        let mutated = original.replace(from, to);
        assert_ne!(mutated, original);
        assert_eq!(p.validate_request(mutated.as_bytes()).unwrap_err().code(), "digest_request_auth_mismatch");
    }
    let request_hex = super::super::hex_lower(&p.request_digest);
    for replacement in ["".to_owned(), "0".repeat(32), RFC_RSPAUTH_AUTH.to_owned()] {
        assert_eq!(p.validate_request(original.replace(&request_hex, &replacement).as_bytes())
            .unwrap_err().code(), "digest_request_auth_mismatch");
    }
    p.validate_request(original.replace(&request_hex, &request_hex.to_uppercase()).as_bytes()).unwrap();
}

#[test]
fn final_request_rejects_missing_duplicate_and_unsolicited_authorization() {
    let p = proof(true, false);
    let original = String::from_utf8(valid_request(true, false)).unwrap();
    let auth = original.lines().find(|line| line.starts_with("Authorization:")).unwrap();
    for bad in [original.replace(auth, "X-Not-Authorization: ignored"),
        original.replace(auth, &format!("{auth}\r\n{auth}")),
        original.replace(auth, &format!("{auth}\r\nProxy-{auth}")),
        original.replace(auth, &format!("{auth},nonce=duplicate")),
        original.replace(auth, &format!("{auth},auts=\"AAAA\"")),
        original.replace(auth, &format!("{auth},userhash=true"))]
    {
        assert_eq!(p.validate_request(bad.as_bytes()).unwrap_err().code(), "digest_request_auth_mismatch");
    }
    let without_qop = proof(false, false);
    assert_eq!(without_qop.validate_request(&valid_request(true, false)).unwrap_err().code(),
        "digest_request_auth_mismatch");
}

#[test]
fn final_request_accepts_folded_case_insensitive_header_names_and_escaped_values() {
    let c = DigestChallenge { nonce: "nonce,with\"quote\\slash".into(), ..challenge(true, false) };
    let p = DigestRequestProof::new(&c, "Mu\"fasa", b"pw", "/dir/index.html", "0a4f113b", "00000001")
        .unwrap();
    let digest = super::super::hex_lower(&p.request_digest);
    let auth = build_authorization_header(&c, "Mu\"fasa", "/dir/index.html", &digest, "0a4f113b", "00000001")
        .replace("Authorization:", "aUtHoRiZaTiOn:")
        .replace(",qop", ",\r\n\tqop");
    p.validate_request(&request(&auth)).unwrap();
}

#[test]
fn opaque_is_exactly_preserved_in_final_request_including_empty_value() {
    for opaque in [None, Some(""), Some("opaque,\"tag\\value")] {
        let c = DigestChallenge { opaque: opaque.map(str::to_owned), ..challenge(true, false) };
        let p = DigestRequestProof::new(&c, "Mufasa", b"Circle Of Life", "/dir/index.html", "0a4f113b", "00000001")
            .unwrap();
        let digest = super::super::hex_lower(&p.request_digest);
        let header = |c: &DigestChallenge| build_authorization_header(c, "Mufasa", "/dir/index.html", &digest,
            "0a4f113b", "00000001");
        p.validate_request(&request(&header(&c))).unwrap();
        for different in [None, Some(""), Some("changed")] {
            if different == opaque { continue; }
            let altered = DigestChallenge { opaque: different.map(str::to_owned), ..c.clone() };
            assert_eq!(p.validate_request(&request(&header(&altered))).unwrap_err().code(),
                "digest_request_auth_mismatch");
        }
    }
}

#[test]
fn empty_unknown_response_extensions_are_allowed_but_known_fields_are_nonempty() {
    let p = proof(true, false);
    assert!(!p.verify_response(&info("x=\"\"")).unwrap().rspauth_verified());
    let frame = response(&format!("Authentication-Info: rspauth={RFC_RSPAUTH_AUTH},{RFC_TUPLE}\r\nAuthentication-Info: x=\"\",opaque=\"\"\r\n"));
    assert!(p.verify_response(&frame).unwrap().rspauth_verified());
    for name in ["nextnonce", "rspauth", "qop", "nc", "cnonce"] {
        assert_response_error(&p, &info(&format!("{name}=\"\"")), "digest_response_auth_invalid");
    }
}

#[test]
fn response_parser_rejects_malformed_empty_and_control_values() {
    let p = proof(false, false);
    for bad in ["", " ", "nextnonce", "nextnonce=", "nextnonce=\"\"",
        "nextnonce=\"unterminated", "nextnonce=\"dangling\\", ",nextnonce=x", "nextnonce=x,",
        "nextnonce=x,,qop=auth", "nextnonce=\"x\"junk", "nextnonce=a=b", "nextnonce=a/b",
        "nextnonce=\"nul\0\"", "nextnonce=\"del\x7f\"", "nextnonce=\"esc\x1b\"",
        "nextnonce=\"tab\t\"", "nextnonce=\"escaped\\\t\"", "nextnonce=\"cr\r\"",
        "nextnonce=\"unicode\u{85}\""]
    {
        assert_response_error(&p, &info(bad), "digest_response_auth_invalid");
    }
    for bad in [b"SIP/2.0 200 OK\r\nAuthentication-Info: nextnonce=a".as_slice(),
        b"SIP/2.0 200 OK\r\n orphan-fold\r\n\r\n".as_slice(),
        b"SIP/2.0 200 OK\r\nNoColon\r\n\r\n".as_slice(),
        b"SIP/2.0 200 OK\r\nAuthentication-Info\0: nextnonce=a\r\n\r\n".as_slice(),
        b"SIP/2.0 200 OK\r\nAuthentication-Info: nextnonce=\"\xff\"\r\n\r\n".as_slice(),
        b"REGISTER sip:r SIP/2.0\r\n\r\n".as_slice(),
        b"SIP/2.0 200 OK\r\r\n\r\n".as_slice()]
    {
        assert_response_error(&p, bad, "digest_response_auth_invalid");
    }
}

#[test]
fn value_name_count_and_combined_wire_budgets_are_enforced() {
    let p = proof(false, false);
    let at_limit = "n".repeat(MAX_PARAM_VALUE_BYTES);
    assert_eq!(p.verify_response(&info(&format!("nextnonce=\"{at_limit}\"")))
        .unwrap().nextnonce(), Some(at_limit.as_str()));
    assert_response_error(&p, &info(&format!("nextnonce=\"{}n\"", at_limit)),
        "digest_response_auth_invalid");
    assert_response_error(&p, &info(&format!("{}=x", "a".repeat(MAX_PARAM_NAME_BYTES + 1))),
        "digest_response_auth_invalid");
    let fields = (0..64).map(|i| format!("Authentication-Info: x{i}=value\r\n")).collect::<String>();
    p.verify_response(&response(&fields)).unwrap();
    assert_response_error(&p, &response(&format!("{fields}Authentication-Info: x64=value\r\n")),
        "digest_response_auth_invalid");
    let many_values = (0..5).map(|i| format!("Authentication-Info: x{i}=\"{}\"\r\n", "a".repeat(3500)))
        .collect::<String>();
    assert_response_error(&p, &response(&many_values), "digest_response_auth_invalid");
    let folded = format!("Authentication-Info: nextnonce=a,\r\n{}x=b\r\n", " ".repeat(MAX_AUTH_BYTES));
    assert_response_error(&p, &response(&folded), "digest_response_auth_invalid");
    let oversized_name = format!("Authentication-Info{}: x=a\r\n", " ".repeat(MAX_AUTH_BYTES));
    assert_response_error(&p, &response(&oversized_name), "digest_response_auth_invalid");
}

#[test]
fn header_envelope_is_bounded_and_body_never_supplies_authentication_info() {
    let p = proof(false, false);
    let prefix = "SIP/2.0 200 OK\r\nX-Pad: ";
    let exact = format!("{prefix}{}\r\n\r\n", "a".repeat(MAX_HEADER_BYTES - prefix.len() - 4));
    assert_eq!(exact.len(), MAX_HEADER_BYTES);
    p.verify_response(exact.as_bytes()).unwrap();
    assert_response_error(&p, exact.replace("X-Pad: ", "X-Pad: a").as_bytes(), "digest_response_auth_invalid");
    let at_line_limit = (0..MAX_HEADER_LINES).map(|_| "X-Pad: a\r\n").collect::<String>();
    p.verify_response(&response(&at_line_limit)).unwrap();
    let many_lines = format!("{at_line_limit}X-Pad: a\r\n");
    assert_response_error(&p, &response(&many_lines), "digest_response_auth_invalid");
    let mut frame = response("");
    frame.extend_from_slice(b"Authentication-Info: rspauth=wrong\r\n\r\n");
    frame.extend(std::iter::repeat_n(0xff, MAX_HEADER_BYTES * 2));
    assert!(!p.verify_response(&frame).unwrap().rspauth_verified());
    let lf = format!("SIP/2.0 200 OK\nAuthentication-Info: rspauth={RFC_RSPAUTH_NO_QOP}\n\n");
    assert!(p.verify_response(lf.as_bytes()).unwrap().rspauth_verified());
}

#[test]
fn constructor_rejects_empty_invalid_context_and_preserves_algorithm_allowlist() {
    for algorithm in ["SHA-256", "MD5-sess", "AKAv1-SHA-256", "AKAv2-SHA-512", "unknown"] {
        let c = DigestChallenge { algorithm: algorithm.into(), ..challenge(true, false) };
        assert_eq!(DigestRequestProof::new(&c, "u", b"p", "sip:r", "c", "00000001")
            .unwrap_err().code(), "digest_algorithm_unsupported");
    }
    let c = DigestChallenge { qop: Some("auth-int".into()), ..challenge(true, false) };
    assert_eq!(DigestRequestProof::new(&c, "u", b"p", "sip:r", "c", "00000001")
        .unwrap_err().code(), "digest_qop_unsupported");
    for c in [DigestChallenge { realm: String::new(), ..challenge(true, false) },
        DigestChallenge { nonce: String::new(), ..challenge(true, false) }]
    {
        assert_eq!(DigestRequestProof::new(&c, "u", b"p", "sip:r", "c", "00000001")
            .unwrap_err().code(), "digest_request_auth_mismatch");
    }
    for (username, uri, cnonce) in [("", "sip:r", "c"), ("u", "", "c"), ("u", "sip:r", ""),
        ("u", "sip:r injected", "c"), ("u\n", "sip:r", "c")]
    {
        assert_eq!(DigestRequestProof::new(&challenge(true, false), username, b"p", uri, cnonce,
            "00000001").unwrap_err().code(), "digest_request_auth_mismatch");
    }
}

#[test]
fn debug_is_redacted_without_scope_uri_identity_nonce_or_digest() {
    let p = proof(true, true);
    let verified = p.verify_response(&response("Proxy-Authentication-Info: nextnonce=secret-next\r\n"))
        .unwrap();
    assert_eq!(format!("{p:?}"), "DigestRequestProof { [REDACTED] }");
    assert_eq!(format!("{verified:?}"), "VerifiedAuthenticationInfo { [REDACTED] }");
    assert_eq!(format!("{:#?}", p.clone()), "DigestRequestProof { [REDACTED] }");
    assert_eq!(format!("{:#?}", verified.clone()), "VerifiedAuthenticationInfo { [REDACTED] }");
}
