"""Supplemental guards for exact existing-device fallback and layered push status."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'

class JegotripPushStatusContractTests(unittest.TestCase):
    def test_existing_device_query_matches_the_original_plain_json_fallback(self):
        source = (P / 'app_login/http.rs').read_text()
        body = source.split('pub async fn verify_trusted_device(',1)[1].split('pub async fn enroll_device',1)[0]
        self.assertIn('"sso_token":""',body)
        self.assertIn('"equipment_type":"1"',body)
        self.assertIn('Authentication::Session,false',body)
        self.assertLess(body.index('if self.session.is_none()'),body.index('self.version_value().await'))
        self.assertNotIn('token.expose_to_backend()',body)
        tests = (P / 'app_login/tests.rs').read_text()
        self.assertIn('existing_device_fallback_is_plain_json_and_never_substitutes_the_app_token',tests)
        self.assertIn('existing_device_fallback_still_requires_the_owned_app_session',tests)
        self.assertIn('seen[1].wire',tests)

    def test_push_status_has_no_provider_operations_or_secret_fields(self):
        source = (P / 'app_login/mod.rs').read_text()
        method = source.split('async fn push_status(',1)[1].split('\n    }',1)[0]
        self.assertIn('accounts.list(',method)
        self.assertIn('push_status::stored_observations',method)
        for forbidden in ['client.login','enroll_device','accounts.start','enable_business','verify_trusted_device']:
            self.assertNotIn(forbidden,method)
        status = (P / 'app_login/push_status.rs').read_text()
        self.assertIn('requires_external_sms_call_test',status)
        self.assertIn('requires_original_binding_evidence',status)
        self.assertIn('platform_offline_wakeup:false',status)
        self.assertIn('call_notification_forwarding:false',status)
        self.assertIn('no_armed_sms_receiver',status)
        self.assertIn('no_armed_call_receiver',status)
        self.assertNotIn('Secret',status)
        self.assertNotIn('body:Vec',status)

    def test_account_alive_classification_survives_the_matched_handoff_not_as_registration(self):
        backend = (P / 'runtime_backend.rs').read_text()
        self.assertIn('account_alive_observation',backend)
        self.assertIn('lock(&attempt.state).account_alive=Some(plan.account_alive())',backend)
        self.assertIn('snapshot.account_alive_state=run.backend.account_alive_observation', (P / 'account_runtime.rs').read_text())
        self.assertIn('matched_common_bootstrap_alive_state_is_preserved', (P / 'runtime_common_backend_tests.rs').read_text())
        self.assertIn('matched_tls_bootstrap_alive_state_is_visible', (P / 'runtime_bootstrap_backend_tests.rs').read_text())
        self.assertNotIn('registerAlive.AccountAlive.Account', backend)

    def test_diagnostic_route_and_chinese_ui_do_not_restore_historical_readiness(self):
        api = (ROOT / 'backend/src/api/app_voice.rs').read_text()
        self.assertIn('/api/jegotrip/push/status',api)
        self.assertIn('manager.push_status().await.map_err',api)
        tests = (ROOT / 'backend/src/api/app_voice_http_tests.rs').read_text()
        self.assertIn('push_status_is_authenticated_read_only',tests)
        self.assertIn('a_stored_native_account_does_not_make_read_only_push_diagnostics_claim',tests)
        ui = (ROOT / 'frontend/src/pages/sim/JegotripPushStatusPane.tsx').read_text()
        self.assertIn('不登录无忧行',ui)
        self.assertIn('仍须本人号码的外部收件测试',ui)
        self.assertIn('api.getJegotripPushStatus()',ui)
        for forbidden in ['localStorage','sessionStorage','submitJegotripLogin','saveJegotrip']:
            self.assertNotIn(forbidden,ui)
        self.assertIn('JegotripPushStatusPane', (ROOT / 'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text())

if __name__ == '__main__':
    unittest.main()
