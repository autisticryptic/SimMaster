"""Supplemental SOURCE fences for original Group.Update; never native acceptance."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name): return (P / name).read_text(encoding='utf-8')
class GroupEventContracts(unittest.TestCase):
    def test_original_kind_headers_and_full_type_zero_relation_map_are_separate(self):
        s = src('group_events.rs')
        for part in ['GROUP_UPDATE_KIND:&[u8]=b"Group.Update"', 'b"Group.GroupId"', 'b"Group.LastUpdateTime"', 'b"Group.UpdateTime"', 'Reader::new(body)?', 'flags:reader.u32()?', 'first:reader.bytes()?', 'second:reader.bytes()?', 'properties:reader.properties()?']:
            self.assertIn(part, s)
        for forbidden in ['Writer::versioned', 'decode_result(', 'System.OrgRelSync"=>', 'token_for(']: self.assertNotIn(forbidden, s)
    def test_uid_fallback_and_strict_time_parser_do_not_infer_missing_time_or_full_roster(self):
        s = src('group_events.rs')
        for part in ['unwrap_or(current_uid)', 'identifier==current_uid', 'GroupId::parse(identifier)', 'value.to_string()!=s', 'update_time<=base_time', 'CodecError::DuplicateKey']:
            self.assertIn(part, s)
        self.assertNotIn('unwrap_or(0)', s)
    def test_event_installation_is_explicit_before_original_login_with_native_clientex_im(self):
        s = src('arc_protocol.rs')
        self.assertIn('!self.rpc.can_arm_native_receivers()||self.relation_events_policy.is_some()', s)
        self.assertIn('!s.client_ex||s.policy.im_messages().is_none()', s)
        self.assertIn('RelationEventBinding::original(im,self.rpc.relation_identity()?,config)', s)
        self.assertIn('relation_events:None', src('runtime_backend.rs'))
        self.assertIn('with_relation_events(p.relation_events)', src('account_config.rs'))
    def test_header_scratch_and_body_decode_are_charged_before_allocating_event_vectors(self):
        s = src('group_event_delivery.rs')
        self.assertIn('let Some(_headers)=reserve_headers(binding,request.arguments)?', s)
        self.assertIn('let count=reader.count(8)?', s)
        self.assertIn('count.checked_mul(std::mem::size_of::<RpcProperty', s)
        self.assertLess(s.index('reserve_native_rpc(peak)'), s.index('group_events::decode_group_update('))
        self.assertLess(s.index('reserve_native_rpc(peak)'), s.index('value.identifier.to_vec()'))
        self.assertIn('body:Zeroizing<Vec<u8>>', s)
    def test_direct_and_batch_preserve_current_im_policy_and_only_event_token_checks(self):
        s = src('group_event_delivery.rs')
        for part in ['ReceiverTransport::Direct', 'ReceiverTransport::Batch', 'incoming.ticket().matches', 'binding.identity.token_for', 'reader.bytes()?!=group_events::GROUP_UPDATE_KIND{return Ok(None);}']:
            self.assertIn(part, s)
        self.assertIn('std::mem::size_of::<Self>()+64', s)
    def test_receipt_requires_exact_original_notice_not_equal_body_or_later_duplicate_deadline(self):
        s = src('group_event_inbox.rs')
        self.assertIn('Arc::ptr_eq(&self.notice,ticket.notice())', s)
        self.assertIn('now<ticket.deadline()&&self.configuration_current().is_ok()', s)
        self.assertIn('database:Arc<Database>', s)
        self.assertIn('configuration:Arc<RelationConfiguration>', s)
        self.assertIn('account_runs::runtime_matches(c,&self.run)', s)
        self.assertIn('Instant::now()>=delivery.ticket().deadline()', s)
    def test_sql_observation_head_and_redacted_event_share_commit_before_broadcast(self):
        s = src('group_event_inbox.rs')
        for part in ['TransactionBehavior::Immediate', 'INSERT INTO app_voice_relation_events', 'INSERT INTO app_voice_relation_event_heads', 'insert_app_event_for_conn(&tx,"app_voice.relation_event_observed"', 'validate(&tx,&run,delivery,&gate,configuration)?;tx.commit()']:
            self.assertIn(part, s)
        self.assertLess(s.index('tx.commit()'), s.index('db.publish_committed_app_event(event)'))
    def test_partial_chain_gap_is_not_roster_baseline_or_unknown_write_resolution(self):
        s = src('group_event_inbox.rs')
        for part in ['None=>("unseeded"', 'notice.update_time()<=current=>("stale",current,target,gap)', 'notice.base_time()==current=>("partial_chain"', '=>("gap",current,target.max(notice.update_time()),1)', '"full_roster_verified":false']:
            self.assertIn(part, s)
        for forbidden in ['UPDATE app_voice_relation_writes', 'DELETE FROM app_voice_relation_intents', 'UPDATE app_voice_im_receive_cursors', 'full_roster_verified:true']:
            self.assertNotIn(forbidden, s)
    def test_saved_projection_copies_only_bounded_text_and_exact_signed_times(self):
        s = src('group_event_inbox.rs')
        self.assertIn("CASE WHEN typeof(fingerprint)='blob' AND length(fingerprint)=32", s)
        self.assertIn("length(CAST(created_at AS BLOB)) BETWEEN 1 AND 64", s)
        self.assertIn('v.updates.checked_add(v.removals)', s)
        self.assertIn('r.get::<_,i64>(4)?.to_string()', s)
        self.assertIn('if local>=1024||total>=4096', s)
        self.assertIn('n>16*1024*1024', s)
    def test_account_task_and_original_shared_early_holders_are_not_new_workers(self):
        s = src('account_group_events.rs')
        for part in ['phase==Phase::HostingPending', 'phase!=Phase::Hosted', 'state.early.len()+state.early_status.len()+state.early_group_events.len()>=MAX_EARLY', 'state.holders>=MAX_HOLDERS||total>MAX_BYTES', 'group_event_inbox::commit', 'configuration.require_current', 'run.backend.complete_group_event']:
            self.assertIn(part, s)
        for forbidden in ['tokio::spawn', '.abort(', 'set_received(', 'update_relations(', 'read_im(']: self.assertNotIn(forbidden, s)
        self.assertIn('group_events::drive(run,state).await?', src('account_im.rs'))
        self.assertIn('BackendEventPayload::GroupEvent', src('account_runtime.rs'))
    def test_actual_reply_packet_retains_event_without_overwriting_mixed_client_holder(self):
        s = src('transport_packets.rs')
        self.assertIn('Option<PacketRetention>,Option<std::sync::Arc<super::group_event_packets::GroupEventPackets>>', s)
        self.assertIn('self.2=Some(guard)', s)
        self.assertIn('self.1=Some(guard)', s)
        self.assertIn('packet.retain_group_event_packet', src('arc_protocol.rs'))
        self.assertIn('packet.retain_group_event_packet', src('offline_backend.rs'))
        s = src('group_event_packets.rs')
        self.assertLess(s.index('reserve_native_rpc(bytes)'), s.index('Vec::with_capacity(count)'))
        self.assertIn('Writer::measured(6)', s); self.assertIn('Writer::measured(19)', s)
    def test_batch_proofs_share_retained_completion_caps_gates_and_exact_retirement(self):
        s = src('offline_backend.rs')
        self.assertIn('group_events.completed.values().map(|(_,proof)|proof.gate())', s)
        self.assertIn('group_events.completed.remove(&(retirement.peer_incarnation(),*sequence))', s)
        self.assertIn('im_pending.len()+status_pending.len()+group_events.pending.len()', s)
        self.assertIn('self.im_completed.len()+self.status_completed.len()+self.group_events.completed.len()', src('group_event_backend.rs'))
        self.assertIn('if self.group_events.pending.contains_key', s)
    def test_common_send_rechecks_exact_retained_packet_before_each_original_future_poll(self):
        s = src('common_runtime.rs')
        self.assertIn('packets.iter().any(|p|!p.group_event_packet_current(now))', s)
        self.assertLess(s.index('!p.group_event_packet_current(now)'), s.index('std::future::Future::poll(sending.as_mut(),cx)'))
        s = src('transport_actor.rs')
        self.assertIn('Operation::CompleteGroupEvent', s)
        self.assertIn('im_gates.push(proof.gate())', s)
        self.assertIn('group_events:Box<GroupEventState>', src('offline_backend.rs'))
        start = s.index('struct Actor<I> {')
        actor = s[start:s.index('\n}', start)]
        self.assertNotIn('group_events:', actor)
    def test_lower_common_record_channel_retries_and_physical_polls_keep_receipt_ownership(self):
        self.assertIn('send_call_retained', src('common_runtime.rs'))
        self.assertIn('queue_record_retained(owner,scope,record.expose_to_transport(),record_end,now,record.group_event_retention())', src('netstream_runtime.rs'))
        self.assertIn('packet.group_event_retention()', src('channel_session.rs'))
        s = src('reliable_channel.rs')
        self.assertIn('piece.retain_group_event_packet(event.clone())', s)
        self.assertIn('queued.body.group_event_retention()', s)
        self.assertIn('item.body.as_ref().and_then(TransportBytes::group_event_retention)', s)
        self.assertIn('!b.group_event_packet_current(now)', s)
        self.assertIn('!queued.body.group_event_packet_current(now)', s)
        check=s[s.index('fn check('):s.index('pub fn tick(')]
        self.assertNotIn('group_event_packet_current',check)
        s = src('router_runtime.rs')
        self.assertIn('poll_original_write(poll_before(&scope,io.write(&bytes[offset..]),end),event)', s)
        self.assertIn('poll_original_write(poll_before(&scope,io.flush(),end),event)', s)
        s = src('group_event_packets.rs')
        self.assertLess(s.index('event.is_some_and(|e|!e.current'), s.index('Future::poll(original.as_mut(),cx)'))
    def test_event_route_is_auth_scoped_get_only_no_write_clear_or_forged_receipt(self):
        s = (ROOT / 'backend/src/api/app_voice.rs').read_text(encoding='utf-8')
        self.assertIn('relation-events",get(relation_updates::event_history)', s)
        self.assertNotIn('relation-events",get(relation_updates::event_history).post', s)
        self.assertIn('manager.relation_event_history(&line,query.limit)', (ROOT / 'backend/src/api/app_voice_relation_updates.rs').read_text(encoding='utf-8'))
    def test_inbound_history_ui_is_separate_click_only_read_no_unknown_write_reconciliation(self):
        s=(ROOT/'frontend/src/pages/sim/AppRelationEventsPane.tsx').read_text(encoding='utf-8')
        for part in ['api.getAppRelationEvents(lineId)', 'key={JSON.stringify([lineId, revision])}', 'current.current.generation === generation', 'if (active()) setRows(result)', 'RELATION_EVENTS_UNKNOWN']:
            self.assertIn(part,s)
        for part in ['updateAppRelations', 'relationMarkerResolved', 'setInterval', 'setAppAccountRunning', 'crypto.', 'localStorage']:
            self.assertNotIn(part,s)
        s=(ROOT/'frontend/src/api/current.ts').read_text(encoding='utf-8');start=s.index('  async getAppRelationEvents(');end=s.index('  async getAppImMessages(',start);block=s[start:end]
        self.assertIn('relation-events?limit=128',block);self.assertIn("method: 'GET', cache: 'no-store', timeoutMs: 10000",block);self.assertIn('skipAuthRedirect: true',block)
    def test_inbound_ui_strict_nine_field_projection_rejects_full_roster_or_rounded_times(self):
        s=(ROOT/'frontend/src/policies/relationEvents.ts').read_text(encoding='utf-8')
        for part in ['Reflect.ownKeys(raw)', 'Object.prototype.hasOwnProperty.call', 'updates + removals > 128', 'BigInt(updateTime) <= BigInt(baseTime)', "own('full_roster_verified') !== false", '9223372036854775807n', 'value.length > 128']:
            self.assertIn(part,s)
        s=(ROOT/'frontend/tests/relationEvents.test.ts').read_text(encoding='utf-8');self.assertIn('Object.keys(row).length, 9',s);self.assertIn("full_roster_verified of [true, 'false', 0, null]",s)
        s=(ROOT/'frontend/package.json').read_text(encoding='utf-8');self.assertIn('tests/relationWriteFlow.test.ts tests/relationEvents.test.ts',s)
        s=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8');self.assertIn(".concat(['jegotrip-relation-events.spec.ts'])",s);self.assertIn('timeout: 45_000',s);self.assertIn('retries: 0',s)
    def test_new_cases_mount_into_existing_actions_and_preserve_old_filters(self):
        for path, child in [('group_events.rs','group_events_tests.rs'),('im_protocol_tests.rs','group_event_protocol_tests.rs'),('group_event_protocol_tests.rs','group_event_lifetime_tests.rs'),('im_account_tests.rs','group_event_account_tests.rs'),('im_read_actor_tests.rs','group_event_io_tests.rs')]:
            self.assertIn(f'#[path="{child}"]', src(path))
        self.assertIn('include!("app_voice_relation_event_http_tests.rs")', (ROOT / 'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
        for path in ['build-release.yml','beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::group_events::tests \\\n', (ROOT / '.github/workflows' / path).read_text(encoding='utf-8'))
    def test_new_tests_cover_real_partial_io_sql_rollback_epoch_and_unknown_write_preservation(self):
        self.assertIn('prefix.accepted.load(Ordering::SeqCst),1', src('group_event_io_tests.rs'))
        self.assertIn('same main fd Drop before Join', src('group_event_io_tests.rs'))
        self.assertIn('scope.native_rpc_resources(),(0,0)', src('group_event_io_tests.rs'))
        self.assertIn('assert_eq!(states,(0,0))', src('group_event_lifetime_tests.rs'))
        self.assertIn('assert_eq!(heads,0)', src('group_event_account_tests.rs'))
        self.assertIn('no unconfirmed group reply', src('group_event_account_tests.rs'))
if __name__ == '__main__': unittest.main()
