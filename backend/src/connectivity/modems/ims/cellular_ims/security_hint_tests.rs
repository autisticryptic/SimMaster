use super::*;
use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};

fn profile() -> &'static CarrierProfile {
    derive_standard_3gpp_profile("460", "02", Standard3gppAccess::LteEpc).unwrap()
}
fn offer(alg: &str, enc: &str) -> String {
    format!("ipsec-3gpp;alg={alg};ealg={enc};prot=esp;mod=trans;spi-c=7001;spi-s=7002;port-c=5070;port-s=5072")
}
fn failure(status: u16, extra: &str) -> RegisterFailure {
    RegisterFailure { error: ImsError::new("ims_register_initial_unexpected_status"),
        response: Some(format!("SIP/2.0 {status} Response\r\n{extra}Content-Length: 0\r\n\r\n").into_bytes()), auth_rounds: 0 }
}
fn hinted(alg: &str, enc: &str) -> RegisterFailure {
    failure(421, &format!("Security-Server: {}\r\nWarning: 399 network.invalid \"Security negotiation\"\r\n", offer(alg, enc)))
}
fn retry() -> CellularImsRegisterVariant {
    let p = profile();
    match decide(p, register_variants(p)[0], &hinted("hmac-sha-1-96", "aes-cbc")) {
        Decision::Retry(v) => v,
        other => panic!("{other:?}"),
    }
}
#[test]
fn cmcc_shaped_421_without_require_preserves_identity_and_declarations() {
    let p = profile(); let original = register_variants(p)[0]; let next = retry();
    assert_eq!(next.security_mechanism, Some(0));
    assert_eq!(next.authorization, original.authorization);
    assert_eq!(next.policy, original.policy);
    assert_eq!(next.security_client_offer, original.security_client_offer);
    assert!(next.server_required_sec_agree);
    assert!(original.security_mechanism.is_none());
    assert_eq!(CELLULAR_IMS_REGISTER_CANDIDATE_LIMIT, 24);
    for status in [421, 494] {
        let f = failure(status, &format!("Security-Server: {}\r\n", offer("hmac-sha1-96", "aes-cbc")));
        assert!(matches!(decide(p, original, &f), Decision::Retry(_)));
    }
}
#[test]
fn hint_reoffer_uses_only_local_mechanism_and_reserved_tuple() {
    let p = profile(); let v = retry();
    let local = SecAgree { spi_c: 10001, spi_s: 10002, port_c: 5062, port_s: 5063 };
    let full = register_variants(p)[0].build_security_offer(local, p).unwrap();
    let single = v.build_security_offer(local, p).unwrap();
    assert!(full.contains(',')); assert!(!single.contains(','));
    assert!(single.contains("ealg=aes-cbc")); assert!(!single.contains("ealg=null"));
    assert_eq!(ipsec::parse_security_server(&single).unwrap(), local);
    assert!(!single.contains("7001")); assert!(!single.contains("5070"));
    // The shared profile is unchanged for other connections/P-CSCFs.
    assert_eq!(register_variants(p)[0].build_security_offer(local, p).unwrap(), full);
}
#[test]
fn untrusted_hint_cannot_select_null_or_add_md5_policy() {
    let p = profile(); let v = register_variants(p)[0];
    for (alg, enc) in [("hmac-sha-1-96", "null"), ("hmac-md5-96", "null"), ("hmac-md5-96", "aes-cbc")] {
        assert!(matches!(decide(p, v, &hinted(alg, enc)), Decision::Stop(_)));
    }
    // Existing normal full-offer/null challenge policy is not removed.
    assert!(select_security_server(p, &[offer("hmac-sha-1-96", "null")]).unwrap().is_some());
}
#[test]
fn malformed_incomplete_or_unknown_hint_never_falls_through_to_generic() {
    let p = profile(); let v = register_variants(p)[0];
    for value in ["tls;q=0.5".to_string(), "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc".into(),
        format!("{};spi-c=10", offer("hmac-sha-1-96", "aes-cbc")),
        offer("hmac-sha-1-96", "aes-cbc").replace("port-c=5070", "port-c=0"),
        format!("{};x=\"unterminated", offer("hmac-sha-1-96", "aes-cbc")),
        format!("{};q=0.8, {};q=0.8", offer("hmac-sha-1-96", "aes-cbc"), offer("hmac-sha-1-96", "null")),
    ] {
        assert!(matches!(decide(p, v, &failure(421, &format!("Security-Server: {value}\r\n"))), Decision::Stop(_)), "{value}");
    }
}
#[test]
fn balanced_but_invalid_extension_values_cannot_authorize_hint_retry() {
    let p = profile(); let v = register_variants(p)[0];
    for extension in ["x=bad\"value\"", "x=\"one\"\"two\"", "x=\"one\"tail", "x=unquoted space", "x="] {
        let header = format!("Security-Server: {};{extension}\r\n", offer("hmac-sha-1-96", "aes-cbc"));
        assert!(matches!(decide(p, v, &failure(421, &header)), Decision::Stop(_)), "{extension}");
    }
    for extension in ["x=token", "x=\"a,b;c\"", "x=[2001:db8::1]", "flag"] {
        let header = format!("Security-Server: {};{extension}\r\n", offer("hmac-sha-1-96", "aes-cbc"));
        assert!(matches!(decide(p, v, &failure(421, &header)), Decision::Retry(_)), "{extension}");
    }
}

