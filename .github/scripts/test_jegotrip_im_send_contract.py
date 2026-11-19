"""Supplemental original IM send safety/source fences; not Rust/SDK/IO proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class ImSendContracts(unittest.TestCase):
 def test_policy_is_strict_optin_separate_from_received_user_and_media(self):
  s=src('im_send_policy.rs');self.assertIn('deny_unknown_fields',s);self.assertIn('Mtc.Auth.CC',s);self.assertIn('im_send:None',src('runtime_backend.rs'));self.assertIn('with_im_send',src('account_config.rs'))
 def test_sender_uri_is_original_login_identity_and_not_returned_mainaccount(self):
  s=src('arc_session.rs');self.assertIn('identity.expose_to_backend().to_vec()',s);s=src('im_send_identity.rs');self.assertIn('self.phase()!=ArcPhase::Authenticated',s);self.assertIn('self.login_uri.as_ref()',s);self.assertNotIn('self.main_account',s);self.assertIn('binding.authenticates(&self.uid,&self.selector)',s)
 def test_original_statistics_namespace_and_outgoing_shape_are_not_received_message(self):
  s=src('im_send_data.rs');self.assertIn('userSend.Message.Message',s);self.assertIn('b"Statistics."',s);self.assertIn('writer.properties(message.attachments)',s);self.assertNotIn('writer.i64(',s);s=src('im_send_identity.rs');self.assertIn('Statistics.CountryCode',s);self.assertNotIn('Notify.CountryCode',s)
 def test_current_scope_and_nonce_hash_marker_precede_any_send_id_and_no_automatic_retry(self):
  s=src('im_outbox.rs');self.assertIn('original-user-send-intent-v1',s);self.assertIn('state=0)',s);self.assertIn('UNIQUE(line_id,user_nonce)',s);self.assertIn('request_id IS NULL',s);self.assertNotIn('tokio::spawn',s);self.assertNotIn('retry(',s)
 def test_original_sdk_grant_and_shared_budget_precede_one_request_arm(self):
  s=src('rpc_im_send_session.rs');self.assertIn('binding.authenticates(&auth.user_id,&auth.selected_session_id)',s);self.assertLess(s.index('reserve_native_rpc(size)'),s.index('permit.arm(id,deadline)'));self.assertLess(s.index('permit.arm(id,deadline)'),s.index('self.next_id=id.checked_add(1)'));self.assertIn('b"#Group"',s)
 def test_private_result_requires_original_request_pointer_and_two_signed64_data(self):
  s=src('rpc_im_send.rs');self.assertIn('std::ptr::eq(Arc::as_ptr(&self.request.permit),permit)',s);self.assertIn('retain_reply(bytes.len().checked_add(96)',s);s=src('im_send_data.rs');self.assertIn('let msg_id=reader.i64()?;let time=reader.i64()?',s);self.assertIn('reader.finish()?',s)
 def test_possible_full_write_retirement_keeps_unknown_and_all_parent_gates(self):
  s=src('im_outbox.rs');self.assertIn('CLOSED_WRITE:u8=5',s);self.assertIn('if before==QUEUED{RETIRED}else{CLOSED_WRITE}',s);self.assertIn('self.action.scope().close()',s);s=src('transport_actor.rs');self.assertIn('send_interest',s);self.assertIn('im_gates.push(work.permit.parent_gate())',s)
 def test_actor_is_owned_and_generic_closed_cause_never_overrides_revocation(self):
  s=src('im_send_actor.rs');self.assertNotIn('tokio::spawn',s);self.assertIn('p.permit.resolve(&reply)',s);s=src('transport_actor.rs');self.assertIn('if result==RuntimeError::Closed{self.im_send_close_reason().unwrap_or(result)}else{result}',s);self.assertIn('if self.scope.session_revoked() { RuntimeError::SessionRevoked } else { result }',s)
 def test_stop_restart_cannot_delete_outbox_unknown_or_create_incoming_receipt(self):
  self.assertNotIn('DELETE FROM app_voice_im_outbox',src('account_runs.rs'));self.assertIn('super::outbox::delete_account(conn,line)',src('im_associations.rs'));s=src('im_outbox.rs');self.assertNotIn('INSERT INTO app_voice_im_messages',s);self.assertNotIn('INSERT INTO sms_messages',s)
 def test_account_drops_controller_and_requires_separate_business_consent(self):
  s=''.join(src('account_im_send.rs').split());self.assertIn('if!consent||!write_consent',s);self.assertIn('drop(controller);letresult=',s);self.assertIn('run.configuration_current().map_err(ImSendFailure::before)?',s);self.assertIn('run.configuration_current().map_err(ImSendFailure::after)?',s)
 def test_http_request_is_strict_no_caller_uid_scope_token_or_reply_and_wipes_inputs(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('pub struct ImSendRequest{expected_revision:u64,nonce:String,recipient:String,content:String',s);self.assertIn('consent:bool,write_consent:bool',s);self.assertIn('impl Drop for ImSendRequest',s);self.assertIn('ImSendMessage::components',s)
 def test_data_identity_and_protocol_suites_are_under_existing_filters(self):
  self.assertIn('im_send_data_tests.rs',src('im_data_tests.rs'));self.assertIn('im_send_identity_tests.rs',src('user_entry_auth_tests.rs'));self.assertIn('im_send_protocol_tests.rs',src('im_protocol_tests.rs'));self.assertNotIn('#[ignore',src('im_send_protocol_tests.rs'))
 def test_input_reservation_precedes_original_controller_and_auth_suspension(self):
  s=src('account_im_send.rs');self.assertLess(s.index('reserve_im_send_input('),s.index('run.controller.lock()'));self.assertLess(s.index('checked_add(run.backend.im_send_timeout())'),s.index('run.controller.lock()'));self.assertIn('run.im_gate.retired()',s);self.assertIn('timeout_at(deadline.into(), run.controller.lock())',s);s=src('runtime_im_send.rs');self.assertIn('message.reserve_input(&attempt.scope',s);self.assertIn('original_with_input',s)
 def test_sender_input_and_arguments_remain_charged_through_original_last_holder(self):
  s=src('im_outbox.rs');self.assertIn('_input_budget:ImSendInput',s);identity=src('im_send_identity.rs');self.assertIn('_budget:Arc',identity);self.assertLess(identity.index('reserve_native_rpc(retained)'),identity.index('uid:Zeroizing::new(auth.user_id.clone())'));self.assertIn('_arguments_budget:Arc',s);self.assertIn('arguments_budget=self.scope().reserve_native_rpc(argument_bytes)',s);self.assertIn('input.matches(&binding.scope(),&message,recipient,nonce)',s);s=src('arc_protocol.rs');self.assertIn('packets._send_budget=Some(original)',s);self.assertIn('OwnedCallPackets::uncharged',s)
 def test_unresolved_send_predicate_is_not_reset_by_selector_or_revision_changes(self):
  s=src('im_outbox.rs');query='SELECT EXISTS(SELECT 1 FROM app_voice_im_outbox WHERE line_id=?1 AND state=0)';self.assertIn(query,s);self.assertNotIn('WHERE line_id=?1 AND identity_key=?2 AND state=0',s);self.assertIn('self.binding().identity().as_slice()',s)
 def test_original_resolve_and_cancel_share_private_transition_not_io_gate(self):
  s=src('im_outbox.rs');result=s[s.index('fn resolve('):s.index('pub(in super::super) fn cancel(&self){let _transition')];self.assertIn('let _transition=lock(&self.transition)',result);self.assertLess(result.index('view(&tx,row)?'),result.index('tx.commit()'));self.assertLess(result.index('tx.commit()'),result.index('self.phase.store(RESOLVED'));self.assertNotIn('with_live(',result);self.assertIn('set_resolve_probe',s);self.assertIn('slot.cancelled=true',s)
 def test_nonzero_sender_result_still_requires_whole_original_footer_and_proven_class(self):
  s=src('rpc_im_send.rs');self.assertIn('decode_user_send_result(0,bytes)?',s);self.assertIn('code>>16==1',s);self.assertIn('decode_user_send_refused_versions(code,bytes)?',s);self.assertNotIn('if code!=0{(false,None,None)}',s);self.assertIn('send_hardening_malformed_or_unproven_nonzero_result',src('im_send_hardening_tests.rs'))
 def test_no_intent_http_observation_is_stage_and_nonce_bound_not_generic_timeout_whitelist(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('"admitted_or_unknown"',s);self.assertIn('error.not_admitted()',s);self.assertLess(s.index('drop(request);',s.index('pub async fn send_im_message')),s.index('manager.send_im_message_with_admission'));ui=(R/'frontend/src/policies/imSend.ts').read_text(encoding='utf-8');self.assertIn("own('im_send_admission') === 'not_admitted'",ui);self.assertIn("own('nonce_tag') === nonceTag",ui);self.assertIn('error instanceof ImSendNotAdmittedError',ui);self.assertNotIn('app_account_im_send_timeout',ui)
if __name__=='__main__':unittest.main()
