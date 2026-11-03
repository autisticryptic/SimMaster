"""Supplemental wiring guards; Rust/browser execution remains Actions-only."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip/app_login'


class JegotripDeviceIdentityContractTests(unittest.TestCase):
    def test_provider_specific_identity_alias_and_history_storage_is_bounded_and_not_a_trust_grant(self):
        source = (P / 'device_identity.rs').read_text()
        for table in ['jegotrip_device_identities', 'jegotrip_device_aliases', 'jegotrip_device_identity_history']:
            self.assertIn(f'CREATE TABLE IF NOT EXISTS {table}', source)
        self.assertIn('COLLATE NOCASE', source)
        self.assertIn('WHERE retired=0 AND account_key IS NOT NULL', source)
        self.assertIn('MAX_IDENTITIES: usize = 64', source)
        self.assertIn('MAX_HISTORY: i64 = 512', source)
        self.assertIn('MAX_ALIASES: i64 = 256', source)
        self.assertIn('retired=1', source)
        self.assertNotIn('DELETE FROM jegotrip_device_identities', source)
        self.assertNotIn('password TEXT', source)
        self.assertNotIn('token TEXT', source)
        self.assertIn('length(CAST(device_id AS BLOB)) BETWEEN 8 AND 64 THEN device_id', source)

    def test_admission_reservation_is_atomic_and_bound_number_confirmation_precedes_business(self):
        source = (P / 'mod.rs').read_text()
        admit = source.split('fn submit_internal(', 1)[1].split('async fn wait_closed', 1)[0]
        self.assertLess(admit.index('try_acquire_owned'), admit.index('device_identity::resolve'))
        self.assertLess(admit.index('admissions.aliases.contains'), admit.index('device_identity::resolve'))
        self.assertIn('Transaction::new_unchecked', admit)
        self.assertLess(admit.index('store::insert_admitted'), admit.index('tx.commit()'))
        self.assertLess(admit.index('tx.commit()'), admit.index('tokio::spawn'))
        flow = source.split('async fn run_flow(', 1)[1]
        self.assertLess(flow.index('client.bound_number(&request).await?'), flow.index('self.confirm_device_identity'))
        self.assertLess(flow.index('self.confirm_device_identity'), flow.index('client.phone_credentials'))
        self.assertLess(flow.index('client.use_device_identity'), flow.index('client.enable_business'))
        self.assertIn('client.account_key(&confirmed)?!=key', flow)
        self.assertIn('native_ownership_pending', flow)
        device = (P / 'device_identity.rs').read_text()
        self.assertIn('app_login_identity_mismatch', device)
        self.assertIn('FROM jegotrip_login_attempts WHERE account_id=?1', device)
        self.assertIn('account_runs::ownership_pending', device)

    def test_all_app_device_bodies_use_the_resolved_value_and_sdk_context_is_not_replaced(self):
        http = (P / 'http.rs').read_text()
        self.assertIn('jegotrip-device-alias-v1', http)
        self.assertIn('realm.as_str().as_bytes()', http)
        self.assertIn('request.identity_key()', http)
        self.assertNotIn('"device_id":self.config.device_id', http)
        self.assertNotIn('"deviceId":self.config.device_id', http)
        self.assertIn('"device_id":self.device_id()?', http)
        self.assertIn('"deviceId":self.device_id()?', http)
        self.assertIn('self.session=None;', http.split('pub fn use_device_identity', 1)[1].split('fn device_id', 1)[0])
        config = (P / 'config.rs').read_text()
        mirror = config.split('struct PrivateHttpRef', 1)[1]
        self.assertIn('device_identity_mode: DeviceIdentityMode', mirror)
        self.assertIn('device_identity_mode: h.device_identity_mode', mirror)
        self.assertIn('device_id:self.sdk.device_id', config)

    def test_mutations_are_revision_guarded_authenticated_and_never_use_raw_device_values_in_urls(self):
        source = (P / 'mod.rs').read_text()
        for name in ['set_device_identity', 'delete_device_identity']:
            part = source.split(f'fn {name}(', 1)[1].split('\n    }', 1)[0]
            self.assertIn('self.device_admin_preflight()', part)
            self.assertIn('!admissions.aliases.is_empty()', part)
        api = (ROOT / 'backend/src/api/app_voice.rs').read_text()
        self.assertIn('/api/jegotrip/device-identities/{id}', api)
        self.assertNotIn('/api/jegotrip/device-identities/{device_id}', api)
        self.assertIn('manager.delete_device_identity(&id,request.expected_revision)', api)
        device = (P / 'device_identity.rs').read_text()
        self.assertIn('row.account_key.as_deref().or(key)', device)
        self.assertIn('app_login_device_identity_revision_conflict', device)
        self.assertIn('app_login_device_identity_in_use', device)
        tests = (ROOT / 'backend/src/api/app_voice_http_tests.rs').read_text()
        self.assertIn('device_identity_routes_require_auth_and_uninitialized_writes_do_not_create_state', tests)
        self.assertIn('authenticated_device_identity_crud_is_cas_bound_and_retains_deleted_ownership', tests)
        self.assertIn('StatusCode::CONFLICT', tests)

    def test_chinese_ui_defaults_to_auto_supports_manual_cas_and_persists_nothing_in_browser_storage(self):
        policy = (ROOT / 'frontend/src/policies/appLogin.ts').read_text()
        pane = (ROOT / 'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text()
        devices = (ROOT / 'frontend/src/pages/sim/JegotripDeviceIdentityPane.tsx').read_text()
        self.assertIn("deviceMode: 'auto'", policy)
        self.assertIn('validJegotripDeviceId', policy)
        self.assertIn('本次登录的设备标识', pane)
        self.assertIn('JegotripDeviceIdentityPane', pane)
        self.assertIn('expected_revision: row?.revision ?? 0', devices)
        self.assertIn("device_id: mode === 'manual' ? value.trim() : null", devices)
        self.assertIn('不代表绕过一键验证', devices)
        for text in [pane, devices]:
            self.assertNotIn('localStorage', text)
            self.assertNotIn('sessionStorage', text)
        config = (ROOT / 'frontend/playwright.jegotrip.config.ts').read_text()
        self.assertIn("VITE_API_PROXY_TARGET: 'http://127.0.0.1:1'", config)

    def test_both_workflows_run_the_new_nonzero_test_module_and_mock_ui_suite(self):
        for name in ['beta-validation.yml', 'build-release.yml']:
            workflow = (ROOT / '.github/workflows' / name).read_text()
            self.assertIn('connectivity::app_voice::providers::jegotrip::app_login::device_identity_tests', workflow)
            self.assertIn('playwright test --config playwright.jegotrip.config.ts', workflow)
            self.assertIn('frontend/test-results/jegotrip-ui-results.json', workflow)
        tests = (P / 'device_identity_tests.rs').read_text()
        for case in ['two_aliases_of_one_confirmed_number_converge', 'an_older_linked_receiver_remains_fenced',
                     'oversized_or_wrong_type_persisted_device_columns', 'persisted_identity_survives_database_reopen']:
            self.assertIn(case, tests)


if __name__ == '__main__':
    unittest.main()
