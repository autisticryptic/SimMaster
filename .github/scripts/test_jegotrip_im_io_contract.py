"""Supplemental native IM owned-read IO fences. Real execution is Actions-only."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')
class ImIoContracts(unittest.TestCase):
 def test_explicit_sync_policy_is_not_an_incoming_consumer_or_write_grant(self):
  s=src('runtime_backend.rs');self.assertIn('im_sync:Option<super::im_sync_policy::NativeImSyncConfig>',s);self.assertIn('p.im_messages().is_none()',s);self.assertIn('self.profile.im_sync.is_some()&&self.im_messages_enabled()',s)
  policy=src('im_sync_policy.rs');self.assertIn('deny_unknown_fields',policy);self.assertIn('pub receive:bool',policy);self.assertNotIn('token',policy)
  cfg=src('account_config.rs');self.assertIn('pub im_sync:Option<super::im_sync_policy::NativeImSyncConfig>',cfg);self.assertIn('im_sync:Option<&\'a super::im_sync_policy::NativeImSyncConfig>',cfg);self.assertIn('.with_im_sync(p.im_sync.as_ref())',cfg)
 def test_queued_cancel_is_atomic_and_partial_write_cancel_closes_only_original_scope(self):
  s=src('im_read_interest.rs');self.assertIn('compare_exchange(QUEUED,WRITING',s);self.assertIn('compare_exchange(WRITING,SUBMITTED',s);self.assertIn('state.swap(RETIRED',s);self.assertIn('if original==WRITING{self.scope.close();}',s);self.assertIn('ImReadInterest(<redacted>)',s)
  self.assertNotIn('Serialize',s);self.assertNotIn('Deserialize',s)
 def test_one_heap_send_future_keeps_original_deadline_and_gate_during_read_writes(self):
  s=src('transport_actor.rs');self.assertIn('Box::pin(self.send_packets',s);self.assertIn('work.interest.begin_write(&work.gate)',s);self.assertIn('work.interest.submitted()',s);self.assertIn('read_interest',s);self.assertIn('interest.retired().await',s);self.assertIn('im_gates.push(work.gate.clone())',s);self.assertIn('im_waiting.push(gate.retired())',s)
  self.assertIn('native_deadline.ok_or(RuntimeError::Timeout)?',s)
 def test_only_original_owned_reply_id_query_and_parent_brand_can_deliver(self):
  s=src('transport_actor.rs');self.assertIn('for reply in self.backend.take_im_replies()',s);self.assertIn('let id=reply.request().call_id()',s);self.assertIn('Arc::ptr_eq(&pending.gate,&reply.request().gate())',s);self.assertIn('Arc::ptr_eq(&pending.query,reply.request().query_arc())',s);self.assertIn('!reply.matches(&self.owner,&self.scope)',s)
 def test_full_submission_read_retirement_never_requeues_or_closes_sdk_for_cancelled_wait(self):
  s=src('transport_actor.rs');body=s.split('fn retire_im_reads',1)[1].split('fn elapsed_ms',1)[0];self.assertIn('self.backend.retire_im_read(id)',body);self.assertIn('self.pending.remove(&id)',body);self.assertNotIn('self.scope.close',body);self.assertNotIn('prepare_im_read',body)
  arc=src('arc_session.rs').split('PendingKind::ImRead=>',1)[1].split('PendingKind::Offline',1)[0];self.assertIn('if self.phase()!=ArcPhase::Authenticated||!request.matches(owner,connection){return Ok(None);}',arc)
 def test_waiter_and_parent_retirement_are_checked_before_late_payload_decode(self):
  s=src('arc_native_actor.rs');block=s.split('if let Some(input)=input{',1)[1].split('}Ok(())',1)[0];self.assertLess(block.index('self.retire_im_reads'),block.index('self.backend.on_common_calls'))
  self.assertIn('filter(|(id,_)|!self.im_pending.contains_key(*id))',s)
 def test_setreceived_remote_writes_are_not_admitted_through_read_api(self):
  s=src('transport_actor.rs');self.assertIn('query.operation()==ImOperation::SetReceived{return Err(RuntimeError::InvalidConfiguration);}',s);self.assertIn('self.common.is_none()||work.query.operation()==ImOperation::SetReceived',s)
  self.assertIn('query.operation()==super::rpc_im::ImOperation::SetReceived',src('arc_session.rs'))
 def test_new_real_io_and_policy_suites_use_existing_filters_and_no_skips(self):
  self.assertIn('im_read_actor_tests.rs',src('arc_native_actor_tests.rs'));self.assertIn('im_read_backend_tests.rs',src('native_receiver_runtime_tests.rs'))
  for name in ['im_read_actor_tests.rs','im_read_backend_tests.rs']:
   self.assertNotIn('#[ignore',src(name));self.assertNotIn('dlopen',src(name))
  for name in ['build-release.yml','beta-validation.yml']:
   s=(R/'.github/workflows'/name).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::transport_actor::native::tests',s);self.assertIn('connectivity::app_voice::providers::jegotrip::runtime_backend::common_tests',s)
if __name__=='__main__':unittest.main()
