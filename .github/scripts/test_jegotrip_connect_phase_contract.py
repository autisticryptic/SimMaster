"""SOURCE-only Router DNS/TCP timeout detail fences; no native/JS execution."""
from pathlib import Path
import re,unittest
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def native(name):return(P/name).read_text(encoding='utf-8')
class ConnectPhaseContracts(unittest.TestCase):
 def test_detail_is_finite_and_never_serializes_private_resolver_or_tcp_error(self):
  s=native('connect_failure_detail.rs');self.assertIn('pub enum ConnectFailureStep { Dns, Tcp }',s)
  for raw in ['String','Vec<','SocketAddr','format!','error.to_string','Deserialize','tokio::spawn','SystemTime']:self.assertNotIn(raw,s)
  self.assertIn('struct Recorded {owner:AttemptOwner,step:ConnectFailureStep}',s)
 def test_observer_has_no_scope_ownership_cycle_and_no_reset_or_grant(self):
  s=native('connect_failure_detail.rs');self.assertIn("scope:&'a RpcConnectionScope",s);self.assertIn('ConnectDiagnosticStore(Mutex<Option<Recorded>>)',s);self.assertIn('if first.is_none()',s)
  for raw in ['scope.close','mark_session_revoked','pub(super) fn reset','pub(super) fn clear']:self.assertNotIn(raw,s)
 def test_only_a_reported_original_timeout_freezes_last_entered_step(self):
  s=native('connect_failure_detail.rs');self.assertIn('if error==ConnectError::Timeout',s);self.assertIn('ConnectFailureStep::from_byte(self.step.load(Ordering::Acquire))',s)
  self.assertIn('self.scope.record_connect_timeout(self.owner,step)',s)
 def test_scope_original_io_gate_budget_and_existing_tls_detail_are_kept(self):
  s=native('rpc_pending.rs');self.assertIn('Arc::ptr_eq(&self.4,budget)',s);self.assertIn('self.4.reserve(self,bytes)',s);self.assertIn('self.4.tls_detail_for(owner)',s);self.assertIn('scope.with_io_gate(||future.as_mut().poll(cx))',native('router_connector.rs'))
  s=native('native_rpc_budget.rs');self.assertIn('self.connect_detail.record(owner,step)',s);self.assertIn('state:Mutex<State>,tls_detail:super::tls_failure_detail::TlsDiagnosticStore',s)
 def test_original_connector_polls_timeouts_address_limits_and_stack_are_unchanged(self):
  s=native('router_connector.rs');self.assertIn('deadline=std::time::Instant::now().checked_add(self.timeout)',s);self.assertIn('poll_before(scope,lookup.resolve(),deadline)',s);self.assertIn('poll_before(scope,TcpStream::connect(address),deadline)',s);self.assertIn('tokio::time::timeout_at(deadline.into(),group)',s);self.assertIn('resolved>64',s);self.assertIn('attempted.len()==16',s)
  self.assertIn('observation.entering(super::connect_failure_detail::ConnectFailureStep::Dns)',s);self.assertIn('observation.entering(super::connect_failure_detail::ConnectFailureStep::Tcp)',s);self.assertIn('observation.finish(*error)',s)
  for raw in ['to_socket_addrs','RUST_MIN_STACK','unsafe','sleep(', 'LOOKUP_TIMEOUT']:self.assertNotIn(raw,s)
 def test_legacy_failure_and_tls_dto_remain_separate(self):
  s=native('startup_diagnostics.rs');fields=s.split('pub struct StartupFailure {',1)[1].split('}',1)[0];self.assertEqual(re.findall(r'pub (\w+):',fields),['phase','stage','code'])
  self.assertIn('pub struct StartupTlsDetail {pub phase:StartupPhase,pub reason:TlsFailureReason}',s);self.assertIn('pub struct StartupConnectDetail {pub phase:StartupPhase,pub step:ConnectFailureStep}',s)
  self.assertIn('failure.stage==StartupStage::RouterConnect&&failure.code=="jegotrip_connector_timeout"',s);self.assertIn('failure.connect_step',s)
 def test_test_hold_is_explicit_same_policy_no_global_or_replacement_connector(self):
  s=native('router_connect_detail_test_gate.rs');self.assertIn('poll_before(scope,async',s);self.assertIn('who!=owner||!source.matches(scope)',s)
  for raw in ['static ', 'TcpStream','getaddrinfo','lookup_ip','tokio::spawn']:self.assertNotIn(raw,s)
  self.assertIn('#[cfg(test)]\npub(crate) mod router_connect_detail_test_gate;',native('mod.rs'))
 def test_runtime_bootstrap_phase_projection_uses_original_trace_and_never_connects(self):
  s=native('runtime_backend.rs').split('fn startup_connect_detail_observation(',1)[1].split('pub(super) fn original_incoming_binding',1)[0]
  self.assertIn('self.original(owner).ok()?',s);self.assertIn('trace.connect_detail(owner,scope,StartupPhase::Bootstrap)',s);self.assertIn('destination.connect_detail(owner,&attempt.scope,StartupPhase::Account)',s)
  for raw in ['.close(', 'connect(', 'fresh(', 'login(']:self.assertNotIn(raw,s)
 def test_new_leaves_are_mounted_with_all_previous_workflow_filters(self):
  for name in ['build-release.yml','beta-validation.yml']:
   s=(ROOT/'.github/workflows'/name).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::connect_failure_detail::tests \\\n',s);self.assertIn('connectivity::app_voice::providers::jegotrip::router_connector::tests \\\n',s)
  for name in ['router_connector_tests.rs','startup_diagnostics_tests.rs','runtime_startup_diagnostics_tests.rs']:self.assertIn('mod connect_detail;',native(name))
  self.assertIn('app_voice_connect_detail_http_tests.rs',(ROOT/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
 def test_ui_matches_only_original_timeout_same_phase_and_own_fields(self):
  s=(ROOT/'frontend/src/policies/appLogin.ts').read_text(encoding='utf-8').split('export function startupConnectDetailLabel(',1)[1].split('export function deviceSyncLabel(',1)[0]
  for fence in ['Object.hasOwn','base.phase !== data.phase',"base.stage !== 'router_connect'","base.code !== 'jegotrip_connector_timeout'",'不授信、不延长时限、不重试或恢复就绪']:self.assertIn(fence,s)
  for raw in ['String(', 'JSON.stringify','api.', 'fetch(', 'console.']:self.assertNotIn(raw,s)
 def test_new_browser_keeps_original_limited_loopback_no_sdk_controls(self):
  s=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn(".concat(['jegotrip-connect-startup-detail.spec.ts'])",s)
  for fence in ['workers: 1','retries: 0','timeout: 45_000',"VITE_API_PROXY_TARGET: 'http://127.0.0.1:1'"]:self.assertIn(fence,s)
  s=(ROOT/'frontend/e2e/jegotrip-connect-startup-detail.spec.ts').read_text(encoding='utf-8');self.assertIn('expect(writes).toEqual([])',s);self.assertIn('expect(reads).toBe(prior + 1)',s)
 def test_account_http_fixtures_keep_no_ready_no_tls_and_all_join_assertions(self):
  s=(ROOT/'backend/src/api/app_voice_connect_detail_http_tests.rs').read_text(encoding='utf-8');self.assertIn('StatusCode::UNAUTHORIZED',s);self.assertIn('no_store(&headers)',s);self.assertIn('"startup_failure"].as_object().unwrap().len(),3',s);self.assertIn('"startup_connect_detail"].as_object().unwrap().len(),2',s);self.assertIn('data.get("startup_tls_detail").is_none()',s);self.assertIn('"io_cleanup_required"],false',s);self.assertIn('SELECT COUNT(*) FROM app_voice_runtime_leases',s)
if __name__=='__main__':unittest.main()
