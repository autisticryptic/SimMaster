"""Supplemental SOURCE fences only; execution/signoff belongs to original Actions."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(name):return(P/name).read_text(encoding='utf-8')
class ClientUserEntryContract(unittest.TestCase):
 def test_prepare_is_synchronous_and_captures_first_deadline(self):
  s=text('account_user_entry.rs');p=s[s.index('pub fn prepare_user_entry'):s.index('pub async fn finish_user_entry')]
  self.assertNotIn('.await',p);self.assertLess(p.index('checked_add(run.backend.user_entry_timeout())'),p.index('reserve_user_entry_action'));self.assertIn('RecipientUri::parse(recipient.as_bytes())',p)
 def test_controller_wait_uses_same_absolute_deadline_and_original_retirement(self):
  s=text('account_user_entry.rs');self.assertIn('timeout_at(deadline.into(),run.controller.lock())',s);self.assertIn('_=scope.session_ended()',s);self.assertIn('_=run.im_gate.retired()',s);self.assertEqual(s.count('checked_add('),1)
 def test_current_manager_arc_and_original_sql_lease_are_checked(self):
  s=text('account_user_entry.rs');self.assertIn('Arc::ptr_eq(current,run)',s);self.assertIn('account_runs::runtime_matches(conn,&run.run_token)',s);self.assertIn('account_runs::require_live(',s);self.assertGreaterEqual(s.count('self.current_user_entry_run(&run)?'),4)
 def test_original_owner_scope_gate_and_hosted_are_not_metadata_grants(self):
  s=text('account_user_entry.rs');self.assertIn('controller.phase()!=Phase::Hosted',s);self.assertIn('controller.resource_owner().as_ref()!=Some(action.owner())',s);self.assertIn('current.matches(&scope)',s);self.assertIn('Arc::ptr_eq(&original.gate(),&run.im_gate)',s)
 def test_budget_is_reserved_before_private_recipient_copy(self):
  s=text('runtime_user_entry.rs');p=s[s.index('fn reserve_user_entry_action'):s.index('async fn resolve_user_entry')]
  self.assertLess(p.index('reserve_native_rpc(recipient.len()+64)'),p.index('UserEntryAction::reserved'));self.assertNotIn('.await',p);self.assertNotIn('RpcConnectionScope::fresh',p)
 def test_private_input_holder_belongs_to_original_scope_until_last_drop(self):
  s=text('user_entry_action.rs');self.assertIn('input:Option<Arc<super::native_rpc_budget::NativeRpcReservation>>',s);self.assertIn('input.belongs_to(&parent)',s);self.assertIn('recipient:Zeroizing::new(recipient.to_vec()),input',s);self.assertNotIn('Deserialize',s);self.assertNotIn('input.take()',s)
 def test_http_temporary_is_destroyed_before_first_manager_await(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');p=s[s.index('pub async fn lookup_user_entry'):s.index('pub async fn delete_im_message')]
  self.assertLess(p.index('manager.prepare_user_entry'),p.index('drop(request)'));self.assertLess(p.index('drop(request)'),p.index('manager.finish_user_entry(prepared).await'))
 def test_original_joins_uid_local_and_global_bounds_are_not_weakened(self):
  s=text('runtime_user_entry.rs');self.assertIn('Duration::from_secs(30)',s);self.assertIn('LocalUidResult::original',s);self.assertIn('attempt.user_entries.acquire',s);self.assertNotIn('tokio::spawn',s)
  b=text('native_rpc_budget.rs');self.assertIn('state.entries>=MAX_MAP_ENTRIES||total>MAX_STREAM_BYTES',b);self.assertNotIn('RUST_MIN_STACK',s)
 def test_client_factory_is_private_exact_input_data_not_canonical_or_new_authority(self):
  s=text('user_entry_client_target.rs');self.assertIn('original.into_reference(owner, source)?',s);self.assertIn('self.reference.original_factory_input()',s);self.assertIn('self.router.matches(router)',s);self.assertNotIn('encode_native(',s)
 def test_client_query_reserves_before_whole_factory_and_cc_copies(self):
  s=text('user_entry_client.rs');self.assertLess(s.index('reserve_native_rpc(target.retained_bytes()'),s.index('factory.to_vec()'));self.assertIn('uri.kind()==AidKind::Uid',s);self.assertIn('uri.authority().is_some_and',s)
 def test_current_actor_auth_is_reserved_before_uid_selector_token_copy(self):
  s=text('rpc_client_user_entry_session.rs');self.assertLess(s.index('reserve_native_rpc(bytes)'),s.index('self.user_entry_auth()'));self.assertNotIn('next_id=',s)
 def test_client_nested_ids_share_hosting_tracker_and_not_outer_session_ids(self):
  s=text('rpc_pending.rs');p=s[s.index('fn prepare_client_user_entry'):s.index('fn take_client_user_entry_replies')];self.assertIn('let sequence=self.next_id',p);self.assertIn('self.next_id=sequence.checked_add(1)',p);self.assertIn('client_reads.insert(sequence,request.clone())',p)
 def test_original_inner_cc_id_and_outer_token_selector_are_distinct(self):
  s=text('user_entry_client.rs');p=s[s.index('pub(super) fn as_batch_request'):s.index('pub(super) fn retired')];self.assertIn('key:b"DeviceInfo.CC"',p);self.assertIn('key:b"id",value:&self.query.factory',p);self.assertNotIn('key:b"token"',p);self.assertNotIn('key:b"pwd"',p);self.assertIn('encode_swap_calls(&auth.user_id, &auth.selected_session_id, payload)',text('arc_session.rs'))
 def test_read_retirement_is_not_mixed_frame_cancellation_or_write_ack(self):
  s=text('batch_scheduler.rs');p=s[s.index('fn retire_client_read'):s.index('/// Cancel local interest')];self.assertIn('self.prepared.is_none()',p);self.assertNotIn('self.close()',p);self.assertIn('if self.prepared.is_some() { self.close(); return; }',s);s=text('transport_actor.rs');self.assertIn('if read_owned_frame{None}',s)
 def test_read_only_outer_rpc_interest_retirement_is_purpose_filtered(self):
  s=text('client_user_entry_actor.rs');self.assertIn('client_read_outer.iter()',s);self.assertIn('sequences.iter().all(|s|!self.backend.client_read_state().client_read_pending.contains_key(s))',s);self.assertNotIn('scope.close()',s);self.assertIn('packets.client_only()',text('transport_actor.rs'))
 def test_matched_results_reuse_strict_original_decoder_and_hold_raw_bytes(self):
  s=text('user_entry_rpc.rs');self.assertIn('matched_uid(reply.result_code,reply.result_stream,uri)?',s);self.assertIn('matched_uid(code,bytes,uri)?',s);self.assertIn('_result:Some(Zeroizing::new(bytes.to_vec()))',s);self.assertLess(s.index('request.retain_reply(retained)'),s.index('matched_uid(code,bytes,uri)?'))
 def test_current_actor_owned_packets_hold_original_request_and_budget(self):
  s=text('arc_protocol.rs');self.assertIn('_client_holders:Vec<std::sync::Arc<',s);self.assertIn('packets._client_holders=holders',s);self.assertIn('reserve_scheduled_client_packets(batch,routing)',s);self.assertIn('client:Some(request)',text('batch_scheduler.rs'))
 def test_original_auxiliary_limits_and_shutdown_are_not_replaced_by_client_read(self):
  s=text('runtime_client_user_entry.rs');self.assertIn('attempt.user_entries.retired()',s);self.assertIn('timeout_at(action.deadline().into(),operation)',s);self.assertNotIn('.acquire(',s);self.assertNotIn('tokio::spawn',s);self.assertNotIn('RpcConnectionScope::fresh',s);self.assertIn('MAX_AUXILIARY_OPERATIONS:usize=4',text('user_entry_ownership.rs'))
 def test_new_runtime_cases_and_original_limits_are_mounted_on_original_filters(self):
  self.assertIn('mod client_user_entry;',text('runtime_common_backend_tests.rs'));self.assertIn('mod client_user_entry;',text('offline_backend_tests.rs'));self.assertIn('mod preparation;',text('user_entry_auth_tests.rs'));self.assertIn('mod user_entry_client_target;',text('mod.rs'));self.assertIn('Duration::from_millis(100)',text('user_entry_client_io_tests.rs'));self.assertNotIn('RUST_MIN_STACK',text('user_entry_client_io_tests.rs'))
 def test_actual_packet_objects_keep_retention_if_descriptor_is_taken(self):
  s=text('transport_packets.rs');self.assertIn('ClientPacketRetention',s);self.assertIn('self.1=Some(guard)',s);s=text('arc_protocol.rs');self.assertIn('for packet in &mut packets.packets{packet.retain_client_packet(actual.clone());}',s)
 def test_matched_client_result_is_admitted_without_scheduler_raw_copy(self):
  s=text('batch_scheduler.rs');self.assertIn('OwnedUserEntryReply::from_client(request.clone(),result.result_code,result.result_stream)',s);self.assertIn('client_requests.get(&sequence)',s);s=text('rpc_pending.rs');self.assertIn('Arc::ptr_eq(&original,&completion.request)',s)
 def test_read_removed_before_encoding_still_has_read_only_outer_retirement(self):
  s=text('transport_actor.rs');self.assertIn('client_wait.is_some()&&!packets.has_business_work()',s);self.assertIn('sequences.push(*sequence)',s);self.assertIn('client_read_outer.insert(packets.call_id,sequences)',s)
 def test_existing_actor_and_attempt_layouts_are_not_extended_for_new_read_state(self):
  s=text('transport_actor.rs');p=s[s.index('struct Actor<I>'):s.index('impl<I> Drop for Actor')];self.assertNotIn('client_read_pending:',p);s=text('runtime_backend.rs');p=s[s.index('struct Attempt {'):s.index('fn lock<T>(mutex')];self.assertNotIn('user_entry_client:',p)
 def test_target_namespace_is_present_in_both_original_workflows(self):
  for name in ['build-release.yml','beta-validation.yml']:
   s=(R/'.github/workflows'/name).read_text(encoding='utf-8');self.assertEqual(s.count('connectivity::app_voice::providers::jegotrip::user_entry_client_target::tests \\'),1);self.assertIn('cargo test --locked',s);self.assertIn('--test-threads=1',s)
 def test_client_only_measured_writer_cannot_free_unwiped_growth_buffers(self):
  s=text('rpc_stream.rs');self.assertIn('Vec::with_capacity(MAX_STREAM_BYTES)',s);self.assertIn('fn measured(capacity: usize)',s);self.assertIn('next > self.0.capacity()',s);self.assertIn('fn versioned_measured',s);s=text('client_user_entry_encoding.rs');self.assertIn('Writer::measured(request_bytes',s);self.assertIn('Writer::versioned_measured(swap_arguments_bytes',s)
 def test_measured_wire_offsets_include_original_type_zero_and_native_version_list(self):
  s=text('client_user_entry_encoding.rs');self.assertIn('17usize.checked_add',s);self.assertIn('45usize.checked_add',s);self.assertIn('writer.u32(1)?',s);self.assertIn('writer.properties(&request.metadata)?',s);s=text('rpc_client_user_entry_session.rs');self.assertIn('rpc.checked_mul(3)',s);self.assertIn('client_encoding_model_bytes()',s)
 def test_client_business_pending_identity_survives_transport_ack_history_removal(self):
  s=text('batch_scheduler.rs');self.assertIn('client_requests:BTreeMap<i32,Arc<',s);self.assertIn('self.client_requests.insert(sequence,request)',s);self.assertIn('self.client_requests.get(&sequence)',s);self.assertIn('self.client_requests.remove(sequence)',s);self.assertNotIn('let client_requests=std::iter::once',s)
 def test_client_reply_reserves_strict_decoder_models_before_original_decode(self):
  s=text('user_entry_rpc.rs');self.assertIn('MAX_AIDS*std::mem::size_of::<user_entry_data::AidUidEntry',s);self.assertIn('MAX_MAP_ENTRIES*std::mem::size_of::<RpcProperty',s);self.assertLess(s.index('request.retain_reply(retained)'),s.index('matched_uid(code,bytes,uri)?'))
 def test_payload_fields_drop_before_their_reservation_anchors(self):
  s=text('user_entry_client.rs');p=s[s.index('struct ClientUserEntryQuery'):s.index('impl fmt::Debug for ClientUserEntryQuery')];self.assertLess(p.index('factory:'),p.index('budget:'));self.assertLess(p.index('cc:'),p.index('budget:'));s=text('user_entry_rpc.rs');p=s[s.index('struct OwnedUserEntryReply'):s.index('impl fmt::Debug for OwnedUserEntryReply')];self.assertLess(p.index('_result:'),p.index('_client:'));self.assertLess(p.index('_client:'),p.index('_budget:'))
 def test_actor_admission_and_actual_main_prefix_regressions_are_mounted(self):
  s=text('transport_actor.rs');self.assertIn('test.after_admission(sequence,&self.scope).await',s);self.assertIn('gate_client_read_test(test)',s);s=text('user_entry_client_io_tests.rs');self.assertIn('mod races;',s);s=text('client_user_entry_runtime_race_tests.rs');self.assertIn('Race::AdmissionEmpty',s);self.assertIn('Race::PrefixMixed',s);self.assertIn('Race::CleanupCancelled',s);self.assertIn('Race::ReplyHolder',s);self.assertIn('Race::SharedBudget',s)
 def test_resumable_test_gate_preserves_actual_io_owner_and_drops_it_before_flag(self):
  s=text('client_user_entry_test_gate.rs');self.assertIn('path.io.take()',s);self.assertIn('test.scope().matches(&path.scope)',s);self.assertLess(s.index('drop(self.inner.take())'),s.index('self.test.dropped.store'));self.assertNotIn('TcpStream::connect',s);self.assertNotIn('RpcConnectionScope::fresh',s);self.assertNotIn('tokio::spawn',s)
if __name__=='__main__':unittest.main()
