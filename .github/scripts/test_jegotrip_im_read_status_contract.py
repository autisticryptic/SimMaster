"""Supplemental manual group read-status source invariants, not native execution."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(n):return(P/n).read_text(encoding='utf-8')
class ReadStatusContracts(unittest.TestCase):
 def test_manual_policy_is_separate_strict_and_no_default_write_or_ready(self):
  s=text('im_read_status_policy.rs');self.assertIn('deny_unknown_fields',s);self.assertIn('pub set_read_status:bool',s);self.assertIn('im_read_status:None',text('runtime_backend.rs'));self.assertIn('with_im_read_status(p.im_read_status.as_ref())',text('account_config.rs'))
 def test_only_current_original_span_consumer_can_issue_live_targets(self):
  s=text('im_read_status.rs');self.assertIn('span:&GapCommitted',s);self.assertIn('span.captured_copy()',s);self.assertIn('span.validate_current(&tx)',s);self.assertIn('targets:Mutex<BTreeMap<i64,Arc<ReadStatusTarget>>>',s);self.assertNotIn('Deserialize',s);self.assertIn('targets.observe(&committed)',text('account_im_sync.rs'))
 def test_expected_index_is_correlation_data_not_wire_or_permission(self):
  s=text('im_read_status.rs');self.assertIn('target.span.end().to_string()!=expected_index',s);self.assertIn('!target.span.is_current',s);self.assertIn('context.binding.matches(&owner,&scope)',s)
 def test_manual_and_automatic_uncertainty_share_original_received_journal(self):
  s=text('im_received_journal.rs');self.assertIn('manual:Option<Arc<',s);self.assertIn('manual.is_none()&&state==1&&target>=index',s);self.assertIn('state==0{return Err(UNRESOLVED);}',s);self.assertIn('manual.arm(&tx,id)',s);self.assertIn('manual.resolve(&tx,id,accepted,result_code)',s)
 def test_possible_manual_write_stays_unknown_and_queued_records_same_original_no_io(self):
  s=text('im_received_journal.rs');self.assertIn('manual.cancel_queued(&tx)',s);self.assertIn('self.context.binding.scope().close()',s);self.assertIn('phase.load(Ordering::SeqCst)!=RESOLVED',s)
 def test_target_retirement_waits_for_original_quiescence_and_never_erases_unknown_history(self):
  s=text('account_runs.rs');self.assertIn('retire_read_status_targets(&tx,token)',s);s=text('im_read_status.rs');part=s.split('fn retire_targets',1)[1].split('fn delete_account',1)[0];self.assertNotIn('DELETE FROM app_voice_im_read_status_actions',part);self.assertIn('MAX_LOCAL:i64=128',s);self.assertIn('MAX_GLOBAL:i64=4096',s)
 def test_runtime_keeps_manual_policy_and_original_authenticated_binding_outside_controller_locks(self):
  s=text('runtime_read_status.rs');self.assertIn('commands.original_im_binding',s);self.assertIn('Arc::ptr_eq(&binding,&action.binding())',s);self.assertIn('commands.write_received',s);self.assertNotIn('im_receipt_policy()',s);self.assertNotIn('tokio::spawn',s);self.assertIn('drop(controller);let result=run.backend.set_read_status(action).await?',text('account_read_status.rs'))
 def test_api_and_ui_require_explicit_intent_no_raw_native_uid_token_box_or_callback(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('pub struct ImReadStatusRequest{expected_revision:u64,expected_index:String,nonce:String,consent:bool}',s);s=(R/'frontend/src/pages/sim/AppImReadStatusPane.tsx').read_text(encoding='utf-8');self.assertLess(s.index('!selected'),s.index('crypto.randomUUID()'));self.assertIn('window.confirm',s);self.assertIn('current.current.generation !== original.generation',s);self.assertNotIn('dangerouslySetInnerHTML',s)
 def test_target_accounting_is_aggregate_and_last_holder_not_map_clear_or_scope_ready(self):
  s=text('im_read_status.rs');self.assertIn('aggregate:Arc<ReadStatusBudget>',s);self.assertIn('aggregate.0>=MAX_GLOBAL as usize||global>16*1024*1024',s);self.assertIn('impl Drop for TargetReservation',s);self.assertNotIn('aggregate.state',s.split('fn clear(&self)',1)[1].split('fn observe',1)[0]);self.assertIn('read_status_budget.clone()',text('account_runtime.rs'))
 def test_same_physical_prefix_and_drop_assertions_do_not_replace_io_or_enlarge_budget(self):
  s=text('im_read_status_io_tests.rs');self.assertIn('original_unarmed(1)',s);self.assertIn('prefix.accepted.load(Ordering::SeqCst),1',s);self.assertIn('prefix.dropped.load(Ordering::SeqCst)',s);self.assertIn('scope.native_rpc_resources(),(0,0)',s);self.assertNotIn('RUST_MIN_STACK',s);s=text('user_entry_io_test_gate.rs');self.assertIn('Self::with_activation(prefix,true)',s)
 def test_manual_absolute_deadline_is_used_for_original_rpc_and_physical_writer(self):
  self.assertIn('let deadline=permit.effective_deadline(deadline)',text('rpc_im_received_session.rs'));self.assertIn('work.permit.effective_deadline(native_deadline.ok_or(RuntimeError::Timeout)?)',text('transport_actor.rs'));self.assertIn('deadline.min(manual.deadline())',text('im_received_journal.rs'))
 def test_ui_whole_history_current_generation_and_serial_precede_new_nonce(self):
  s=(R/'frontend/src/pages/sim/AppImReadStatusPane.tsx').read_text(encoding='utf-8');self.assertIn('readStatusBaselineReady(loaded, context, current.current.generation)',s);self.assertIn('current.current.readSerial === serial',s);self.assertIn('row.id <= after',s);self.assertIn('readStatusHistory(history.data)',s);self.assertIn('im-read-status-actions?limit=128',(R/'frontend/src/api/current.ts').read_text(encoding='utf-8'))
 def test_configuration_revision_is_not_an_acknowledgement_of_same_native_box_unknown(self):
  fence='WHERE line_id=?1 AND identity_key=?2 AND box_id=?3 AND state=0';self.assertIn(fence,text('im_read_status.rs'));self.assertIn(fence,text('im_received_journal.rs'));self.assertIn('fresh_unknown(true)',text('im_read_status_account_tests.rs'))
 def test_only_submitted_write_provenance_maps_generic_closed_and_never_overrides_revocation(self):
  s=text('im_received_journal.rs');self.assertIn('const CLOSED_WRITE:u8=5',s);self.assertIn('if phase==QUEUED{RETIRED}else{CLOSED_WRITE}',s);self.assertIn('phase.load(Ordering::SeqCst)>=RETIRED',s);s=text('transport_actor.rs');mapping='if result==RuntimeError::Closed{self.received_write_close_reason().unwrap_or(result)}else{result}';revoked='if self.scope.session_revoked() { RuntimeError::SessionRevoked } else { result }';self.assertIn(mapping,s);self.assertLess(s.index(mapping),s.index(revoked));self.assertIn('if p.permit.closed_original_io(){self.received_write_retirement.get_or_insert(reason);}',text('im_received_actor.rs'))
 def test_retired_target_ledger_does_not_restore_permission_from_old_holder_or_live_sdk(self):
  s=text('im_read_status.rs');self.assertIn('self.resources.retired.store(true,Ordering::SeqCst)',s);self.assertIn('self.target.live()&&self.parent_gate.live()',s);self.assertIn('!target.live()||!target.span.is_current',s);self.assertIn('retired_target_store_holder_is_not_current_permission',text('im_read_status_protocol_tests.rs'))
 def test_original_full_cancelled_assertion_and_typed_no_io_provenance_cases_remain_strict(self):
  s=text('im_read_status_io_tests.rs');self.assertIn('assert_eq!(closed.reason,reason)',s);self.assertIn('manual_read_status_io_full_wait_drop_keeps_both_unknown_and_joins_original_stream',s);self.assertIn('RuntimeError::Cancelled};bounded(scope.closed())',s);self.assertNotIn('setDefaultTimeout',s);self.assertNotIn('#[ignore',s);self.assertIn('queued_retirement_never_supplies_a_generic_sdk_close_cause',text('im_read_status_protocol_tests.rs'))
if __name__=='__main__':unittest.main()
