"""Buddy check SOURCE fences supplement, never substitute for original Actions."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class BuddyCheckContracts(unittest.TestCase):
 def test_new_node_test_promises_are_explicit_without_changing_titles_or_assertions(self):
  s=(R/'frontend/tests/buddyRelationCheck.test.ts').read_text(encoding='utf-8');self.assertEqual(s.count('\nvoid test('),5);self.assertNotIn('\ntest(',s)
 def test_policy_is_independent_and_absent_preserves_legacy_serialization(self):
  s=src('account_config.rs');self.assertIn('pub buddy_relation_check:Option<',s);self.assertIn('with_buddy_relation_check(p.buddy_relation_check)',s)
  s=src('native_receiver.rs');self.assertIn('buddy_relation_check:false',s);self.assertIn('if self.buddy_relation_check||self.im_messages.is_none()',s)
  s=src('runtime_backend.rs').split('fn with_buddy_relation_check',1)[1].split('fn with_relation_roster',1)[0];self.assertIn('self.common.is_none()||!self.client_ex',s);self.assertNotIn('directory_queries',s)
 def test_wire_is_version_zero_one_uid_and_raw_unsigned32_footer_not_roster_or_write(self):
  s=src('buddy_relation_check.rs');self.assertIn('checkRelation.GroupServer.Group',s);self.assertIn('Writer::versioned_measured(9+uid.len())',s);self.assertIn('w.bytes(uid)?',s);self.assertIn('let relation_type=r.u32()?',s);self.assertIn('accepted&&footer.reason.is_empty()',s)
  for bad in ['setRelations','createOrganize','setProps','UPDATE app_voice','INSERT INTO','read_directory(']:self.assertNotIn(bad,s)
 def test_decode_precharges_footer_vectors_before_allocation_and_keeps_budget_to_drop(self):
  s=src('buddy_relation_check.rs');b=s.split('pub(super) fn decode_size',1)[1].split('pub(super) fn decode<',1)[0]
  for bad in ['Vec::','vec![','.to_vec()','.properties()?']:self.assertNotIn(bad,b)
  b=s.split('pub(super) fn decode<',1)[1];self.assertLess(b.index('reserve_native_rpc'),b.index('read_footer'));self.assertIn('impl Drop for CheckResult',s)
 def test_query_uses_original_policy_context_gate_scope_and_admission_deadline(self):
  s=src('rpc_im.rs');self.assertIn('!binding.policy().buddy_relation_check()',s);self.assertIn('query.buddy_context_current(binding.owner(),&binding.scope(),&gate)',s);self.assertIn('BuddyCheck{deadline:original',s);self.assertIn('context.as_ref().is_some_and',s)
  s=src('arc_session.rs');self.assertIn('!query.buddy_context_current(&self.owner,&self.connection,&gate)',s);b=s.split('fn prepare_buddy_check_read',1)[1].split('fn pending_buddy_check',1)[0];self.assertLess(b.index('reserve_native_rpc'),b.index('query.arguments('));self.assertIn('self.next_id=id.checked_add(1)',b)
 def test_account_input_copies_only_after_original_budget_and_before_await(self):
  s=src('runtime_backend.rs').split('fn reserve_buddy_relation_check',1)[1].split('async fn check_buddy_relation',1)[0];self.assertLess(s.index('reserve_native_rpc'),s.index('ImQuery::buddy_check'));self.assertIn('state.logout_started||state.unregister_started',s)
  s=src('account_buddy_relation_check.rs');self.assertIn('CheckContext::capture',s);self.assertIn('Arc::ptr_eq(reply.request().query_arc(),&query)',s);self.assertIn('timeout_at(deadline.into()',s);self.assertIn('check_revision(line,revision)',s)
 def test_context_checks_sql_then_live_and_packet_guard_survives_real_channel_retries(self):
  s=src('buddy_relation_check.rs');self.assertIn('configuration.require_current',s);self.assertIn('account_runs::require_live',s);self.assertIn('.is_ok()&&live()',s);self.assertIn('self.0.retired.store',s);self.assertIn('pub(super) Arc<super::rpc_im::ImQuery>',s)
  s=src('group_event_packets.rs');self.assertIn('capture_buddy_check',s);self.assertIn('self.buddy_check.as_ref().is_none_or(|r|r.active())',s)
  s=src('arc_protocol.rs');self.assertIn('packet.retain_buddy_check_packet',s);self.assertIn('capture_buddy_check(&self.rpc.connection_scope()',s)
  s=src('router_runtime.rs');self.assertIn('poll_original_write',s);s=src('reliable_channel.rs');self.assertIn('GroupEventPackets',s)
 def test_http_private_target_is_wiped_before_await_and_projection_is_two_fields(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('buddy-relation/check",post(check_buddy_relation)',s);b=s.split('pub async fn check_buddy_relation',1)[1].split('pub async fn read_native_directory',1)[0];self.assertLess(b.index('drop(request)'),b.index('.await'));self.assertIn('struct BuddyRelationCheckRequest',s)
  s=src('account_buddy_relation_check.rs');self.assertIn('pub accepted:bool,pub relation_type:u32',s)
 def test_ui_explicit_read_never_uses_persistence_nonce_write_or_auto_retry(self):
  s=(R/'frontend/src/pages/sim/AppBuddyRelationCheckPane.tsx').read_text(encoding='utf-8');self.assertIn('const captured = { ...body }',s);self.assertIn('window.confirm(',s);self.assertIn('current.current.generation === generation',s);self.assertIn('props.hosted, props.installed',s)
  for bad in ['localStorage','setInterval','setTimeout','updateAppRelations','setAppAccountRunning','crypto.']:self.assertNotIn(bad,s)
  s=(R/'frontend/src/policies/buddyRelationCheck.ts').read_text(encoding='utf-8');self.assertIn('Reflect.ownKeys',s);self.assertIn('0xffffffff',s);self.assertIn('Unknown-WRITE',s);self.assertIn('.exec(uid)?.[0] !== uid',s)
 def test_original_filters_mount_protocol_account_physical_io_and_http_regressions(self):
  for parent,leaf in [('im_protocol_tests.rs','buddy_relation_check_tests.rs'),('group_directory_account_tests.rs','buddy_relation_check_account_tests.rs'),('im_read_actor_tests.rs','buddy_relation_check_io_tests.rs')]:self.assertIn('#[path="'+leaf+'"]',src(parent))
  self.assertIn('include!("app_voice_buddy_check_http_tests.rs")',(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
