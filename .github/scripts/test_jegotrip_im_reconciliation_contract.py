"""Supplemental independently consented original READ-only observation fences."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return(P/name).read_text(encoding='utf-8')
class ReconciliationContracts(unittest.TestCase):
 def test_observation_never_mints_write_permits_resolves_unknown_or_mutates_database(self):
  s=src('im_read_reconciliation.rs')
  for no in ['UserReadPermit','UPDATE ','INSERT ','DELETE ','state=1','state=3','publish_committed_app_event','GapCommitted']:self.assertNotIn(no,s)
  self.assertIn('TransactionBehavior::Deferred',s);self.assertIn('reported_last_read',s)
 def test_current_receipt_and_original_message_native_identity_box_index_fingerprint_are_checked(self):
  s=src('im_read_reconciliation.rs');self.assertIn('WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4',s);self.assertIn('AND box_id=?5 AND message_index=?6 AND fingerprint=?7',s);self.assertIn('context.validate(&tx)?',s);self.assertIn('current.fingerprint != self.record.fingerprint',s)
 def test_query_uses_original_binding_query_gate_and_fixed_deadline_only(self):
  s=src('im_read_reconciliation.rs')
  for k in ['Arc::ptr_eq(reply.request().binding(), &self.context.binding)','Arc::ptr_eq(reply.request().query_arc(), &self.query)','Arc::ptr_eq(&reply.request().gate(), &self.action.gate())','Instant::now() < self.deadline','ImQuery::refresh(&[label], -1)']:self.assertIn(k,s)
 def test_missing_or_rejected_positions_never_mean_no_io_or_original_ack(self):
  s=src('im_read_reconciliation.rs');self.assertIn('if !status.accepted { "rejected" } else if entry.is_some() { "reported" } else { "unreported" }',s);self.assertIn('entry.and_then(|e| e.last_read)',s);self.assertNotIn('read >=',s);self.assertNotIn('accepted.then_some("confirmed")',s)
 def test_runtime_reads_original_arc_without_installing_user_write_or_readsync_permission(self):
  s=src('runtime_im_reconciliation.rs');self.assertIn('self.profile.common.is_some() && self.im_messages_enabled()',s);self.assertIn('commands.original_im_binding',s);self.assertIn('commands.read_im',s);self.assertIn('timeout_at(action.deadline().into(), operation)',s)
  for no in ['user_read_write','write_received','self.user_read_enabled()','tokio::spawn','im_sync']:self.assertNotIn(no,s)
 def test_account_checks_separate_consent_revision_hosted_owner_and_drops_controller_before_io(self):
  s=src('account_im_reconciliation.rs');self.assertIn('if !consent',s);self.assertIn('self.check_revision(line, revision)?',s);self.assertIn('controller.phase() != Phase::Hosted',s);self.assertIn('run.configuration_current()?',s);self.assertIn('drop(controller);\n        run.backend.reconcile_user_read(action).await',s)
 def test_api_accepts_only_canonical_local_receipt_revision_and_read_consent(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('pub struct ImReconciliationRequest { expected_revision: u64, consent: bool }',s);self.assertIn('id.to_string() != receipt_id',s);self.assertIn('/im-user-reads/{receipt_id}/reconcile',s);self.assertIn('app_account_im_reconciliation_consent_required',s)
 def test_ui_only_projects_own_finite_data_and_never_overwrites_original_history_state(self):
  p=(R/'frontend/src/policies/imReconciliation.ts').read_text(encoding='utf-8');self.assertIn('Object.prototype.hasOwnProperty.call',p);self.assertIn('9223372036854775807n',p);self.assertIn('未知记录保持',p)
  p=(R/'frontend/src/pages/sim/AppImReconciliationPane.tsx').read_text(encoding='utf-8');self.assertIn('window.confirm',p);self.assertIn('current.current.generation !== original.generation',p);self.assertIn('setObserved',p)
  for no in ['markAppImUserRead','dangerouslySetInnerHTML','state: \'confirmed\'','setUserUncertain']:self.assertNotIn(no,p)
 def test_native_tests_are_mounted_and_read_cancellation_is_not_write_cancellation(self):
  self.assertIn('im_reconciliation_protocol_tests.rs',src('im_user_read_protocol_tests.rs'));self.assertIn('im_reconciliation_account_tests.rs',src('im_user_read_account_tests.rs'));self.assertIn('im_reconciliation_io_tests.rs',src('im_user_read_io_tests.rs'));s=src('im_reconciliation_io_tests.rs');self.assertIn('ImReadWait::original',s);self.assertIn('assert!(scope.is_open())',s);self.assertIn('handle.logout',s);self.assertIn('assert_eq!(unknown(&temp),0)',s)
 def test_new_cases_keep_standard_stack_timeout_retry_and_actions_only_budgets(self):
  for f in ['im_reconciliation_protocol_tests.rs','im_reconciliation_account_tests.rs','im_reconciliation_io_tests.rs']:
   s=src(f)
   for no in ['#[ignore','RUST_MIN_STACK','stack_size','Command::new("adb")','dlopen']:self.assertNotIn(no,s)
  s=(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8')
  for k in ['timeout: 45_000','timeout: 10_000','workers: 1','retries: 0','timeout: 120_000']:self.assertIn(k,s)
if __name__=='__main__':unittest.main()
