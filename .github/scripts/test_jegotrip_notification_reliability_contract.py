"""Source-only notification reliability gates; no SDK/device/compiled execution."""
from pathlib import Path
import re
import unittest
ROOT=Path(__file__).resolve().parents[2]
MODULE=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def source(name):return (MODULE/name).read_text(encoding='utf-8')
class NotificationReliabilityContracts(unittest.TestCase):
    def test_known_sdk_commands_get_negative_results_not_fabricated_events(self):
        text=source('rpc_client_ex.rs')
        for name in ['Notify.Command','Notify.MsgIdx','Notify.Box','Notify.CallId','Notify.Caller']:
            self.assertIn('b"'+name+'"',text)
        self.assertIn('properties.len() == keys.len()',text)
        self.assertIn('if !recognized_notification_command(&properties) { return Err(CodecError::InvalidField); }',text)
        self.assertIn('notify_accepted = false',text)
        self.assertIn('ClientExKind::NotifyCalls => notify_accepted',text)
        self.assertIn('let notice = if accepted',text)
    def test_opaque_business_fields_do_not_create_routes_or_payloads(self):
        text=source('rpc_client_ex.rs')
        helper=text[text.index('fn recognized_notification_command'):text.index('pub(super) enum ClientExControl')]
        for forbidden in ['parse::<','from_utf8','TcpStream','Sqlite','insert_sms','Command::new','SystemTime','Instant::']:
            self.assertNotIn(forbidden,helper)
        self.assertIn('ClientExKind::CheckAlive => alive.0',text)
        self.assertIn('map_or((false,0),|alive|alive.check(&self.owner,&self.scope,&self.token',text)
        self.assertIn('self.scope.mark_session_revoked()',text)
        self.assertIn('if !properties.is_empty()',text)
        self.assertIn('nonempty_notification_hook_map_is_rejected_not_swallowed_as_a_successful_hint',source('rpc_client_ex_tests.rs'))
    def test_worker_expiry_is_uncertainty_not_retryable_failure(self):
        text=source('account_notifications.rs')
        work=text[text.index('async fn send_one_before'):text.index('pub(crate) struct NotificationWorker')]
        self.assertIn('_=tokio::time::sleep_until(deadline)=>return',work)
        self.assertNotIn('sleep_until(deadline)=>NoticeOutcome::Failed',work)
        self.assertIn('struct DispatchGuard',text)
        self.assertIn('uncertain(&self.database,work)',text)
        self.assertIn("state='uncertain',claim=NULL,title='',body=''",text)
        self.assertIn("WHERE id=?1 AND claim=?2 AND state='sending'",text)
    def test_original_time_retry_and_queue_budgets_are_not_enlarged(self):
        text=source('account_notifications.rs')
        for constant in ['const ATTEMPT_TIMEOUT:Duration=Duration::from_secs(30);','const MAX_ATTEMPTS:i64=5;','const TTL:i64=24*60*60;','const WORKERS:usize=4;','const MAX_JOBS:i64=2048;']:
            self.assertIn(constant,text)
        self.assertIn('.min(ATTEMPT_TIMEOUT)',text)
        self.assertIn('deadline.min(tokio::time::Instant::now()+budget)',text)
        self.assertIn('NoticeOutcome::Failed if work.attempt<MAX_ATTEMPTS',text)
        self.assertIn('NoticeOutcome::Deferred(delay)',text)
    def test_synchronous_dispatch_construction_is_deferred_and_rechecked(self):
        text=source('account_notifications.rs')
        work=text[text.index('async fn send_one_before'):text.index('pub(crate) struct NotificationWorker')]
        self.assertIn('if tokio::time::Instant::now()>=deadline{return;}',work)
        self.assertIn('result=async {',work)
        self.assertIn('dispatch_shutdown.is_shutting_down()||tokio::time::Instant::now()>=deadline',work)
        self.assertIn('Some(dispatcher.deliver(work).await)',work)
        self.assertNotIn('result=dispatcher.deliver(work)',work)
    def test_new_suites_run_through_existing_parent_filters(self):
        for child,parent,count in [('client_ex_notification_tests.rs','rpc_client_ex_tests.rs',10),('account_notification_deadline_tests.rs','account_notifications_tests.rs',8),('runtime_notification_rejection_tests.rs','runtime_common_client_ex_tests.rs',2)]:
            body=source(child)
            self.assertEqual(len(re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+\w+',body)),count)
            self.assertIn(child,source(parent));self.assertNotIn('#[ignore]',body)
        for name in ['build-release.yml','beta-validation.yml']:
            body=(ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            for module in ['rpc_client_ex::tests','account_notifications::tests','runtime_backend::common_tests']:
                self.assertIn('connectivity::app_voice::providers::jegotrip::'+module,body)
            self.assertIn('Empty regression filter',body);self.assertIn('--test-threads=1',body)
if __name__=='__main__':unittest.main()
