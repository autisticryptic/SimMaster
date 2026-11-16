"""Supplemental original Status.Update ownership and durable-consumption fences."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return(P/name).read_text(encoding='utf-8')
class StatusConsumerContracts(unittest.TestCase):
 def test_typed_status_is_separate_from_message_or_query_and_requires_original_native_binding(self):
  s=src('im_status_delivery.rs');self.assertIn('ReceiverTransport::Direct',s);self.assertIn('ReceiverTransport::Batch',s);self.assertIn('binding.policy().validate(ReceiverObject::Message',s);self.assertIn('decode_status_update',s);self.assertIn('ImStatusTicket',s);self.assertIn('IncomingImStatus(<redacted>)',s);self.assertNotIn('Serialize',s)
 def test_private_proof_is_minted_only_after_current_sql_commit_not_a_public_raw_constructor(self):
  s=src('im_status_inbox.rs');self.assertIn('pub struct CommittedImStatus{binding:',s);self.assertIn('self.validate(&tx)?;if !current()',s);self.assertIn('tx.commit().map_err(|_|STORAGE)',s);self.assertIn('Arc::ptr_eq(notice.binding(),&self.binding)',s);self.assertIn('Instant::now()<delivery.ticket().deadline()',s);self.assertNotIn('fn recorded(',s);self.assertNotIn('Serialize',s)
 def test_partial_key_updates_preserve_per_property_time_and_reject_equal_time_changed_values(self):
  s=src('im_status_inbox.rs');self.assertIn('PRIMARY KEY(line_id,revision,identity_key,label,status_key)',s);self.assertIn('if time==notice.time()&&value.as_slice()!=p.value',s);self.assertIn('if time>=notice.time(){continue;}',s);self.assertIn('MAX_STATUS_PER_ACCOUNT:i64=128',s);self.assertIn('MAX_STATUS_TOTAL:i64=4096',s);self.assertIn('MAX_PROPERTIES:i64=128',s);self.assertIn('retained(&tx)?>MAX_TOTAL_BYTES',s)
 def test_status_observation_cannot_create_message_event_cursor_media_or_automatic_remote_write(self):
  for file in ['im_status_inbox.rs','account_im_status.rs']:
   s=src(file)
   for forbidden in ['INSERT INTO app_voice_im_messages','UPDATE app_voice_im_receive_cursors','im.received','setRead.Message.Message','prepare_im_read(','.read_im(','media_ready']:
    self.assertNotIn(forbidden,s)
 def test_both_native_direct_and_batch_status_holders_are_guarded_not_generic_sms_completions(self):
  s=src('arc_protocol.rs');self.assertIn('DirectResponse::ImStatus',s);self.assertIn('capture_im_status',s);self.assertIn('ProtocolEvent::ImStatus',s)
  s=src('offline_backend.rs');self.assertIn('im_status_delivery::batch(binding,&incoming)',s);self.assertIn('self.status_pending.contains_key',s);self.assertIn('status_completed.remove(&(retirement.peer_incarnation(),*sequence))',s);self.assertIn('self.status_completed.values().any(|(ticket,proof)|!proof.authorizes',s);self.assertIn('self.im_completed.len()+self.status_completed.len()',s)
 def test_status_full_submission_batch_proof_keeps_parent_gate_deadline_in_original_io_actor(self):
  s=src('transport_actor.rs');self.assertIn('Operation::CompleteImStatus(data)',s);self.assertIn('im_gates.push(proof.gate());let end=data.0.deadline()',s);self.assertIn('Box::pin(self.send_packets',s);self.assertIn('im_waiting.push(gate.retired())',s);self.assertIn('self.unhosting_pending.is_some() && matches!(&command.operation,Operation::CompleteImStatus(data) if data.1.is_some())',s)
 def test_same_hosting_ack_original_consumer_and_shared_early_inflight_budgets_are_kept(self):
  s=src('account_im_status.rs');self.assertIn('phase==Phase::HostingPending',s);self.assertIn('phase!=Phase::Hosted',s);self.assertIn('state.early.len()+state.early_status.len()>=MAX_EARLY',s);self.assertIn('state.holders>=MAX_HOLDERS||total>MAX_BYTES',s);self.assertIn('drop(delivery);state.holders=state.holders.checked_sub(1)',s);self.assertNotIn('tokio::spawn',s);self.assertNotIn('.abort(',s);self.assertIn('BackendEventPayload::ImStatus',src('account_runtime.rs'))
 def test_original_durable_private_status_tables_are_cleaned_only_after_authorized_account_delete(self):
  s=src('im_status_inbox.rs');self.assertIn('account_store::metadata(conn,line)?.is_some()',s);self.assertIn('DELETE FROM {table} WHERE line_id=?1',s);self.assertIn('super::statuses::delete_account(conn,line)?',src('im_associations.rs'));self.assertIn('statuses::initialize_schema(conn)',src('im_inbox.rs'))
 def test_new_status_sql_and_real_common_suites_are_in_existing_action_filters_without_skips(self):
  self.assertIn('im_status_protocol_tests.rs',src('im_protocol_tests.rs'));self.assertIn('im_status_account_tests.rs',src('im_account_tests.rs'))
  for file in ['im_status_protocol_tests.rs','im_status_account_tests.rs']:
   s=src(file);self.assertNotIn('#[ignore',s);self.assertNotIn('dlopen',s);self.assertNotIn('Command::new("adb")',s)
 def test_status_entry_descriptor_regression_is_in_existing_native_actions_filter(self):
  s=src('im_runtime_storage_tests.rs');self.assertIn('fn original_status_entries_are_heap_descriptors_without_new_task_stack_or_startup_poll()',s);self.assertIn('test_im_status_entry_descriptors()',s);self.assertIn('test_entry_descriptors()',src('account_runtime.rs'));self.assertNotIn('#[test]',src('account_im_status.rs'))
  for name in ['build-release.yml','beta-validation.yml']:
   self.assertIn('connectivity::app_voice::providers::jegotrip::transport_actor::native::tests',(R/'.github/workflows'/name).read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
