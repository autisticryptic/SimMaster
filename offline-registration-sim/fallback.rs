//! Operator-independent fallback regressions. All identities use synthetic 001/01.
//! The peer checks actual REGISTER bytes; production adapters decide every retry.
use super::*;

const HINT_AES: &str = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans;spi-c=7001;spi-s=7002;port-c=5070;port-s=5072";
const CHALLENGE_AES: &str = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans;spi-c=20001;spi-s=20002;port-c=6002;port-s=6003";

fn enabled(scenario: Scenario) -> bool {
    scenario.mode.starts_with("fallback_")
}
fn explicit_requirement(mode: &str) -> bool {
    matches!(mode, "fallback_require" | "fallback_proxy_require" | "fallback_494"
        | "fallback_required_policy" | "fallback_dynamic_identity"
        | "fallback_hint_421" | "fallback_hint_494" | "fallback_required_missing"
        | "fallback_required_missing_proxy" | "fallback_494_missing" | "fallback_494_missing_proxy"
        | "fallback_required_direct200" | "fallback_494_direct200")
}
fn tag(frame: &[u8], name: &str, value: &str) -> bool {
    sip_frame::header_values(frame, name).iter().any(|line|
        line.split(',').any(|token| token.trim().eq_ignore_ascii_case(value)))
}
fn authenticated(frame: &[u8]) -> bool {
    ["Authorization", "Proxy-Authorization"].iter().any(|name|
        header(frame, name).is_some_and(|v| parameters(&v).get("response").is_some_and(|v| !v.is_empty())))
}

