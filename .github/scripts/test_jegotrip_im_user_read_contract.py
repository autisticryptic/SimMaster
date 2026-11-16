"""Supplemental independently consented original user mark-read source fences."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class UserReadContracts(unittest.TestCase):
 def test_independent_policy_does_not_borrow_received_write_or_read_sync(self):
  p=src('im_user_read_policy.rs');self.assertIn('deny_unknown_fields',p);self.assertIn('pub mark_read:bool',p);self.assertIn('im_user_read:None',src('runtime_backend.rs'));self.assertIn('with_user_read(p.im_user_read.as_ref())',src('account_config.rs'))
 def test_user_method_argument_order_versions_index_string_and_epoch_time_are_original(self):
  s=src('im_user_read_data.rs');self.assertIn('setStatus.GroupServer.Group',s);self.assertIn('P:P_UDMR-N',s);self.assertIn('Writer::versioned()',s);self.assertIn('w.bytes(index.to_string().as_bytes())?',s);self.assertIn('w.i64(epoch_ms)',s);self.assertNotIn('setRead.Message.Message',s)
 def test_native_binding_is_captured_not_reconstructed_from_password_or_uid(self):
  s=src('im_user_read_actor.rs');self.assertIn('Operation::GetImBinding',s);self.assertIn('original_im_binding',src('runtime_user_read.rs'));self.assertIn('self.backend.captured_im_binding()',src('transport_actor.rs'));self.assertNotIn('ImBinding::authenticated',s)
 def test_current_local_message_body_and_original_uid_context_precede_target_query(self):
  s=src('im_user_read_journal.rs');self.assertIn('FROM app_voice_im_messages WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4',s);self.assertIn('deleted!=0||index<0',s);self.assertIn('key.kind()!=BoxKind::P2P',s);self.assertIn('ImQuery::refresh(&[label],-1)',s)
 def test_purpose_reply_requires_exact_binding_query_gate_and_matching_actual_latest(self):
  s=src('im_user_read_journal.rs');self.assertIn('Arc::ptr_eq(reply.request().binding(),&self.context.binding)',s);self.assertIn('Arc::ptr_eq(reply.request().query_arc(),&self.query)',s);self.assertIn('e.latest.is_some_and(|n|n>=self.index)',s);self.assertIn('self.verify_message(&tx,true)?',s)
 def test_unknown_write_marker_precedes_io_and_blocks_another_nonce_scope_or_target(self):
  s=src('im_user_read_journal.rs');self.assertIn('UNIQUE(line_id,user_nonce)',s);self.assertIn('AND box_id=?4 AND state=0',s);self.assertIn('if unknown{return Err(UNCERTAIN);}',s);self.assertIn('scope_epoch,state,updated_at)',s);self.assertIn('MAX_LOCAL:i64=128',s);self.assertIn('MAX_GLOBAL:i64=4096',s)
 def test_same_nonce_replay_is_only_a_readonly_finite_view_and_conflicts_cannot_rebind(self):
  s=src('im_user_read_journal.rs');self.assertIn('BoundUserRead::Replay(view)',s);self.assertIn('app_account_im_user_read_nonce_conflict',s);self.assertIn('saved_box.as_slice()!=box_id.as_slice()',s);self.assertNotIn('Deserialize',s)
 def test_write_identity_and_token_come_from_original_sdk_and_id_never_reuses(self):
  s=src('rpc_user_read_session.rs');self.assertIn('binding.authenticates(&auth.user_id,&auth.selected_session_id)',s);self.assertIn('value:&auth.token',s);self.assertIn('self.next_id=id.checked_add(1)',s);self.assertIn('kind:PendingKind::UserRead',s)
 def test_raw_boolean_read_or_received_reply_cannot_resolve_a_user_action(self):
  s=src('im_user_read_journal.rs');self.assertIn('reply:&OwnedUserReadReply',s);self.assertIn('reply.belongs_to(self)',s);self.assertIn('AND run_nonce=?7 AND scope_epoch=?8 AND fingerprint=?9',s);self.assertIn('self.phase.load(Ordering::SeqCst)!=SUBMITTED',s)
 def test_no_io_cancel_is_known_but_possible_write_retains_unknown_and_same_scope_retirement(self):
  s=src('im_user_read_journal.rs');self.assertIn('before==QUEUED',s);self.assertIn('SET state=3',s);self.assertIn('matches!(before,WRITING|SUBMITTED)',s);self.assertIn('self.context.action.scope.close()',s);self.assertIn('Weak<UserReadPermit>',s)
 def test_controller_stop_callbacks_and_original_joins_remain_available_under_one_action_deadline(self):
  s=src('account_runtime.rs');self.assertIn('drop(controller);run.backend.mark_user_read(action).await',s);self.assertIn('if !consent',s);s=src('runtime_user_read.rs');self.assertIn('timeout_at(action.deadline().into(),operation)',s);self.assertIn('parent_gate.retired()',s);self.assertNotIn('tokio::spawn',s)
 def test_status_lookup_never_exposes_box_nonce_token_or_native_request_id(self):
  s=src('im_user_read_journal.rs').split('fn metadata(',1)[1].split('struct UserReadAction',1)[0]
  for k in ['identity_key','box_id','scope_epoch','user_nonce','request_id']:self.assertNotIn(k,s)
  self.assertIn('im-user-reads', (R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8'))
 def test_api_body_contains_only_revision_consent_nonce_and_local_id(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('struct ImUserReadRequest{expected_revision:u64,nonce:String,consent:bool}',s);self.assertIn('/mark-read",post(mark_im_user_read)',s);self.assertIn('id.to_string()!=message_id',s)
 def test_frontend_requires_explicit_confirmation_new_nonce_and_unknown_query_only(self):
  s=(R/'frontend/src/pages/sim/AppImMessagesPane.tsx').read_text(encoding='utf-8');self.assertIn('window.confirm',s);self.assertIn('crypto.randomUUID()',s);self.assertIn('userUncertain',s);self.assertIn('userReadBaselineReady(userOutcomes, lineId, active.current.generation)',s);self.assertIn('userReadUnknownResolved(old, snapshot, lineId, generation)',s);self.assertIn('active.current.generation !== original.generation',s);self.assertNotIn('dangerouslySetInnerHTML',s)
  state=(R/'frontend/src/policies/imUserReadState.ts').read_text(encoding='utf-8');self.assertIn('row.id > unknown.after',state);self.assertIn('userReadKnownUnknown(snapshot, line, generation, unknown.id, unknown.index)',state);self.assertIn('snapshot.generation === generation',state)
 def test_no_original_budgets_or_action_filters_are_extended_by_user_read_suite(self):
  p=(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8')
  for k in ['timeout: 45_000','timeout: 10_000','workers: 1','retries: 0','timeout: 120_000']:self.assertIn(k,p)
  for f in ['im_user_read_protocol_tests.rs','im_user_read_account_tests.rs','im_user_read_io_tests.rs']:self.assertNotIn('#[ignore',src(f))
 def test_original_io_uses_heap_future_current_action_gates_and_does_not_install_another_owner(self):
  s=src('transport_actor.rs');self.assertIn('work.permit.begin_write(packets.call_id)',s);self.assertIn('im_gates.push(work.permit.parent_gate())',s);self.assertIn('user_interest',s);self.assertIn('Box::pin(self.send_packets',s);self.assertNotIn('tokio::spawn',src('im_user_read_actor.rs'))
if __name__=='__main__':unittest.main()
