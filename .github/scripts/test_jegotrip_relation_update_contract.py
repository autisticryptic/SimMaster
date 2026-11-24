"""SOURCE-only original Group/Buddy relation-change schema/guard fences, not execution."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def source(name):return(P/name).read_text(encoding='utf-8')
class RelationUpdateContracts(unittest.TestCase):
 def test_method_is_original_set_relations_not_a_reused_read(self):
  self.assertIn('SET_RELATIONS_METHOD:&str="setRelations.GroupServer.Group"',source('group_relation_updates.rs'))
  self.assertIn('RELATIONS_METHOD:&str="getRelations2.GroupServer.Group"',source('group_directory_data.rs'))
 def test_schema_preserves_version_identifier_two_maps_and_bool_map(self):
  s=source('group_relation_updates.rs');self.assertIn('Writer::versioned_measured(capacity)',s);self.assertIn('writer.bytes(identifier)?;write_relations(&mut writer,additions)?;write_relations(&mut writer,updates)?',s);self.assertIn('writer.bytes(r.label)?;writer.boolean(r.flag)?',s)
 def test_relation_fields_keep_original_u32_two_bytes_and_property_map(self):
  s=source('group_relation_updates.rs');self.assertIn('writer.u32(v.flags)?;writer.bytes(v.first)?;writer.bytes(v.second)?;writer.properties(&v.properties)?',s);self.assertIn('compare_keys(a.label,b.label)',s)
 def test_measurement_checks_before_secret_writer_allocation(self):
  s=source('group_relation_updates.rs');self.assertIn('let capacity=encoded_size(identifier,additions,updates,removals)?',s);self.assertIn('if value.len()>MAX_FIELD_BYTES',s);self.assertIn('if *total>MAX_STREAM_BYTES',s);self.assertIn('if values.len()>MAX_MAP_ENTRIES',s);self.assertIn('CodecError::DuplicateKey',s)
 def test_result_is_bool_two_signed_times_then_complete_original_footer(self):
  s=source('group_relation_updates.rs');self.assertIn('let base_time=reader.i64()?;let update_time=reader.i64()?;let footer=read_footer(&mut reader)?;reader.finish()?',s);self.assertIn('accepted:accepted&&footer.reason.is_empty()',s)
 def test_remote_version_errors_cannot_trigger_automatic_unknown_write_retry(self):
  s=source('group_relation_updates.rs');self.assertIn('if code!=0{return Err(CodecError::RemoteFailure);}',s)
  for forbidden in ['tokio::spawn','connect(', 'login(', 'scope.close(', 'unsafe','Deserialize','#[ignore']:
   self.assertNotIn(forbidden,s)
 def test_mtc_type_mapping_is_finite_and_not_a_role_grant(self):
  s=source('group_relation_updates.rs');self.assertIn('257=>Ok(0),258=>Ok(1),259=>Ok(2),4097=>Ok(10)',s);self.assertIn('_=>Err(CodecError::InvalidField)',s)
 def test_original_removal_false_and_independent_maps_have_strict_new_coverage(self):
  s=source('group_relation_updates_tests.rs');self.assertIn('preserves_original_false_flag_not_true_default',s);self.assertIn('assert!(!r.boolean().unwrap())',s);self.assertIn('keep_independent_native_maps_but_reject_high_level_conflicting_action',s);self.assertIn('reject_conflicting_changes',s)
 def test_new_module_and_original_two_workflow_filters_are_explicit(self):
  self.assertIn('pub(crate) mod group_relation_updates;',source('mod.rs'));self.assertIn('#[path="group_relation_updates_tests.rs"]',source('group_relation_updates.rs'))
  for f in ['build-release.yml','beta-validation.yml']:
   s=(ROOT/'.github/workflows'/f).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::group_relation_updates::tests \\\n',s)
 def test_new_reply_debug_is_redacted_with_no_credential_or_ready_projection(self):
  s=source('group_relation_updates.rs');self.assertIn('OriginalRelationUpdateResult(<redacted>)',s);self.assertIn('OriginalRelationRemoval(<redacted>)',s)
  for forbidden in ['pub token:', 'pub owner:', 'pub scope:', 'pub ready:', 'pub granted:']:
   self.assertNotIn(forbidden,s)
 def test_relation_journal_is_initialized_and_deleted_only_after_authorized_account_removal(self):
  self.assertIn('super::relation_journal::initialize_schema(conn)?',source('im_inbox.rs'))
  self.assertIn('super::relation_journal::delete_account(conn, line)?',source('account_inbox.rs'))
  s=source('relation_journal.rs');self.assertIn('if account_store::metadata(c,line)?.is_some(){return Err(SCOPE);}',s);self.assertIn("name='app_voice_relation_writes'",s)
 def test_nonce_body_conflicts_and_any_uncertain_intent_precede_new_rpc_allocation(self):
  s=source('relation_journal.rs');self.assertIn('UNIQUE(line_id,user_nonce)',s);self.assertIn('app_account_relation_nonce_conflict',s);self.assertIn('line_id=?1 AND state=0',s);self.assertIn('if unknown{return Err(UNKNOWN);}',s)
  self.assertIn('let id=self.next_id.ok_or(CodecError::InvalidCallId)?',source('rpc_relation_update_session.rs'))
 def test_original_run_configuration_snapshot_is_private_frozen_and_checked_before_first_write(self):
  s=source('account_relation_updates.rs');self.assertIn('TransactionBehavior::Deferred',s);self.assertIn('let account=account_store::load(&tx,line)?',s);self.assertIn('RelationConfiguration::capture(&tx',s)
  s=source('relation_journal.rs');self.assertIn('RelationConfiguration(Zeroizing<[u8;32]>)',s);self.assertIn('length(private_config)=?2 AND revision=?3',s);self.assertIn('self.configuration.require_current',s);self.assertIn('self.action.validate(c)?;self.fixed(c)',s)
 def test_current_native_relation_identity_is_sealed_independent_and_precharged(self):
  s=source('relation_identity.rs');self.assertIn('if !self.native_receiver_scope()',s);self.assertIn('binding.authenticates(&self.uid,&self.selector)',s);self.assertLess(s.index('reserve_native_rpc(size)'),s.index('auth.user_id.clone()'))
  for key in ['Deserialize','ImSendIdentity','UserEntryAuth','pub token:','pub uid:']:self.assertNotIn(key,s)
 def test_actual_relation_request_and_detached_packet_hold_original_shared_budget(self):
  s=source('rpc_relation_update.rs');self.assertIn('size.checked_mul(3)',s);self.assertIn('key:b"id",value:b"#Group"',s);self.assertIn('permit.token_for(owner,scope)!=Some(token)',s);self.assertIn('request.budget.retain_reply',s)
  self.assertIn('packet.retain_relation_packet',source('arc_protocol.rs'));self.assertIn('Relation(std::sync::Arc<super::rpc_relation_update::RelationRpcRequest>)',source('transport_packets.rs'))
 def test_relation_replies_share_existing_reply_count_and_byte_caps(self):
  s=source('rpc_user_read_session.rs');self.assertIn('self.relation_replies.len()',s);self.assertIn('self.relation_replies.iter()',s)
  s=source('rpc_relation_update_session.rs');self.assertIn('self.im_reply_count()>=MAX_MAP_ENTRIES',s);self.assertIn('self.im_reply_bytes().checked_add',s)
 def test_actor_relation_write_uses_original_commands_parent_gates_and_fixed_deadline(self):
  s=source('transport_actor.rs');self.assertIn('RelationUpdate(Box<RelationWork>)',s);self.assertIn('Self::RelationUpdate(work)=>work.permit.retained_bytes()',s);self.assertIn('work.permit.begin_write(packets.call_id)',s);self.assertIn('im_gates.push(work.permit.parent_gate())',s);self.assertIn('work.permit.deadline()',s);self.assertIn('permit.cancelled().await',s);self.assertIn('self.retire_relation_updates(now)',s)
  s=source('relation_actor.rs');self.assertIn('RelationPermitWait::original',s);self.assertIn('p.permit.resolve(&reply)',s)
 def test_queued_not_sent_is_not_possible_or_submitted_unknown_write_refund(self):
  s=source('relation_journal.rs');self.assertIn('if old==QUEUED',s);self.assertIn('else{self.action.scope().close();}',s);self.assertIn('state=3',s);self.assertIn('phase.load(Ordering::SeqCst)!=SUBMITTED',s)
  s=source('relation_io_tests.rs');self.assertIn('prefix.accepted.load(Ordering::SeqCst),1',s);self.assertIn('original one-second action deadline',s);self.assertIn('same main fd Drop precedes IO Join',s)
 def test_one_relation_event_is_atomic_with_private_result_and_is_not_a_network_notification_schema(self):
  s=source('relation_journal.rs');self.assertIn('insert_app_event_for_conn(&tx,"app_voice.relations_updated"',s);self.assertIn('tx.commit().map_err(|_|STORAGE)?;self.phase.store(RESOLVED',s)
  for field in ['"uid":','"label":','"token":','"ready":','"nonce":']:self.assertNotIn(field,s)
 def test_http_borrowed_headers_are_precharged_and_json_is_dropped_before_await(self):
  s=(ROOT/'backend/src/api/app_voice_relation_updates.rs').read_text(encoding='utf-8');self.assertLess(s.index('manager.reserve_relation_http_scratch'),s.index('changes(&request.additions)'));self.assertIn('scratch.matches(&p)',s);self.assertLess(s.index('drop(request);drop(scratch)'),s.index('manager.finish_relation_update(p).await'));self.assertIn('deny_unknown_fields',s)
  s=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('relation-writes",get(relation_updates::relation_history).post(relation_updates::update_relations)',s)
 def test_new_original_protocol_io_account_and_http_cases_are_mounted_without_changing_old_filters(self):
  for file,path in [('im_protocol_tests.rs','relation_protocol_tests.rs'),('im_read_actor_tests.rs','relation_io_tests.rs'),('im_received_account_tests.rs','relation_account_tests.rs')]:self.assertIn(f'#[path="{path}"]',source(file))
  self.assertIn('include!("app_voice_relation_http_tests.rs")',(ROOT/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
 def test_new_io_and_account_cases_keep_unknown_original_scope_and_real_peer_joins(self):
  s=source('relation_io_tests.rs');self.assertIn('AuxiliaryIoTest::original_unarmed(1)',s);self.assertIn('tokio::join!(Box::pin(bounded(peer)),Box::pin(bounded(client)))',s);self.assertIn('scope.native_rpc_resources(),(0,0)',s)
  s=source('relation_account_tests.rs');self.assertIn('unknown_write_survives_stop_restart_and_blocks_new_nonce_other_group',s);self.assertIn('controller wait does not renew',s);self.assertIn('original_runtime_configuration',s)
 def test_history_ui_is_explicitly_read_only_without_post_nonce_or_current_sdk_permission(self):
  s=(ROOT/'frontend/src/pages/sim/AppRelationHistoryPane.tsx').read_text(encoding='utf-8');self.assertIn('api.getAppRelationHistory(lineId)',s);self.assertIn('current.current.generation !== original.generation',s);self.assertIn('此视图没有关系写入按钮',s)
  for text in ['crypto.','sendAppIm','updateRelations(',"method: 'POST'",'setAppAccountRunning']:self.assertNotIn(text,s)
  s=(ROOT/'frontend/src/policies/relationHistory.ts').read_text(encoding='utf-8');self.assertIn('不生成新 nonce',s);self.assertIn('!fields.includes(key)',s);self.assertIn('9223372036854775807n',s)
 def test_new_ui_tests_append_to_existing_actions_lists_and_keep_budgets_and_retries(self):
  s=(ROOT/'frontend/package.json').read_text(encoding='utf-8');self.assertIn('tests/connectStartupDetail.test.ts tests/relationHistory.test.ts',s)
  s=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn(".concat(['jegotrip-relation-history.spec.ts'])",s);self.assertIn('timeout: 45_000',s);self.assertIn('workers: 1',s);self.assertIn('retries: 0',s)
 def test_relation_actor_state_preserves_original_actor_initializer_shape_and_has_no_scope_cycle(self):
  s=source('transport_actor.rs');self.assertNotIn('relation_pending:',s);self.assertNotIn('relation_retirement:',s);self.assertIn('self.backend.relation_actor_state_mut().pending.insert',s)
  s=source('offline_backend.rs');self.assertIn('relation_actor:Box<super::transport_actor::RelationActorState>',s);self.assertIn('self.relation_actor.clear()',s)
  s=source('relation_actor.rs');self.assertIn('std::mem::take(&mut self.pending)',s);self.assertIn('p.permit.cancel()',s);self.assertNotIn('Arc<RelationActorState>',s)
 def test_runtime_relation_methods_remain_internal_to_jegotrip_not_public_http_or_unrelated_providers(self):
  s=source('runtime_relation_updates.rs')
  for name in ['relation_updates_enabled','relation_update_timeout','reserve_relation_http_scratch','reserve_relation_message']:
   self.assertIn('pub(in super::super) fn '+name,s)
  self.assertIn('pub(in super::super) async fn update_relations',s);self.assertNotIn('pub fn ',s);self.assertNotIn('pub async fn ',s)
 def test_actor_relation_exit_completes_waits_with_original_reason_before_backend_cleanup(self):
  s=source('relation_actor.rs');clear=s[s.index('fn clear(&mut self)'):s.index('fn finish(&mut self')]
  self.assertIn('p.permit.cancel()',clear);self.assertNotIn('reply.send',clear)
  self.assertIn('p.reply.send(Err(error))',s);self.assertIn('impl Drop for RelationActorState',s)
  s=source('transport_actor.rs');begin=s.index('let result = self.loop_run().await;');finish=s.index('self.backend.relation_actor_state_mut().finish(result);',begin);close=s.index('self.scope.close(); self.backend.close();',begin)
  self.assertLess(finish,close);self.assertIn('self.scope.session_revoked()',s[begin:finish])
 def test_before_await_intent_is_committed_in_synchronous_account_prepare_not_native_auth(self):
  s=source('account_relation_updates.rs');begin=s.index('pub(crate) fn prepare_relation_update');end=s.index('pub(crate) async fn finish_relation_update');block=s[begin:end]
  self.assertIn('RelationIntent::capture(',block);self.assertIn('RelationIntentWait::original',block);self.assertNotIn('.await',block)
  s=source('relation_intent.rs');self.assertIn('TransactionBehavior::Immediate',s);self.assertIn('INSERT INTO app_voice_relation_intents',s);self.assertIn('tx.commit()',s)
 def test_preparing_body_and_nonce_fence_cannot_be_bypassed_by_new_group_or_read(self):
  s=source('relation_intent.rs');self.assertIn('UNIQUE(line_id,user_nonce)',s);self.assertIn('body.as_slice()!=body_hash.as_slice()',s);self.assertIn('app_account_relation_preparing',s);self.assertIn('if pending{return Err',s);self.assertIn('if unknown{return Err(UNKNOWN);}',s)
  self.assertIn('if legacy{return Err("app_account_relation_legacy_intent");}',s)
 def test_retired_preparing_nonce_is_not_adopted_or_rearmed_after_owner_drop(self):
  s=source('relation_intent.rs');self.assertIn('(2,None)=>return Err("app_account_relation_nonce_retired")',s);self.assertIn('SET state=2',s);self.assertIn('state=0 AND write_id IS NULL',s);self.assertIn('impl Drop for RelationIntentWait',s)
 def test_intent_original_database_run_config_scope_message_and_parent_remain_bound(self):
  s=source('relation_intent.rs');self.assertIn('Arc::ptr_eq(database,&self.database)',s);self.assertIn('Arc::ptr_eq(run,&self.run)',s);self.assertIn('Arc::ptr_eq(configuration,&self.configuration)',s);self.assertIn('Arc::ptr_eq(parent,&self.parent)',s);self.assertIn('Arc::ptr_eq(&self.message,message)',s);self.assertIn('deadline==self.deadline',s)
 def test_actual_native_action_binding_and_preparation_binding_share_original_sql_transaction(self):
  s=source('relation_journal.rs');self.assertIn('intent.commit_binding(&tx,id)?',s);self.assertIn('intent.require_bound(c,row)?',s)
  s=source('relation_intent.rs');self.assertIn('self.fixed(c,0,None)?',s);self.assertIn('SET state=1,write_id=?2',s);self.assertIn('self.fixed(c,1,Some(write))',s)
 def test_preparation_cancellation_uses_weak_actual_action_not_an_arc_scope_budget_cycle(self):
  s=source('relation_intent.rs');self.assertIn('transition:Mutex<Weak<RelationAction>>',s);self.assertIn('*slot=Arc::downgrade(action)',s);self.assertIn('self.gate.retire();if let Some(action)=action{action.cancel();}',s);self.assertNotIn('self.scope.close()',s)
  s=source('relation_journal.rs');self.assertIn('std::mem::size_of::<RelationIntent>()',s);self.assertIn('message:Arc<RelationMessage>',s)
 def test_runtime_needs_preparing_ticket_before_current_sdk_identity_and_no_new_login(self):
  s=source('runtime_relation_updates.rs');self.assertLess(s.index('intent.belongs_to'),s.index('commands.original_im_binding'));self.assertIn('intent.bind(binding,identity,policy)',s);self.assertIn('_=intent_gate.retired()',s)
  for text in ['connect(', 'login(', 'tokio::spawn', 'retry(']:self.assertNotIn(text,s)
 def test_intent_history_is_separate_read_only_projection_without_current_auth_or_grant(self):
  s=source('relation_intent.rs');self.assertIn('state:match state{0=>"preparing",1=>"bound",2=>"not_sent"',s);self.assertIn('RelationIntentView(<redacted>)',s);self.assertIn('original-relation-intent-ui-nonce-v1',s)
  s=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('relation-intents",get(relation_updates::intent_history)',s);self.assertNotIn('relation-intents",get(relation_updates::intent_history).post',s)
 def test_new_preparation_tests_cover_durable_owner_cancel_restart_and_http_data_only(self):
  self.assertIn('#[path="relation_intent_protocol_tests.rs"]',source('relation_protocol_tests.rs'));self.assertIn('#[path="relation_intent_account_tests.rs"]',source('relation_account_tests.rs'))
  s=source('relation_intent_account_tests.rs');self.assertIn('unknown_preparation_is_not_adopted_by_explicit_stop_new_sdk_or_empty_write_history',s);self.assertIn('No await before these assertions',s)
  self.assertIn('include!("app_voice_relation_intent_http_tests.rs")',(ROOT/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
 def test_original_start_contract_is_exact_accepted_not_fabricated_hosted_http_ok(self):
  s=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('success(StatusCode::ACCEPTED, manager.start',s)
  s=(ROOT/'backend/src/api/app_voice_relation_http_tests.rs').read_text(encoding='utf-8');self.assertIn('assert_eq!(status,StatusCode::ACCEPTED);no_store',s);self.assertIn('phase!="hosted"',s)
 def test_new_write_editor_is_separate_and_needs_explicit_consent_nonce_and_original_installation(self):
  p=ROOT/'frontend/src/pages/sim/AppRelationWritePane.tsx';s=p.read_text(encoding='utf-8');self.assertIn('window.confirm(',s);self.assertIn('form.consent, form.writeConsent',s);self.assertIn('if (!installed) return null',s);self.assertIn('!ready || !hosted || !body',s)
  self.assertIn('AppRelationWritePane',(ROOT/'frontend/src/pages/sim/AppAccountsPanel.tsx').read_text(encoding='utf-8'))
 def test_new_write_ui_uses_account_web_lock_without_unguarded_fallback_or_auto_business_retry(self):
  s=(ROOT/'frontend/src/pages/sim/AppRelationWritePane.tsx').read_text(encoding='utf-8');self.assertIn("mode: 'exclusive', ifAvailable: true",s);self.assertIn('navigator.locks.request',s);self.assertIn('if (!lock || !active())',s)
  for forbidden in ['setInterval','setTimeout','setAppAccountRunning','sendAppIm','imSendNotAdmitted']:self.assertNotIn(forbidden,s)
 def test_ui_durable_opaque_marker_precedes_post_and_errors_do_not_clear_unknown(self):
  s=(ROOT/'frontend/src/policies/relationWriteFlow.ts').read_text(encoding='utf-8');self.assertLess(s.index('storeRelationMarker(ports.storage, line, marker)'),s.index('await ports.post(body)'));self.assertIn('relationMarkerResolved(marker, after)',s);self.assertIn('relationMarkerResolved(marker, snapshot)',s);self.assertIn("return { status: 'unresolved'",s)
  s=(ROOT/'frontend/src/policies/relationWriteStorage.ts').read_text(encoding='utf-8');self.assertIn('JSON.stringify(current) !== JSON.stringify(marker)',s);self.assertIn('loadRelationMarker(storage, line) !== null',s);self.assertNotIn('rawBody',s)
 def test_write_input_is_cloned_before_async_and_server_identity_is_not_a_ui_field(self):
  s=(ROOT/'frontend/src/policies/relationWriteFlow.ts').read_text(encoding='utf-8');self.assertLess(s.index('captureRelationWriteBody(input)'),s.index('await ports.read()'))
  s=(ROOT/'frontend/src/policies/relationWrite.ts').read_text(encoding='utf-8');self.assertIn('target === \'buddy\' && groupId !== \'\'',s);self.assertIn("const nativeTypes = [11, 12, 13, 14, 15, 16, 18, 257, 258, 259, 4097]",s);self.assertIn("false : own(entry, 'flag')",s)
 def test_nonce_ui_tags_match_original_length_prefixed_domains_and_unknown_requires_own_terminal(self):
  s=(ROOT/'frontend/src/policies/relationWrite.ts').read_text(encoding='utf-8');self.assertIn('setBigUint64(offset, BigInt(part.length), false)',s);self.assertIn('original-relation-intent-ui-nonce-v1',s);self.assertIn('original-relation-ui-nonce-v1',s);self.assertIn('row.nonceTag === marker.intentTag && row.id > marker.afterIntent',s);self.assertIn('row.nonceTag === marker.writeTag',s)
 def test_write_ui_generation_key_and_storage_changes_invalidate_stale_baselines(self):
  s=(ROOT/'frontend/src/pages/sim/AppRelationWritePane.tsx').read_text(encoding='utf-8');self.assertIn('key={JSON.stringify([props.lineId, props.revision, props.hosted, props.installed])}',s);self.assertIn("window.addEventListener('storage', changed)",s);self.assertIn('current.current.generation === original.generation',s);self.assertIn('if (write) setInput(empty)',s)
 def test_write_ui_api_uses_original_timeout_and_authenticated_finite_get_post_only(self):
  s=(ROOT/'frontend/src/api/current.ts').read_text(encoding='utf-8');start=s.index('  async getAppRelationIntents(');end=s.index('  async getAppRelationHistory(',start);block=s[start:end];self.assertIn('relation-intents?limit=128',block);self.assertIn("method: 'POST'",block);self.assertIn("cache: 'no-store', timeoutMs: 10000",block);self.assertNotIn('65000',block)
 def test_new_browser_and_unit_write_tests_are_appended_without_changing_existing_strict_lists(self):
  s=(ROOT/'frontend/package.json').read_text(encoding='utf-8');self.assertIn('tests/relationHistory.test.ts tests/relationWrite.test.ts tests/relationWriteFlow.test.ts',s)
  s=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn(".concat(['jegotrip-relation-write.spec.ts'])",s);self.assertIn('timeout: 45_000',s);self.assertIn('retries: 0',s)
 def test_relation_target_combobox_exposes_exact_stable_accessible_name_not_selected_option_text(self):
  s=(ROOT/'frontend/src/pages/sim/AppRelationWritePane.tsx').read_text(encoding='utf-8');self.assertIn("SelectDisplayProps: { 'aria-label': '关系修改目标', 'aria-labelledby': undefined }",s)
  s=(ROOT/'frontend/e2e/jegotrip-relation-write.spec.ts').read_text(encoding='utf-8');self.assertIn("getByRole('combobox', { name: '关系修改目标', exact: true })",s)
 def test_new_group_write_browser_fixture_is_valid_borrowed_json_not_weakened_button_assertion(self):
  import json,re
  s=(ROOT/'frontend/e2e/jegotrip-relation-write.spec.ts').read_text(encoding='utf-8');inputs=re.findall(r"\.fill\('([^']*)'\)",s)
  arrays=[value for value in inputs if value.startswith('[')];self.assertGreaterEqual(len(arrays),5)
  for value in arrays:self.assertIsInstance(json.loads(value),list)
  self.assertIn('await expect(send).toBeEnabled(); await send.click();',s);self.assertIn("group_id: '1+2'",s)
if __name__=='__main__':unittest.main()
