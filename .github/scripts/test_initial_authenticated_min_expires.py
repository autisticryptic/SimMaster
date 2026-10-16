"""Static production seams; protocol/loopback execution is required on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class InitialAuthenticatedLeaseRetry(unittest.TestCase):
    def test_protected_423_returns_the_original_channel_to_the_shared_budget(self):
        text = (ROOT/'backend/src/connectivity/modems/ims/vowifi/live.rs').read_text(encoding='utf-8')
        helper = text[text.index('fn retain_protected_register_response('):text.index('fn live_digest_error_code(')]
        self.assertIn('401 | 407 | 423', helper)
        self.assertIn('retain_protected_register_channel(initial, protected)', helper)
        self.assertNotIn('install_ims_esp_policy', helper)
        self.assertNotIn('authenticated_min_expires_failure', text)
        protected = text[text.index('async fn run_protected_authenticated_register_candidates('):text.index('struct LiveImsEspPolicyCandidate')]
        self.assertIn('retain_protected_register_response(initial_channel, protected_channel, summary.status_code)', protected)

    def test_lease_builder_consumes_fresh_request_proof_without_reauth(self):
        text = (ROOT/'backend/src/connectivity/modems/ims/vowifi/live.rs').read_text(encoding='utf-8')
        builder = text[text.index('fn rebuild_initial_authenticated_register('):text.index('fn retain_protected_register_response(')]
        self.assertIn('auth.sent_challenge().is_some()', builder)
        self.assertIn('.authorization(&username, &uri)', builder)
        self.assertIn('validate_live_register_request', builder)
        self.assertNotIn('.authenticate(', builder)
        self.assertNotIn('nonce_count = 0', builder)
        self.assertNotIn('RegisterNegotiationBudget::default', builder)
        start = text.index('impl RegisterAuthenticator<SipChannel> for VowifiRegisterAuthenticator')
        adapter = text[start:text.index('fn live_digest_error_code(', start)]
        self.assertIn('self.context.verify_worker(self.line_id)?', adapter)
        self.assertIn('.verify(self.line_id).await', adapter)
        self.assertIn('self.last_security_verify.as_deref()', adapter)

    def test_actions_execute_real_initial_authenticator_regressions(self):
        for workflow in ['build-release.yml','beta-validation.yml']:
            text = (ROOT/'.github/workflows'/workflow).read_text(encoding='utf-8')
            self.assertIn('connectivity::modems::ims::vowifi::live::response_auth_tests::initial_min_expires_tests', text)
            self.assertIn('Empty regression filter', text)
        tests = (ROOT/'backend/src/connectivity/modems/ims/vowifi/initial_min_expires_tests.rs').read_text(encoding='utf-8')
        self.assertIn('VowifiRegisterAuthenticator {', tests)
        self.assertIn('run_register_observed_with_budget(', tests)
        self.assertIn('run_unregister(', tests)
        self.assertIn('retain_protected_register_response(', tests)


if __name__ == '__main__':
    unittest.main()
