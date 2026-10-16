"""R5 static seams only. Actual REGISTER/reader mock execution is Actions-only."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
BASE = ROOT/'backend/src/connectivity/modems/ims'

class ProvisioningContract(unittest.TestCase):
    def test_fresh_sample_is_bound_and_not_silently_unconfigured(self):
        text = (BASE/'uicc_provisioning.rs').read_text(encoding='utf-8')
        self.assertIn('expected.verify(&observe().await?)?', text)
        self.assertIn('let second = read().await;', text)
        self.assertIn('ims_uicc_provisioning_unstable', text)
        self.assertIn('ims_uicc_provisioning_changed', text)
        self.assertNotIn('unwrap_or_default()', text)
        self.assertNotIn('tokio::time::timeout', text)

    def test_production_checks_before_send_and_before_publication(self):
        cell = (BASE/'cellular_ims/live.rs').read_text(encoding='utf-8')
        self.assertIn('.with_provisioning(device_identity.provisioning.clone())', cell)
        self.assertIn('.with_provisioning(session.provisioning.clone())', cell)
        self.assertGreaterEqual(cell.count('verify_ims_registration_material('), 7)
        wifi = (BASE/'vowifi/register_refresh_live.rs').read_text(encoding='utf-8')
        self.assertEqual(wifi.count('uicc.verify_registration(&self.line_id).await'), 2)
        self.assertLess(wifi.index('uicc.verify_registration'), wifi.index('let initial = auth.request()?'))
        self.assertIn('retries.observe(logical_endpoint, &budget.failure_metadata(failure))', wifi)
        operator = (BASE/'vowifi/operator.rs').read_text(encoding='utf-8')
        self.assertIn('factory.unregister_accepted().await', operator)
        self.assertIn('self.code().starts_with("ims_uicc_")', operator)

    def test_actions_execute_new_freshness_production_modules(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            for module in ['uicc_ims::provisioning::tests', 'refresh_tests::provisioning_tests', 'response_auth_tests::provisioning_tests']:
                self.assertIn(module, text)
            self.assertIn('Empty regression filter', text)

if __name__ == '__main__':
    unittest.main()
