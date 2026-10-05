"""Wiring/privacy guards only. Rust and protocol execution belong on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT / 'backend/src/connectivity/modems/ims/cellular_ims'


class SecurityHintBoundaryTests(unittest.TestCase):
    def test_hint_decision_precedes_any_generic_or_dynamic_retry(self):
        source = (BASE / 'live.rs').read_text()
        body = source[source.index('let registration = match registration_result'):source.index('let artifacts = match channel')]
        self.assertLess(body.index('security_hint::decide('), body.index('next_dynamic_register_variant_with_roaming('))
        self.assertIn('security_hint::Decision::Stop(reason)', body)
        self.assertIn('return Err(error)', body.split('security_hint::Decision::Stop(reason)', 1)[1])
        self.assertIn('device_identity.diagnostic_required_security || variant.security_mechanism.is_some()', body)
        self.assertIn('authenticator.mode != RegistrationMode::Ipsec', body)
        self.assertIn('channel.security_verify().is_none()', body)

    def test_hint_is_not_a_key_install_or_identity_rewrite(self):
        source = (BASE / 'security_hint.rs').read_text()
        for forbidden in ('install_plan', 'run_usim_aka', 'build_authorization', 'home_plmn', 'roaming_visited', 'Command::'):
            self.assertNotIn(forbidden, source)
        self.assertIn('index != 0', source)
        self.assertIn('eq_ignore_ascii_case("aes-cbc")', source)
        self.assertIn('unique.len() <= 1', source)
        self.assertIn('failure.auth_rounds != 0', source)
        self.assertIn('Some(421 | 494)', source)
        self.assertIn('security_hint_cannot_narrow_to_weaker_policy', source)
        self.assertNotIn('Some(403)', source)

    def test_authentication_and_refresh_keep_the_effective_restriction(self):
        source = (BASE / 'live.rs').read_text()
        auth = source[source.index('async fn prepare_authenticated_channel('):source.index('async fn authenticated_request(', source.index('async fn prepare_authenticated_channel('))]
        self.assertLess(auth.index('select_security_server_for_mechanism'), auth.index('identity::run_usim_aka'))
        self.assertIn('self.security_mechanism.is_some() && security_server.is_none()', auth)
        self.assertGreaterEqual(source.count('.with_security_mechanism('), 3)
        self.assertIn('variant.build_security_offer(pending_security_binding, session.profile)', source)
        self.assertIn('session.register_variant.security_mechanism.is_some() { 1 } else { 3 }', source)
        self.assertIn('CELLULAR_IMS_REGISTER_CANDIDATE_LIMIT: usize = 24;', source)
        shared = (ROOT / 'backend/src/connectivity/core/register.rs').read_text()
        self.assertIn('const MAX_AUTH_ROUNDS: u8 = 2;', shared)
        self.assertIn('Duration::from_secs(32)', shared)

    def test_adapter_and_ci_exercise_same_hint_decision(self):
        adapter = (ROOT / 'offline-registration-sim/cellular_adapter.rs').read_text()
        self.assertIn('security_hint::decide(', adapter)
        self.assertIn('Decision::Stop(_) => return false', adapter)
        self.assertIn('select_security_server_for_mechanism', adapter)
        self.assertIn('fn accept_success(', adapter)
        for name in ('beta-validation.yml', 'build-release.yml'):
            text = (ROOT / '.github/workflows' / name).read_text()
            self.assertIn('cellular_ims::live::security_hint::tests', text)
            self.assertIn('cellular_ims::live::register_failure_diagnostics::tests', text)
            self.assertIn('run.py --security-hint', text)
            self.assertIn('ci-results/security-hint.json', text)

    def test_diagnostics_do_not_feed_acceptance(self):
        diagnostics = (BASE / 'register_failure_diagnostics.rs').read_text().split('#[cfg(test)]', 1)[0]
        self.assertNotIn('Vec<String>', diagnostics)
        self.assertIn('Vec<&\'static str>', diagnostics)
        self.assertIn('MAX_HEADER_BYTES: usize = 16 * 1024', diagnostics)
        hint = (BASE / 'security_hint.rs').read_text()
        self.assertNotIn('summarize', hint)
        live = (BASE / 'live.rs').read_text()
        self.assertIn('diagnostics_malformed = diagnostic.malformed', live)
        self.assertIn('warning_classes = ?diagnostic.warning_classes', live)
        self.assertIn('response_cseq = diagnostic.response_cseq', live)
        self.assertNotIn('response_cseq = sip::header_value(response, "CSeq")', live)
        parser = (BASE / 'security_agreement.rs').read_text()
        self.assertIn('valid_parameter_value(value.trim())', parser)


if __name__ == '__main__':
    unittest.main()
