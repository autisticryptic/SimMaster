"""Static production-wiring guards only; no Rust, sockets, or simulation run."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
def read(path): return (ROOT / path).read_text(encoding='utf-8')
BASE = 'backend/src/connectivity/modems/ims/vowifi/'

class VowifiRegisterRetryContract(unittest.TestCase):
    def test_both_accesses_use_one_io_free_retry_policy(self):
        core = read('backend/src/connectivity/core/register_retry.rs')
        cell = read('backend/src/connectivity/modems/ims/cellular_ims/register_retry.rs')
        wifi = read(BASE + 'register_retry_live.rs')
        self.assertIn('core::register_retry::{', cell)
        self.assertIn('core::register_retry::{', wifi)
        self.assertNotIn('struct EndpointRetryState', cell)
        for forbidden in ['tokio::', 'UdpSocket', 'TcpStream', '.await', 'registration_diagnostics']:
            self.assertNotIn(forbidden, core)
        for required in ['MAX_ENDPOINT_RETRY_ENTRIES: usize = 32', 'from_secs(86_400)', 'RetryAfter::Invalid', 'old.max(new)', 'RetainUntilExpiry']:
            self.assertIn(required, core)

    def test_initial_controller_and_refresh_share_real_gate_not_diagnostic_values(self):
        live = read(BASE + 'live.rs')
        retry = read(BASE + 'register_retry_live.rs')
        refresh = read(BASE + 'register_refresh_live.rs')
        self.assertIn('register_retry_live::run_candidates(&retries', live)
        self.assertIn('err.register_failure.is_some_and(register_retry_live::response_requires_wait)', live)
        self.assertIn('metadata.kind != RegisterFailureKind::TemporaryEndpoint', retry)
        self.assertIn('scope.pause_restart().await', retry)
        self.assertIn('run_register_observed_with_budget', refresh)
        self.assertIn('budget.failure_metadata(&failure)', refresh)
        self.assertIn('metadata.is_temporary() && *expiry > Instant::now()', refresh)
        self.assertIn('original_expiry.is_some_and(|expiry| expiry <= Instant::now())', refresh)
        self.assertNotIn('diagnostics.report()', refresh)

    def test_worker_and_subscription_changes_never_relabel_old_context_as_current(self):
        live = read(BASE + 'live.rs')
        retry = read(BASE + 'register_retry_live.rs')
        refresh = read(BASE + 'register_refresh_live.rs')
        self.assertIn('worker_binding: Option<UeWorkerBinding>', live)
        self.assertIn('binding.matches(&current.binding)', live)
        self.assertIn('self.context.verify_worker(&self.line_id)', refresh)
        self.assertIn('entry.owner.same_subscription(owner)', retry)
        self.assertIn('owner.subscription_is_current(line)', retry)
        cleanup = live.split('pub async fn clear_live_runtime_for_line(', 1)[1].split('\nasync fn clear_live_ims_session(', 1)[0]
        self.assertNotIn('clear_live_ims_refresh_failure_for_line(line_id).await', cleanup)
        self.assertNotIn('register_retry_live::', cleanup)
        self.assertIn('failure.rebuild_pending = false', cleanup)

    def test_runtime_and_api_use_typed_recovery_without_extending_lease_or_counting_polls(self):
        operator = read(BASE + 'operator.rs')
        runtime = read(BASE + 'runtime.rs')
        executor = read(BASE + 'executor.rs')
        api = read('backend/src/api/handlers.rs')
        self.assertIn('factory.refresh_observed(&mut session.channel, session.context.expires_at)', operator)
        self.assertIn('factory.unregister_admitted().await', operator)
        self.assertIn('if let Ok(registration) = &result', operator)
        self.assertIn('self.retained_until.is_some_and(|expiry| expiry <= Instant::now())', runtime)
        self.assertIn('self.live_readiness.refresh_prior', runtime)
        self.assertIn('self.publish_live_stages(generation, revocation_serial, next).await', runtime)
        self.assertIn('runtime.revoke_live_ims_registration(LOSS_REASON).await', read(BASE + 'reg_event_live.rs'))
        self.assertIn('#[serde(skip)]\n    pub(crate) register_recovery', executor)
        self.assertIn('if let Some(recovery) = last_status.register_recovery', api)
        self.assertIn('if refresh_cycle && recovery.attempted', api)
        self.assertGreaterEqual(api.count('vowifi_register_wait_status(app, scope).await'), 3)

    def test_actions_execute_production_controller_refresh_operator_and_api_tests(self):
        for path in ['.github/workflows/build-release.yml', '.github/workflows/beta-validation.yml']:
            text = read(path)
            for name in ['vowifi::live::register_retry_live::tests', 'vowifi::runtime::retry_tests',
                         'vowifi::live::response_auth_tests', 'vowifi_register_wait_admission_preserves_profile_intent_and_failure_budget']:
                self.assertIn(name, text)
            self.assertIn('selected no tests', text)
        self.assertIn('mod retry_recovery_tests;', read(BASE + 'response_auth_tests.rs'))
        tests = read(BASE + 'register_retry_refresh_tests.rs')
        self.assertIn('install_registered_channel(context, channel).await', tests)
        self.assertIn('refresh_registered_for_line(', tests)
        self.assertIn('factory.refresh_observed(', tests)
        self.assertIn('real_tcp_eof_during_refresh_is_dead_flow', tests)

if __name__ == '__main__': unittest.main()
