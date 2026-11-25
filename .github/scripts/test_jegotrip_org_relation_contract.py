"""Supplemental SOURCE fences, not execution or complete Group/Buddy acceptance."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class OrgRelationContracts(unittest.TestCase):
 def test_org_schema_is_created_only_by_explicit_account_optin_not_unrelated_database_start(self):
  self.assertNotIn('org_relation_inbox::initialize_schema',src('im_inbox.rs'))
  s=src('account_store.rs');self.assertEqual(s.count('if config.profile.org_relation_sync.is_some(){super::org_relation_inbox::initialize_schema(&tx)?;}'),2)
  s=src('org_relation_inbox.rs');self.assertIn('if !installed(c)?{return Ok((0,0,0));}',s);self.assertIn('if !installed(c)?{return Ok(Vec::new());}',s)
  self.assertIn('without_control_tables',src('org_relation_sync_tests.rs'));self.assertIn('install_control_tables_atomically',src('org_relation_sync_tests.rs'))
 def test_system_record_is_not_group_update_map_chat_json_or_local_notification(self):
  s=src('org_relation_sync.rs')
  for v in ['KIND:&[u8]=b"System.OrgRelSync"','SYSTEM_BOX:&[u8]=b"P2P/0_1"','message.parameters','b"UpdateRel"','let flags=r.u32()?','first=r.bytes()?','second=r.bytes()?','properties=r.properties()?']:self.assertIn(v,s)
  for v in ['decode_group_update(', 'serde_json::from', 'base64::', 'GroupEventTicket']:self.assertNotIn(v,s)
 def test_missing_relation_uses_original_type20_and_optional_times_not_zero_or_im_index(self):
  s=src('org_relation_sync.rs');self.assertIn('flags:20,first:b"",second:b"",properties:Vec::new()',s);self.assertIn('filter(|v|!v.is_empty()).map(signed).transpose()',s);self.assertIn('get(b"OrgDel")==Some(b"1".as_slice())',s)
  for v in ['unwrap_or(0)','message.index','message.time']:self.assertNotIn(v,s[s.index('pub(super) fn decode'):])
 def test_optin_is_independent_native_common_clientex_im_and_defaults_omitted(self):
  s=src('account_config.rs');self.assertIn('pub org_relation_sync:Option<',s);self.assertIn('with_org_relation_sync(p.org_relation_sync)',s)
  s=src('native_receiver.rs');self.assertIn('org_relation_sync:false',s);self.assertIn('self.org_relation_sync||self.im_messages.is_none()',s)
  s=src('runtime_backend.rs');self.assertIn('if self.common.is_none()||!self.client_ex',s)
 def test_control_context_binds_original_run_binding_gate_config_and_database(self):
  s=src('org_relation_inbox.rs')
  for v in ['Arc::ptr_eq(&self.run,run)','Arc::ptr_eq(&self.binding,binding)','Arc::ptr_eq(&self.gate,gate)','std::ptr::eq(self.database.as_ref(),db)','self.configuration.require_current','account_runs::runtime_matches']:self.assertIn(v,s)
  self.assertIn('stored.config.profile.org_relation_sync.is_some()',src('account_relation_updates.rs'))
  self.assertIn('run.backend.original_scope(binding.owner())',src('account_im.rs'))
 def test_control_lives_in_separate_tables_never_chat_or_outbound_write_result(self):
  s=src('org_relation_inbox.rs')
  for v in ['app_voice_org_relation_messages','app_voice_org_relation_heads','app_voice_org_relation_members','app_voice.org_relation_observed','"full_roster_verified":false']:self.assertIn(v,s)
  for v in ['UPDATE app_voice_relation_writes','DELETE FROM app_voice_relation_intents','INSERT INTO app_voice_im_messages','full_roster_verified:true']:self.assertNotIn(v,s)
 def test_control_and_chat_share_original_quota_and_no_collision_at_same_message_index(self):
  s=src('org_relation_inbox.rs');self.assertIn('local.checked_add(chat_local)',s);self.assertIn('bytes.checked_add(chat_bytes)',s);self.assertIn('if chat{return Err("app_account_org_relation_conflict")',s)
  for file in ['im_inbox.rs','im_cursor.rs']:s=src(file);self.assertIn('org_relation_inbox::usage(',s);self.assertIn('org_relation_inbox::contains_saved(',s);self.assertIn('control_local.checked_add(local)',s)
 def test_control_and_cursor_use_same_original_sql_transaction_and_no_skip_on_unknown(self):
  s=src('im_cursor.rs');self.assertIn('org:Option<Arc<super::super::org_relation_inbox::OrgRecord>>',s);self.assertIn('self.context.org.as_ref().ok_or(SCOPE)?.insert(tx,org)',s);self.assertIn('for r in &records{let(id,event)=self.insert_record(&tx,r)?',s);self.assertIn('self.advance(&tx,box_id,plan.baseline,plan.consumed,gap,end,&span_hash)?',s);self.assertIn('org_permit(&r.key,r.fingerprint,id,r.org.clone())',s)
  self.assertIn('if record.key.index()<=consumed&&record.org.is_some()',s)
 def test_new_login_requires_original_body_reconsumption_and_old_record_is_not_a_current_hint_receipt(self):
  s=src('org_relation_inbox.rs');self.assertIn('AND receipt_epoch=?5',s);self.assertIn('epoch.ok_or(STORAGE)?==self.binding.receipt_epoch()',s);self.assertIn('let old_id=',s);self.assertIn('if let Some(id)=old_id',s)
  self.assertIn('new_login_notification_cannot_redeem_old_consumption_until_original_body_reconsumed',src('org_relation_protocol_tests.rs'))
 def test_notice_and_control_records_precharge_shared_budget_and_preserve_measured_bytes(self):
  s=src('im_delivery.rs');self.assertLess(s.index('org_relation_sync::message_scratch'),s.index('let Some(decoded)=im_data::decode_more_message'));self.assertIn('decoded.message.encode_measured(body.len())',s)
  s=src('org_relation_inbox.rs');self.assertLess(s.index('reserve_native_rpc(peak)'),s.index('org_relation_sync::decode(box_id,message)'));self.assertIn('std::mem::size_of::<OrgReceipt>()',s);self.assertIn('Some(r)=>r._budget.clone()',s)
 def test_original_consumption_receipt_rechecks_saved_record_config_gate_deadline_after_sql(self):
  s=src('org_relation_inbox.rs');self.assertIn('self.context.record_current(&self.key,self.id,&self.fingerprint)',s);self.assertIn('&&self.context.gate.live()&&ticket.matches',s);self.assertIn('Instant::now()<ticket.completion_deadline()',s)
  s=src('im_associations.rs');self.assertIn('proof.original_org_record(&tx)?',s);self.assertIn('Some((id,hash,true))=>AssociationAdmission::Replay(Some(self.org_permit',s);self.assertIn('Some((id,hash,false))=>AssociationAdmission::Replay((id>0).then(||self.permit',s)
 def test_original_reply_packet_lifetime_retains_both_group_and_system_control_without_grant_conversion(self):
  s=src('group_event_packets.rs');self.assertIn('capture_with_org',s);self.assertIn('p.org_receipt()',s);self.assertIn('own.org.push((t.captured_copy(),p.captured_copy()))',s)
  self.assertIn('GroupEventPackets::capture_with_org',src('arc_protocol.rs'));self.assertIn('GroupEventPackets::capture_with_org',src('offline_backend.rs'))
  self.assertIn('same_fd',src('org_relation_io_tests.rs'));self.assertIn('scope.native_rpc_resources(),(0,0)',src('org_relation_io_tests.rs'))
 def test_http_is_get_only_with_bounded_redacted_view_not_raw_control_or_write(self):
  s=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('org-relation-events",get(relation_updates::org_event_history)',s);self.assertNotIn('org-relation-events",get(relation_updates::org_event_history).post',s)
  s=src('org_relation_inbox.rs');self.assertIn('CASE WHEN continuity IN(',s);self.assertIn('length(CAST(created_at AS BLOB)) BETWEEN 1 AND 64',s);self.assertIn('OrgRelationView(<redacted>)',s)
 def test_readonly_ui_requires_explicit_click_and_keyed_generation_no_reconciliation(self):
  s=(ROOT/'frontend/src/pages/sim/AppOrgRelationEventsPane.tsx').read_text(encoding='utf-8');self.assertIn('key={JSON.stringify([lineId, revision])}',s);self.assertIn('api.getAppOrgRelationEvents(lineId)',s);self.assertIn('if (active()) setRows(rows)',s)
  for v in ['updateAppRelations','localStorage','setInterval','setAppAccountRunning']:self.assertNotIn(v,s)
  s=(ROOT/'frontend/src/policies/orgRelationEvents.ts').read_text(encoding='utf-8');self.assertIn("own('full_roster_verified') !== false",s);self.assertIn('不确认或清除 Unknown-WRITE',s)
 def test_all_new_regressions_mounted_with_original_workflow_filters_and_browser_budgets(self):
  for parent,child in [('org_relation_sync.rs','org_relation_sync_tests.rs'),('im_protocol_tests.rs','org_relation_protocol_tests.rs'),('org_relation_protocol_tests.rs','org_relation_cursor_tests.rs'),('im_account_tests.rs','org_relation_account_tests.rs'),('im_read_actor_tests.rs','org_relation_io_tests.rs')]:self.assertIn(f'#[path="{child}"]',src(parent))
  for file in ['build-release.yml','beta-validation.yml']:self.assertIn('connectivity::app_voice::providers::jegotrip::org_relation_sync::tests \\\n',(ROOT/'.github/workflows'/file).read_text(encoding='utf-8'))
  self.assertIn('include!("app_voice_org_relation_http_tests.rs")',(ROOT/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
  s=(ROOT/'frontend/package.json').read_text(encoding='utf-8');self.assertIn('tests/relationEvents.test.ts tests/orgRelationEvents.test.ts',s)
  s=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn(".concat(['jegotrip-org-relation-events.spec.ts'])",s);self.assertIn('timeout: 45_000',s);self.assertIn('retries: 0',s)
if __name__=='__main__':unittest.main()
