"""Original scoped Group IM read request fences, supplementary to Actions tests."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(p):return (P/p).read_text(encoding='utf-8')
class ImRpcReadContracts(unittest.TestCase):
 def test_group_reads_have_only_fixed_original_method_and_current_native_identity(self):
  s=src('arc_session.rs').split('fn prepare_im_read',1)[1].split('fn take_im_replies',1)[0]
  self.assertIn('self.native_receiver_scope()',s);self.assertIn('binding.authenticates(&auth.user_id,&auth.selected_session_id)',s);self.assertIn('query.arguments(&auth.user_id)?',s)
  self.assertIn('value:super::rpc_im::GROUP_OBJECT',s);self.assertIn('value:&auth.token',s)
  self.assertNotIn('routing:',s);self.assertNotIn('phone_login_token',s)
 def test_unproved_readstatus_write_is_denied_before_rpc_id_allocation(self):
  s=src('arc_session.rs').split('fn prepare_im_read',1)[1].split('fn take_im_replies',1)[0]
  self.assertIn('query.operation()==super::rpc_im::ImOperation::SetReceived',s);self.assertLess(s.index('ImOperation::SetReceived'),s.index('let id=self.next_id'))
  self.assertIn('self.next_id=id.checked_add(1)',s);self.assertIn('im:Some(request)',s)
 def test_only_pending_current_request_can_produce_owned_data_no_callback_or_sql(self):
  s=src('arc_session.rs').split('PendingKind::ImRead=>',1)[1].split('PendingKind::Offline',1)[0]
  self.assertIn('pending.im.take()',s);self.assertIn('request.matches(owner,connection)',s);self.assertIn('OwnedImReply::original(request,reply.result_code,reply.result_stream)',s)
  self.assertIn('self.im_replies.push(result);return Ok(None)',s)
  self.assertNotIn('Callback::',s);self.assertNotIn('sqlite',s)
 def test_owned_result_current_gate_and_original_deadline_cannot_be_rebound(self):
  s=src('rpc_im.rs')
  for text in ['deadline:Instant','call_id:i32','self.gate.live()&&Instant::now()<self.deadline','self.binding.matches(owner,scope)','request.decode(code,bytes)?','OwnedImReply(<redacted>)','Zeroizing<Vec<u8>>']:
   self.assertIn(text,s)
  self.assertNotIn('Serialize',s);self.assertNotIn('Deserialize',s)
 def test_recv_records_are_bounded_by_original_request_not_arbitrary_positive_bool(self):
  s=src('rpc_im.rs')
  for text in ['result.messages.len()>*count as usize','m.index<*start||m.index>end','result.messages[..i].iter().any(|p|p.index==m.index)','start.checked_add(i64::from(*count)-1)']:
   self.assertIn(text,s)
 def test_original_protocol_uses_owned_actor_and_explicit_sync_policy_not_incoming_permission(self):
  self.assertIn('self.rpc.prepare_im_read(binding,query,gate,deadline)?',src('arc_protocol.rs'));self.assertIn('self.protocol.prepare_im_read(query,gate,deadline)',src('offline_backend.rs'))
  self.assertIn('Operation::ImRead(work)',src('transport_actor.rs'))
  self.assertIn('self.backend.prepare_im_read(work.query.clone(),work.gate.clone()',src('transport_actor.rs'))
  self.assertIn('!self.im_sync_enabled()',src('runtime_backend.rs'))
  self.assertNotIn('prepare_im_read(',src('account_im.rs'))
  self.assertIn('self.pending.clear();self.im_replies.clear()',src('arc_session.rs'))
 def test_new_rust_suite_preserves_existing_filter_and_does_not_skip(self):
  self.assertIn('im_rpc_read_tests.rs',src('im_protocol_tests.rs'));self.assertNotIn('#[ignore',src('im_rpc_read_tests.rs'))
  for name in ['build-release.yml','beta-validation.yml']:
   self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_client_ex::tests',(R/'.github/workflows'/name).read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
