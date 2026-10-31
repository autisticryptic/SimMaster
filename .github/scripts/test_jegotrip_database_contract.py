"""Static guards for provider-specific tables and database-backed account auth."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip/app_login'


class JegotripDatabaseContractTests(unittest.TestCase):
    def test_provider_tables_preserve_legacy_ledger_and_unrelated_runtime_storage(self):
        store = (P / 'store.rs').read_text()
        self.assertIn('ALTER TABLE app_voice_login_attempts RENAME TO jegotrip_login_attempts', store)
        self.assertIn("intent TEXT NOT NULL DEFAULT 'enable_hosting'", store)
        self.assertIn('app_login_migration_conflict', store)
        self.assertIn('WHERE owner<>?2 AND completed=0', store)
        self.assertIn('CREATE TABLE IF NOT EXISTS jegotrip_login_settings', (P / 'settings.rs').read_text())
        self.assertIn('CREATE TABLE IF NOT EXISTS jegotrip_accounts', (P / 'accounts.rs').read_text())
        self.assertNotIn('DROP TABLE app_voice_accounts', store)
        self.assertNotIn('password TEXT', store)
        self.assertNotIn('token TEXT', store)

    def test_app_only_authentication_returns_before_phone_credentials_and_business(self):
        source = (P / 'mod.rs').read_text()
        self.assertLess(source.index('if intent==LoginIntent::Authenticate {return Ok(());}'), source.index('client.phone_credentials(&bound).await'))
        self.assertLess(source.index('client.bound_number(&request).await?'), source.index('accounts::record(c,'))
        self.assertIn('self.selected_configuration()', source)
        self.assertIn('configuration.runtime.is_none()', source)
        self.assertIn('configuration=configuration.ok_or', source)
        self.assertIn('app_login_native_configuration_missing', source)
        self.assertIn('request.password = Secret::from_owned(String::new())', source)

    def test_configuration_gets_are_metadata_only_and_db_clear_keeps_revision(self):
        settings = (P / 'settings.rs').read_text()
        self.assertIn('configuration BLOB', settings)
        self.assertIn('current!=expected', settings)
        self.assertIn('current.checked_add(1)', settings)
        source = (P / 'mod.rs').read_text()
        public = source.split('pub struct ConfigurationView {', 1)[1].split('\n}', 1)[0]
        for forbidden in ['Secret', 'base_url', 'salt', 'password', 'token', 'root_der']:
            self.assertNotIn(forbidden, public)
        self.assertIn('settings::StoredConfiguration::Database', source)
        self.assertLess(source.index('settings::StoredConfiguration::Database'), source.index('settings::StoredConfiguration::Missing'))

    def test_frontend_uses_dedicated_routes_and_does_not_save_passwords_or_fake_ready(self):
        pane = (ROOT / 'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text()
        panel = (ROOT / 'frontend/src/pages/sim/AppAccountsPanel.tsx').read_text()
        config = (ROOT / 'frontend/src/pages/sim/JegotripConfigurationDialog.tsx').read_text()
        for forbidden in ['localStorage', 'sessionStorage']:
            self.assertNotIn(forbidden, pane)
            self.assertNotIn(forbidden, config)
        self.assertIn('api.submitJegotripLogin', pane)
        self.assertIn('api.getJegotripAccounts()', panel)
        self.assertIn('api.saveJegotripConfiguration', config)
        policy = (ROOT / 'frontend/src/policies/appLogin.ts').read_text()
        self.assertIn('const emptyDraft = emptyAppLoginDraft', pane)
        self.assertIn("startReceiver: true, intent: 'enable_hosting'", policy)
        self.assertIn("draft.intent ?? 'enable_hosting'", policy)
        self.assertIn('const accepting = !!setup?.accepting', pane)
        self.assertNotIn('<MenuItem value="authenticate">', pane)
        self.assertIn('登录并启动无忧行', pane)
        self.assertIn("setText('')", config)

    def test_login_frontend_declares_draft_type_and_has_no_empty_error_map_entry(self):
        pane = (ROOT / 'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text()
        policy = (ROOT / 'frontend/src/policies/appLogin.ts').read_text()
        self.assertIn('type AppLoginDraft', pane.split("from '../../policies/appLogin'", 1)[0])
        self.assertNotIn("',,", policy)
        # Source guards are supplemental; both Actions still execute TypeScript
        # and lint rather than treating these checks as compilation evidence.
        for name in ['build-release.yml', 'beta-validation.yml']:
            self.assertIn('pnpm run type-check', (ROOT / '.github/workflows' / name).read_text())

    def test_http_configuration_fixture_does_not_broaden_private_nonce_visibility(self):
        native = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'
        runtime = (native / 'account_runtime.rs').read_text()
        http = (ROOT / 'backend/src/api/app_voice_http_tests.rs').read_text()
        fixture = http.split('async fn authenticated_jegotrip_configuration_is_database_backed_cas_and_http_only_without_file()', 1)[1].split('\n    #[tokio::test]', 1)[0]
        self.assertIn('pub(super) fn local_nonce()', runtime)
        self.assertNotIn('account_runtime::local_nonce()', fixture)
        self.assertIn('ring::rand::SecureRandom::fill', fixture)
        self.assertIn('claim_process_ownership(&lock_path)', fixture)

    def test_https_database_fixture_retains_the_drop_owned_certificate(self):
        tests = (P / 'tests.rs').read_text()
        fixture = tests.split('async fn database_app_only_login_uses_real_https_and_never_builds_a_native_peer()', 1)[1].split('\nasync fn https_once(', 1)[0]
        self.assertIn('Secret::from_owned(cert.root.clone())', fixture)
        self.assertNotIn('Secret::from_owned(cert.root)', fixture)
        self.assertIn('TlsAcceptor::from(server_config.clone())', fixture)

    def test_primary_api_defaults_to_full_hosting_but_still_requires_consent(self):
        source = (P / 'mod.rs').read_text()
        self.assertIn('pub enum LoginIntent { Authenticate, #[default] EnableHosting }', source)
        self.assertIn('fn default_start_receiver() -> bool { true }', source)
        self.assertIn('#[serde(default="default_start_receiver")]', source)
        self.assertIn('if !request.consent', source)
        self.assertLess(source.index('configuration.runtime.is_none()'), source.index('let client=self.factory.build(&configuration.http)?'))


if __name__ == '__main__':
    unittest.main()
