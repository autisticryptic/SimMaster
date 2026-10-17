"""R7 static seams. Worker message/epoch races execute only on Actions."""
from pathlib import Path
import re
import unittest
ROOT = Path(__file__).resolve().parents[2]
CELL = ROOT/'backend/src/connectivity/modems/ims/cellular_ims'

class XfrmWorkerBindingContract(unittest.TestCase):
    def test_install_and_cleanup_accept_only_captured_binding(self):
        text = (CELL/'ipsec.rs').read_text(encoding='utf-8')
        install = text[text.index('pub async fn install_plan_in_worker('):text.index('pub fn uninstall_plan(')]
        cleanup = text[text.index('pub async fn uninstall_plan_in_worker('):text.index('mod worker_tests;')]
        for body in [install, cleanup]:
            self.assertIn('binding: &UeWorkerBinding', body)
            self.assertIn('binding.apply_net_config(ops).await', body)
            self.assertNotIn('UeWorkerHandle', body)
            self.assertNotIn('.bind()', body)
        self.assertIn('uninstall_plan_in_worker(plan, binding).await', install)
        self.assertIn('code::RUNTIME_UE_WORKER_GENERATION_CHANGED', install)

    def test_every_production_xfrm_call_uses_the_session_epoch(self):
        calls = []
        for path in [CELL/'live.rs', CELL/'pcscf_refresh.rs']:
            text = path.read_text(encoding='utf-8')
            calls += re.findall(r'ipsec::(?:un)?install_plan_in_worker\(([^)]*)\)', text)
        self.assertEqual(len(calls), 8)
        for arguments in calls:
            self.assertIn('.worker_binding', arguments)
            self.assertNotIn('.xfrm_worker', arguments)

    def test_actions_run_the_actual_worker_epoch_races(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            self.assertIn('connectivity::modems::ims::cellular_ims::ipsec::worker_tests', text)
            self.assertIn('Empty regression filter', text)
        tests = (CELL/'ipsec_worker_tests.rs').read_text(encoding='utf-8')
        for symbol in ['enable_test_net_config_handler', 'lock_current_generation',
                       'advance_test_generation', 'install_plan_in_worker', 'uninstall_plan_in_worker']:
            self.assertIn(symbol, tests)

if __name__ == '__main__':
    unittest.main()
