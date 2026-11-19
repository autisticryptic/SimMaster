"""Supplementary SOURCE-only original Info DATA invariants, not execution proof."""
from pathlib import Path
import re,unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
class InfoSourceContracts(unittest.TestCase):
 def test_exact_original_info_prefixes_do_not_reinterpret_text(self):
  s=(P/'im_info.rs').read_text(encoding='utf-8');self.assertIn('strip_prefix(b"System.Info.")',s);self.assertIn('strip_prefix(b"Info.")',s);self.assertIn('else{return Ok(None);}',s);self.assertIn('TextField::Text=>return None',s)
 def test_recognized_content_and_base_strings_keep_opaque_data(self):
  s=(P/'im_info.rs').read_text(encoding='utf-8')
  for key in ['"content"','"imdnId"','"csnId"','"displayName"','"userData"','"OnlineMsg"']:self.assertIn(key,s)
  self.assertIn('map.next_value::<String>()?',s);self.assertIn('de::IgnoredAny',s);self.assertNotIn('serde_json::Value',s)
 def test_bounds_duplicate_keys_and_zeroizing_holders_remain(self):
  s=(P/'im_info.rs').read_text(encoding='utf-8')
  for token in ['kind.len()>128','MAX_FIELD_BYTES','MAX_MAP_ENTRIES','bounded_json(message.content)','keys.iter().any','Zeroizing<Vec<u8>>','Zeroizing<String>','"ImInfo(<redacted>)"']:self.assertIn(token,s)
  self.assertIn("value.contains('\\0')",s)
 def test_pure_info_module_has_no_auth_storage_or_io_capabilities(self):
  s=(P/'im_info.rs').read_text(encoding='utf-8')
  for forbidden in ['RpcConnectionScope','AttemptOwner','ImReplyTicket','ImSendPermit','Database','execute(','spawn(','TcpStream','write_all','unsafe']:self.assertNotIn(forbidden,s)
 def test_new_leafs_are_additive_and_keep_original_text_assertions(self):
  self.assertIn('pub(crate) mod im_info;',(P/'mod.rs').read_text(encoding='utf-8'));self.assertIn('#[path="im_info_tests.rs"]\nmod info;',(P/'im_data_tests.rs').read_text(encoding='utf-8'))
  t=(P/'im_text_tests.rs').read_text(encoding='utf-8');self.assertIn('b"Info.Text"',t);self.assertIn('b"System.Info.Text"',t);self.assertIn('unwrap().is_none()',t)
  names=re.findall(r'#\[test\]\s*fn\s+(\w+)',(P/'im_info_tests.rs').read_text(encoding='utf-8'));self.assertEqual(len(names),12);self.assertEqual(len(set(names)),12)
 def test_no_stack_budget_timeout_retry_dependency_or_ignore_bypass(self):
  for file in ['im_info.rs','im_info_tests.rs']:
   s=(P/file).read_text(encoding='utf-8');self.assertNotIn('#[ignore',s);self.assertNotIn('RUST_MIN_STACK',s);self.assertNotIn('spawn_blocking',s)
  for file in ['.github/workflows/build-release.yml','.github/workflows/beta-validation.yml']:
   s=(R/file).read_text(encoding='utf-8');self.assertNotIn('RUST_MIN_STACK',s)
if __name__=='__main__':unittest.main()
