"""Supplemental UI-baseline/unknown and original user WRITE lifecycle fences."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(p):return(R/p).read_text(encoding='utf-8')
class UserReadHardeningContracts(unittest.TestCase):
 def test_current_scene_and_generation_baseline_precede_explicit_user_nonce(self):
  s=text('frontend/src/pages/sim/AppImMessagesPane.tsx');self.assertIn('userReadBaselineReady(userOutcomes, lineId, active.current.generation)',s);self.assertIn('userOutcomes.context !== userReadContext',s);self.assertLess(s.index('userReadBaselineReady(userOutcomes'),s.index('crypto.randomUUID()'));self.assertIn('JSON.stringify([lineId, userRead?.revision, userRead?.installed, userRead?.hosted])',s);self.assertIn('[lineId, userRead?.installed, userReadContext]',s)
 def test_existing_unknown_cannot_be_cleared_by_a_prior_or_newer_terminal(self):
  s=text('frontend/src/policies/imUserReadState.ts');self.assertIn('row.id > unknown.after',s);self.assertIn("row.state === 'uncertain'",s);self.assertIn('userReadKnownUnknown(snapshot, line, generation, unknown.id, unknown.index)',s);self.assertIn('row.messageId === unknown.id && row.index === unknown.index',s)
 def test_controls_and_outcomes_reject_a_prior_account_or_configuration(self):
  s=text('frontend/src/pages/sim/AppImMessagesPane.tsx');self.assertIn('userOutcomes.context === userReadContext ? userOutcomes : null',s);self.assertIn('!currentUserOutcomes || userReadKnownUnknown',s);self.assertIn('generation: original.generation, context: userReadContext',s)
 def test_original_submission_observation_is_test_only_not_new_permission(self):
  s=text('backend/src/connectivity/app_voice/providers/jegotrip/im_user_read_journal.rs');self.assertIn('#[cfg(test)]\n    pub(in super::super) fn write_was_submitted(&self)->bool{self.phase.load(Ordering::SeqCst)==SUBMITTED}',s);self.assertIn('compare_exchange(WRITING,SUBMITTED,Ordering::SeqCst',s);self.assertIn('self.context.action.scope.close()',s)
 def test_full_physical_write_drop_parent_and_sql_failure_have_actual_join_and_unknown_assertions(self):
  s=text('backend/src/connectivity/app_voice/providers/jegotrip/im_user_read_lifecycle_io_tests.rs')
  for k in ['commands.user_read_write','permit.write_was_submitted()','drop(writing)','parent.retire()','BEFORE UPDATE OF state ON app_voice_im_user_reads','closed.io_released()','assert_eq!(state(&temp),(0,Some(id)))','peer.next().await.is_none()']:self.assertIn(k,s)
  self.assertNotIn('ImReadWait',s);self.assertNotIn('action.cancel()',s);self.assertNotIn('#[ignore',s)
 def test_lifecycle_cases_are_mounted_under_retained_native_filter(self):
  s=text('backend/src/connectivity/app_voice/providers/jegotrip/im_user_read_io_tests.rs');self.assertIn('#[path="im_user_read_lifecycle_io_tests.rs"]\nmod lifecycle;',s);s=text('backend/src/connectivity/app_voice/providers/jegotrip/im_read_actor_tests.rs');self.assertIn('#[path="im_user_read_io_tests.rs"]\nmod user_reads;',s)
 def test_browser_race_cases_do_not_relax_time_retry_workers_or_execute_native_sdk(self):
  s=text('frontend/e2e/jegotrip-im-user-read-state.spec.ts');self.assertIn('releaseBaseline()',s);self.assertIn('releaseOld()',s);self.assertIn('toBeDisabled()',s);self.assertIn('await route.abort()',s)
  for k in ['setDefaultTimeout','chromium.launch','localhost:12345','retries:']:self.assertNotIn(k,s)
  p=text('frontend/playwright.jegotrip.config.ts')
  for k in ['timeout: 45_000','timeout: 10_000','workers: 1','retries: 0','timeout: 120_000']:self.assertIn(k,p)
if __name__=='__main__':unittest.main()
