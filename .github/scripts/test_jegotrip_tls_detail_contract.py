"""Supplemental SOURCE-only finite TLS diagnostics fences, not native execution."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(file):return(P/file).read_text(encoding='utf-8')
class TlsDetailContract(unittest.TestCase):
 def test_detail_classifier_never_formats_or_serializes_raw_rustls_error_context(self):
  s=text('tls_failure_detail.rs');self.assertIn('from_rustls(error:&rustls::Error)',s);self.assertNotIn('format!(',s);self.assertNotIn('error.to_string()',s);self.assertIn('NotValidForNameContext{..}',s);self.assertIn('General',text('tls_failure_detail_tests.rs'))
 def test_scope_original_io_gate_and_budget_identity_are_not_diagnostic_mutex(self):
  s=text('rpc_pending.rs');self.assertIn('Arc<Mutex<()>>',s);self.assertIn('Arc::ptr_eq(&self.4,budget)',s);self.assertIn('self.4.reserve(self,bytes)',s);self.assertIn('tls_detail:super::tls_failure_detail::TlsDiagnosticStore',text('native_rpc_budget.rs'))
 def test_tls_error_and_refusal_behavior_keep_old_exact_code_and_san_checks(self):
  s=text('tls_engine.rs');self.assertIn('Self::HandshakeFailed => "jegotrip_tls_handshake_failed"',s);self.assertIn('has_exact_identity(peer.as_ref(), &self.authority)',s);self.assertIn('with_root_certificates(roots).with_no_client_auth()',s);self.assertIn('with_safe_default_protocol_versions()',s);self.assertNotIn('dangerous()',s)
 def test_detail_capture_happens_before_handshake_failed_and_not_by_error_text(self):
  s=text('tls_engine.rs');self.assertIn('detail_scope.record_tls_detail(&detail_owner,reason);TlsError::HandshakeFailed',s);self.assertIn('TlsFailureReason::from_rustls(&error)',s);self.assertIn('TlsFailureReason::ExactNameMissing',s)
 def test_legacy_startup_dto_stays_exact_three_field_and_detail_is_separate(self):
  s=text('startup_diagnostics.rs');p=s[s.index('pub struct StartupFailure'):s.index('pub struct StartupTlsDetail')];self.assertIn('pub phase:StartupPhase',p);self.assertIn('pub stage:StartupStage',p);self.assertIn('pub code:&\'static str',p);self.assertNotIn('pub reason:',p);self.assertIn('pub struct StartupTlsDetail',s)
 def test_detail_is_original_owner_scope_first_failure_not_late_reason(self):
  s=text('startup_diagnostics.rs');self.assertIn('if &self.owner!=owner||!self.scope.matches(scope)',s);self.assertIn('failure.stage==StartupStage::RouterTls',s);self.assertIn('failure.tls_reason',s);self.assertIn('if first.is_none()',s)
 def test_new_detail_cases_have_original_workflow_filter_and_mounts(self):
  for file in ['build-release.yml','beta-validation.yml']:
   self.assertIn('connectivity::app_voice::providers::jegotrip::tls_failure_detail::tests',(R/'.github/workflows'/file).read_text(encoding='utf-8'))
  self.assertIn('mod tls_detail;',text('startup_diagnostics_tests.rs'));self.assertIn('mod tls_detail;',text('runtime_startup_diagnostics_tests.rs'));self.assertIn('app_voice_tls_detail_http_tests.rs',(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
 def test_ui_finite_detail_is_same_phase_and_router_tls_without_raw_coercion(self):
  s=(R/'frontend/src/policies/appLogin.ts').read_text(encoding='utf-8');self.assertIn('base.phase !== data.phase',s);self.assertIn("base.stage !== 'router_tls'",s);self.assertIn('Object.hasOwn',s);self.assertNotIn('String(base.phase)',s);self.assertIn('tlsStartupDetail.test.ts',(R/'frontend/package.json').read_text(encoding='utf-8'))
 def test_endpoint_router_uses_same_receive_classifier_before_any_router_control(self):
  s=text('tls_engine.rs');self.assertIn('let result = self.engine.receive(bytes, None, now)',s);r=text('router_runtime.rs');self.assertIn('EndpointTlsEngine::for_original',r);self.assertIn('self.engine.on_wire_bytes(&self.owner,&self.scope,bytes,Instant::now())?',r);self.assertIn('while path.engine.phase()!=TlsPhase::Verified',r)
 def test_original_engine_and_runtime_tests_mount_new_leaves_without_replacing_old_cases(self):
  self.assertIn('#[path="tls_engine_detail_tests.rs"]\nmod detail;',text('tls_engine_tests.rs'));self.assertIn('endpoint_router_tls_detail_separates_real_chain_name_ip_and_exact_san_rejections',text('tls_engine_detail_tests.rs'));self.assertIn('runtime_tls_detail_bootstrap_account_projection_keeps_first_phase_and_clears_only_after_stop_join',text('runtime_tls_detail_tests.rs'))
 def test_new_browser_is_selected_in_original_bounded_loopback_config(self):
  s=(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn("'jegotrip-tls-startup-detail.spec.ts'",s);self.assertIn('timeout: 45_000',s);self.assertIn('workers: 1',s);self.assertIn('retries: 0',s);self.assertIn("VITE_API_PROXY_TARGET: 'http://127.0.0.1:1'",s)
 def test_no_scope_cycle_or_new_budget_or_reset_path_is_created_by_detail_store(self):
  s=text('native_rpc_budget.rs');self.assertIn('state:Mutex<State>,tls_detail:super::tls_failure_detail::TlsDiagnosticStore',s);d=text('tls_failure_detail.rs');self.assertIn('struct Recorded {owner:AttemptOwner,reason:TlsFailureReason}',d);self.assertNotIn('RpcConnectionScope',d);self.assertNotIn('pub(super) fn reset',d);self.assertNotIn('pub(super) fn clear',d)
 def test_diagnostic_issuer_fixtures_are_distinct_without_changing_original_generator(self):
  s=text('tls_engine_detail_tests.rs');self.assertIn('pub(crate) struct DiagnosticCertificates(super::Certificates)',s);self.assertIn('original.directory.file_name()',s);self.assertIn('use DiagnosticCertificates as Certificates;',s);self.assertIn('impl std::ops::Deref for DiagnosticCertificates',s);self.assertIn('R::BadSignature',s)
  original=text('tls_engine_tests.rs');self.assertIn('"-subj", "/CN=fixture.invalid"',original);self.assertIn('pub(crate) use detail::DiagnosticCertificates;',original);self.assertIn('tls_engine::tests::DiagnosticCertificates as Certificates',text('runtime_tls_detail_tests.rs'))
if __name__=='__main__':unittest.main()
