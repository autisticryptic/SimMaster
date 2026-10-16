"""R4 source seams only; production protocol regressions run on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
CELL = ROOT / 'backend/src/connectivity/modems/ims/cellular_ims'
WIFI = ROOT / 'backend/src/connectivity/modems/ims/vowifi'


class UnregisterRetryContract(unittest.TestCase):
    def test_cellular_records_control_before_diagnostics_on_captured_session(self):
        text = (CELL / 'live.rs').read_text(encoding='utf-8')
        block = text[text.index('async fn unregister_live_session('):text.index('async fn start_live_listener(')]
        failure = block[block.index('Err(failure) =>'):]
        self.assertIn('session.endpoint_retries.lock().await.observe_unregister_failure(', failure)
        self.assertLess(failure.index('observe_unregister_failure('), failure.index('runtime.observe_diagnostics'))
        self.assertNotIn('for_card(', failure)
        self.assertIn('observe_retry_diagnostics(runtime, admission)', failure)

    def test_wifi_captures_transaction_observer_before_send(self):
        text = (WIFI / 'operator.rs').read_text(encoding='utf-8')
        block = text[text.index('async fn unregister_registration('):text.index('fn next_interval(')]
        self.assertLess(block.index('factory.unregister_admitted().await'), block.index('run_unregister_observed('))
        self.assertIn('observer.observe(metadata).await', block)
        self.assertLess(block.index('observer.observe(metadata).await'), block.index('Some(metadata), admission'))
        live = (WIFI / 'live.rs').read_text(encoding='utf-8')
        hook = live[live.index('struct VowifiUnregisterFailureObserver'):live.index('fn reg_event_owner_valid', live.index('struct VowifiUnregisterFailureObserver'))]
        self.assertIn('retries: register_retry_live::Scope', hook)
        self.assertIn('self.retries.observe_unregister_failure(self.endpoint, &metadata)', hook)
        diagnostics = hook[hook.index('fn diagnostics_unregister_finished'):]
        self.assertNotIn('observe_unregister_failure(', diagnostics)
        self.assertNotIn('Scope::for_owner', diagnostics)

    def test_unregister_wait_is_independent_of_batch_reset(self):
        text = (ROOT / 'backend/src/connectivity/core/register_retry.rs').read_text(encoding='utf-8')
        self.assertIn('unregister_wait: Option<RetryAdmission>', text)
        self.assertIn('&mut self.unregister_wait', text)
        self.assertIn('self.unregister_wait.unwrap_or(RetryAdmission::Allowed)', text)
        self.assertIn('pub(crate) fn registered(&mut self) { self.restart = None; }', text)
        self.assertIn('let previous = self.restart_admission(now);', text)

    def test_actions_execute_all_new_production_regression_modules(self):
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT / '.github/workflows' / workflow).read_text(encoding='utf-8')
            for module in ['connectivity::core::register_retry::unregister_tests',
                           'connectivity::modems::ims::cellular_ims::live::refresh_tests::unregister_retry_tests',
                           'connectivity::modems::ims::vowifi::live::response_auth_tests::retry_unregister_tests']:
                self.assertIn(module, text)
            self.assertIn('Empty regression filter', text)


if __name__ == '__main__':
    unittest.main()
