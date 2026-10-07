use super::*;
use crate::connectivity::modems::ims::vowifi::profiles::{derive_standard_3gpp_profile, Standard3gppAccess};

fn fixture() -> (CarrierProfile, CellularImsRegisterVariant, CellularImsRegisterVariant) {
    let p = *derive_standard_3gpp_profile("001", "01", Standard3gppAccess::LteEpc).unwrap();
    let variants = register_variants(&p);
    (p, variants[0], variants[1])
}
fn failure(status: u16, headers: &str) -> RegisterFailure {
    RegisterFailure { error: ImsError::new("ims_register_initial_unexpected_status"), auth_rounds: 0,
        response: Some(format!("SIP/2.0 {status} Response\r\n{headers}Content-Length: 0\r\n\r\n").into_bytes()) }
}
const OFFER: &str = "Security-Server: ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;prot=esp;mod=trans;spi-c=7001;spi-s=7002;port-c=5070;port-s=5072\r\n";

#[test]
fn local_proactive_require_and_unrequired_offer_are_not_server_demands() {
    let (p, first, bare) = fixture();
    assert!(first.policy.require_sec_agree && first.policy.proxy_require_sec_agree);
    assert!(!first.server_required_sec_agree);
    for headers in ["", OFFER] {
        let mut state = RegisterFallbackState::new(first);
        assert_eq!(state.observe(&p, first, &failure(421, headers)), Ok(first));
        let next = state.apply(bare);
        assert_eq!(next.authorization, CellularImsInitialAuthorization::UriFirstEmptyAka);
        assert!(next.policy.advertise_sec_agree);
        assert!(!next.policy.require_sec_agree && !next.policy.proxy_require_sec_agree);
        assert!(!next.server_required_sec_agree);
    }
}

#[test]
fn explicit_security_demands_survive_static_fallback_without_rewriting_sent_policy() {
    let (p, _, bare) = fixture();
    for (status, headers) in [(421, "Require: sec-agree\r\n"), (421, "Proxy-Require: SEC-AGREE\r\n"),
        (494, ""), (494, "Require: sec-agree\r\nProxy-Require: sec-agree\r\n")] {
        let mut state = RegisterFallbackState::new(bare);
        let observed = state.observe(&p, bare, &failure(status, headers)).unwrap();
        assert_eq!(observed, CellularImsRegisterVariant { server_required_sec_agree: true, ..bare });
        assert_eq!(state.apply(bare), bare.requiring_sec_agree());
        let require_only = observed.requiring_sec_agree_without_proxy();
        assert_eq!(state.observe(&p, require_only, &failure(400, "")), Ok(require_only));
    }
    let mut required_policy = p;
    required_policy.ims.register.sec_agree_mode = "required";
    let mut state = RegisterFallbackState::new(bare);
    state.observe(&required_policy, bare, &failure(400, "")).unwrap();
    assert_eq!(state.apply(bare), bare.requiring_sec_agree());
    assert_eq!(RegisterFallbackState::new(bare.requiring_sec_agree()).apply(bare), bare.requiring_sec_agree());
}

#[test]
fn added_empty_aka_and_supported_are_sticky_across_static_candidates() {
    let (p, _, mut bare) = fixture();
    bare.authorization = CellularImsInitialAuthorization::None;
    bare.policy.advertise_sec_agree = false;
    let mut state = RegisterFallbackState::new(bare);
    state.observe(&p, bare, &failure(421, "Require: sec-agree\r\n")).unwrap();
    let added = bare.with_empty_aka_authorization();
    assert_eq!(state.observe(&p, added, &failure(400, "")), Ok(added));
    for candidate in [bare, bare.without_access_network_info(), bare.without_sip_instance()] {
        let next = state.apply(candidate);
        assert_eq!(next.authorization, CellularImsInitialAuthorization::UriFirstEmptyAka);
        assert!(next.policy.advertise_sec_agree && next.policy.require_sec_agree);
        assert!(next.policy.proxy_require_sec_agree && next.server_required_sec_agree);
        assert_eq!(next.policy.include_access_network_info, candidate.policy.include_access_network_info);
        assert_eq!(next.policy.include_sip_instance, candidate.policy.include_sip_instance);
    }
}

#[test]
fn post_auth_and_terminal_failures_leave_both_current_and_state_unchanged() {
    let (p, _, bare) = fixture();
    for (status, rounds) in [(421, 1), (494, 2), (403, 0), (401, 0), (407, 0), (500, 0)] {
        for first in [bare, bare.with_empty_aka_authorization()] {
            let mut state = RegisterFallbackState::new(first);
            let before = state;
            let current = bare.with_empty_aka_authorization();
            let mut f = failure(status, "Require: unknown\r\n");
            f.auth_rounds = rounds;
            assert_eq!(state.observe(&p, current, &f), Ok(current));
            assert_eq!(state, before);
        }
    }
}

