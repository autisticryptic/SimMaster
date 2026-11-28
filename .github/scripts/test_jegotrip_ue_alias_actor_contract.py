"""SOURCE guards only. Original Actions execute actual Common actor/IO tests."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def s(name):return(P/(name+'.rs')).read_text(encoding='utf-8')
class AliasActor(unittest.TestCase):
 def test_actor_activation_is_private_and_no_account_or_write_entry(self):
  for name in ['account_config','account_runtime','account_group_create','runtime_backend','runtime_relation_updates']:
   self.assertNotIn('login_with_alias_source',s(name));self.assertNotIn('read_aliases(',s(name))
  self.assertIn('Operation::AliasLogin(Box::new(work))',s('ue_alias_actor'));self.assertNotIn('Serialize',s('ue_alias_action'))
 def test_original_plan_router_and_exact_login_data_rechecked_before_id(self):
  self.assertIn('bind_alias_transport(router_policy)',s('arc_native_actor'));self.assertIn('!binding.transport_matches(r)',s('offline_ue_alias'));self.assertIn('self.digest==fingerprint(inputs)',s('ue_alias_login'))
  self.assertIn('inputs.phone_login_token',s('ue_alias_login'));self.assertIn('inputs.client_properties',s('ue_alias_login'));self.assertNotIn('clone()',s('ue_alias_login').split('fn into_origin',1)[1])
  a=s('ue_alias_actor').split('async fn login_with_alias_source',1)[1].split('async fn read_aliases',1)[0];self.assertLess(a.index('scope.reserve_native_rpc(bytes)'),a.index('Login::capture(inputs)?'))
 def test_same_original_deadline_and_one_shot_claim(self):
  self.assertIn('claimed.compare_exchange(false,true',s('ue_alias_action'));self.assertIn('end.min(work.action.deadline())',s('ue_alias_actor'));self.assertIn('timeout_at(action.deadline().into(),operation)',s('ue_alias_actor'))
  self.assertIn('if now>=end {if let Operation::ReadAliases',s('transport_actor'))
 def test_queue_and_partial_frame_cancellation_not_a_new_write_or_connection(self):
  a=s('ue_alias_actor');self.assertNotIn('tokio::spawn',a);self.assertNotIn('TcpStream',a);self.assertIn('AliasActionWait::original',a)
  self.assertIn('alias_wait.is_some()||packets.client_only()',s('transport_actor'));self.assertIn('let completion=if read_owned_frame{None}',s('transport_actor'))
  self.assertIn('!self.alias_sequences_pending(sequences)',s('client_user_entry_actor'));self.assertIn('chain(self._alias_sequence)',s('arc_protocol'))
 def test_no_success_until_all_outgoing_and_visible_events_finish(self):
  a=s('transport_actor');body=a.split('async fn deliver(&mut self',1)[1].split('async fn deliver_events',1)[0]
  self.assertLess(body.index('self.deliver_relation_replies(outcome)?'),body.index('self.finish_alias_delivery(outcome)'))
  self.assertIn('p.wait.outcome()!=Some(Ok(()))',s('ue_alias_actor'));self.assertIn('v.err().map(|e|Err(error(e)))',s('ue_alias_actor'))
 def test_no_new_inline_actor_state_or_stack_override(self):
  actor=s('transport_actor').split('struct Actor<I>',1)[1].split('impl<I> Drop for Actor',1)[0];self.assertNotIn('alias:',actor)
  self.assertIn('alias:Option<Box<super::ue_alias::AliasPending>>',s('client_user_entry_actor'));self.assertNotIn('RUST_MIN_STACK',s('ue_alias_actor_io_tests'))
 def test_existing_real_tcp_prefix_gate_and_two_connections_only(self):
  t=s('ue_alias_actor_io_tests');self.assertIn('ClientReadTestPoint::Prefix(1)',t);self.assertIn('assert_eq!(self.connections.load(Ordering::SeqCst),2)',t);self.assertIn('third alias socket',t)
  self.assertIn('test.dropped.load',t);self.assertIn('test.accepted.load',t);self.assertIn('bounded(source).await.unwrap()',t)
  self.assertIn('wait.attach_actor_interest(work.action.cancellation_gate(),work.action.parent_gate())',s('ue_alias_actor'))
  self.assertIn('interest.live()&&parent.live()',s('ue_alias_snapshot'))
 def test_no_test_filter_or_existing_suite_replacement(self):
  self.assertIn('#[path="ue_alias_actor_io_tests.rs"]\nmod ue_alias;',s('arc_native_actor_tests'))
  for name in ['build-release.yml','beta-validation.yml']:
   w=(R/'.github/workflows'/name).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::transport_actor::native::tests',w);self.assertNotIn('RUST_MIN_STACK',w)
 def test_completion_moves_only_wait_not_boxed_pending_sender(self):
  body=s('ue_alias_actor').split('fn finish_alias_delivery',1)[1].split('fn retire_alias_read',1)[0]
  self.assertIn('let result=match outcome{Ok(())=>AliasObservation::original(p.wait,p.work.action.clone()).map_err(error),Err(error)=>Err(error)};',body)
  self.assertIn('p.work.reply.send(result)',body);self.assertNotIn('outcome.and_then(',body)
 def test_expiry_uses_typed_same_read_sample_not_an_earlier_actor_tick(self):
  a=s('ue_alias_actor').split('fn retire_alias_read',1)[1]
  self.assertIn('match p.wait.sampled_outcome()',a);self.assertIn('AliasOutcome::Expired=>Some(Err(RuntimeError::Timeout))',a)
  q=s('ue_alias_snapshot').split('fn sampled_outcome',1)[1].split('fn outcome',1)[0]
  self.assertEqual(q.count('Instant::now()'),1);self.assertNotIn('is_waiting(',q)
  self.assertIn('if observation.expired(){RuntimeError::Timeout}',s('ue_alias_actor'))
 def test_invite_fixture_setup_precedes_claim_without_changing_production_deadline(self):
  t=s('native_invite_liveness_tests');setup=t.split('fn native_context()',1)[1].split('fn receipt(',1)[0]
  self.assertLess(setup.index('Certificates::generate'),setup.index('super::native_context()'))
  body=t.split('fn receipt(',1)[1].split('#[test]',1)[0]
  self.assertIn('native_invite_receipts::consume',body);self.assertNotIn('Certificates::generate',body);self.assertNotIn('TempDatabase::new',body)
  self.assertNotIn('fixture_consumed',t);self.assertIn('Instant::now() >= self.deadline',s('native_invite_notifications'))
 def test_only_submitted_alias_result_wait_is_excluded_from_connection_io_expiry(self):
  q=s('arc_protocol');self.assertIn('self._client_only&&self._alias_sequence.is_some()&&self._client_holders.is_empty()',q)
  actor=s('transport_actor');self.assertIn('if !packets.alias_only(){self.pending.insert(packets.call_id,rpc_deadline);}',actor)
  self.assertIn('if !is_response && !packets.alias_only()',actor)
  self.assertLess(actor.index('let sent ='),actor.index('if let Some(wait)=alias_wait{'))
  io=s('arc_native_actor');self.assertIn('filter(|(id,_)|!self.im_pending.contains_key(*id)).map(|(_,end)|*end).chain(extra)',io)
  self.assertIn('chain(self.common.as_ref().and_then(CommonPath::next_deadline))',io)
if __name__=='__main__':unittest.main()
