"""Supplementary source fences, not Rust/native/real-network verification."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(name):return(P/name).read_text(encoding='utf-8')
class UserEntryRuntimeContract(unittest.TestCase):
 def test_parent_wrapper_does_not_reenter_original_router_child_mutex(self):
  s=text('user_entry_io.rs');self.assertIn('parent.with_io_gate',s);self.assertNotIn('child.with_io_gate',s);self.assertIn('if self.current()',s);self.assertIn('session_revoked()',s)
 def test_auth_capture_requires_matched_original_native_session_not_credentials(self):
  s=text('user_entry_auth.rs');self.assertIn('ArcPhase::Authenticated',s);self.assertIn('common_transport_claimed',s);self.assertIn('self.authentication.as_ref()',s);self.assertNotIn('Deserialize',s);self.assertNotIn('password',s)
 def test_query_has_exact_method_target_cc_monotonic_once_armed_and_current_whole_group(self):
  s=text('user_entry_rpc.rs');self.assertIn('AID_TO_UID_METHOD',s);self.assertIn('reference.identity()',s);self.assertIn('DeviceInfo.CC',s);self.assertIn('lease.id()',s);self.assertIn('lease.arm(',s);self.assertIn('if matched.is_some()',s);self.assertIn('Arc::ptr_eq(action,&self.query.action)',s);self.assertNotIn('loginSession.',s);self.assertNotIn('setStatus.',s);self.assertNotIn('setRead.',s)
 def test_source_parent_router_and_realm_remain_separate_provenance(self):
  s=text('user_entry_target.rs');self.assertIn('UserEntryReference::from_bootstrap',s);self.assertIn('parent.matches(source)',s);self.assertIn('router.original_identity()',s);self.assertIn('self.router.matches(router)',s);self.assertIn('ReferenceTarget::Direct',s)
 def test_stop_waits_for_original_auxiliary_owner_without_detached_production_tasks(self):
  s=text('runtime_backend.rs');self.assertEqual(s.count('attempt.user_entries.retired_and_empty().await'),2);b=text('user_entry_ownership.rs');self.assertIn('if lock(&self.state).scopes.is_empty(){return;}',b);self.assertIn('MAX_PENDING_BYTES:usize=128*1024',b)
  for name in ['user_entry_rpc.rs','user_entry_ownership.rs','runtime_user_entry.rs','account_user_entry.rs']:self.assertNotIn('tokio::spawn',text(name));self.assertNotIn('.abort(',text(name))
 def test_explicit_account_read_drops_controller_before_original_io(self):
  s=text('account_user_entry.rs');self.assertIn('if !consent',s);self.assertIn('self.check_revision(line,revision)',s);self.assertIn('controller.phase()!=Phase::Hosted',s);self.assertLess(s.index('drop(controller)'),s.index('run.backend.resolve_user_entry(action).await'))
 def test_api_is_authenticated_exact_body_no_store_and_no_uid_or_write_parameters(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('pub struct UserEntryLookupRequest{expected_revision:u64,recipient:String,consent:bool}',s);self.assertIn('valid_user_entry_recipient(&request.recipient)',s);self.assertIn('zeroize(&mut self.recipient)',s);self.assertIn('/user-entry/lookup',s)
 def test_ui_only_explicit_query_and_generation_has_no_uid_projection(self):
  s=(R/'frontend/src/pages/sim/AppUserEntryPane.tsx').read_text(encoding='utf-8');self.assertIn('window.confirm',s);self.assertIn('current.current.generation !== original.generation',s);self.assertNotIn('setInterval',s);self.assertNotIn('setTimeout',s);self.assertNotIn('localStorage',s);self.assertNotIn('dangerouslySetInnerHTML',s);self.assertNotIn('nonce',s)
 def test_actual_typed_timeout_is_not_erased_by_generic_transport_mapping(self):
  s=text('user_entry_rpc.rs');self.assertIn('E::Timeout|E::Connect(ConnectError::Timeout)|E::Tls(TlsError::Timeout)',s);self.assertIn('E::Netstream(error)=>netstream_error(error)',s);self.assertIn('.await.map_err(common_error)?',s)
 def test_timeout_cause_fix_does_not_expand_budgets_or_relabel_generic_io(self):
  s=text('user_entry_rpc.rs');self.assertIn('_=>UserEntryError::Transport',s);self.assertNotIn('saturating_duration_since',s);self.assertNotIn('RUST_MIN_STACK',s);t=text('user_entry_common_tests.rs');self.assertIn('Some("app_account_user_entry_timeout")',t);self.assertIn('Duration::from_secs(3)',t)
if __name__=='__main__':unittest.main()