#[test]
fn disabled_security_stops_only_on_a_requirement() {
    let (mut p, _, bare) = fixture();
    p.ims.register.sec_agree_mode = "disabled";
    let mut state = RegisterFallbackState::new(bare);
    assert_eq!(state.observe(&p, bare, &failure(421, OFFER)), Ok(bare));
    for (status, headers) in [(421, "Require: sec-agree\r\n"), (421, "Proxy-Require: sec-agree\r\n"), (494, "")] {
        let before = state;
        assert_eq!(state.observe(&p, bare, &failure(status, headers)), Err("required_security_disabled"));
        assert_eq!(state, before);
    }
    assert!(state.observe(&p, bare.requiring_sec_agree(), &failure(400, "")).is_err());
}

#[test]
fn unknown_or_empty_required_extensions_stop_without_learning_state() {
    let (p, first, bare) = fixture();
    for status in [421, 494] {
        for headers in ["Require: sec-agree, unknown\r\n", "Proxy-Require: unknown\r\n", "Require: \r\n",
            "Require: sec-agree\r\nRequire: other\r\n", "Require: sec-agree\r\nProxy-Require: other\r\n",
            "Require: sec-agree\r\nRequire: \r\n", "Require: sec-agree,\r\n",
            "Proxy-Require: ,sec-agree\r\n"] {
            let mut state = RegisterFallbackState::new(bare);
            let before = state;
            assert_eq!(state.observe(&p, first, &failure(status, headers)), Err("unsupported_required_extension"));
            assert_eq!(state, before);
        }
    }
}

#[test]
fn new_pcscf_state_is_independent_and_never_changes_profile_or_security_offer() {
    let (p, _, bare) = fixture();
    let original = p;
    let binding = SecAgree { spi_c: 10001, spi_s: 10002, port_c: 5062, port_s: 5063 };
    let full = bare.build_security_offer(binding, &p).unwrap();
    let mut state = RegisterFallbackState::new(bare);
    state.observe(&p, bare.with_empty_aka_authorization(), &failure(494, OFFER)).unwrap();
    assert_eq!(state.apply(bare).build_security_offer(binding, &p).unwrap(), full);
    let restricted = CellularImsRegisterVariant { security_mechanism: Some(0),
        security_client_offer: CellularImsSecurityClientOffer::FullSpaced, ..bare };
    let offer = restricted.build_security_offer(binding, &p).unwrap();
    let observed = state.observe(&p, restricted, &failure(400, "")).unwrap();
    assert_eq!(observed.security_mechanism, restricted.security_mechanism);
    let next = state.apply(restricted);
    assert_eq!(next.security_mechanism, restricted.security_mechanism);
    assert_eq!(next.security_client_offer, restricted.security_client_offer);
    assert_eq!(next.build_security_offer(binding, &p).unwrap(), offer);
    assert_eq!(p, original);
    assert_eq!(RegisterFallbackState::new(bare).apply(bare), bare);
}

#[test]
fn applying_seed_preserves_configured_first_request_and_require_only_policy() {
    let (mut p, _, _) = fixture();
    for mode in ["optional", "required", "disabled"] {
        for proxy in [false, true] {
            for authorization in ["none", "aka_empty"] {
                p.ims.register.sec_agree_mode = mode;
                p.ims.register.proxy_require_sec_agree_headers = proxy;
                p.ims.register.initial_authorization = authorization;
                let primary = register_variants(&p)[0];
                let state = RegisterFallbackState::new(primary);
                assert_eq!(state.apply(primary), primary, "{mode} {proxy} {authorization}");
            }
        }
    }
}

#[test]
fn proxy_only_server_requirement_escalates_current_candidate_before_static_fallback() {
    let (p, _, generic) = fixture();
    let f = failure(421, "Proxy-Require: sec-agree\r\n");
    let mut state = RegisterFallbackState::new(generic);
    let current = state.observe(&p, generic, &f).unwrap();
    assert!(!current.policy.require_sec_agree);
    assert!(current.server_required_sec_agree);
    assert_eq!(next_dynamic_register_variant(&p, current, &f), Some(generic.requiring_sec_agree()));
}

