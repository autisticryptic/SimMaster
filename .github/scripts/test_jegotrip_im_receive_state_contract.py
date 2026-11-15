"""Supplemental checked original IM receive ordering DATA fences, no IO proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
class ImReceiveStateContracts(unittest.TestCase):
 def test_timestamp_and_box_baseline_are_distinct_and_missing_status_cannot_seed_a_cursor(self):
  s=(P/'im_receive_state.rs').read_text(encoding='utf-8');self.assertIn('update_time:-1,initial_status:false,boxes:BTreeMap::new()',s)
  self.assertIn('entry.reported_range()?',s);self.assertIn('result.update_time<=self.update_time',s);self.assertIn('entry.reported_range().is_some()',s)
  self.assertNotIn('unwrap_or(0)',s);self.assertNotIn('unwrap_or(-1)',s)
 def test_gap_arithmetic_is_wide_checked_ten_bound_and_contiguous_commit_only(self):
  s=(P/'im_receive_state.rs').read_text(encoding='utf-8');self.assertIn('i128::from(self.target)-i128::from(start)+1',s);self.assertIn('GAP_BATCH_COUNT',s)
  self.assertIn('original.start!=cursor.consumed.checked_add(1)',s);self.assertIn('end>cursor.target',s);self.assertIn('cursor.consumed=end',s)
 def test_remote_status_or_observation_cannot_advance_local_consumption(self):
  s=(P/'im_receive_state.rs').read_text(encoding='utf-8');update=s.split('fn observe_status',1)[1].split('fn next_gap',1)[0]
  self.assertNotIn('old.consumed=',update);self.assertIn('old.target=old.target.max(cursor.target)',update)
  self.assertIn('if !self.initial_status{return Err',s);self.assertIn('index.checked_sub(1)',s)
 def test_box_state_is_bounded_zeroized_and_not_serializable_or_an_io_grant(self):
  s=(P/'im_receive_state.rs').read_text(encoding='utf-8');self.assertIn('MAX_BOXES:usize=128',s);self.assertIn('self.boxes.len().checked_add(added)',s)
  self.assertIn('impl Drop for BoxKey',s);self.assertIn('self.0.zeroize()',s);self.assertIn('ImReceiveState(<redacted>)',s)
  for forbidden in ['Serialize','Deserialize','AttemptOwner','RpcConnectionScope','connect(','tokio::spawn','std::fs','SdkLoginRequest','NativeImConfig']:
   self.assertNotIn(forbidden,s)
 def test_new_tests_are_mounted_under_original_status_filter_and_never_ignore_failures(self):
  self.assertIn('im_receive_state_tests.rs',(P/'im_status_tests.rs').read_text(encoding='utf-8'))
  self.assertNotIn('#[ignore',(P/'im_receive_state_tests.rs').read_text(encoding='utf-8'))
  for workflow in ['build-release.yml','beta-validation.yml']:
   self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_incoming::tests',(R/'.github/workflows'/workflow).read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
