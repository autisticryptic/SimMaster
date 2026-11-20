"""SOURCE-only Info reception invariants; Actions execute Rust/HTTP/JS/browser."""
from pathlib import Path
import re,unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return(P/name).read_text(encoding='utf-8')
class InfoRuntimeSourceContracts(unittest.TestCase):
 def test_receive_info_is_explicit_omitted_when_false_and_does_not_change_native_identity_hash(self):
  s=src('im_delivery.rs');self.assertIn('#[serde(default,skip_serializing_if="no_info")]',s);self.assertIn('fn no_info(value:&bool)->bool{!*value}',s)
  self.assertIn('self.config.receive_info',s);self.assertIn('b"jegotrip-native-im-context-v1\\0",uid,selector',s);self.assertIn('NativeReceiverBinding::original',s);self.assertNotIn('uid,selector,config',s)
  self.assertIn('fn new(enabled:bool)->Self{Self::with_info(enabled,false)}',src('im_protocol_tests.rs'))
 def test_normalized_info_is_text_first_bounded_opaque_data_not_an_io_or_write_capability(self):
  s=src('im_content.rs');self.assertLess(s.index('ImText::decode(message)'),s.index('ImInfo::decode(message)'));self.assertIn('if !binding.receives_info(){return Ok(None);}',s)
  self.assertIn('ImContent(<redacted>)',s)
  for f in ['ImReplyTicket','CommittedIm','Database','UserReadPermit','ImSendPermit','execute(','tokio::spawn','TcpStream']:self.assertNotIn(f,s)
  i=src('im_info.rs')
  for t in ['MAX_FIELD_BYTES','MAX_MAP_ENTRIES','bounded_json(message.content)','keys.iter().any','Zeroizing<String>','TextField::Text=>return None']:self.assertIn(t,i)
 def test_schema_is_additive_private_info_epoch_is_bounded_and_tombstones_do_not_resurrect(self):
  s=src('im_inbox.rs')
  for t in ["body_kind TEXT NOT NULL DEFAULT 'text'","info_type BLOB CHECK",'system_info INTEGER NOT NULL DEFAULT 0','info_epoch TEXT CHECK',"length(CAST(info_epoch AS BLOB))=32",'ALTER TABLE app_voice_im_messages ADD COLUMN','UNIQUE(line_id,revision,identity_key,box_id,message_index)']:self.assertIn(t,s)
  view=s.split('pub struct ImMessageView{',1)[1].split('}',1)[0]
  self.assertEqual(view.count('skip_serializing_if="Option::is_none"'),3)
  for secret in ['info_epoch','identity_key','fingerprint','body:','sender:','token:']:self.assertNotIn(secret,view)
  self.assertIn("SET deleted=1,body=X'',text='',sender=X'',attachment_count=0,info_type=NULL,system_info=0",s);self.assertNotIn('DELETE FROM app_voice_im_messages',s);self.assertNotIn('REPLACE INTO app_voice_im_messages',s)
 def test_raw_info_subtype_and_current_epoch_bytes_share_old_quotas_until_real_consumption(self):
  s=src('im_inbox.rs');c=src('im_cursor.rs')
  self.assertIn('MAX_TOTAL_BYTES:i64=16*1024*1024',s);self.assertIn('MAX_PER_ACCOUNT:i64=1024',s);self.assertIn('MAX_TOTAL:i64=4096',s)
  for text in [s,c]:self.assertIn('COALESCE(length(info_type),0)+COALESCE(length(CAST(info_epoch AS BLOB)),0)',text)
  self.assertIn('info_epoch.map_or(0,|v|v.len())',s);self.assertIn('self.info_epoch.as_ref().map_or(0,|v|v.len())',c);self.assertIn('body_kind=\'info\'',s)
  self.assertIn('self.binding.receives_info()||!self.gate.live()||!self.binding.matches',s);self.assertIn('bytes.checked_add(self.binding.receipt_epoch().len()as i64)',s)
 def test_info_lookup_and_explicit_user_read_require_same_current_consumption_not_old_sql_or_flags(self):
  s=src('im_inbox.rs');u=src('im_user_read_journal.rs')
  self.assertIn("body_kind='info' AND ?6=1 AND info_epoch=?7",s);self.assertIn('i64::from(self.binding.receives_info()),self.binding.receipt_epoch()',s)
  self.assertIn("body_kind='info' AND ?5=1 AND info_epoch=?6",u);self.assertIn("body_kind='info' AND ?9=1 AND info_epoch=?10",u)
  for t in ['self.verify_message(&tx,true)?','Arc::ptr_eq(reply.request().query_arc(),&self.query)','if unknown{return Err(UNCERTAIN);}','SET state=?4,result_code=?5']:self.assertIn(t,u)
  self.assertIn('if saved.as_slice()!=fingerprint.as_slice()',s);self.assertIn('self.mark_info_consumed(&tx,id)?',s)
 def test_info_gaps_keep_original_binding_query_gate_sql_cas_and_atomic_event_order(self):
  c=src('im_cursor.rs')
  for t in ['ImContent::capture(binding,message)?.ok_or(UNSUPPORTED)?','Arc::ptr_eq(&plan.query,reply.request().query_arc())','Arc::ptr_eq(&reply.request().gate(),&self.context.gate)','TransactionBehavior::Immediate','self.context.validate(&tx)?;self.reply_current(reply)?','AND baseline=?9 AND consumed=?10 AND target>=?5','for r in &records{let(id,event)=self.insert_record(&tx,r)?','for event in events{db.publish_committed_app_event(event)']:self.assertIn(t,c)
  self.assertNotIn('Notify.MsgIdx',c);self.assertNotIn('prepare_im_write',c)
  a=src('account_im.rs');self.assertIn('MAX_GROUPS:usize=64',a);self.assertIn('MAX_HOLDERS:usize=256',a);self.assertIn('MAX_BYTES:usize=1024*1024',a);self.assertIn('MAX_EARLY:usize=16',a);self.assertNotIn('tokio::spawn',a)
 def test_new_native_http_unit_and_browser_leafs_are_mounted_without_replacing_old_tests(self):
  files={'im_info_runtime_protocol_tests.rs':12,'im_info_runtime_epoch_tests.rs':3,'im_info_runtime_cursor_tests.rs':6,'im_info_runtime_account_tests.rs':8,'im_info_runtime_sync_account_tests.rs':4,'im_info_runtime_io_tests.rs':5}
  for name,count in files.items():
   s=src(name);names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',s);self.assertEqual(len(names),count,name);self.assertEqual(len(names),len(set(names)),name);self.assertNotIn('#[ignore',s)
  for parent,child in [('im_protocol_tests.rs','im_info_runtime_protocol_tests.rs'),('im_info_runtime_protocol_tests.rs','im_info_runtime_epoch_tests.rs'),('im_info_runtime_protocol_tests.rs','im_info_runtime_cursor_tests.rs'),('im_account_tests.rs','im_info_runtime_account_tests.rs'),('im_sync_account_tests.rs','im_info_runtime_sync_account_tests.rs'),('im_read_actor_tests.rs','im_info_runtime_io_tests.rs')]:self.assertIn(child,src(parent))
  api=(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8');self.assertIn('include!("app_voice_im_http_tests.rs")',api);self.assertIn('include!("app_voice_im_info_http_tests.rs")',api)
  http=(R/'backend/src/api/app_voice_im_info_http_tests.rs').read_text(encoding='utf-8');self.assertEqual(http.count('#[tokio::test]'),2)
  unit=(R/'frontend/tests/imHistory.test.ts').read_text(encoding='utf-8');self.assertEqual(unit.count("void test('"),7)
  e=(R/'frontend/e2e/jegotrip-im-info.spec.ts').read_text(encoding='utf-8');self.assertEqual(e.count("test('"),1)
  for name in ['im-info-desktop.png','im-info-mobile.png']:self.assertIn(name,e)
 def test_exact_ui_metadata_and_original_actions_stack_timeouts_filters_and_retries_remain(self):
  p=(R/'frontend/src/policies/imHistory.ts').read_text(encoding='utf-8');self.assertIn("own('kind')",p);self.assertIn("own('info_type')",p);self.assertIn("own('system_info')",p);self.assertIn('/^[A-Za-z0-9_.-]{1,128}$/',p);self.assertIn('rows.length === 16',p)
  pane=(R/'frontend/src/pages/sim/AppImMessagesPane.tsx').read_text(encoding='utf-8')
  for s in ['dangerouslySetInnerHTML','localStorage','setAppAccountRunning']:self.assertNotIn(s,pane)
  config=(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8')
  for t in ["'jegotrip-im-messages.spec.ts'","'jegotrip-im-info.spec.ts'",'timeout: 45_000','expect: { timeout: 10_000 }','workers: 1','retries: 0','timeout: 120_000','http://127.0.0.1:1']:self.assertIn(t,config)
  for f in ['build-release.yml','beta-validation.yml']:
   s=(R/'.github/workflows'/f).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_client_ex::tests',s);self.assertIn('http_router_tests::app_voice_accounts',s);self.assertNotIn('RUST_MIN_STACK',s)
if __name__=='__main__':unittest.main()
