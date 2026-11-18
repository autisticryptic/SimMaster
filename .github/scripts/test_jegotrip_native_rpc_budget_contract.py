"""Supplementary source accounting/ownership fences; not native IO proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(name):return(P/name).read_text(encoding='utf-8')
class NativeRpcBudgetContracts(unittest.TestCase):
 def test_original_scope_owns_shared_budget_and_cloned_scopes_do_not_reset_it(self):
  s=text('rpc_pending.rs');self.assertIn('Arc<super::native_rpc_budget::NativeRpcBudget>',s);self.assertIn('Arc::ptr_eq(&self.4,budget)',s);self.assertIn('self.4.reserve(self,bytes)',s)
 def test_budget_preserves_original_128_entries_128k_and_current_scope_admission(self):
  s=text('native_rpc_budget.rs');self.assertIn('MAX_MAP_ENTRIES',s);self.assertIn('MAX_STREAM_BYTES',s);self.assertIn('state.entries>=MAX_MAP_ENTRIES||total>MAX_STREAM_BYTES',s);self.assertIn('!scope.is_open()||scope.session_revoked()',s);self.assertNotIn('Deserialize',s)
 def test_pending_and_reply_bytes_keep_last_original_holder_charge(self):
  s=text('native_rpc_budget.rs');self.assertIn('original.bytes.checked_add(bytes)',s);self.assertIn('state.bytes.checked_add(bytes)',s);self.assertIn('impl Drop for NativeRpcReservation',s);self.assertIn('state.entries.saturating_sub(1)',s)
 def test_im_read_admission_reserves_before_original_monotonic_id_advance(self):
  s=text('arc_session.rs').split('fn prepare_im_read',1)[1].split('fn take_im_replies',1)[0];self.assertLess(s.index('reserve_native_rpc(retained)?'),s.index('self.next_id=id.checked_add(1)'));self.assertIn('ImOperation::SetReceived',s)
 def test_received_and_user_write_reserve_before_durable_permit_arm(self):
  for name,arm in [('rpc_im_received_session.rs','permit.arm_request(id,deadline)'),('rpc_user_read_session.rs','permit.arm(id,deadline)')]:
   s=text(name);self.assertLess(s.index('reserve_native_rpc(retained)?'),s.index(arm))
 def test_private_matched_reply_factories_retain_same_original_reservation(self):
  for name in ['rpc_im.rs','rpc_im_received.rs','im_user_read_rpc.rs']:self.assertIn('request.budget.retain_reply(bytes.len())?',text(name));self.assertIn('budget.belongs_to',text(name))
 def test_auxiliary_shared_entry_admission_precedes_child_scope_and_id(self):
  s=text('user_entry_ownership.rs');self.assertLess(s.index('self.parent.reserve_native_rpc(0)?'),s.index('s.next=id.checked_add(1)'));self.assertLess(s.index('self.parent.reserve_native_rpc(0)?'),s.index('RpcConnectionScope::fresh()'));self.assertIn('arm_request(bytes)?',s);self.assertIn('budget.retain_reply(bytes)?',s)
 def test_auxiliary_reply_owns_reservation_until_result_drop_and_no_scope_relabel(self):
  s=text('user_entry_rpc.rs');self.assertIn('_budget:Arc<super::native_rpc_budget::NativeRpcReservation>',s);self.assertIn('lease.retain_reply(reply.result_stream.len())',s);self.assertIn('path.close();return Ok(result)',s)
 def test_io_budget_lock_does_not_enclose_network_sql_or_join(self):
  s=text('native_rpc_budget.rs');
  for f in ['.await','TcpStream','INSERT INTO','UPDATE app_','with_io_gate','tokio::spawn','.abort(']:self.assertNotIn(f,s)
 def test_partial_auxiliary_test_hook_wraps_same_fd_and_releases_it_before_drop_flag(self):
  s=text('user_entry_io_test_gate.rs');self.assertIn('path.io.take()',s);self.assertIn('bytes.len().min(left)',s);self.assertLess(s.index('drop(self.inner.take())'),s.index('self.test.dropped.store'));self.assertNotIn('TcpStream::connect',s);self.assertNotIn('tokio::spawn',s)
if __name__=='__main__':unittest.main()
