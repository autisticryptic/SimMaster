"""Source fences for native UID-local semantics; not compiled/native IO proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(name):return(P/name).read_text(encoding='utf-8')
class UidLocalContracts(unittest.TestCase):
 def test_local_reference_is_owned_bootstrap_data_not_decoder_or_uid_authority(self):
  s=text('user_entry_local.rs');self.assertIn('UserEntryReference::from_bootstrap',s);self.assertIn('source.matches(parent)',s);self.assertIn('reference.reference_for(owner,source)',s);self.assertIn('router.original_identity()',s);self.assertNotIn('Deserialize',s)
 def test_uid_branch_checks_current_original_actor_auth_before_any_auxiliary_scope_or_id(self):
  s=text('runtime_user_entry.rs');a=s.index('if let Some(reference)=local_reference');b=s.index('let lease=attempt.user_entries.acquire');self.assertLess(a,b);branch=s[a:b];self.assertIn('commands.original_user_entry_auth',branch);self.assertIn('LocalUidResult::original',branch);self.assertNotIn('resolve_original(',branch);self.assertNotIn('RpcConnectionScope::fresh',branch);self.assertIn('state.unregister_started',branch)
 def test_local_result_keeps_action_and_current_parent_without_minting_a_network_reply(self):
  s=text('user_entry_local.rs');self.assertIn('recipient.kind()!=AidKind::Uid',s);self.assertIn('Arc::ptr_eq(action,&self.action)',s);self.assertIn('self.auth.matches',s);self.assertIn('report:"local_uid"',s);self.assertNotIn('OwnedUserEntryReply{',s);self.assertNotIn('UserReadPermit{',s)
 def test_local_branch_has_no_socket_rpc_task_sql_or_retry_side_effects(self):
  s=text('user_entry_local.rs');
  for value in ['tokio::spawn','TcpStream','encode_request','setStatus.','setRead.','INSERT INTO','UPDATE app_','.abort(']:self.assertNotIn(value,s)
 def test_direct_alias_resolver_scope_and_old_deadline_protection_are_kept(self):
  s=text('user_entry_rpc.rs');self.assertIn('ReferenceTarget::Direct',s);self.assertIn('lease.arm',s);self.assertIn('DeviceInfo.CC',s);self.assertIn('Some(reply)=matched else{continue;}',s);self.assertIn('map_err(common_error)',s)
 def test_uid_local_api_ui_report_is_distinct_finite_and_never_private_uid_projection(self):
  s=(R/'frontend/src/policies/userEntry.ts').read_text(encoding='utf-8');self.assertIn("report === 'local_uid' && kind !== 'uid'",s);self.assertIn('未查询服务端',s);self.assertIn('不是回执或授信',s);p=(R/'frontend/src/pages/sim/AppUserEntryPane.tsx').read_text(encoding='utf-8');self.assertIn('localOnly && !userEntryUid(value)',p);self.assertIn('current.current.generation !== original.generation',p);self.assertNotIn('dangerouslySetInnerHTML',p)
if __name__=='__main__':unittest.main()