#[test]
fn unrelated_status_challenge_or_extensions_cannot_authorize_new_retry() {
    let p = profile(); let v = register_variants(p)[0];
    let header = format!("Security-Server: {}\r\n", offer("hmac-sha-1-96", "aes-cbc"));
    for status in [400, 401, 403, 407, 420, 500] {
        assert_eq!(decide(p, v, &failure(status, &header)), Decision::NotApplicable);
    }
    for extra in ["Require: sec-agree, unknown\r\n", "Proxy-Require: other\r\n", "WWW-Authenticate: Digest nonce=\"test\"\r\n"] {
        assert!(matches!(decide(p, v, &failure(421, &(header.clone()+extra))), Decision::Stop(_)));
    }
    let mut f = failure(421, &header); f.auth_rounds = 1;
    assert_eq!(decide(p, v, &f), Decision::NotApplicable);
    assert_eq!(decide(p, v, &failure(421, "")), Decision::NotApplicable);
    let mut disabled = *p; disabled.ims.register.sec_agree_mode = "disabled";
    assert!(matches!(decide(&disabled, v, &failure(421, &header)), Decision::Stop(_)));
}
#[test]
fn single_offer_and_repeat_hint_stop_instead_of_looping_or_downgrading() {
    let p = profile(); let v = retry();
    for f in [hinted("hmac-sha-1-96", "aes-cbc"), failure(403, ""), failure(400, ""),
        RegisterFailure { error: ImsError::new("ims_register_initial_receive_failed"), response: None, auth_rounds: 0 }] {
        assert!(matches!(decide(p, v, &f), Decision::Stop(_)));
    }
    let mut single = *p; single.ims.register.security_client_mechanisms = &["hmac-sha-1-96/aes-cbc/esp/trans"];
    assert!(matches!(decide(&single, register_variants(&single)[0], &hinted("hmac-sha-1-96", "aes-cbc")), Decision::Stop(_)));
}
#[test]
fn narrowed_challenge_is_strict_and_verify_comes_from_challenge_not_hint() {
    let p = profile();
    let values = vec!["tls;q=0.9".to_string(), format!("{};q=0.4", offer("hmac-sha1-96", "aes-cbc"))];
    let selected = select_security_server_for_mechanism(p, &values, Some(0)).unwrap().unwrap();
    assert_eq!(selected.verify, values.join(", "));
    assert!(select_security_server_for_mechanism(p, &[offer("hmac-sha-1-96", "null")], Some(0)).is_err());
    assert!(select_security_server_for_mechanism(p, &values, Some(99)).is_err());
    // Caller must reject a missing challenge offer on an unprotected hint flow.
    assert!(select_security_server_for_mechanism(p, &[], Some(0)).unwrap().is_none());
}
#[test]
fn other_profiles_and_no_hint_legacy_escalation_are_unchanged() {
    let p = crate::connectivity::modems::ims::vowifi::profiles::GB_EE_23433;
    assert_eq!(decide(&p, register_variants(&p)[0], &hinted("hmac-sha-1-96", "aes-cbc")), Decision::NotApplicable);
    let p = profile(); let generic = register_variants(p)[1];
    let f = failure(421, "Require: sec-agree\r\n");
    assert_eq!(decide(p, generic, &f), Decision::NotApplicable);
    assert!(next_dynamic_register_variant(p, generic, &f).is_some());
    assert!(!pre_authentication_variant_failure(&failure(403, "")));
}
