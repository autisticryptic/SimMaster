"""Supplemental original-call notification fences; execution is Actions-only."""
from pathlib import Path
import unittest,json,re
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def read(n):return (P/n).read_text(encoding='utf-8')
class CallNotificationContracts(unittest.TestCase):
    def test_feature_is_explicit_and_old_private_format_omits_false(self):
        cfg=read('account_config.rs');self.assertIn('#[serde(default)]\n    pub call_notifications:bool',cfg);self.assertIn('#[serde(skip_serializing_if="std::ops::Not::not")]\n    call_notifications:bool',cfg)
        self.assertIn('call_notifications:media.call_notifications',cfg);self.assertIn('call_notifications:m.call_notifications',cfg)
    def test_jobs_have_real_call_tokens_not_fake_sms_or_modem_identity(self):
        text=read('account_call_notifications.rs');self.assertIn('CREATE TABLE IF NOT EXISTS app_voice_call_notification_jobs',text)
        self.assertIn('call_history_id INTEGER NOT NULL',text);self.assertIn('app_call_token=?3',text);self.assertIn('run.nonce()',text)
        self.assertNotIn('INSERT INTO sms_messages',text);self.assertNotIn('modem',text.split('use super::',1)[1])
    def test_original_scope_and_record_gate_admission_before_dispatch(self):
        text=read('account_call_notifications.rs');self.assertIn('account_runs::runtime_matches',text);self.assertIn('account_runs::require_live',text);self.assertIn('m.revision!=run.revision()',text)
        worker=read('account_call_notification_runtime.rs');self.assertIn('call.ticket.with_media_gate(||queue::enqueue',worker);self.assertIn('queue::admitted',worker)
    def test_ringing_after_inform_is_required_and_sender_does_not_grant_media(self):
        calls=read('account_calls.rs');self.assertLess(calls.index('if !result.accepted()||!self.active(call)'),calls.index('self.start_call_notifications(call)'))
        worker=read('account_call_notification_runtime.rs');self.assertIn('state.phase==Phase::Ringing&&!state.accept_queued&&!state.end_queued',worker)
        self.assertNotIn('Phase::Connected',worker);self.assertNotIn('history::connected',worker)
    def test_old_sms_worker_cap_and_destination_serialization_are_shared_not_expanded(self):
        text=read('account_notifications.rs');self.assertIn('const WORKERS:usize=4',text);self.assertIn('active+super::account_call_notifications::sending_count',text);self.assertIn('channel_sending(&tx,&channel_id)',text)
        call=read('account_call_notifications.rs');self.assertIn('const WORKERS:usize=4',call);self.assertIn('sms_sending(&tx,None)',call);self.assertIn('sms_sending(&tx,Some(&channel_id))',call)
    def test_deadlines_unknown_claims_and_real_joins_are_owned(self):
        worker=read('account_call_notification_runtime.rs');self.assertIn('deadline.min',worker) if 'deadline.min' in worker else self.assertIn('queue::WINDOW.min(remaining)',worker)
        self.assertIn('queue::uncertain(&self.db,work)',worker);self.assertIn('while jobs.join_next().await.is_some()',worker);self.assertIn('let outcome=task.await;slot.take()',worker)
        self.assertNotIn('.abort()',worker);self.assertIn('call.ticket.revoked()',worker)
    def test_answer_end_and_recovery_do_not_replay_or_change_old_history(self):
        text=read('account_call_notifications.rs');self.assertIn("state='sending' THEN 'uncertain' ELSE 'cancelled'",text);self.assertIn('recover(conn:&Connection)',text)
        self.assertNotIn('UPDATE call_history',text);calls=read('account_calls.rs');self.assertIn('self.join_call_notifications(call).await?',calls)
    def test_only_captured_call_rules_and_channels_are_rendered_and_fingerprinted(self):
        text=(ROOT/'backend/src/services/notify/app_call_notifications.rs').read_text();self.assertIn('NotificationEventType::Call',text);self.assertIn('pair_fingerprint(rule,channel)',text);self.assertIn('render_call_bounded',text)
        self.assertIn('text.len()>MAX_BODY_BYTES-out.len()',text);self.assertIn('CallNoticeOutcome::Uncertain',text);self.assertNotIn('send_call_notification(',text)
    def test_api_remains_authenticated_metadata_only(self):
        api=(ROOT/'backend/src/api/app_voice.rs').read_text();self.assertIn('/call-notifications", get(get_call_notifications)',api);self.assertIn('manager.call_notification_status',api)
        text=read('account_call_notifications.rs').split('pub struct CallNoticeStatus{',1)[1].split('}',1)[0]
        for secret in ['token','claim','channel','phone','body','title']:self.assertNotIn(secret,text)
    def test_frontend_is_read_only_and_keeps_owning_account_generation(self):
        text=(ROOT/'frontend/src/pages/sim/AppCallNotificationsPane.tsx').read_text();self.assertIn('api.getAppCallNotifications(lineId)',text);self.assertIn('active = false',text);self.assertNotIn('submit',text)
        policy=(ROOT/'frontend/src/policies/callNotifications.ts').read_text();self.assertIn('Object.prototype.hasOwnProperty.call',policy);self.assertIn('不代表来电已接听或媒体就绪',policy)
    def test_old_assertions_and_new_cases_are_mounted_in_existing_action_filters(self):
        self.assertIn('mod notifications;',read('account_calls_tests.rs'));self.assertIn('mod calls;',read('account_notifications_tests.rs'))
        for n in ['account_call_notification_queue_tests.rs','account_call_notifications_tests.rs']:
            text=read(n);self.assertNotIn('#[ignore',text);self.assertNotIn('RUST_MIN_STACK',text)
        package=json.loads((ROOT/'frontend/package.json').read_text());self.assertIn('tests/callNotifications.test.ts',package['scripts']['test:unit'])
        browser=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text();self.assertIn('jegotrip-call-notifications.spec.ts',browser)
        for value in ['timeout: 45_000','timeout: 10_000','workers: 1','retries: 0','timeout: 120_000']:self.assertIn(value,browser)
if __name__=='__main__':unittest.main()
