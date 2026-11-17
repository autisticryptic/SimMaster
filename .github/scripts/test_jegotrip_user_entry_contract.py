"""Source-derived UserEntry resolver DATA/provenance fences, not native IO proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class UserEntryContracts(unittest.TestCase):
 def test_original_method_pairset_count_versions_and_key_order_not_group_write(self):
  s=src('user_entry_data.rs');self.assertIn('aidToUid.UserEntry.User',s);self.assertIn('Writer::versioned()',s);self.assertIn('w.u32(sorted.len()as u32)',s);self.assertIn('compare_keys(a.kind.bytes(),b.kind.bytes()).then_with',s);self.assertNotIn('setStatus.GroupServer.Group',s)
 def test_map_keys_are_resolved_uids_and_original_aid_pair_is_retained(self):
  s=src('user_entry_data.rs');self.assertIn('let uid=r.bytes()?;let kind=r.bytes()?;let value=r.bytes()?',s);self.assertIn('entries.push(AidUidEntry{uid,aid})',s);self.assertIn('entries.iter().any(|e|e.uid==uid)',s);self.assertIn('read_footer(&mut r)?;r.finish()?',s)
 def test_uri_data_has_no_guessed_phone_group_scope_or_authority_routing(self):
  s=src('user_entry_data.rs');self.assertIn('rest.iter().rposition(|b|*b==b\'@\')',s);self.assertIn('MAX_RECIPIENT_BYTES:usize=512',s);self.assertIn('MAX_AIDS:usize=128',s)
  for no in ['RouterPath::open','UserReadPermit','ImBinding::authenticated','connect(']:self.assertNotIn(no,s)
 def test_reference_is_from_original_bootstrap_auxiliary_field_and_source_scope(self):
  s=src('user_entry_reference.rs');self.assertIn('reply.material_for(owner,source)',s);self.assertIn('material.user_entry',s);self.assertIn('decode_configuration(material.user_entry,key)',s);self.assertIn('self.source.matches(source)',s);self.assertIn('source.is_open()&&!source.session_revoked()',s)
 def test_source_reference_does_not_steal_account_entry_or_authorize_sdk_io(self):
  s=src('user_entry_reference.rs')
  for no in ['material.account_entry','GROUP_OBJECT','prepare_login','RuntimeHandle','tokio::spawn','AccountTrust::supplied','UserReadPermit']:self.assertNotIn(no,s)
 def test_data_and_reference_tests_are_mounted_under_original_deployment_filter(self):
  self.assertIn('mod user_entry_data;',src('mod.rs'));self.assertIn('mod user_entry_reference;',src('mod.rs'));s=src('deployment_tests.rs');self.assertIn('#[path="user_entry_data_tests.rs"]\nmod recipient_data;',s);self.assertIn('#[path="user_entry_reference_tests.rs"]\nmod user_entry;',s)
 def test_new_tests_preserve_actions_only_stack_and_deadline_protections(self):
  for n in ['user_entry_data_tests.rs','user_entry_reference_tests.rs']:
   for no in ['#[ignore','RUST_MIN_STACK','stack_size','Command::new("adb")','dlopen']:self.assertNotIn(no,src(n))
if __name__=='__main__':unittest.main()
