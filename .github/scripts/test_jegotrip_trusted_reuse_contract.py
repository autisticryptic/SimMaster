"""Supplemental SOURCE guards for explicit already-trusted UUID reuse.
Rust/native/HTTPS/browser simulation must still run on both Actions; no local run.
"""
from pathlib import Path
import re
import unittest
ROOT=Path(__file__).resolve().parents[2]
P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip/app_login'
def text(path):return (ROOT/path).read_text(encoding='utf-8')
def source(name):return (P/name).read_text(encoding='utf-8')

class JegotripTrustedReuseContracts(unittest.TestCase):
    def test_reuse_intents_are_explicit_nondefaults_need_native_and_an_original_uuid(self):
        backend=source('mod.rs')
        self.assertIn('Authenticate, #[default] EnableHosting, AttachHosting, CheckDeviceTrust, ReuseHosting, EnableTrustedHosting',backend)
        self.assertIn('Self::EnableHosting|Self::AttachHosting|Self::ReuseHosting|Self::EnableTrustedHosting',backend)
        submit=backend.split('fn submit_jegotrip(',1)[1].split('fn submit_internal',1)[0]
        self.assertIn('if !request.consent',submit)
        self.assertIn('device_identity::valid_phone_app_uuid',submit)
        self.assertIn('intent.reuses_trusted_device()',submit)
        request=backend.split('pub struct JegotripLoginRequest {',1)[1].split('\n}',1)[0]
        for forbidden in ['trust_granted','sso_token','enrollId','device_sync_profile','hardware_imei']:
            self.assertNotIn(forbidden,request)

    def test_strict_identity_cannot_generate_rotate_retire_or_rebind_before_confirmation(self):
        identity=source('device_identity.rs')
        resolve=identity.split('fn resolve_reuse(',1)[1].split('/// Successful App user-info',1)[0]
        for forbidden in ['generated()', 'legacy_seed', 'retired=1', 'alias(conn,']:
            self.assertNotIn(forbidden,resolve)
        self.assertIn('valid_phone_app_uuid(value)',resolve)
        self.assertIn('in_use(conn,row.account_key.as_deref())',resolve)
        confirm=identity.split('fn confirm_reuse(',1)[1].split('fn current(',1)[0]
        self.assertLess(confirm.index('active_account(conn,account_key)'),confirm.index('let selected=confirm('))
        self.assertIn('canonical.snapshot.id!=used.id',confirm)
        self.assertIn('app_login_identity_mismatch',confirm)
        self.assertIn('in_use(conn,Some(account_key))',confirm)
        self.assertLess(confirm.index('let selected=confirm('),confirm.index('alias(conn,alias_key,&selected.id)'))

    def test_wire_uses_get_call_status_original_fields_and_no_vtd_or_enrollment_in_reuse(self):
        http=source('http.rs');flow=source('trusted_reuse.rs')
        for forbidden in ['client.enroll_device','client.ensure_device_enrollment','client.verify_trusted_device','sso_token', 'generated()']:
            self.assertNotIn(forbidden,flow)
        self.assertIn('if self.trusted_reuse { return Err(Error::new("app_login_request_invalid")); }',http)
        login=http.split('pub async fn login(',1)[1].split('pub async fn bound_number(',1)[0]
        self.assertIn('if !self.trusted_reuse {',login)
        status=http.split('pub async fn call_status(',1)[1].split('pub async fn business_enabled',1)[0]
        self.assertIn('"device_id":self.device_id()?',status)
        self.assertIn('Authentication::Session,false',status)
        self.assertIn('body.get("localCsForward")',http)
        self.assertNotIn('body.get("local_cs_forward")',http)
        self.assertIn('if number.to_string()!=text {return None;}',http)

    def test_current_readback_and_native_validation_precede_every_remote_write_or_creation(self):
        flow=source('trusted_reuse.rs')
        self.assertIn('if status.authentication_code!=Some(1)',flow)
        self.assertLess(flow.index('require_current_trust(initial?)'),flow.index('client.phone_credentials'))
        self.assertLess(flow.index('config.validate('),flow.index('client.sync_device('))
        self.assertLess(flow.index('let mut latest=require_current_trust(latest?)'),flow.index('client.sync_device('))
        self.assertLess(flow.index('let mut latest=require_current_trust(latest?)'),flow.index('client.enable_business'))
        self.assertLess(flow.index('latest=require_current_trust(after_sync?)'),flow.index('accounts.create'))
        self.assertLess(flow.index('require_current_trust(confirmed?)'),flow.index('accounts.create'))
        self.assertNotIn('if observation.',flow)
        attach=flow.split('if intent==LoginIntent::ReuseHosting {',1)[1].split('} else if',1)[0]
        self.assertNotIn('client.enable_business',attach)
        self.assertIn('app_login_business_not_enabled',attach)

    def test_uncertain_device_and_business_writes_are_durable_and_not_replayed_by_version_gate(self):
        flow=source('trusted_reuse.rs');http=source('http.rs');backend=source('mod.rs')
        self.assertLess(flow.index('sync=DeviceSyncObservation::request_uncertain()'),flow.index('client.sync_device'))
        self.assertLess(flow.index('self.record_reuse_observation(owner,id,guard,Ok(latest),sync.clone())'),flow.index('client.sync_device'))
        self.assertLess(flow.index('Some("request_uncertain")'),flow.index('client.enable_business'))
        gate=backend.split('async fn run_with_version_gate(',1)[1].split('async fn run_flow',1)[0]
        self.assertLess(gate.index('client.trusted_reuse_write_started()'),gate.index('client.refresh_app_version'))
        sync=http.split('async fn sync_device(',1)[1].split('pub async fn enable_business',1)[0]
        self.assertLess(sync.index('trusted_reuse_write_started.store(true'),sync.index('self.post(path'))
        enable=http.split('pub async fn enable_business(',1)[1]
        self.assertLess(enable.index('trusted_reuse_write_started.store(true'),enable.index('self.post('))
        for forbidden in ['closeVoip','setSafeNum','enroll_device','retry(']:self.assertNotIn(forbidden,flow)

    def test_report_keeps_original_uuid_and_current_accountid_and_never_mints_missing_push_fields(self):
        profile=source('device_sync.rs');http=source('http.rs');config=source('config.rs')
        for pair in ['"device_id":device_id','"imei":device_id','"userId":account_id','"os_type":"0"']:
            self.assertIn(pair,profile)
        self.assertIn('body.get("accountid")',http)
        self.assertIn('bound.app_account_id.as_ref()',http)
        self.assertNotIn('body.get("user_id")',http)
        self.assertIn('if let Some(field)=field',profile)
        self.assertIn('valid_text(v.expose_to_backend(),4096)',profile)
        self.assertIn('self.app_version_code==0',profile)
        self.assertIn('DeviceSyncProfile(<redacted>)',profile)
        self.assertIn('device_sync_profile: h.device_sync_profile.as_ref().map',config)
        for forbidden in ['eprintln!','SystemRandom','DeviceUtil','AuthnHelper','hostname','std::fs']:
            self.assertNotIn(forbidden,profile)

    def test_history_remains_additive_two_kib_metadata_and_no_ready_is_restored(self):
        model=source('device_trust.rs');store=source('store.rs')
        self.assertIn('MAX_OBSERVATION_BYTES: usize = 2048',model)
        self.assertIn('skip_serializing_if="Option::is_none"',model)
        self.assertIn('pub call_status:Option<TrustQueryObservation>',model)
        self.assertIn('pub device_sync:Option<DeviceSyncObservation>',model)
        self.assertIn('WHERE attempt_id=?3 AND owner=?4 AND completed=0',store)
        for forbidden in ['pub token:', 'pub device_id:', 'pub ready:', 'pub userId:', 'pub device_token:']:
            self.assertNotIn(forbidden,model)
        self.assertNotIn('ALTER TABLE jegotrip_device_identities',store)

    def test_runtime_regressions_are_nonzero_nested_and_both_workflows_keep_original_execution(self):
        tests=source('trusted_reuse_tests.rs');parent=source('tests.rs')
        names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',tests)
        self.assertGreaterEqual(len(names),30);self.assertEqual(len(names),len(set(names)))
        self.assertIn('#[path = "trusted_reuse_tests.rs"]\nmod trusted_reuse;',parent)
        for name in ['trusted_reuse_post_sync_enable_native_sms_ack_and_stop_logout_join_keep_the_original_scope_and_tokens',
                     'trusted_reuse_status_uses_real_pinned_https_and_keeps_the_original_plain_json_shape',
                     'invalid_native_context_stops_before_even_an_opted_in_device_sync_write',
                     'trusted_reuse_write_marker_survives_session_reset_for_sync_and_explicit_business_attempts',
                     'old_successful_trust_observation_cannot_authorize_a_later_reuse_when_the_server_now_returns_two']:
            self.assertIn('fn '+name,tests)
        self.assertNotIn('#[ignore',tests)
        for name in ['build-release.yml','beta-validation.yml']:
            workflow=text('.github/workflows/'+name)
            self.assertIn('connectivity::app_voice::providers::jegotrip::app_login::tests \\\n',workflow)
            self.assertIn('cargo test --locked --no-run',workflow)
            self.assertIn('playwright test --config playwright.jegotrip.config.ts',workflow)

    def test_chinese_ui_requires_phone_mode_and_different_consent_for_reuse_and_enable(self):
        policy=text('frontend/src/policies/appLogin.ts');pane=text('frontend/src/pages/sim/AppAccountLoginPane.tsx')
        self.assertIn("(!reusing || deviceMode === 'phone_app')",policy)
        self.assertIn("deviceMode !== 'phone_app' || reusing",policy)
        self.assertIn("intent: mode === 'phone_app' ? 'reuse_hosting'",pane)
        self.assertIn("deviceMode: mode, consent: false",pane)
        self.assertIn('不请求 VTD',pane)
        self.assertIn('不申请新授信或开启业务',pane)
        self.assertIn('明确开启托管',pane)
        self.assertIn('SDK 会话冲突',pane)
        for forbidden in ['localStorage','sessionStorage','indexedDB','console.log']:
            self.assertNotIn(forbidden,pane)
        browser=text('frontend/e2e/jegotrip-device-identity.spec.ts')
        self.assertEqual(browser.count("test('"),2)
        self.assertIn("toContainText('复用已授信 UUID 接管托管')",browser)
        self.assertIn("toHaveAttribute('aria-disabled', 'true')",browser)
        self.assertIn('expect(model.logins).toEqual([])',browser)

if __name__=='__main__':unittest.main()
