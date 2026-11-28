"""Source guards for the protocol-only UE alias read. Native execution is in the original Actions."""
from pathlib import Path
import unittest
P=Path(__file__).resolve().parents[2]/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name): return (P/(name+'.rs')).read_text(encoding='utf-8')

class AliasProtocol(unittest.TestCase):
    def test_not_a_public_account_operation_or_member_write(self):
        for module in ('account_group_create','group_owner_only','runtime_relation_updates','transport_actor'):
            self.assertNotIn('begin_ue_alias_read',src(module));self.assertNotIn('reported_ue_pair',src(module))
        self.assertNotIn('pub mod ue_alias',src('mod'));self.assertNotIn('Serialize',src('ue_alias_snapshot'))
    def test_frozen_login_and_real_nested_tracker_sequence(self):
        s=src('offline_ue_alias');self.assertLess(s.index('initial_common_object_is'),s.index('self.prepare_login('));self.assertLess(s.index('self.prepare_login('),s.index('self.ue_origin = Some'))
        self.assertIn('inputs.domain_name != fields.domain_name',src('ue_alias_snapshot'));self.assertIn('b"DomainName" | b"Arc.Realm"',src('ue_alias_snapshot'))
        t=src('rpc_pending').split('fn queue_ue_alias',1)[1].split('fn prepare_client_user_entry',1)[0];self.assertLess(t.index('scheduler.queue_ue_alias('),t.index('self.next_id = sequence.checked_add(1)'))
    def test_group_commit_not_a_direct_or_epoch_ack(self):
        s=src('offline_backend');self.assertEqual(s.count('self.with_alias_group(owner, scope,'),3);self.assertIn('delivery.alias_result.take()',s)
        self.assertIn('if events.is_ok() && self.phase() == ArcPhase::Authenticated',src('offline_ue_alias'))
        b=src('batch_scheduler');self.assertLess(b.index('self.pending_client.contains(&sequence) && self.submitted_client.contains(&sequence)'),b.index('read.stage(result.result_code'))
        self.assertIn('result.belongs_to(r)',src('batch_ue_alias'))
    def test_reply_allocation_and_detached_packet_hold_original_budget(self):
        s=src('ue_alias_data');self.assertLess(s.index('scope.reserve_native_rpc(peak)?'),s.index('Vec::with_capacity(count)'));self.assertIn('read_footer(&mut r)?',s);self.assertIn('r.finish()?',s)
        self.assertIn('batch.alias_holder()',src('arc_protocol'));self.assertIn('_alias:Option<Arc<',src('client_packet_retention'))
    def test_old_full_reply_is_not_a_fresh_read_after_preemption_or_failure(self):
        s=src('ue_alias_snapshot');self.assertIn('Arc::ptr_eq(&self.read, read)',s);self.assertIn('impl Drop for AliasWait',s);self.assertIn('s.observed = Arc::downgrade(&self.read)',s);self.assertGreaterEqual(s.count('s.observed = Weak::new();'),2)
        self.assertIn('Instant::now() >= read.deadline',s);self.assertIn('!origin.scope.matches(scope)',s)
    def test_dedicated_leaf_runs_under_original_native_filter(self):
        self.assertIn('#[path="ue_alias_protocol_tests.rs"]\nmod ue_alias;',src('client_user_entry_protocol_tests'));self.assertIn('ue_alias_entire_inbound_group_finishes_before_fresh_view_is_published',src('ue_alias_protocol_tests'))
        for wf in ('build-release.yml','beta-validation.yml'):
            text=(Path(__file__).resolve().parents[2]/'.github/workflows'/wf).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::offline_backend::tests',text)
if __name__=='__main__': unittest.main()