// Explicit policy variants, not PLMN/carrier branches. The unmodified production
// derived profile is used by every other case, including both bare-421 successes.
pub(super) fn profile(scenario: Scenario, base: &'static CarrierProfile) -> &'static CarrierProfile {
    if !matches!(scenario.mode, "fallback_required_policy" | "fallback_disabled" | "fallback_dynamic_identity") {
        return base;
    }
    let mut profile = *base;
    match scenario.mode {
        "fallback_required_policy" => profile.ims.register.sec_agree_mode = "required",
        "fallback_disabled" => {
            profile.ims.register.sec_agree_mode = "disabled";
            profile.ims.register.require_sec_agree_headers = false;
            profile.ims.register.proxy_require_sec_agree_headers = false;
        }
        "fallback_dynamic_identity" => {
            profile.ims.register.initial_authorization = "none";
        }
        _ => unreachable!(),
    }
    Box::leak(Box::new(profile))
}

impl Peer {
    pub(super) fn proxy_challenge(&self) -> bool {
        matches!(self.scenario.mode, "proxy" | "hint_proxy" | "hint_proxy_changed" | "fallback_proxy"
            | "fallback_required_missing_proxy" | "fallback_494_missing_proxy")
    }
    pub(super) fn fallback_challenge_headers(&self) -> String {
        if !enabled(self.scenario) || matches!(self.scenario.mode, "fallback_no_security"
            | "fallback_required_missing" | "fallback_required_missing_proxy"
            | "fallback_494_missing" | "fallback_494_missing_proxy") {
            return String::new();
        }
        let mut offer = CHALLENGE_AES.to_string();
        if self.scenario.mode == "fallback_challenge_null" {
            offer = offer.replace("ealg=aes-cbc", "ealg=null");
        }
        if self.scenario.mode == "fallback_bad_challenge" {
            offer = offer.replace("alg=hmac-sha-1-96", "alg=hmac-md5-96");
        }
        format!("Security-Server: {offer}\r\n")
    }

    /// Return true only when this synthetic peer already replied. Authenticated
    /// requests continue through the shared independent Digest-AKA verifier.
    pub(super) fn fallback_request(&mut self, frame: &[u8], auth: bool, expires: u32) -> bool {
        if !enabled(self.scenario) { return false; }
        let mode = self.scenario.mode;
        let first = self.sent.len() == 1;
        let first_frame = &self.sent[0];
        assert_eq!(auth, authenticated(frame));
        for name in ["From", "To", "Call-ID", "P-Preferred-Identity"] {
            assert_eq!(header(frame, name), header(first_frame, name), "identity changed: {name}");
        }
        assert_eq!(frame.split(|b| *b == b'\r').next(), first_frame.split(|b| *b == b'\r').next());
        assert!(header(frame, "P-Visited-Network-ID").is_none());
        let deferred_identity = mode == "fallback_dynamic_identity" && self.sent.len() <= 2;
        if !auth && !deferred_identity {
            let authorization = header(frame, "Authorization").expect("fallback lost empty AKA identity");
            assert!(authorization.starts_with("Digest uri="), "empty AKA must remain URI-first");
            let fields = parameters(&authorization);
            for (name, expected) in [("username", self.user.as_str()), ("realm", self.realm.as_str()),
                ("uri", self.uri.as_str()), ("nonce", ""), ("response", "")] {
                assert_eq!(fields.get(name).map(String::as_str), Some(expected), "empty AKA field {name}");
            }
        }
        if deferred_identity { assert!(header(frame, "Authorization").is_none()); }
        if self.sent.len() == 2 && !explicit_requirement(mode) && mode != "fallback_disabled" {
            // The historical generic envelope changes ONLY the proactive
            // Require/Proxy-Require declarations, not identity or capability.
            for name in ["Authorization", "Supported", "Contact", "P-Access-Network-Info", "Route"] {
                assert_eq!(header(frame, name), header(first_frame, name), "generic changed {name}");
            }
        }
        if mode == "fallback_disabled" {
            assert!(header(frame, "Security-Client").is_none());
            assert!(!tag(frame, "Require", "sec-agree"));
        } else {
            assert!(tag(frame, "Supported", "sec-agree"));
            let offered = header(frame, "Security-Client").unwrap();
            let singleton = !first && matches!(mode, "fallback_hint_421" | "fallback_hint_494");
            if singleton {
                assert!(!offered.contains(','));
                assert!(offered.contains("ealg=aes-cbc"));
                assert!(!offered.contains("ealg=null"));
                assert!(offered.contains("spi-c=10001"));
                assert_eq!(Some(offered), header(&self.sent[1], "Security-Client"), "reoffer binding changed");
            } else {
                assert_eq!(Some(offered.clone()), header(first_frame, "Security-Client"), "bare offer narrowed or rebound");
                assert!(offered.contains("ealg=aes-cbc") && offered.contains("ealg=null"));
            }
            let required = first || explicit_requirement(mode);
            assert_eq!(tag(frame, "Require", "sec-agree"), required, "Require lost or invented");
            assert_eq!(tag(frame, "Proxy-Require", "sec-agree"), required, "Proxy-Require lost or invented");
        }
        if auth {
            if mode == "fallback_no_security" {
                assert!(header(frame, "Security-Verify").is_none(), "421 offer promoted to Security-Verify");
            } else {
                let verify = header(frame, "Security-Verify").expect("challenge selection missing");
                assert!(!verify.contains("spi-c=7001"), "421 offer promoted to Security-Verify");
                let expected = if mode == "fallback_challenge_null" {
                    CHALLENGE_AES.replace("ealg=aes-cbc", "ealg=null")
                } else { CHALLENGE_AES.to_string() };
                assert_eq!(verify, expected, "only 401/407 may bind Security-Verify");
            }
            return false;
        }
        assert!(header(frame, "Security-Verify").is_none());
        if first {
            let (status, required) = match mode {
                "fallback_initial_403" => (403, ""),
                "fallback_require" | "fallback_hint_421" | "fallback_dynamic_identity"
                    | "fallback_required_missing" | "fallback_required_missing_proxy"
                    | "fallback_required_direct200" => (421, "Require: sec-agree\r\n"),
                "fallback_proxy_require" => (421, "Proxy-Require: sec-agree\r\n"),
                "fallback_494" | "fallback_hint_494" | "fallback_disabled"
                    | "fallback_494_missing" | "fallback_494_missing_proxy" | "fallback_494_direct200" => (494, ""),
                "fallback_unknown_require" => (421, "Require: sec-agree, fixture-required\r\n"),
                "fallback_unknown_proxy" => (421, "Require: sec-agree\r\nProxy-Require: fixture-required\r\n"),
                "fallback_unknown_494" => (494, "Proxy-Require: fixture-required\r\n"),
                "fallback_required_policy" => (415, ""),
                _ => (421, ""),
            };
            let no_offer = matches!(mode, "fallback_require" | "fallback_proxy_require" | "fallback_494"
                | "fallback_required_policy" | "fallback_dynamic_identity" | "fallback_initial_403"
                | "fallback_required_missing" | "fallback_required_missing_proxy"
                | "fallback_494_missing" | "fallback_494_missing_proxy"
                | "fallback_required_direct200" | "fallback_494_direct200");
            let mut extra = required.to_string();
            if !no_offer {
                let offer = match mode {
                    "fallback_hint_null" => HINT_AES.replace("ealg=aes-cbc", "ealg=null"),
                    "fallback_hint_unknown" => HINT_AES.replace("alg=hmac-sha-1-96", "alg=hmac-md5-96"),
                    _ => HINT_AES.to_string(),
                };
                extra.push_str(&format!("Security-Server: {offer}\r\n"));
            }
            self.reply(frame, status, &extra);
        } else if matches!(mode, "fallback_required_direct200" | "fallback_494_direct200") {
            self.reply(frame, 200, "Expires: 3600\r\n");
        } else if mode == "fallback_generic_403" {
            self.reply(frame, 403, "");
        } else if mode == "fallback_repeat" {
            self.reply(frame, 421, &format!("Security-Server: {HINT_AES}\r\n"));
        } else if mode == "fallback_dynamic_identity" && deferred_identity {
            self.reply(frame, 400, "");
        } else if matches!(mode, "fallback_require" | "fallback_proxy_require" | "fallback_494"
            | "fallback_required_policy" | "fallback_dynamic_identity") && header(frame, "P-Access-Network-Info").is_some() {
            // 415 avoids the separate 400-driven formatting ladder. The next
            // STATIC no-PANI candidate must carry the already learned state.
            self.reply(frame, 415, "");
        } else if mode == "fallback_min_pre" && expires < 7200 {
            self.reply(frame, 423, "Min-Expires: 7200\r\n");
        } else {
            self.challenge(frame);
        }
        true
    }

    pub(super) fn fallback_authenticated(&mut self, frame: &[u8], expires: u32) -> bool {
        if !enabled(self.scenario) { return false; }
        match self.scenario.mode {
            "fallback_auth_403" => self.reply(frame, 403, ""),
            "fallback_auth_bound" => self.challenge(frame),
            "fallback_min_post" if expires < 7200 => self.reply(frame, 423, "Min-Expires: 7200\r\n"),
            "fallback_min_bound" => self.reply(frame, 423, &format!("Min-Expires: {}\r\n", expires + 3600)),
            _ => self.reply(frame, 200, &format!("Expires: {expires}\r\n")),
        }
        true
    }
}

pub(super) fn assert_case(peer: &Peer, candidates: &[&str], rounds: u8) {
    if !enabled(peer.scenario) { return; }
    let mode = peer.scenario.mode;
    let (statuses, count, proofs, expected_rounds): (&[u16], usize, usize, u8) = match mode {
        "fallback_initial_403" => (&[403], 1, 0, 0),
        "fallback_unknown_require" | "fallback_unknown_proxy" => (&[421], 1, 0, 0),
        "fallback_unknown_494" | "fallback_disabled" => (&[494], 1, 0, 0),
        "fallback_generic_403" => (&[421, 403], 2, 0, 0),
        "fallback_auth_403" => (&[421, 401, 403], 2, 1, 1),
        "fallback_bad_challenge" => (&[421, 401], 2, 0, 1),
        "fallback_require" | "fallback_proxy_require" => (&[421, 415, 401, 200], 3, 1, 1),
        "fallback_494" => (&[494, 415, 401, 200], 3, 1, 1),
        "fallback_required_policy" => (&[415, 401, 200], 2, 1, 1),
        "fallback_required_missing" => (&[421, 401], 2, 0, 1),
        "fallback_required_missing_proxy" => (&[421, 407], 2, 0, 1),
        "fallback_494_missing" => (&[494, 401], 2, 0, 1),
        "fallback_494_missing_proxy" => (&[494, 407], 2, 0, 1),
        "fallback_required_direct200" => (&[421, 200], 2, 0, 0),
        "fallback_494_direct200" => (&[494, 200], 2, 0, 0),
        "fallback_dynamic_identity" => (&[421, 400, 415, 401, 200], 4, 1, 1),
        "fallback_proxy" => (&[421, 407, 200], 2, 1, 1),
        "fallback_hint_494" => (&[494, 401, 200], 2, 1, 1),
        "fallback_repeat" => (&[421, 421, 421, 421], 4, 0, 0),
        "fallback_min_pre" => (&[421, 423, 401, 200], 2, 1, 1),
        "fallback_min_post" => (&[421, 401, 423, 200], 2, 2, 1),
        "fallback_min_bound" => (&[421, 401, 423, 423, 423], 2, 3, 1),
        "fallback_auth_bound" => (&[421, 401, 401, 401], 2, 2, 2),
        _ => (&[421, 401, 200], 2, 1, 1),
    };
    assert_eq!(peer.statuses, statuses, "unexpected response path: {mode}");
    assert_eq!(peer.sent.len(), statuses.len(), "unexpected retransmit/retry: {mode}");
    assert_eq!(candidates.len(), count, "candidate bound: {mode}");
    assert!(candidates.len() <= 24);
    assert_eq!(rounds, expected_rounds);
    assert_eq!(peer.digest_verified, proofs);
    assert_eq!(candidates[0], "standard_3gpp_conservative");
    if matches!(mode, "fallback_hint_421" | "fallback_hint_494") {
        assert_eq!(candidates[1], "standard_3gpp_security_hint_reoffer");
    } else {
        assert!(!candidates.contains(&"standard_3gpp_security_hint_reoffer"), "bare 421 must not select a cipher");
        if count > 1 && !explicit_requirement(mode) {
            assert_eq!(candidates[1], "generic_ims_register_fallback");
        }
    }
    if mode == "fallback_repeat" {
        assert_eq!(candidates, &["standard_3gpp_conservative", "generic_ims_register_fallback",
            "generic_ims_register_no_pani", "generic_ims_register_no_instance"]);
    }
    // Within each real driver exchange, 401/407 and 423 increment CSeq; a new
    // candidate starts at 1. A 423 must not change identity or security binding.
    for index in 1..peer.sent.len() {
        let previous = &peer.sent[index - 1];
        let current = &peer.sent[index];
        let cseq = |frame: &[u8]| header(frame, "CSeq").unwrap().split_whitespace().next().unwrap().parse::<u32>().unwrap();
        let previous_status = peer.statuses[index - 1];
        if matches!(previous_status, 401 | 407 | 423) {
            assert_eq!(cseq(current), cseq(previous) + 1);
        } else { assert_eq!(cseq(current), 1); }
        if previous_status == 423 {
            for name in ["From", "To", "Call-ID", "Supported", "Require", "Proxy-Require", "Security-Client",
                "Security-Verify", "P-Access-Network-Info", "P-Preferred-Identity"] {
                assert_eq!(header(current, name), header(previous, name), "423 changed {name}");
            }
            assert_eq!(authenticated(current), authenticated(previous));
            let contact_binding = |frame: &[u8]| header(frame, "Contact").unwrap()
                .split(';').filter(|part| !part.starts_with("expires="))
                .map(str::to_string).collect::<Vec<_>>();
            assert_eq!(contact_binding(current), contact_binding(previous), "423 changed Contact binding");
            if !authenticated(current) { assert_eq!(header(current, "Authorization"), header(previous, "Authorization")); }
            assert_eq!(header(current, "Expires").unwrap().parse::<u32>().unwrap(),
                header(previous, "Expires").unwrap().parse::<u32>().unwrap() + 3600);
        }
    }
}

#[tokio::test]
async fn offline_global_register_fallback_matrix() {
    let cases = [
        ("bare421_offer_generic_401", "fallback_aes", true),
        ("bare421_offer_generic_407", "fallback_proxy", true),
        ("bare421_does_not_narrow_null_challenge", "fallback_challenge_null", true),
        ("bare421_null_offer_does_not_select", "fallback_hint_null", true),
        ("bare421_unknown_offer_does_not_select", "fallback_hint_unknown", true),
        ("ordinary_403_stops", "fallback_initial_403", false),
        ("generic_403_stops", "fallback_generic_403", false),
        ("authenticated_403_never_changes_candidate", "fallback_auth_403", false),
        ("unoffered_challenge_stops", "fallback_bad_challenge", false),
        ("bare421_never_becomes_security_verify", "fallback_no_security", true),
        ("421_require_survives_static_fallbacks", "fallback_require", true),
        ("421_proxy_require_survives_static_fallbacks", "fallback_proxy_require", true),
        ("494_survives_static_fallbacks", "fallback_494", true),
        ("421_unknown_require_stops", "fallback_unknown_require", false),
        ("421_unknown_proxy_require_stops", "fallback_unknown_proxy", false),
        ("494_unknown_extension_stops", "fallback_unknown_494", false),
        ("explicit421_offer_single_aes_reoffer", "fallback_hint_421", true),
        ("explicit494_offer_single_aes_reoffer", "fallback_hint_494", true),
        ("repeated_bare421_candidate_bound", "fallback_repeat", false),
        ("generic_423_before_aka_preserves_shape", "fallback_min_pre", true),
        ("generic_423_after_aka_preserves_shape", "fallback_min_post", true),
        ("generic_423_authenticated_bound", "fallback_min_bound", false),
        ("generic_aka_round_bound", "fallback_auth_bound", false),
        ("configured_required_policy_survives_static", "fallback_required_policy", true),
        ("disabled_policy_cannot_be_overridden", "fallback_disabled", false),
        ("dynamic_identity_supported_survive_static", "fallback_dynamic_identity", true),
        ("421_requirement_rejects_missing_security_401", "fallback_required_missing", false),
        ("421_requirement_rejects_missing_security_407", "fallback_required_missing_proxy", false),
        ("494_requirement_rejects_missing_security_401", "fallback_494_missing", false),
        ("494_requirement_rejects_missing_security_407", "fallback_494_missing_proxy", false),
        ("421_requirement_rejects_plain_200", "fallback_required_direct200", false),
        ("494_requirement_rejects_plain_200", "fallback_494_direct200", false),
    ];
    let mut results = Vec::new();
    for (id, mode, expected) in cases {
        let result = run_case_on_network(Scenario { id, wifi: false, mode, expected, fallbacks: true }, "001", "01", true).await;
        eprintln!("REGISTER_FALLBACK_SIMULATION {id} passed={} registered={} requests={}",
            result["passed"], result["observed_success"], result["request_count"]);
        results.push(result);
    }
    let passed = results.iter().all(|r| r["passed"] == true);
    let report = json!({
        "suite_id": "simadmin-offline-global-register-fallback-v1", "evidence_kind": "offline_simulation",
        "passed": passed, "scenarios": results, "hardware_used": false, "live_network_verified": false,
        "scope": "operator-independent 001/01 fixtures; real derivation, builders, fallback state, shared REGISTER driver and independently verified Digest-AKA",
        "wire_assertions": ["empty AKA identity and Supported survive", "only proactive Require is withdrawn",
            "required extensions survive static fallback", "421 offers cannot bind Security-Verify or narrow algorithms",
            "401/407 challenge binds security", "423 preserves shape and advances CSeq", "candidate and authentication bounds"],
        "limitations": ["No carrier certification or catalog pruning evidence", "No real SIM, socket, modem, XFRM, bearer or live network verification"]
    });
    if let Ok(path) = std::env::var("SIMADMIN_REGISTER_FALLBACK_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(passed, "one or more global REGISTER fallback regressions failed");
}
