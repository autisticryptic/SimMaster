"""Supplemental controlled original received WRITE fences; Actions execute Rust/IO."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return(P/name).read_text(encoding='utf-8')
class ReceiptContracts(unittest.TestCase):
 def test_installation_is_separate_strict_and_needs_current_native_read_sync(self):
  s=src('im_receipt_policy.rs');self.assertIn('deny_unknown_fields',s);self.assertIn('pub set_received:bool',s);self.assertIn('with_im_receipts',src('account_config.rs'));s=src('runtime_backend.rs');self.assertIn('self.im_sync.is_none()',s);self.assertIn('im_receipts:None',s);self.assertIn('self.im_receipt_policy().is_none()',s)
 def test_only_private_contiguous_gap_can_admit_a_fixed_write_and_durable_marker(self):
  s=src('im_received_journal.rs');self.assertIn('span:&GapCommitted',s);self.assertIn('if !span.is_current(&database)',s);self.assertIn('if state==0{return Err(UNRESOLVED);}',s);self.assertIn('state=0,request_id=NULL',s);self.assertIn('scope_epoch',s);self.assertNotIn('Deserialize',s)
 def test_original_id_arms_once_on_a_marker_before_io_and_never_reuses_rpc_id(self):
  s=src('im_received_journal.rs');self.assertIn('state=0 AND request_id IS NULL',s);self.assertIn('if request.is_some(){return Err(SCOPE);}',s)
  s=src('rpc_im_received_session.rs');self.assertIn('binding.authenticates(&auth.user_id,&auth.selected_session_id)',s);self.assertIn('permit.arm_request(id,deadline)',s);self.assertIn('self.next_id=id.checked_add(1)',s);self.assertIn('kind:PendingKind::ImWrite',s)
 def test_read_api_still_denies_setreceived_but_durable_write_has_separate_original_actor_path(self):
  s=src('arc_session.rs');read=s.split('fn prepare_im_read',1)[1].split('fn take_im_replies',1)[0];self.assertIn('ImOperation::SetReceived',read);self.assertLess(read.index('ImOperation::SetReceived'),read.index('let id=self.next_id'))
  self.assertIn('Operation::WriteReceived(work)',src('transport_actor.rs'));self.assertIn('prepare_received_write(permit,deadline)',src('offline_backend.rs'))
 def test_possible_partial_and_submitted_write_cancel_keep_unknown_and_close_original_scope(self):
  s=src('im_received_journal.rs');self.assertIn('original==QUEUED',s);self.assertIn('state=3',s);self.assertIn('matches!(original,WRITING|SUBMITTED)',s);self.assertIn('self.context.binding.scope().close()',s);self.assertIn('if phase>=RESOLVED{return;}',s)
 def test_only_owned_native_reply_can_resolve_same_nonce_request_span_and_current_sql_lease(self):
  s=src('im_received_journal.rs');self.assertIn('reply:&OwnedReceivedReply',s);self.assertIn('reply.belongs_to(self)',s);self.assertIn('AND attempt_nonce=?2 AND state=0 AND request_id=?3',s);self.assertIn('AND target_index=?6 AND span_hash=?7',s);self.assertIn('self.context.validate(&tx)',s)
  s=src('rpc_im_received.rs');self.assertIn('std::ptr::eq(Arc::as_ptr(&self.request.permit),permit)',s);self.assertIn('Arc::ptr_eq(&self.request.query,&permit.query())',s);self.assertNotIn('Serialize',s)
 def test_original_matched_negative_version_results_never_trigger_automatic_replay(self):
  s=src('rpc_im_received.rs');self.assertIn('if self.code!=0{return Ok(false);}',s);self.assertNotIn('retry(',s);self.assertNotIn('prepare_received_write(',s);self.assertIn('app_account_im_receipt_not_replayed',src('im_received_journal.rs'))
 def test_write_deadline_and_cancel_differ_from_read_interest_only_retirement(self):
  s=src('im_received_actor.rs');self.assertIn('p.permit.cancel();self.backend.retire_received_write(id)',s);self.assertIn('now>=p.deadline',s);self.assertIn('p.permit.resolve_original(&reply)',s);self.assertNotIn('ImReadInterest',s);self.assertNotIn('tokio::spawn',s)
 def test_one_selected_heap_write_chain_leaves_controller_and_callbacks_available(self):
  s=src('account_im_sync.rs');self.assertIn('enum Completion{Read(',s);self.assertIn('Write(Result<OwnedReceivedReply',s);self.assertIn('self.writing.take()',s);self.assertIn('self.reading.is_some()||self.writing.is_some()',s);self.assertIn('backend.write_received(&owner,&scope,permit).await',s);self.assertNotIn('tokio::spawn',s)
  self.assertIn('im_sync::Completion::Write(result)',src('account_runtime.rs'))
 def test_received_io_uses_same_original_heap_send_and_parent_gate_not_unprotected_socket(self):
  s=src('transport_actor.rs');self.assertIn('work.permit.begin_write(packets.call_id)',s);self.assertIn('im_gates.push(work.permit.gate())',s);self.assertIn('permit.cancelled().await',s);self.assertIn('Box::pin(self.send_packets',s);self.assertIn('MAX_PENDING_RPC',s)
 def test_unknown_journal_is_not_cleared_by_run_quiescence_or_normal_restart(self):
  s=src('account_runs.rs');self.assertNotIn('DELETE FROM app_voice_im_read_receipts',s);self.assertIn('super::receipts::delete_account(conn,line)',src('im_associations.rs'));self.assertIn('account_store::metadata(conn,line)?.is_some()',src('im_received_journal.rs'))
 def test_metadata_api_is_no_store_authenticated_and_has_no_caller_bool_write_endpoint(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('im-receipts", get(get_im_receipts)',s);self.assertNotIn('post(get_im_receipts)',s);self.assertIn('im_received_receipts',s)
  s=src('im_received_journal.rs').split('fn metadata(',1)[1].split('struct ReceivedWait',1)[0];self.assertNotIn('request_id',s);self.assertNotIn('attempt_nonce',s);self.assertNotIn('scope_epoch',s)
 def test_frontend_receipt_ui_is_readonly_finite_and_never_raw_html_or_replay(self):
  s=(R/'frontend/src/pages/sim/AppImReceiptsPane.tsx').read_text(encoding='utf-8');self.assertIn('getAppImReceipts',s);self.assertNotIn('dangerouslySetInnerHTML',s);self.assertNotIn('write_received',s);self.assertIn('generation.current === original',s)
  s=(R/'frontend/src/policies/imReceipts.ts').read_text(encoding='utf-8');self.assertIn('Object.prototype.hasOwnProperty.call',s);self.assertIn('rows.length === 16',s);self.assertIn('9223372036854775807n',s)
 def test_new_protocol_real_io_account_http_and_ui_suites_are_mounted_without_ignores(self):
  self.assertIn('im_received_protocol_tests.rs',src('im_protocol_tests.rs'));self.assertIn('im_received_io_tests.rs',src('im_read_actor_tests.rs'));self.assertIn('im_received_account_tests.rs',src('im_account_tests.rs'))
  self.assertIn('app_voice_im_receipt_http_tests.rs',(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
  self.assertIn('imReceipts.test.ts',(R/'frontend/package.json').read_text(encoding='utf-8'));self.assertIn('jegotrip-im-receipts.spec.ts',(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8'))
  for f in ['im_received_protocol_tests.rs','im_received_io_tests.rs','im_received_account_tests.rs']:self.assertNotIn('#[ignore',src(f))
if __name__=='__main__':unittest.main()
