"""SOURCE-only explicit Group management WRITE boundaries, not execution proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class GroupManagementContracts(unittest.TestCase):
 def test_original_methods_and_mapping_literals_are_not_membership_or_read_aliases(self):
  s=src('group_management.rs')
  for x in ['setProps.GroupServer.Group','disbandOrganize.GroupServer.Group','Group.Org.Name','Group.Org.Type','Group.Org.Notify','Group.Org.JoinAuth','Group.Org.PropsControl','DiscussionGroup','LargeGroup','LargeRoom']:self.assertIn(x,s)
  self.assertIn('if op==GroupWriteOperation::Disband',s);self.assertIn('(None,None)',s)
 def test_default_off_independent_grants_and_explicit_destruction(self):
  self.assertIn('pub group_management:Option<',src('account_config.rs'));self.assertIn('group_management:None',src('runtime_backend.rs'));self.assertIn('Members(NativeRelationUpdateConfig)',src('relation_update_policy.rs'));self.assertIn('Management(super::group_management',src('relation_update_policy.rs'))
  self.assertIn('Disband&&!destructive_consent',src('account_relation_updates.rs'))
 def test_same_shared_nonce_intents_and_unknown_rows_guard_all_operation_families(self):
  s=src('relation_journal.rs');self.assertIn('original-group-management-body-v1',s);self.assertIn('original-relation-body-v1',s);self.assertIn('counts:[0;3]',s)
  s=src('relation_intent.rs');self.assertIn('management::capture(&tx',s);self.assertIn('line_id=?1 AND state=0',s);self.assertIn('if pending{return Err',s);self.assertIn('body.as_slice()!=body_hash.as_slice()',s)
  s=src('account_relation_updates.rs').split('fn prepare_group_management(',1)[1].split('/// Accounting only',1)[0];self.assertIn('RelationIntent::capture',s);self.assertNotIn('.await',s)
 def test_optional_descriptors_never_install_global_ddl_or_fake_times(self):
  s=src('account_store.rs');self.assertEqual(s.count('if config.profile.group_management.is_some(){super::group_management_store::initialize_schema(&tx)?;}'),2)
  self.assertNotIn('group_management_store::initialize_schema',src('im_inbox.rs'));self.assertIn('management::require',src('relation_intent.rs'));self.assertIn('management::require',src('relation_journal.rs'))
  self.assertIn('SetProperties=>base.is_some()||update.is_some()',src('relation_journal.rs'))
 def test_actual_packet_guard_reuses_scope_and_no_new_actor_worker(self):
  self.assertIn('capture_management',src('arc_protocol.rs'));self.assertIn('r.permit().packet_current(r.id())',src('group_event_packets.rs'));s=src('relation_journal.rs');self.assertIn('fn packet_current',s);self.assertIn('.is_ok()&&self.current(id)',s)
  self.assertNotIn('tokio::spawn',src('group_management.rs'))
 def test_http_input_is_strict_and_wiped_before_original_finish_await(self):
  s=(R/'backend/src/api/app_voice_group_management.rs').read_text(encoding='utf-8');self.assertIn('deny_unknown_fields',s);self.assertLess(s.index('reserve_group_management_http_scratch'),s.index('.collect::<Vec<_>>()'));self.assertLess(s.index('drop(request);drop(scratch)'),s.index('manager.finish_relation_update(p).await'))
 def test_browser_uses_same_original_lock_and_persisted_marker_without_fallback(self):
  s=(R/'frontend/src/pages/sim/AppGroupManagementPane.tsx').read_text(encoding='utf-8');self.assertIn('simadmin-relation-write-v1:',s);self.assertIn("ifAvailable: true",s);self.assertIn('setForm(empty)',s)
  s=(R/'frontend/src/policies/groupManagementFlow.ts').read_text(encoding='utf-8');self.assertLess(s.index('storeRelationMarker('),s.index('await ports.post(body)'));self.assertIn('relationMarkerResolved',s);self.assertIn("target: 'group', additions: 0",s)
if __name__=='__main__':unittest.main()
