//! Challenge/stale regression cases use synthetic data only, no UICC or network.
use super::*;

// Existing two-argument cases model an original, just-authenticated nonce.
// Provenance-changing nextnonce cases below pass the origin explicitly.
fn challenge_action(incoming: &DigestChallenge, previous: Option<&DigestChallenge>) -> Result<ChallengeAction, ImsError> {
    let origin = previous.and_then(|value| decode_aka_nonce(&value.nonce).ok());
    super::challenge_action(incoming, previous, origin.as_ref())
}

fn nonce(vector: u8, server_data: u8) -> String {
    let mut raw = vec![vector; 32];
    raw.push(server_data);
    BASE64_STANDARD.encode(raw)
}
fn challenge(vector: u8, server_data: u8, stale: bool) -> DigestChallenge {
    parse_digest_challenge(&format!(
        "Digest realm=\"ims.example\",nonce=\"{}\",algorithm=AKAv1-MD5,qop=\"auth\",opaque=\"server-tag\",stale={stale}",
        nonce(vector, server_data)), false).unwrap()
}

#[test]
fn digest_stale_parser_preserves_flag_but_never_echoes_it_as_authorization() {
    let base = "Digest realm=\"ims.example\",nonce=\"nonce\",algorithm=AKAv1-MD5";
    assert!(!parse_digest_challenge(base, false).unwrap().stale);
    for value in ["true", "TRUE", "\"true\"", " \"TrUe\""] {
        let parsed = parse_digest_challenge(&format!("{base},stale={value}"), true).unwrap();
        assert!(parsed.stale && parsed.proxy);
        let header = build_authorization_header(&parsed, "subscriber", "sip:ims.example", "1234", "client", "00000001");
        assert!(!header.contains("stale="));
    }
    for value in ["false", "FALSE", "\"false\""] {
        assert!(!parse_digest_challenge(&format!("{base},stale={value}"), false).unwrap().stale);
    }
}

#[test]
fn digest_stale_and_duplicate_parameters_fail_closed() {
    let base = "Digest realm=\"ims.example\",nonce=\"nonce\"";
    for suffix in [",stale=yes", ",stale=1", ",stale=", ",stale=\"\"", ",stale=true,STALE=false",
        ",realm=\"other\"", ",nonce=\"other\"", ",algorithm=MD5,algorithm=AKAv1-MD5",
        ",stale=\"true", ",stale=true,", ",stale=true junk", ",stale=true\r\nX: injected"] {
        assert!(parse_digest_challenge(&format!("{base}{suffix}"), false).is_err(), "{suffix:?}");
    }
}

#[test]
fn digest_parameter_quoting_is_utf8_safe_and_rejects_ambiguous_tails() {
    let parsed = parse_digest_challenge("Digest realm = \"ims.example\", nonce = \"n\", opaque = \"caf\u{e9},\\\"x\\\\y\"", false).unwrap();
    assert_eq!(parsed.opaque.as_deref(), Some("caf\u{e9},\"x\\y"));
    for value in ["Digest realm=\"r\",nonce=\"n\"oops", "Digest realm=\"r\",nonce=\"n\\", "Digest realm=\"r\",nonce=\"\""] {
        assert!(parse_digest_challenge(value, false).is_err());
    }
    let enormous = format!("Digest realm=\"r\",nonce=\"{}\"", "x".repeat(16 * 1024));
    assert!(parse_digest_challenge(&enormous, false).is_err());
    let too_many = (0..64).fold("Digest realm=\"r\",nonce=\"n\"".to_string(), |s, n| format!("{s},p{n}=v"));
    assert!(parse_digest_challenge(&too_many, false).is_err());
}

#[test]
fn digest_stale_reuses_credentials_only_for_unchanged_aka_vector() {
    let previous = challenge(1, 1, false);
    assert_eq!(challenge_action(&challenge(1, 2, true), Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    assert_eq!(challenge_action(&challenge(2, 2, true), Some(&previous)).unwrap(), ChallengeAction::Authenticate);
    assert_eq!(challenge_action(&challenge(1, 2, false), Some(&previous)).unwrap(), ChallengeAction::Authenticate);
    let mut opaque_previous = previous.clone();
    opaque_previous.nonce = "opaque-nextnonce".into();
    assert_eq!(challenge_action(&challenge(2, 2, true), Some(&opaque_previous)).unwrap(), ChallengeAction::Authenticate);
}

#[test]
fn digest_stale_rejects_missing_context_same_nonce_and_scope_changes() {
    let previous = challenge(1, 1, false);
    assert_eq!(challenge_action(&challenge(1, 2, true), None).unwrap_err().code(), "digest_stale_without_context");
    assert_eq!(challenge_action(&challenge(1, 1, true), Some(&previous)).unwrap_err().code(), "digest_stale_nonce_unchanged");
    for change in 0..4 {
        let mut incoming = challenge(1, 2, true);
        match change {
            0 => incoming.realm = "other.example".into(),
            1 => incoming.proxy = true,
            2 => incoming.algorithm = "MD5".into(),
            _ => incoming.qop = None,
        }
        assert_eq!(challenge_action(&incoming, Some(&previous)).unwrap_err().code(), "digest_stale_scope_changed");
    }
}

#[test]
fn digest_stale_plain_digest_remains_same_scope_and_algorithm() {
    let mut previous = challenge(1, 1, false);
    previous.algorithm = "MD5".into(); previous.nonce = "opaque-old".into();
    let mut incoming = previous.clone(); incoming.stale = true; incoming.nonce = "opaque-new".into();
    assert_eq!(challenge_action(&incoming, Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    incoming.algorithm = "AKAv1-MD5".into();
    assert!(challenge_action(&incoming, Some(&previous)).is_err());
}

#[test]
fn digest_nextnonce_cannot_replace_credential_producing_aka_vector() {
    let authenticated = challenge(1, 1, false);
    let origin = decode_aka_nonce(&authenticated.nonce).unwrap();
    let last_request = challenge(2, 1, false); // server-issued nextnonce, no AKA
    assert_eq!(super::challenge_action(&challenge(2, 2, true), Some(&last_request), Some(&origin)).unwrap(), ChallengeAction::Authenticate);
    assert_eq!(super::challenge_action(&challenge(1, 2, true), Some(&last_request), Some(&origin)).unwrap(), ChallengeAction::ReuseCredentials);
    assert_eq!(super::challenge_action(&challenge(2, 2, true), Some(&last_request), None).unwrap(), ChallengeAction::Authenticate);
}

#[test]
fn digest_stale_flag_does_not_reset_or_grant_a_driver_challenge_budget() {
    // The decision API carries no counters; callers retain the shared engine's
    // bounded rounds. A repeated stale nonce is rejected even with old keys.
    let previous = challenge(1, 1, false);
    let incoming = challenge(1, 2, true);
    assert_eq!(challenge_action(&incoming, Some(&previous)).unwrap(), ChallengeAction::ReuseCredentials);
    assert_eq!(challenge_action(&incoming, Some(&incoming)).unwrap_err().code(), "digest_stale_nonce_unchanged");
}
