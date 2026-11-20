"""SOURCE-only directory READ fences; original Actions execute Rust/HTTP/UI."""
from pathlib import Path
import re,unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return(P/name).read_text(encoding='utf-8')
class DirectoryReadSourceContracts(unittest.TestCase):
 def test_directory_policy_is_independent_omitted_false_and_same_original_sdk_binding(self):
  s=src('im_delivery.rs')
  for t in ['#[serde(default,skip_serializing_if="no_directory")]','pub directory_queries:bool','fn no_directory(value:&bool)->bool{!*value}','self.config.directory_queries','b"jegotrip-native-im-context-v1\\0",uid,selector']:self.assertIn(t,s)
  self.assertIn('NativeReceiverBinding::original',s);self.assertNotIn('uid,selector,config',s)
 def test_directory_wire_data_is_original_methods_versions_prefix_and_not_permission(self):
  s=src('group_directory_data.rs')
  for t in ['getRelations2.GroupServer.Group','getPropsPrefixs.GroupServer.Group','Writer::versioned()?','w.i64(since)?','w.u32(1)?;w.bytes(b"")?','if code!=0','CodecError::DuplicateKey','read_footer(&mut r)?;r.finish()?']:self.assertIn(t,s)
  for t in ['TcpStream','tokio::spawn','Database','UserReadPermit','ImSendPermit','ReceivedWritePermit','std::fs','Zeroizing::new(Vec']:self.assertNotIn(t,s)
  self.assertIn('value.iter().all(u8::is_ascii_digit)',s);self.assertIn("value[0]!=b'0'",s)
 def test_preparation_reserves_original_input_before_copy_or_controller_await(self):
  s=src('runtime_backend.rs').split('pub(super) fn reserve_directory_query',1)[1].split('pub(super) async fn read_directory',1)[0]
  self.assertLess(s.index('reserve_native_rpc('),s.index('ImQuery::group_properties('));self.assertIn('group.map_or(0,|id|id.len())+64',s)
  for t in ['!state.configured','state.retired','state.logout_started','state.unregister_started','attempt.scope.session_revoked()','query.fix_directory_deadline(deadline)']:self.assertIn(t,s)
  self.assertNotIn('.await',s);self.assertNotIn('login(',s)
 def test_directory_query_and_native_id_and_deadline_cannot_rebind_or_extend(self):
  q=src('rpc_im.rs');a=src('arc_session.rs');t=src('transport_actor.rs')
  for text in ['budget.belongs_to(scope)','deadline.is_none()','limit.min(*original)','query.clipped_deadline(deadline)','query.directory_query()&&!binding.directory_queries()']:self.assertIn(text,q)
  block=a.split('pub(super) fn prepare_im_read',1)[1].split('pub(super) fn take_im_replies',1)[0]
  self.assertLess(block.index('query.clipped_deadline(deadline)'),block.index('self.next_id'))
  self.assertLess(block.index('!query.directory_input_matches(&self.connection)'),block.index('self.next_id'))
  self.assertIn('Operation::ImRead(work)=>native_deadline.map(|end|work.query.clipped_deadline(end))',t)
 def test_finish_requires_original_manager_run_sql_owner_scope_query_gate_and_fixed_end(self):
  s=src('account_directory.rs')
  for text in ['runtime_matches(conn,&run.run_token)','require_live(conn','Arc::ptr_eq(current,&run)','controller.phase()!=Phase::Hosted','controller.resource_owner().as_ref()!=Some(&owner)','scope.session_ended()','timeout_at(deadline.into(),run.controller.lock())','drop(controller);live_run(&run)?','Arc::ptr_eq(reply.request().query_arc(),&query)','Arc::ptr_eq(&reply.request().gate(),&run.im_gate)']:self.assertIn(text,s)
  self.assertEqual(s.count('if Instant::now()>=deadline'),2)
  for text in ['tokio::spawn','INSERT INTO','UPDATE app_voice','clear_unknown','send_im_message(','prepare_received_write']:self.assertNotIn(text,s)
 def test_http_is_authenticated_namespace_strict_bounded_wiped_before_await_no_store(self):
  a=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8')
  self.assertIn('.route("/api/non-3gpp/accounts/{line_id}/native-directory/read",post(read_native_directory))',a)
  self.assertIn('#[serde(deny_unknown_fields)]\npub struct DirectoryReadRequest',a)
  block=a.split('pub async fn read_native_directory',1)[1].split('pub async fn get_im_messages',1)[0]
  for text in ['request.since.len()>20','id.len()>508','parse::<i64>()','since.to_string()!=request.since','manager.prepare_native_directory','drop(request);']:self.assertIn(text,block)
  self.assertLess(block.index('drop(request);'),block.index('manager.finish_native_directory(prepared).await'))
  self.assertIn('zeroize::Zeroize::zeroize(&mut self.since)',a);self.assertIn('HeaderValue::from_static("no-store")',a)
 def test_summary_and_ui_are_counts_and_full_signed64_not_raw_relationships_or_sdk_grants(self):
  s=src('account_directory.rs').split('pub struct DirectoryReadView{',1)[1].split('}',1)[0]
  for text in ['uid:','token:','body:','scope:','label:','flags:','property_values:']:self.assertNotIn(text,s)
  p=(R/'frontend/src/policies/nativeDirectory.ts').read_text(encoding='utf-8');pane=(R/'frontend/src/pages/sim/AppNativeDirectoryPane.tsx').read_text(encoding='utf-8')
  for text in ["own(value, 'operation')","own(value, 'target_kind')","own(value, 'original_time')",'BigInt(value)','-9223372036854775808n','9223372036854775807n','不解决未知 WRITE']:self.assertIn(text,p)
  for text in ['window.confirm(', 'active.current.generation !== original.generation','active.current.context !== original.context','setResult(null)','不创建关系、群组或业务']:self.assertIn(text,pane)
  for text in ['dangerouslySetInnerHTML','setAppAccountRunning','localStorage','setInterval','setTimeout']:self.assertNotIn(text,pane)
 def test_new_native_http_and_ui_leafs_are_additive_mounted_with_all_cases(self):
  files={'group_directory_data_tests.rs':13,'group_directory_protocol_tests.rs':13,'group_directory_account_tests.rs':8,'group_directory_io_tests.rs':6}
  for name,count in files.items():
   s=src(name);names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',s);self.assertEqual(len(names),count,name);self.assertEqual(len(names),len(set(names)));self.assertNotIn('#[ignore',s)
  for parent,child in [('im_data_tests.rs','group_directory_data_tests.rs'),('im_protocol_tests.rs','group_directory_protocol_tests.rs'),('im_account_tests.rs','group_directory_account_tests.rs'),('im_read_actor_tests.rs','group_directory_io_tests.rs')]:self.assertIn(child,src(parent))
  self.assertIn('include!("app_voice_native_directory_http_tests.rs")',(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
  self.assertEqual((R/'backend/src/api/app_voice_native_directory_http_tests.rs').read_text(encoding='utf-8').count('#[tokio::test]'),3)
  self.assertEqual((R/'frontend/tests/nativeDirectory.test.ts').read_text(encoding='utf-8').count("void test('"),6)
  b=(R/'frontend/e2e/jegotrip-native-directory.spec.ts').read_text(encoding='utf-8');self.assertEqual(b.count("test('"),2)
  for photo in ['native-directory-desktop.png','native-directory-mobile.png','native-directory-generation-desktop.png','native-directory-generation-mobile.png']:self.assertIn(photo,b)
 def test_original_actions_sdk_logout_joins_stack_timeouts_and_budget_are_preserved(self):
  c=(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8')
  for t in ["'jegotrip-im-info.spec.ts'","'jegotrip-native-directory.spec.ts'",'timeout: 45_000','expect: { timeout: 10_000 }','workers: 1','retries: 0','timeout: 120_000']:self.assertIn(t,c)
  for file in ['build-release.yml','beta-validation.yml']:
   s=(R/'.github/workflows'/file).read_text(encoding='utf-8');self.assertIn('http_router_tests::app_voice_accounts',s);self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_client_ex::tests',s);self.assertNotIn('RUST_MIN_STACK',s)
  a=src('group_directory_account_tests.rs');i=src('group_directory_io_tests.rs')
  for t in ['finish_account(&mut peer)','ownership_pending(c, &self.line)','original group.len()+64','pause_original_polls_for_test']:self.assertIn(t,a)
  for t in ['handle.task.is_none()','Duration::from_millis(35)','interest.is_writing()','gate_im_read_write','RuntimeError::SessionRevoked','directory_logout']:self.assertIn(t,i)
if __name__=='__main__':unittest.main()
