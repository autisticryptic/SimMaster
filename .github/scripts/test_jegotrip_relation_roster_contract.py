"""SOURCE fences for explicit original full/partial roster; not runtime proof."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class RosterContracts(unittest.TestCase):
 def test_new_privacy_fixture_groupid_does_not_alias_rfc3339_calendar_text(self):
  s=src('relation_roster_tests.rs');block=s[s.index('fn roster_history_and_debug_never_export'):]
  self.assertIn('Some(b"711111111-822222222")',block);self.assertIn('for private in ["711111111-822222222"',block);self.assertIn('assert!(!s.contains(private))',block)
  self.assertIn('1-2','2026-11-25T23:00:00Z');self.assertNotIn('711111111-822222222','2026-11-25T23:00:00Z')
 def test_roster_history_rejects_bad_limit_before_account_scope_without_changing_http_assertions(self):
  s=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');block=s[s.index('pub async fn relation_roster_history('):s.index('pub async fn get_im_messages(')]
  guard='if !(1..=128).contains(&query.limit){return failure("app_account_directory_request_invalid");}'
  self.assertIn(guard,block);self.assertLess(block.index(guard),block.index('manager.relation_roster_history('))
  tests=(ROOT/'backend/src/api/app_voice_roster_http_tests.rs').read_text(encoding='utf-8')
  self.assertIn('for suffix in ["?limit=0","?limit=129","?limit=1&token=private","?limit=1&clear_gap=true"]',tests)
  self.assertIn('assert_eq!(status,StatusCode::BAD_REQUEST);no_store(&headers);no_secrets(&body);',tests)
 def test_directory_decoding_borrows_and_charges_both_maps_names_and_footer_before_allocations(self):
  s=src('group_directory_data.rs');block=s[s.index('fn add_headers('):s.index('pub(super) fn decode_relations_scoped')]
  for bad in ['Vec::','vec![','.collect(','.to_vec()']:self.assertNotIn(bad,block)
  self.assertIn('for _ in 0..2',block);self.assertIn('measure_properties(r,size)',block);self.assertIn('std::mem::size_of::<Relation',block);self.assertIn('std::mem::size_of::<&[u8]>()',block)
  for kind in ['relations','properties']:
   block=s.split('pub(super) fn decode_'+kind+'_scoped',1)[1].split('\n}',1)[0]
   self.assertLess(block.index('scope.reserve_native_rpc('),block.index('let mut value=decode_'+kind+'('));self.assertIn('value._decode_budget=Some(budget)',block)
   self.assertIn('directory::decode_'+kind+'_scoped(&self.binding.scope(),code,bytes)',src('rpc_im.rs'))
  self.assertIn('impl Drop for RelationsResult',s);self.assertIn('impl Drop for PropertiesResult',s)
 def test_roster_capacity_uses_transactional_final_delta_without_weakening_original_caps(self):
  s=src('relation_roster.rs');block=s.split(' fn apply(',1)[1].split('/// Binds',1)[0]
  self.assertLess(block.index('DELETE FROM app_voice_relation_roster_members'),block.index('for m in self.values.iter().filter(|m|!m.remove)'))
  self.assertIn('local>=1024||total>=4096',block);self.assertIn('n>16*1024*1024',block)
  for name in ['roster_net_neutral_partial_at_local_and_global_capacity','roster_net_neutral_type20_event_at_capacity','roster_capacity_replacement_deletes_roll_back']:self.assertIn(name,src('relation_roster_tests.rs'))
 def test_directory_summary_drops_decoded_scratch_before_original_reply(self):
  s=src('account_directory.rs');self.assertIn('let view=match(reply.request().query(),reply.result()',s);self.assertIn('};view',s)
 def test_roster_policy_requires_independent_directory_and_both_event_consumer_installations(self):
  s=src('runtime_backend.rs');self.assertIn('with_relation_roster',s);self.assertIn('self.relation_events.is_none()',s);self.assertIn('if !native.org_relation_sync()',s)
  s=src('native_receiver.rs');self.assertIn('relation_roster:false',s);self.assertIn('self.im_messages.is_none_or(|im|!im.directory_queries)',s)
 def test_roster_schema_only_installed_with_explicit_account_config_and_removed_after_authorized_account_delete(self):
  s=src('account_store.rs');self.assertEqual(s.count('if config.profile.relation_roster.is_some(){super::relation_roster::initialize_schema(&tx)?;}'),2)
  self.assertNotIn('relation_roster::initialize_schema',src('im_inbox.rs'));self.assertIn('super::relation_roster::delete_account(conn, line)?',src('account_inbox.rs'))
 def test_original_typed_reply_query_binding_gate_config_and_database_are_not_replaced_by_history(self):
  s=src('relation_roster.rs')
  for x in ['std::ptr::eq(self.database.as_ref(),db)','Arc::ptr_eq(reply.request().query_arc(),&self.query)','Arc::ptr_eq(reply.request().binding(),&self.binding)','Arc::ptr_eq(&reply.request().gate(),&self.gate)','configuration.require_current','account_runs::runtime_matches','reply.matches(self.binding.owner(),&self.binding.scope())']:self.assertIn(x,s)
 def test_original_partial_bool_selects_added_updated_removed_and_full_uses_first_relationlist_only(self):
  s=src('relation_roster.rs');self.assertIn('if value.reported{Changes::capture(&self.binding,&value.first,&value.second,&value.names)?}',s);self.assertIn('Changes::capture(&self.binding,&value.first,&[],&[])',s);self.assertIn('h.observed!=Some(*since)',s)
 def test_read_serial_prevents_older_snapshot_overwriting_any_intervening_event_or_read(self):
  s=src('relation_roster.rs');self.assertIn('serial=head(c,&run,&binding,&target)?.map(|h|h.serial)',s);self.assertIn('old.map(|h|h.serial)!=self.serial',s);self.assertIn('outcome:"superseded"',s);self.assertIn('h.serial.checked_add(1)',s)
 def test_gap_keeps_observed_member_state_until_explicit_snapshot_or_current_since_result(self):
  s=src('relation_roster.rs');self.assertIn('!h.gap&&!h.full_required&&base==h.observed',s);self.assertIn('if !stale&&contiguous{changes.apply(c,h.id)?;}',s);self.assertIn('let required=h.full_required||!stale&&unknown',s);self.assertIn('complete=1,gap=0,full_required=0',s);self.assertIn('value.time<target',s)
 def test_unsequenced_orgdel_and_equal_time_event_require_full_not_empty_incremental_clearance(self):
  s=src('relation_roster.rs');self.assertIn('if new==old',s);self.assertIn('h.full_required',s);self.assertIn('outcome:"baseline_required"',s)
  self.assertIn('if data.org_deleted{None}else{data.base}',src('org_relation_inbox.rs'))
 def test_member_body_headers_are_measured_before_copy_and_sql_members_are_bounded(self):
  s=src('relation_roster.rs');self.assertLess(s.index('reserve_native_rpc(size)'),s.index('Vec::with_capacity(adds.len()'));self.assertIn('Writer::measured(measure(r)',s);self.assertIn('w.bytes(r.label)',s);self.assertIn('local>=1024||total>=4096',s);self.assertIn('n>16*1024*1024',s)
 def test_roster_and_original_event_share_transaction_no_unknown_write_state_is_modified(self):
  self.assertIn('super::relation_roster::observe(&tx',src('group_event_inbox.rs'));self.assertIn('super::relation_roster::observe(c',src('org_relation_inbox.rs'))
  s=src('relation_roster.rs');self.assertIn('TransactionBehavior::Immediate',s);self.assertIn('app_voice.relation_roster_observed',s)
  for x in ['app_voice_relation_writes','app_voice_relation_intents','tokio::spawn','read_directory(']:self.assertNotIn(x,s)
 def test_account_explicit_preparation_keeps_original_budget_and_deadline_without_auto_query(self):
  s=src('account_relation_roster.rs');self.assertIn('self.prepare_native_directory',s);self.assertIn('if !directory.run.backend.relation_roster_enabled()',s);self.assertLess(s.index('RosterRead::capture('),s.index('run.backend.read_directory('));self.assertIn('timeout_at(deadline.into()',s);self.assertIn('original.commit(&run.database,&reply)',s)
 def test_roster_http_read_requires_original_dto_consent_and_keeps_legacy_directory_view(self):
  s=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('relation-roster/read",post(read_relation_roster)',s);self.assertIn('relation-roster",get(relation_roster_history)',s);self.assertIn('manager.prepare_relation_roster',s)
  s=src('account_directory.rs');self.assertNotIn('complete_roster:',s);self.assertIn('reported:Some(value.reported)',s)
 def test_roster_ui_is_explicit_immutable_read_and_historical_state_not_current_write_grant(self):
  s=(ROOT/'frontend/src/pages/sim/AppRelationRosterPane.tsx').read_text(encoding='utf-8');self.assertIn('const captured = body ? { ...body } : null',s);self.assertIn('window.confirm(',s);self.assertIn('current.current.generation === generation',s);self.assertIn('!hosted || !body',s)
  for x in ['updateAppRelations','setInterval','crypto.','localStorage','setAppAccountRunning']:self.assertNotIn(x,s)
  s=(ROOT/'frontend/src/policies/relationRoster.ts').read_text(encoding='utf-8');self.assertIn('historicalComplete',s);self.assertIn('不确认或清除 Unknown-WRITE',s);self.assertIn('IsPartialUpdate=false',s)
 def test_roster_new_tests_mount_under_original_filters_and_preserve_all_timeouts(self):
  self.assertIn('#[path="relation_roster_tests.rs"]',src('im_protocol_tests.rs'));self.assertIn('#[path="relation_roster_account_tests.rs"]',src('group_directory_account_tests.rs'));self.assertIn('include!("app_voice_roster_http_tests.rs")',(ROOT/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
  s=(ROOT/'frontend/package.json').read_text(encoding='utf-8');self.assertIn('tests/orgRelationEvents.test.ts tests/relationRoster.test.ts',s)
  s=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn(".concat(['jegotrip-relation-roster.spec.ts'])",s);self.assertIn('timeout: 45_000',s);self.assertIn('retries: 0',s)
if __name__=='__main__':unittest.main()
