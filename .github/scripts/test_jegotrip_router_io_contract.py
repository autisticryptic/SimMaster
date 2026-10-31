"""Static guards for original-owned production Router IO, not runtime proof."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
NATIVE = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'

class RouterIoContract(unittest.TestCase):
    def test_endpoint_selection_never_discards_ri_or_substitutes_a_tls_authority(self):
        source = (NATIVE / 'router_connector.rs').read_text()
        self.assertIn('parsed.explicit_tls_router_groups()', source)
        self.assertIn('matching.len()!=1', source)
        self.assertIn('used.iter().any(|used|!*used)', source)
        self.assertIn('same_authority(host,anchor.authority)', source)
        self.assertIn('validate_tls_anchor(anchor.authority,anchor.root)', source)
        self.assertIn('attempted.contains(&(target.router_id,address))', source)
        self.assertIn('Arc::ptr_eq(&self.policy,&policy.identity)', source)
        self.assertIn('prepare_socket_addrs', source)
        for forbidden in ['to_socket_addrs', 'dangerous()', 'with_custom_certificate_verifier', 'Callback::', 'from_utf8_lossy']:
            self.assertNotIn(forbidden, source)

    def test_original_gate_and_deadline_precede_each_network_poll(self):
        source = (NATIVE / 'router_connector.rs').read_text()
        self.assertIn('scope.with_io_gate(||future.as_mut().poll(cx))', source)
        self.assertIn('std::time::Instant::now()>=deadline', source)
        self.assertIn('poll_before(scope,lookup.resolve(),deadline)', source)
        self.assertIn('poll_before(scope,TcpStream::connect(address),deadline)', source)
        runtime = (NATIVE / 'router_runtime.rs').read_text()
        self.assertIn('timeouts.connect.min(timeouts.startup)', runtime)
        self.assertIn('poll_before(&scope,io.write(&bytes[offset..]),end)', runtime)
        self.assertIn('poll_before(&scope,io.flush(),end)', runtime)
        self.assertIn('poll_before(&self.scope,io.read(&mut buffer),end)', runtime)

    def test_tls_router_runtime_has_original_drop_cleanup_and_no_detached_tasks_or_sdk_grants(self):
        source = (NATIVE / 'router_runtime.rs').read_text()
        self.assertIn('connected.into_original(&owner,&scope,policy)', source)
        self.assertIn('EndpointTlsEngine::for_original', source)
        self.assertIn('RouterConnectTracker::for_original', source)
        self.assertIn('while path.engine.phase()!=TlsPhase::Verified', source)
        self.assertIn('self.io=None', source)
        self.assertIn('impl Drop for RouterPath', source)
        self.assertIn('impl Drop for CancelGuard', source)
        self.assertIn('if !self.original(owner,scope)', source)
        for forbidden in ['tokio::spawn', 'Callback::', 'unsafe', 'mem::forget', 'phone_login_token', 'sip_password']:
            self.assertNotIn(forbidden, source)

    def test_actions_run_live_loopback_and_original_negative_cases(self):
        for name in ['router_connector', 'router_runtime']:
            source = (NATIVE / (name + '_tests.rs')).read_text()
            self.assertIn('#[tokio::test]', source)
            self.assertIn('Certificates::generate', source)
            for workflow in ['build-release.yml', 'beta-validation.yml']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{name}::tests', (ROOT / '.github/workflows' / workflow).read_text())
        source = (NATIVE / 'router_runtime_tests.rs').read_text()
        self.assertIn('task.abort()', source)
        self.assertIn('task.await.unwrap_err().is_cancelled()', source)
        self.assertIn('wrong_endpoint_root_never_sends_router_credentials', source)

if __name__ == '__main__':
    unittest.main()
