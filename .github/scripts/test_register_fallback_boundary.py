"""Static REGISTER fallback boundaries; wire execution remains Actions-only."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / "backend/src/connectivity/modems/ims/cellular_ims"
SIM = ROOT / "offline-registration-sim"


def source(path):
    return path.read_text(encoding="utf-8")


def rust_cases(path):
    matrix = source(path).split("let cases = [", 1)[1].split("];", 1)[0]
    return re.findall(r'\("([^"]+)",\s*"([^"]+)",\s*(true|false)\)', matrix)


class RegisterFallbackBoundaryTests(unittest.TestCase):
    def test_generic_keeps_identity_and_advertisement_not_local_require(self):
        live = source(BASE / "live.rs")
        generic = live.split('label: "generic_ims_register_fallback",', 1)[1].split("let mut variants", 1)[0]
        self.assertRegex(generic, r"\bauthorization,")
        self.assertNotIn("authorization: CellularImsInitialAuthorization::None", generic)
        self.assertIn("advertise_sec_agree: advertise || primary.server_required_sec_agree", generic)
        self.assertIn("require_sec_agree: primary.server_required_sec_agree", generic)
        self.assertIn("server_required_sec_agree: primary.server_required_sec_agree", generic)

    def test_attempt_local_state_precedes_hint_and_dynamic_retry_in_both_callers(self):
        live = source(BASE / "live.rs")
        body = live.split("let registration = match registration_result", 1)[1].split("let artifacts = match channel", 1)[0]
        self.assertLess(body.index("fallback_state.observe("), body.index("security_hint::decide("))
        self.assertLess(body.index("security_hint::decide("), body.index("next_dynamic_register_variant_with_roaming("))
        self.assertIn("None => fallback_state.apply(", live)
        self.assertIn("Some(variant) => variant", live)
        self.assertIn("failure.auth_rounds == 0", body)
        adapter = source(SIM / "cellular_adapter.rs")
        advance = adapter.split("fn advance(", 1)[1].split("fn label(", 1)[0]
        self.assertIn("RegisterFallbackState::new(", adapter)
        self.assertLess(advance.index("failure.auth_rounds != 0"), advance.index("self.fallback.observe("))
        self.assertLess(advance.index("self.fallback.observe("), advance.index("security_hint::decide("))
        self.assertIn("self.fallback.apply(", advance)
        self.assertIn("Err(_) => return false", advance)

    def test_requirements_monotonically_survive_new_static_candidates(self):
        state = source(BASE / "register_fallback.rs")
        for field in ("empty_aka", "advertise_sec_agree", "server_required_sec_agree"):
            self.assertIn(field, state)
        self.assertIn("self.empty_aka |=", state)
        self.assertIn("self.advertise_sec_agree |=", state)
        self.assertIn("next.policy.advertise_sec_agree |= self.advertise_sec_agree", state)
        self.assertIn("next = next.requiring_sec_agree()", state)
        self.assertIn('Some(494)', state)
        self.assertIn('["Require", "Proxy-Require"]', state)
        self.assertIn('Err("unsupported_required_extension")', state)
        self.assertIn('Err("required_security_disabled")', state)
        # The state is not a second retry engine, AKA engine, or carrier switch.
        for forbidden in ("run_register", "run_usim_aka", "Command::", "home_plmn", "mcc", "mnc"):
            self.assertNotIn(forbidden, state)

    def test_confirmed_security_precedes_aka_and_candidates_deduplicate_before_budget(self):
        live = source(BASE / "live.rs")
        self.assertIn(".with_required_security(fallback_state.requires_protection(profile))", live)
        initial = live.split("let mut fallback_state =", 1)[1]
        self.assertLess(initial.index("candidate_history.record("), initial.index("candidate_attempts.saturating_add(1)"))
        auth = live.split("async fn prepare_authenticated_channel(", 1)[1].split("async fn authenticated_request(", 1)[0]
        self.assertLess(auth.index("self.required_security && security_server.is_none()"), auth.index("identity::run_usim_aka"))
        adapter = source(SIM / "cellular_adapter.rs")
        self.assertIn("RegisterCandidateHistory::default()", adapter)
        self.assertIn("self.history.record(", adapter)
        self.assertIn("self.fallback.requires_protection(self.profile)", adapter)
        state = source(BASE / "register_fallback.rs")
        self.assertIn("confirmed_security_required", state)
        self.assertIn("old.policy == candidate.policy", state)

    def test_bare_421_gate_cannot_parse_or_select_the_offer(self):
        hint = source(BASE / "security_hint.rs")
        gate = hint.split("if register_failure_status(failure) == Some(421)", 1)[1]
        self.assertLess(gate.index("return Decision::NotApplicable"), gate.index("security_agreement::select("))
        self.assertIn('response_has_only_extension(response, "Require", "sec-agree")', gate)
        self.assertIn('response_has_only_extension(response, "Proxy-Require", "sec-agree")', gate)
        self.assertIn("variant.security_mechanism.is_some()", hint)
        self.assertIn("index != 0", hint)
        self.assertIn('eq_ignore_ascii_case("aes-cbc")', hint)

    def test_matrices_are_synthetic_and_keep_all_twelve_hint_cases(self):
        hint = source(SIM / "security_hint.rs")
        fallback = source(SIM / "fallback.rs")
        self.assertEqual(len(rust_cases(SIM / "security_hint.rs")), 12)
        cases = rust_cases(SIM / "fallback.rs")
        self.assertEqual(len(cases), 32)
        self.assertEqual(len({case[0] for case in cases}), 32)
        self.assertEqual(len({case[1] for case in cases}), 32)
        for text in (hint, fallback):
            self.assertIn('"001", "01", true', text)
            for forbidden in ("cmcc", '"460"', "home_plmn", "roaming_visited"):
                self.assertNotIn(forbidden, text.lower())
            self.assertIn('"hardware_used": false', text)
            self.assertIn('"live_network_verified": false', text)
        simulator = source(SIM / "simulator.rs")
        hint_peer = simulator.split('if self.scenario.mode.starts_with("hint_") {', 1)[1].split('if matches!(self.scenario.mode, "second_security"', 1)[0]
        self.assertIn('format!("Require: sec-agree\\r\\nSecurity-Server:', hint_peer)

    def test_fallback_wire_assertions_cover_required_negative_and_bounded_paths(self):
        fallback = source(SIM / "fallback.rs")
        modes = {case[1] for case in rust_cases(SIM / "fallback.rs")}
        required = {"fallback_aes", "fallback_proxy", "fallback_challenge_null", "fallback_hint_unknown",
                    "fallback_initial_403", "fallback_generic_403", "fallback_auth_403", "fallback_bad_challenge",
                    "fallback_no_security", "fallback_require", "fallback_proxy_require", "fallback_494",
                    "fallback_unknown_require", "fallback_unknown_proxy", "fallback_unknown_494",
                    "fallback_hint_421", "fallback_hint_494", "fallback_repeat", "fallback_min_pre",
                    "fallback_min_post", "fallback_min_bound", "fallback_auth_bound", "fallback_required_policy",
                    "fallback_disabled", "fallback_dynamic_identity"}
        self.assertTrue(required <= modes)
        for assertion in ('"generic changed {name}"', '"bare offer narrowed or rebound"',
                          '"421 offer promoted to Security-Verify"', '"only 401/407 may bind Security-Verify"',
                          '"Require lost or invented"', '"Proxy-Require lost or invented"',
                          '"423 changed {name}"', '"candidate bound: {mode}"',
                          'assert_eq!(peer.digest_verified, proofs)', 'assert_eq!(peer.statuses, statuses',
                          'assert_eq!(cseq(current), cseq(previous) + 1)', '"response", ""'):
            self.assertIn(assertion, fallback)
        simulator = source(SIM / "simulator.rs")
        for call in ("run_register_observed(&mut peer, &request, &mut auth)",
                     "cellular_ims::live::offline_sim_adapter::builder(profile, identity, route)",
                     "fallback::assert_case(&peer, &candidates, rounds)",
                     "fields[\"response\"] != server_proof(self.algorithm(), &fields)"):
            self.assertIn(call, simulator)
        self.assertIn("self.fallback_authenticated(frame, expires)", simulator)
        self.assertLess(simulator.index("self.digest_verified += 1"), simulator.index("self.fallback_authenticated(frame, expires)"))
        # The added matrix supplies peer behavior, never its own builder/retry driver.
        for forbidden in ("build_register_from_profile(", "next_dynamic_register_variant(", "std::process", "TcpStream", "UdpSocket"):
            self.assertNotIn(forbidden, fallback)


if __name__ == "__main__":
    unittest.main()