#[test]
fn bare_421_offer_uses_identity_preserving_envelope_without_narrowing_security() {
    let (p, first, generic) = fixture();
    let mut state = RegisterFallbackState::new(first);
    let f = failure(421, OFFER);
    let observed = state.observe(&p, first, &f).unwrap();
    assert_eq!(security_hint::decide(&p, observed, &f), security_hint::Decision::NotApplicable);
    assert!(next_dynamic_register_variant(&p, observed, &f).is_none());
    let next = state.apply(generic);
    assert_eq!(next.authorization, first.authorization);
    assert_eq!(next.policy.advertise_sec_agree, first.policy.advertise_sec_agree);
    assert!(!next.policy.require_sec_agree && !next.policy.proxy_require_sec_agree);
    let binding = SecAgree { spi_c: 10001, spi_s: 10002, port_c: 5062, port_s: 5063 };
    assert_eq!(next.build_security_offer(binding, &p).unwrap(), first.build_security_offer(binding, &p).unwrap());
    assert!(next.security_mechanism.is_none());
}

#[test]
fn explicit_demand_survives_exhausted_dynamic_formats_into_next_static_candidate() {
    let (p, first, generic) = fixture();
    let mut state = RegisterFallbackState::new(first);
    // No Security-Server here: exercise the normal format ladder, not reoffer.
    let mut current = state.observe(&p, first, &failure(494, "")).unwrap();
    let bad_format = failure(400, "");
    let mut steps = 0;
    while let Some(next) = next_dynamic_register_variant(&p, current, &bad_format) {
        assert!(steps < 4, "dynamic format ladder must terminate");
        current = state.observe(&p, next, &bad_format).unwrap();
        steps += 1;
    }
    assert_eq!(steps, 3);
    assert!(!current.policy.proxy_require_sec_agree);
    let next = state.apply(generic.without_access_network_info());
    assert_eq!(next.authorization, first.authorization);
    assert!(next.policy.require_sec_agree && next.policy.proxy_require_sec_agree);
    assert!(next.server_required_sec_agree);
    assert!(!next.policy.include_access_network_info);
}

#[test]
fn timeout_probe_does_not_become_a_confirmed_static_or_aka_requirement() {
    let (p, _, generic) = fixture();
    let mut state = RegisterFallbackState::new(generic);
    let timeout = RegisterFailure { error: ImsError::new("ims_register_initial_receive_failed"),
        response: None, auth_rounds: 0 };
    let probe = next_dynamic_register_variant(&p, generic, &timeout).unwrap();
    let observed = state.observe(&p, probe, &failure(400, "")).unwrap();
    assert!(observed.server_required_sec_agree, "legacy dynamic format probes remain available");
    assert!(!state.requires_protection(&p));
    let next = state.apply(generic);
    assert!(!next.server_required_sec_agree && !next.policy.require_sec_agree);
    state.observe(&p, probe, &failure(494, "")).unwrap();
    assert!(state.requires_protection(&p));
    assert!(state.apply(generic).policy.require_sec_agree);
}

#[test]
fn inherited_duplicates_do_not_exhaust_budget_before_tail_format_candidates() {
    let (mut p, _, _) = fixture();
    p.ims.register.initial_authorization = "none";
    p.ims.register.require_sec_agree_headers = false;
    p.ims.register.proxy_require_sec_agree_headers = false;
    p.ims.register.include_visited_network = true;
    p.ims.register.include_route_header = true;
    p.ims.register.always_add_sip_instance = true;
    let variants = register_variants(&p);
    let mut state = RegisterFallbackState::new(variants[0]);
    let mut history = RegisterCandidateHistory::default();
    let mut remaining = variants.into_iter();
    let mut pending = None;
    let mut sent = Vec::new();
    loop {
        let candidate = match pending.take().or_else(|| remaining.next().map(|v| state.apply(v))) {
            Some(v) => v,
            None => break,
        };
        if !history.record(candidate, state.requires_protection(&p)) { continue; }
        assert!(sent.len() < CELLULAR_IMS_REGISTER_CANDIDATE_LIMIT);
        let f = if sent.is_empty() { failure(421, "Require: sec-agree\r\n") } else { failure(400, "") };
        sent.push(candidate);
        let observed = state.observe(&p, candidate, &f).unwrap();
        pending = next_dynamic_register_variant(&p, observed, &f);
    }
    assert_eq!(sent.len(), 22);
    assert!(sent.iter().any(|v| !v.policy.include_sip_instance
        && v.security_client_offer == CellularImsSecurityClientOffer::Compact
        && !v.policy.proxy_require_sec_agree));
    let mut alias = sent[0];
    alias.label = "different_log_label";
    alias.server_required_sec_agree = !alias.server_required_sec_agree;
    assert!(!history.record(alias, false));
    assert!(history.record(alias, true), "a newly confirmed protection constraint is distinct");
}