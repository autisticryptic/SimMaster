"""Supplemental source guards only; Rust/peer/UI execution remains Actions-only."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2]
P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name): return (P/name).read_text(encoding='utf-8')
class InviteNotificationContracts(unittest.TestCase):
    def test_explicit_policy_default_and_original_private_bytes(self):
        policy=src('native_receiver.rs');self.assertIn('#[serde(default)]\n    pub invite_notifications: Option<',policy)
        self.assertIn('if policy.invite_notifications().is_some() { return Err(CodecError::InvalidField); }',policy)
        self.assertIn('policy.invite_notifications().is_some() && !client_ex',policy)
        config=src('account_config.rs');self.assertIn('&& (!p.client_ex || !p.native_call_control || self.call_media.is_none())',config)
        self.assertIn('#[serde(skip_serializing_if = "Option::is_none")]\n    invite_notifications:',config)
        self.assertIn('invite_notifications: config.invite_notifications.as_ref()',config)
    def test_policy_is_a_bounded_local_timeout_not_a_guessed_sdk_interval(self):
        data=src('native_invite_notifications.rs');self.assertIn('!(100..=30_000).contains(&self.timeout_ms)',data)
        self.assertIn('now.checked_add(config.timeout()?)',data);self.assertIn('notification_id == 0',data)
        self.assertIn('MAX_INVITE_VALUE_BYTES: usize = 4 * 1024',data)
        self.assertNotIn('interval_seconds',data);self.assertNotIn('AccountAlive:stcp',data)
    def test_only_exact_current_token_native_invites_leave_old_message_nack_path(self):
        codec=src('rpc_client_ex.rs');self.assertIn('notify_accepted = false;',codec)
        self.assertIn('p.key == b"Notify.Command" && p.value == b"Invite"',codec)
        self.assertIn('property(b"token") == Some(self.token.as_slice())',codec)
        self.assertIn('native.policy().invite_notifications()',codec)
        self.assertNotIn('Callback::Channels',codec);self.assertNotIn('NativeEvent::IncomingMessage',codec)
    def test_direct_holds_magic_and_original_reply_and_never_invents_network_notifyend(self):
        protocol=src('arc_protocol.rs');part=protocol.split('if let Some(data) = action.invite {',1)[1].split('let reply = encode_reply(request.call_id',1)[0]
        for value in ['DirectReplyTicket','DirectResponse::Invite','self.direct_pending.insert','ProtocolEvent::InviteNotification','return Ok(true)']:self.assertIn(value,part)
        self.assertNotIn('record_direct(',part);self.assertNotIn('encode_reply(',part)
        for name in ['native_invite_notifications.rs','account_invite_notifications.rs','native_invite_receipts.rs']:
            for value in ['notifyEnd.ClientEx.Client','notifyEnd.CallServerEx','native library','insert_sms','enable_business']:self.assertNotIn(value,src(name))
    def test_batch_holds_original_sequence_and_cannot_complete_as_sms(self):
        backend=src('offline_backend.rs');self.assertIn('IncomingInviteNotification::from_batch(data, incoming.into_ticket())',backend)
        self.assertIn('invite_pending.insert(key, (delivery.notification_id(), delivery.fingerprint(), delivery.deadline()))',backend)
        self.assertIn('self.invite_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence()))',backend)
        self.assertIn('self.invite_pending.get(&key) != Some(&(ticket.notification_id(), *ticket.fingerprint(), ticket.deadline()))',backend)
        self.assertIn('if accepted { self.invite_completed.insert(key, ticket.captured_copy()); }',backend)
    def test_only_database_receipt_can_confirm_production_consumption_claim(self):
        data=src('native_invite_notifications.rs');self.assertIn('consumed: AtomicBool::new(false)',data)
        self.assertIn('receipt: &super::native_invite_receipts::InviteConsumptionReceipt',data)
        self.assertIn('receipt.matches(self.notification_id,&self.fingerprint)',data)
        self.assertIn('self.matches(&claim.owner,&claim.scope) && claim.may_ack',data)
        fixture=data.split('pub(super) fn fixture_consumed',1)[0];self.assertTrue(fixture.rstrip().endswith('#[cfg(test)]'))
        self.assertIn('ticket.claim.as_ref().is_none_or(|original| Arc::ptr_eq(original,claim))',data)
        self.assertNotIn('Deserialize',data.split('pub struct InviteReplyTicket',1)[1])
    def test_durable_association_checks_original_run_revision_and_history_token(self):
        store=src('native_invite_receipts.rs')
        for value in ['TransactionBehavior::Immediate','account_runs::require_live','account_runs::runtime_matches','row.revision != run.revision()',"direction='incoming' AND end_time IS NULL",'app_call_token=?3','notification_id.to_be_bytes()','prior.as_slice() != fingerprint','MAX_RECEIPTS: i64 = 2048','FOREIGN KEY(call_history_id) REFERENCES call_history(id) ON DELETE CASCADE']:self.assertIn(value,store)
        for value in ['password TEXT','sip_password','caller TEXT','root_der','DROP TABLE','UPDATE call_history']:self.assertNotIn(value,store)
        self.assertIn('super::native_invite_receipts::initialize_schema(conn)',src('account_call_history.rs'))
    def test_actual_call_inform_and_local_indication_precede_durable_claim(self):
        consumer=src('account_invite_notifications.rs');self.assertIn('call.ticket.native_id().expose_to_backend() == group.notification_id',consumer)
        self.assertIn('if phase == Phase::Checking { return Ok(None); }',consumer)
        self.assertIn('Phase::Ringing|Phase::Accepting|Phase::Connected',consumer)
        self.assertLess(consumer.index('native_invite_receipts::consume'),consumer.index('group.claim.confirm(&receipt)'))
        self.assertLess(consumer.index('group.claim.confirm(&receipt)'),consumer.index('call.invite_consumed.store(true'))
        calls=src('account_calls.rs');part=calls.split('async fn ring(',1)[1].split('pub(super) async fn accept',1)[0]
        self.assertLess(part.index('result.accepted()'),part.index('state.phase=Phase::Ringing'))
        self.assertLess(part.index('OperatorEvent::Incoming'),part.index('lock(&self.invites).wake()'))
    def test_queue_budgets_count_inflight_and_do_not_evacuate_unconsumed_groups(self):
        data=src('native_invite_notifications.rs');self.assertIn('MAX_INVITE_GROUPS: usize = 64',data);self.assertIn('MAX_INVITE_HOLDERS: usize = 256',data);self.assertIn('MAX_INVITE_RETAINED_BYTES: usize = 1024 * 1024',data)
        queue=src('account_invite_notifications.rs');self.assertIn('struct RetainedInvite',queue);self.assertIn('impl Drop for RetainedInvite',queue)
        self.assertIn('budget.holders.checked_sub(1)',queue);self.assertIn('budget.bytes.checked_sub(self.bytes)',queue)
        pop=queue.split('fn pop(',1)[1].split('impl Drop for InviteQueue',1)[0];self.assertNotIn('checked_sub',pop)
        self.assertNotIn('groups.remove(',queue);self.assertNotIn('groups.clear(',queue)
    def test_fixed_first_deadline_and_cancelled_claim_survive_io_and_epoch_replay(self):
        data=src('native_invite_notifications.rs');self.assertIn('ticket.completion_deadline.min(deadline).min(claim.deadline)',data)
        consumer=src('account_invite_notifications.rs');self.assertIn('incoming.deadline() < group.deadline',consumer)
        self.assertIn('incoming.ticket().clipped_to(group.deadline,&group.claim)',consumer)
        actor=src('transport_actor.rs');self.assertIn('ticket.positive_is_current()',actor);self.assertIn('original.min(invite)',actor)
        self.assertIn('waiting.push(claim.cancelled())',actor)
        backend=src('offline_backend.rs');self.assertIn('self.invite_completed.values().any(|ticket| !ticket.positive_is_current())',backend)
        self.assertIn('for sequence in retirement.acknowledged()',backend)
        self.assertNotIn('invite_completed.remove',backend.split('pub fn prepare_swap',1)[1].split('pub fn retire_request',1)[0])
    def test_retire_then_owned_join_and_weak_worker_no_detached_consumer(self):
        queue=src('account_invite_notifications.rs');self.assertIn('self.task_admission().await',queue);self.assertIn('tasks.push(tokio::spawn(run_group(Arc::downgrade(self),group)))',queue)
        self.assertIn('drop(service); // no strong CallService -> Join -> CallService cycle',queue)
        calls=src('account_calls.rs');self.assertIn('lock(&self.invites).retire();',calls)
        close=calls.split('pub(super) async fn close',1)[1].split('pub(super) fn resources_released',1)[0]
        self.assertLess(close.index('self.retire()'),close.index('let _=task.await'))
        for value in ['.abort()','JoinSet::detach_all','RUST_MIN_STACK','stack_size(','std::mem::forget']:self.assertNotIn(value,queue)
    def test_old_control_and_media_grants_are_not_promoted_by_consumed_label(self):
        codec=src('rpc_client_ex.rs');self.assertIn('ClientExKind::CheckAlive => alive.0',codec)
        self.assertIn('map_or((false,0),|alive|alive.check',codec)
        calls=src('account_calls.rs');self.assertIn('sdk_invite_notification:self.invite_consumed.load(Ordering::SeqCst).then_some("consumed")',calls)
        for name in ['native_invite_notifications.rs','native_invite_receipts.rs','account_invite_notifications.rs']:
            text=src(name);self.assertNotIn('Phase::Hosted',text);self.assertNotIn('state.phase=Phase::Connected',text);self.assertNotIn('CallOperation::Accept',text)
    def test_invite_ui_uses_existing_auth_calls_read_only_with_no_answer_or_token_render(self):
        api=(R/'frontend/src/api/current.ts').read_text(encoding='utf-8')
        self.assertIn('async getAppCalls(lineId: string)',api)
        self.assertIn("{ cache: 'no-store', timeoutMs: 10000 }",api)
        pane=(R/'frontend/src/pages/sim/AppInviteNotificationsPane.tsx').read_text(encoding='utf-8')
        self.assertIn('if (!enabled) return',pane);self.assertIn('loaded?.line === lineId',pane)
        self.assertIn('active = false; if (timer) clearTimeout(timer)',pane)
        self.assertIn('Message IM 消费器尚未装配',pane)
        for forbidden in ['setAppAccountRunning','answer_call','submitJegotripLogin','localStorage','sessionStorage','peer:', 'sdp:']:
            self.assertNotIn(forbidden,pane)
    def test_sdk_invite_metadata_parser_is_bounded_own_properties_and_not_a_readiness_label(self):
        policy=(R/'frontend/src/policies/sdkInviteNotifications.ts').read_text(encoding='utf-8')
        self.assertIn('value.slice(0, 128)',policy);self.assertIn('rows.length === 16',policy)
        self.assertIn('Number.isSafeInteger(id)',policy);self.assertIn("Object.prototype.hasOwnProperty.call(entry, 'sdk_invite_notification')",policy)
        self.assertIn('不代表渠道送达、来电已接听或媒体就绪',policy)
        self.assertIn('不推断设备授信或接收成功',policy)
        selector=(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8')
        self.assertIn("'jegotrip-invite-notifications.spec.ts'",selector)
        for budget in ['timeout: 45_000','timeout: 10_000','workers: 1','retries: 0','timeout: 120_000']:
            self.assertIn(budget,selector)
    def test_new_tests_stay_under_original_actions_filters(self):
        self.assertIn('native_invite_protocol_tests.rs',src('rpc_client_ex_tests.rs'))
        self.assertIn('account_invite_queue_filter_tests.rs',src('account_calls_tests.rs'))
        self.assertIn('native_invite_account_tests.rs',src('native_receiver_account_tests.rs'))
        for workflow in ['build-release.yml','beta-validation.yml']:
            text=(R/'.github/workflows'/workflow).read_text(encoding='utf-8')
            for selected in ['rpc_client_ex::tests','account_calls::tests','runtime_backend::common_tests']:self.assertIn('connectivity::app_voice::providers::jegotrip::'+selected,text)
            self.assertIn('--test-threads=1',text);self.assertIn('Empty regression filter',text)
        for name in ['native_invite_protocol_tests.rs','account_invite_queue_filter_tests.rs','native_invite_account_tests.rs']:
            for value in ['#[ignore]','dlopen','Command::new("adb")','time::sleep(Duration::from_secs(60))']:self.assertNotIn(value,src(name))
if __name__=='__main__':unittest.main()
