"""Static wiring guards; executable protocol/browser validation remains Actions-only."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip/app_login'


class AppLoginContractTests(unittest.TestCase):
    def test_login_request_cannot_import_personal_sdk_tokens_or_override_deployment(self):
        source = (P / 'mod.rs').read_text()
        request = source.split('pub struct LoginRequest {', 1)[1].split('\n}', 1)[0]
        for field in ['attempt_id', 'username', 'password', 'country_code', 'enable_hosting', 'start_receiver']:
            self.assertIn(f'pub {field}:', request)
        for forbidden in ['phone_login_token', 'sip_password', 'enrollment_confirmed', 'deployment', 'callback']:
            self.assertNotIn(forbidden, request)
        self.assertIn('#[serde(deny_unknown_fields)]', source)
        self.assertIn('request.password = Secret::from_owned(String::new())', source)

    def test_business_change_is_durable_before_send_and_never_implicitly_closed_or_replayed(self):
        source = (P / 'mod.rs').read_text()
        self.assertLess(source.index('Some("request_uncertain")'), source.index('client.enable_business().await'))
        self.assertLess(source.index('config.validate('), source.index('client.enable_business().await'))
        self.assertLess(source.index('client.business_enabled().await? { return Err'), source.index('accounts.create(config'))
        self.assertIn('app_login_attempt_exists', source)
        http = (P / 'http.rs').read_text()
        self.assertNotIn('querySafeDevice', http)
        self.assertNotIn('PHONE_MESSAGE_SAVE_CHECK', http)
        self.assertNotIn('"cs_forward":"1"', http)
        self.assertIn('"cs_forward":"2"', http)

    def test_https_is_fixed_origin_and_does_not_echo_private_errors(self):
        http = (P / 'http.rs').read_text()
        config = (P / 'config.rs').read_text()
        self.assertIn('crate::platform::dns::http_client_builder()', http)
        self.assertIn('redirect(reqwest::redirect::Policy::none())', http)
        self.assertNotIn('danger_accept_invalid', http)
        self.assertIn('url.origin()!=base.origin()', config)
        self.assertIn('url.scheme()!="https"', config)
        self.assertIn('crypto::MAX_BODY.saturating_sub(bytes.len())', http)
        self.assertIn('crypto::success(&doc)?; crypto::decrypt_body', http)

    def test_startup_owns_no_login_until_explicit_submit_and_shutdown_retires_provisioning_first(self):
        main = (ROOT / 'backend/src/main.rs').read_text()
        state = (ROOT / 'backend/src/state.rs').read_text()
        self.assertLess(main.index('claim_process_ownership('), main.index('app_logins.initialize('))
        self.assertLess(main.index('logins.shutdown()'), main.index('accounts.shutdown_all()'))
        self.assertIn('AppLoginManager::new(database.clone(), Arc::downgrade(&app_accounts))', state)
        source = (P / 'mod.rs').read_text()
        self.assertIn('accounts.login_process_owner()', source)
        schema = (P / 'store.rs').read_text()
        self.assertIn("WHERE owner<>?2 AND completed=0", schema)
        self.assertNotIn('private_config', schema)
        self.assertNotIn('password TEXT', schema)
        self.assertNotIn('token TEXT', schema)

    def test_frontend_uses_separate_app_account_api_and_does_not_persist_passwords(self):
        pane = (ROOT / 'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text()
        panel = (ROOT / 'frontend/src/pages/sim/AppAccountsPanel.tsx').read_text()
        dialog = (ROOT / 'frontend/src/pages/sim/VirtualSimDialog.tsx').read_text()
        for forbidden in ['localStorage', 'sessionStorage', 'createVirtualSimProfile', 'phone_login_token']:
            self.assertNotIn(forbidden, pane)
        self.assertIn('api.submitAppLogin', pane)
        self.assertIn('api.getAppLoginAttempt(id)', pane)
        self.assertIn("password: '', consent: false", pane)
        self.assertIn('incoming_message_ready', panel)
        self.assertIn('incoming_call_ready', panel)
        self.assertIn("accessKind !== 'five_codes'", dialog)
        self.assertIn("provider.id !== 'app_voice'", dialog)

    def test_both_actions_execute_new_rust_and_frontend_checks(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::app_login::tests \\\n', (ROOT / '.github/workflows' / name).read_text())
        self.assertIn('tests/appLogin.test.ts', (ROOT / 'frontend/package.json').read_text())
        tests = (P / 'tests.rs').read_text()
        for name in ['real_native_login_and_sms_use_fetched_phone_credentials_not_app_session_token',
                     'wrong_https_root_refuses_before_any_login_body_is_delivered',
                     'uncertain_business_transport_result_is_durable_and_attempt_id_is_not_replayed',
                     'final_ledger_failure_is_observable_and_never_replays_business']:
            self.assertIn(f'fn {name}', tests)


if __name__ == '__main__':
    unittest.main()
