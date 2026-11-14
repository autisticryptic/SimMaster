"""Source fences for separate local SDK preparation; behavioral proof is Actions-only."""
from pathlib import Path
import re
import unittest
ROOT=Path(__file__).resolve().parents[2]
P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def source(path):return (ROOT/path).read_text(encoding='utf-8')
def login(name):return (P/'app_login'/name).read_text(encoding='utf-8')

class LocalReceiverContracts(unittest.TestCase):
    def test_local_operation_is_explicit_and_keeps_original_identity_and_legacy_defaults(self):
        text=login('mod.rs')
        self.assertIn('#[default] EnableHosting',text)
        self.assertIn('Self::PrepareLocalReceiver=>"prepare_local_receiver"',text)
        self.assertIn('fn requires_original_device(self)->bool {self.reuses_trusted_device() || matches!(self,Self::PrepareLocalReceiver|Self::Communication)}',text)
        self.assertIn('let snapshot=if intent.requires_original_device()',text)
        self.assertIn('self.confirm_device_identity(guard,&key,intent.requires_original_device())',text)
        self.assertIn('if (intent.reuses_trusted_device() || matches!(intent,LoginIntent::PrepareLocalReceiver|LoginIntent::Communication))',text)
        self.assertIn('if !request.consent',text)
        self.assertIn('return self.run_local_receiver(owner,request,configuration,client,guard,&bound).await;',text)

    def test_current_device_checks_gate_phone_material_and_local_creation_without_forwarding_conversion(self):
        text=login('local_receiver.rs')
        self.assertLess(text.index('require_current_device(initial)?'),text.index('client.receiver_phone_credentials'))
        self.assertLess(text.index('config.validate('),text.index('let latest=client.receiver_device_recognition()'))
        self.assertLess(text.index('require_current_device(latest)?'),text.index('accounts.create'))
        self.assertIn('Some(TrustDecision {authentication_code:Some(1)})=>Ok(())',text)
        self.assertIn('let status=client.call_status().await;',text)
        self.assertIn('let _=status?;',text)
        for forbidden in ['client.enable_business','client.sync_device','client.enroll_device','with_trusted_reuse()',
                          'require_current_trust','if observation.','if status.authentication_code','if status.hosting_enabled']:
            self.assertNotIn(forbidden,text)
        self.assertIn('self.step(owner,id,"creating_account",None)?',text)

    def test_local_http_policy_cannot_enroll_enable_or_report_and_requires_actual_phone_material(self):
        text=login('http.rs')
        self.assertIn('if self.config.endpoints.verification_trustful_device.is_none()',text)
        self.assertIn('let mut client=self.with_trusted_reuse()?;client.local_receiver=true;',text)
        self.assertIn('let encrypted=crypto::secret_field(body,"sip_password")?;',text)
        material=text.split('async fn fetch_phone_material(',1)[1].split('pub async fn verify_trusted_device(',1)[0]
        self.assertNotIn('body.get("status")',material)
        for start,end in [('pub(super) async fn sync_device(', 'pub async fn enable_business('),
                          ('pub async fn enable_business(', '\n}')]:
            part=text.split(start,1)[1].split(end,1)[0]
            self.assertIn('if self.local_receiver {return Err(Error::new("app_login_request_invalid"));}',part)
        old=text.split('pub async fn verify_trusted_device(',1)[1].split('pub(super) async fn receiver_device_recognition(',1)[0]
        self.assertIn('if self.trusted_reuse { return Err(Error::new("app_login_request_invalid")); }',old)
        body=text.split('async fn query_existing_device(',1)[1].split('pub async fn enroll_device(',1)[0]
        self.assertIn('"sso_token":""',body);self.assertIn('"equipment_type":"1"',body)
        self.assertIn('Authentication::Session,false',body)

    def test_observations_remain_distinguished_bounded_data_with_no_ready_or_secret_fields(self):
        text=login('device_trust.rs')
        self.assertIn('pub enum DeviceTrustPurpose { LocalReceiver }',text)
        self.assertIn('pub purpose:Option<DeviceTrustPurpose>',text)
        self.assertIn('purpose:None',text)
        self.assertIn('MAX_OBSERVATION_BYTES: usize = 2048',text)
        self.assertIn('self.device_sync.is_none() && status.valid()',text)
        self.assertIn('self.existing_device.state!=TrustQueryState::NotRequested',text)
        self.assertIn('self.business_query_error==status.error && self.business_query_server_code==status.server_code',text)
        for forbidden in ['pub ready:', 'pub token:', 'pub device_id:', 'pub trusted:']:
            self.assertNotIn(forbidden,text)
        self.assertIn('length(CAST(device_trust AS BLOB)) BETWEEN 1 AND 2048',login('store.rs'))

    def test_old_remote_business_flow_and_native_ack_remain_separate(self):
        old=login('trusted_reuse.rs')
        self.assertIn('if status.authentication_code!=Some(1)',old)
        self.assertIn('app_login_business_not_enabled',old)
        self.assertNotIn('PrepareLocalReceiver',old)
        self.assertNotIn('receiver_device_recognition',old)
        controller=(P/'controller.rs').read_text(encoding='utf-8')
        self.assertIn('backend.update_status(owner.clone(), STATUS_REGISTER, request)',controller)
        self.assertIn('self.session.on_hosting_ack(owner, success)',controller)
        self.assertIn('self.session.owns(owner)',controller)

    def test_frontend_explicit_mode_and_distinct_labels_do_not_claim_ready_or_write_remote_business(self):
        policy=source('frontend/src/policies/appLogin.ts');pane=source('frontend/src/pages/sim/AppAccountLoginPane.tsx')
        self.assertIn("(!localReceiver || deviceMode === 'phone_app')",policy)
        self.assertIn('不是本地SDK注册结果',policy)
        self.assertIn('本地接收账号已准备；SDK与收件状态请查看独立账号',policy)
        self.assertIn('prepare_local_receiver',pane)
        self.assertIn('不改呼转、不报告设备',pane)
        self.assertIn('consent: false',pane)
        self.assertIn('setup?.accepting_local_receiver',pane)
        self.assertIn('localReceiverTaskLabel(attempt)',pane)
        for forbidden in ['localStorage','sessionStorage','console.log']:
            self.assertNotIn(forbidden,pane)

    def test_existing_and_new_ui_suites_run_in_the_original_actions_without_budget_changes(self):
        config=source('frontend/playwright.jegotrip.config.ts')
        self.assertIn("testMatch: ['jegotrip-device-identity.spec.ts', 'jegotrip-local-receiver.spec.ts', 'jegotrip-startup-diagnostics.spec.ts', 'jegotrip-alive-status.spec.ts', 'jegotrip-communication.spec.ts']",config)
        self.assertIn('timeout: 45_000',config);self.assertIn('workers: 1',config);self.assertIn('retries: 0',config)
        self.assertEqual(source('frontend/e2e/jegotrip-device-identity.spec.ts').count("test('"),4)
        e2e=source('frontend/e2e/jegotrip-local-receiver.spec.ts')
        self.assertEqual(e2e.count("test('"),2)
        self.assertIn('expect(model.logins).toHaveLength(1)',e2e)
        self.assertIn('expect(model.writes).toEqual([])',e2e)
        self.assertIn('toHaveValue(\'\')',e2e)
        for name in ['build-release.yml','beta-validation.yml']:
            text=source('.github/workflows/'+name)
            self.assertIn('connectivity::app_voice::providers::jegotrip::app_login::tests \\\n',text)
            self.assertIn('http_router_tests::app_voice_accounts',text)
            self.assertIn('playwright test --config playwright.jegotrip.config.ts',text)

    def test_new_rust_http_and_ui_tests_cover_positive_and_failure_boundaries(self):
        tests=login('local_receiver_tests.rs')
        names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',tests)
        self.assertEqual(len(names),28);self.assertEqual(len(names),len(set(names)))
        self.assertNotIn('#[ignore',tests)
        for name in ['local_receiver_native_login_registration_sms_ack_and_stop_join_do_not_enable_forwarding',
                     'old_positive_diagnostic_cannot_authorize_a_new_local_receiver_attempt',
                     'local_receiver_rechecks_device_after_phone_material_and_stops_on_a_current_change',
                     'local_receiver_final_observation_storage_failure_stops_before_account_creation']:
            self.assertIn(name,names)
        self.assertIn('fn local_receiver_api_missing_existing_device_query_returns_503',source('backend/src/api/app_voice_http_tests.rs'))
        self.assertIn('fn local_receiver_api_uses_its_own_intent',source('backend/src/api/app_voice_http_tests.rs'))
        self.assertIn('local receiver preparation is a separate explicit original-UUID operation',source('frontend/tests/appLogin.test.ts'))

if __name__=='__main__':unittest.main()
