"""Supplemental SOURCE fences for exact native reference ownership, not native execution."""
from pathlib import Path
import re,unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(name):return(P/name).read_text(encoding='utf-8')
class ReferenceOwnershipSource(unittest.TestCase):
 def test_exact_raw_input_is_private_zeroizing_and_distinct_from_canonical_encoder(self):
  s=text('object_reference.rs');self.assertIn('original: Zeroizing<Vec<u8>>',s);self.assertIn('pub(super) fn original_factory_input',s);self.assertIn('original: Zeroizing::new(bytes.to_vec())',s);self.assertIn('pub fn encode_native',s);self.assertNotIn('pub fn original_factory_input',s)
 def test_accounting_is_all_owned_payload_and_does_not_encode_allocate_or_generate_permissions(self):
  s=text('object_reference.rs').split('pub(super) fn retained_bytes',1)[1].split('/// Exact native identity',1)[0]
  for x in ['self.original.len()','self.identity.len()','self.components.iter()','adapter.len()','e.scheme.len()','e.host.len()','id.literal.len()']:self.assertIn(x,s)
  for x in ['encode_native','to_vec','reserve_native_rpc','deserialize','connect(']:self.assertNotIn(x,s)
 def test_source_authority_is_checked_before_snapshot_move(self):
  s=text('user_entry_reference.rs');self.assertIn('into_reference(self,owner:&AttemptOwner,source:&RpcConnectionScope)',s);self.assertIn('self.reference_for(owner,source).is_none()',s);self.assertIn('Ok(self.reference)',s);self.assertIn('source.session_revoked()',s);self.assertNotIn('Deserialize',s)
 def test_qualified_target_keeps_actual_raw_snapshot_without_canonical_reparse_or_route_expansion(self):
  s=text('user_entry_target.rs');self.assertIn('ReferenceTarget::Direct',s);self.assertIn('endpoints.len()>16',s);self.assertIn('original.into_reference(owner,source)',s);self.assertIn('reference.original_factory_input().len()',s);self.assertNotIn('encode_native',s);self.assertNotIn('parse_object_reference',s);self.assertIn('parent.matches(source)',s);self.assertIn('self.router.matches(router)',s)
 def test_uid_local_keeps_original_reference_and_zero_io_without_granting_alias_or_readiness(self):
  s=text('user_entry_local.rs');self.assertIn('reference:super::object_reference::ObjectReference',s);self.assertIn('reference.into_reference(owner,source)',s);self.assertIn('reference.reference_for(owner,source)',s);self.assertIn('self.reference.retained_bytes()',s);self.assertIn('recipient.kind()!=AidKind::Uid',s)
  for x in ['encode_request','send_call','read_im','tokio::spawn','reserve_native_rpc']:self.assertNotIn(x,s)
 def test_common_plan_preserves_original_input_and_combined_existing_local_and_auxiliary_budget(self):
  s=text('runtime_provision.rs').split('impl ProvisionedCommonLogin',1)[1];self.assertIn('parse_object_reference(reference.original_factory_input())',s);self.assertNotIn('reference.encode_native()',s);self.assertIn('let mut total=entry.retained_bytes()',s);self.assertIn('checked_add(r.retained_bytes())',s);self.assertIn('checked_add(local_bytes)',s);self.assertIn('checked_add(t.retained_bytes())',s);self.assertIn('n>128*1024',s);self.assertIn('Ok(target.map(Arc::from))',s)
 def test_new_rust_leaves_are_mounted_without_ignored_cases_or_larger_stack(self):
  mounts={'object_reference_tests.rs':'object_reference_factory_input_tests.rs','user_entry_reference_tests.rs':'user_entry_reference_ownership_tests.rs','user_entry_uid_local_tests.rs':'user_entry_uid_local_ownership_tests.rs','runtime_provision_tests.rs':'runtime_common_reference_ownership_tests.rs'}
  for parent,leaf in {**mounts,'runtime_common_backend_tests.rs':'runtime_reference_profile_ownership_tests.rs'}.items():self.assertEqual(text(parent).count('"'+leaf+'"'),1);self.assertIn('mod ',text(parent));s=text(leaf);self.assertNotIn('#[ignore',s);self.assertNotIn('RUST_MIN_STACK',s);self.assertTrue(re.search(r'#\[test\]\s*fn',s))
 def test_original_wire_and_write_boundaries_are_not_changed_by_ownership_data(self):
  for name in ['object_reference.rs','user_entry_reference.rs','user_entry_target.rs','user_entry_local.rs']:
   s=text(name);self.assertNotIn('Deserialize',s);self.assertNotIn('set_read(',s);self.assertNotIn('resolve_unknown',s);self.assertNotIn('tokio::spawn',s)
  self.assertIn('self.parent.matches(parent)',text('user_entry_target.rs'));self.assertIn('self.parent.matches(parent)',text('user_entry_local.rs'));self.assertIn('entry.retained_bytes().checked_add(reference.len())',text('runtime_backend.rs'));self.assertIn('const MAX_PROFILE_BYTES: usize = 128 * 1024',text('runtime_backend.rs'))
if __name__=='__main__':unittest.main()
