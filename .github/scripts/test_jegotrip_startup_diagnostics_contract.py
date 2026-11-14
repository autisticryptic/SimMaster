"""Static wiring fences only; the runtime/peer/UI proof executes in the two Actions."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'

def source(path):
    return (ROOT / path).read_text(encoding='utf-8')

def native(name):
    return (P / name).read_text(encoding='utf-8')

class StartupDiagnosticContracts(unittest.TestCase):
    def test_diagnostic_shape_is_finite_and_not_deserializable_authority(self):
        text = native('startup_diagnostics.rs')
        self.assertIn('pub enum StartupPhase { Bootstrap, Account }', text)
        self.assertIn('pub enum StartupStage { Preparation, Attempt, RouterConnect, RouterTls, RouterControl, RouterIo, LogicalChannel, CommonFraming }', text)
        fields = text.split('pub struct StartupFailure {', 1)[1].split('}', 1)[0]
        self.assertEqual(re.findall(r'pub (\w+):', fields), ['phase', 'stage', 'code'])
        self.assertIn("pub code:&'static str", fields)
        for forbidden in ['Deserialize', 'unsafe', 'TcpStream', 'tokio::spawn', 'SystemTime', 'String', 'Vec<']:
            self.assertNotIn(forbidden, text)

    def test_trace_requires_original_owner_and_scope_and_retains_one_first_failure(self):
        text = native('startup_diagnostics.rs')
        self.assertEqual(text.count('if &self.owner!=owner || !self.scope.matches(scope)'), 2)
        self.assertIn('first:Mutex<Option<Failure>>', text)
        self.assertIn('if first.is_none() {*first=Some(failure);}', text)
        self.assertIn('StartupTrace(<redacted>)', text)
        self.assertNotIn('scope.close(', text)
        self.assertNotIn('scope.is_open(', text)  # history survives close, but the brand must match
        self.assertNotIn('mark_session_revoked(', text)

    def test_original_common_path_captures_before_preserving_existing_error_mapping(self):
        text = native('arc_native_actor.rs')
        for capture, mapping in [('router', 'router_error'), ('netstream', 'netstream_error'), ('common', 'common_error')]:
            self.assertIn(f'self.startup_trace.{capture}(&self.owner,&self.scope,error);{mapping}(error)', text)
        self.assertIn('StartupTrace::original(owner.clone(),scope.clone())', text)
        self.assertIn('if !self.transferred{self.scope.close();}', text)
        self.assertIn('timeouts.startup=timeouts.startup.min(self.attempt_end.duration_since(now))', text)
        self.assertIn('timeout_at(self.attempt_end.into(),connecting)', text)

    def test_runtime_projection_cannot_use_another_attempt_or_create_ready(self):
        text = native('runtime_backend.rs')
        block = text.split('fn startup_failure_observation(', 1)[1].split('pub(super) fn original_incoming_binding', 1)[0]
        self.assertIn('self.original(owner).ok()?', block)
        self.assertIn('trace.snapshot(owner,scope,StartupPhase::Bootstrap)', block)
        self.assertIn('destination.snapshot(owner,&attempt.scope,StartupPhase::Account)', block)
        for forbidden in ['configure', 'ready', 'connect(', 'login(', 'fresh(', '.close(']:
            self.assertNotIn(forbidden, block)
        view = native('account_runtime.rs')
        self.assertIn('startup_failure:None', view)
        self.assertIn('snapshot.startup_failure=run.backend.startup_failure_observation(&owner)', view)
        self.assertIn('if let Some(owner)=controller.resource_owner()', view)

    def test_new_runtime_tests_are_in_existing_actions_filters_and_keep_peer_failures(self):
        self.assertIn('mod startup_diagnostics;', native('arc_native_actor_tests.rs'))
        self.assertIn('runtime_startup_diagnostics_tests.rs', native('runtime_common_backend_tests.rs'))
        names = []
        for path, expected in [('startup_diagnostics_tests.rs', 12), ('runtime_startup_diagnostics_tests.rs', 3)]:
            text = native(path)
            found = re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)', text)
            self.assertEqual(len(found), expected)
            self.assertNotIn('#[ignore', text)
            names.extend(found)
        self.assertEqual(len(names), len(set(names)))
        for name in ['native_name_mismatch_retains_router_tls_diagnostic_without_sending_router_or_sdk_payload',
                     'cancelled_native_startup_closes_io_but_does_not_fabricate_an_observed_failure_or_grant',
                     'successful_common_startup_has_no_failure_and_does_not_authenticate_the_sdk',
                     'account_view_exposes_bounded_startup_failure_without_ready_and_stop_does_not_restore_it']:
            self.assertIn(name, names)
        for workflow in ['build-release.yml', 'beta-validation.yml']:
            text = source('.github/workflows/' + workflow)
            for suffix in ['transport_actor::native::tests', 'runtime_backend::common_tests']:
                self.assertIn('connectivity::app_voice::providers::jegotrip::' + suffix + ' \\\n', text)

    def test_ui_observes_diagnostics_but_never_reflects_unknown_strings_or_retries(self):
        text = source('frontend/src/policies/appLogin.ts').split('export function startupFailureLabel(', 1)[1].split('export function deviceSyncLabel(', 1)[0]
        self.assertIn('Object.prototype.hasOwnProperty.call(values, key)', text)
        self.assertIn("typeof failure === 'object' && failure !== null", text)
        self.assertIn('仅失败阶段观察，不恢复连接或就绪', text)
        self.assertIn('未识别的原生启动错误', text)
        for forbidden in ['fetch(', 'api.', 'localStorage', 'sessionStorage', 'console.', 'setTimeout', 'JSON.stringify']:
            self.assertNotIn(forbidden, text)
        pane = source('frontend/src/pages/sim/JegotripPushStatusPane.tsx')
        self.assertIn('startupFailureLabel(account.startup_failure)', pane)
        self.assertIn('api.getJegotripPushStatus()', pane)

    def test_old_browser_suites_and_all_budgets_remain_selected(self):
        config = source('frontend/playwright.jegotrip.config.ts')
        for name in ['jegotrip-device-identity.spec.ts', 'jegotrip-local-receiver.spec.ts', 'jegotrip-startup-diagnostics.spec.ts']:
            self.assertIn(name, config)
        for budget in ['timeout: 45_000', 'expect: { timeout: 10_000 }', 'workers: 1', 'retries: 0', 'timeout: 120_000']:
            self.assertIn(budget, config)
        self.assertEqual(source('frontend/e2e/jegotrip-device-identity.spec.ts').count("test('"), 4)
        self.assertEqual(source('frontend/e2e/jegotrip-local-receiver.spec.ts').count("test('"), 2)
        current = source('frontend/e2e/jegotrip-startup-diagnostics.spec.ts')
        self.assertEqual(current.count("test('"), 1)
        self.assertIn('expect(writes).toEqual([])', current)
        self.assertIn('expect(statusReads).toBe(readsBeforeRefresh + 1)', current)

if __name__ == '__main__':
    unittest.main()
