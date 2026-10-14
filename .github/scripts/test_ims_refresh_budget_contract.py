"""Structural guard for the real refresh controller, not a protocol simulation."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]

class RefreshNegotiationBudgetContract(unittest.TestCase):
    def flow(self):
        source = (ROOT / 'backend/src/connectivity/modems/ims/cellular_ims/live.rs').read_text()
        return source.split('async fn refresh_live_registration(', 1)[1].split('\nfn lost_refresh_attempt(', 1)[0]

    def test_one_budget_spans_the_real_candidate_loop(self):
        flow = self.flow()
        self.assertEqual(flow.count('RegisterNegotiationBudget::default()'), 1)
        self.assertLess(flow.index('let mut lease_budget'), flow.index('for variant in candidates'))
        self.assertIn('run_register_observed_with_budget(', flow)
        self.assertIn('&mut session.channel, &initial, &mut authenticator, &mut lease_budget', flow)
        self.assertNotIn('run_register_observed(', flow)

    def test_structured_metadata_stops_expansion_and_reaches_the_returned_error(self):
        flow = self.flow()
        self.assertIn('let metadata = lease_budget.failure_metadata(&failure)', flow)
        self.assertIn('map_refresh_register_failure(&failure).with_register_failure(metadata)', flow)
        self.assertIn('&& metadata.permits_variant_fallback()', flow)
        self.assertIn('.with_register_failure(lease_budget.failure_metadata(&failure))', flow)
        self.assertNotIn('.is_some_and(status_permits_register_variant_fallback)', flow)
        self.assertIn('observe_refresh_failure(', flow)
        self.assertIn('authenticator.rollback_security(&mut session.channel).await', flow)

    def test_production_regressions_are_connected_to_executed_actions_filters(self):
        test_root = (ROOT / 'backend/src/connectivity/modems/ims/cellular_ims/refresh_tests.rs').read_text()
        self.assertIn('#[path = "refresh_budget_tests.rs"]', test_root)
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT / '.github/workflows' / workflow).read_text()
            self.assertIn('connectivity::modems::ims::cellular_ims::live::refresh_tests', text)
            self.assertIn('selected no tests', text)
        text = (ROOT / 'backend/src/connectivity/modems/ims/cellular_ims/refresh_budget_tests.rs').read_text()
        self.assertGreaterEqual(text.count('refresh_live_registration('), 6)
        self.assertIn('existing_non_ipsec_refresh_keeps_initial_header_compatibility_without_423', text)
        self.assertIn('refresh_authenticated_400_does_not_reopen_the_header_or_aka_budget', text)
        self.assertIn('refresh_423_then_503_retains_only_original_lease_and_enforces_existing_wait', text)

if __name__ == '__main__':
    unittest.main()
