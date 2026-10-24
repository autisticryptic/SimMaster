"""Static application-account boundaries; Rust/SQLite/HTTP execution is Actions-only."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROVIDER = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'


def source(path):
    return (ROOT / path).read_text(encoding='utf-8')


class AppAccountContractTests(unittest.TestCase):
    def test_private_schema_and_cas_are_independent_from_sim_and_readiness(self):
        store = (PROVIDER / 'account_store.rs').read_text(encoding='utf-8')
        self.assertIn('CREATE TABLE IF NOT EXISTS app_voice_accounts', store)
        self.assertIn('TransactionBehavior::Immediate', store)
        self.assertIn('pub const MAX_ACCOUNTS: i64 = 64', store)
        self.assertIn('revision = ?5', store)
        self.assertIn('expected.checked_add(1)', store)
        for forbidden in ['virtual_sim_profile', 'ON CONFLICT', 'INSERT OR REPLACE', 'last_success', 'last_ready', 'iccid', 'imsi']:
            self.assertNotIn(forbidden, store)

    def test_secret_configuration_has_only_private_bounded_zeroizing_serialization(self):
        config = (PROVIDER / 'account_config.rs').read_text(encoding='utf-8')
        self.assertIn('const MAX_PRIVATE_BYTES: usize = 256 * 1024', config)
        self.assertIn('Zeroizing::new(Vec::with_capacity(MAX_PRIVATE_BYTES))', config)
        self.assertIn('PrivateWriter(&mut bytes)', config)
        self.assertIn('format_version != 1', config)
        self.assertIn('validate_explicit_profile(ConnectionInput', config)
        self.assertIn('sdk.selected_session_id.as_ref().ok_or(INVALID)', config)
        for name in ['AccountConfig', 'ProfileConfig', 'ProfileProperty', 'IncomingBinding']:
            self.assertNotIn(f'impl Serialize for {name}', config)
        self.assertNotIn('serde_json::json!', config)
        self.assertNotIn('serde_json::Value', config)
        self.assertNotIn('impl Serialize for Secret', (PROVIDER / 'credentials.rs').read_text(encoding='utf-8'))

    def test_runtime_keeps_cleanup_original_and_never_installs_call_or_sms_provider(self):
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        self.assertIn('slot.operation.lock().await', runtime)
        self.assertIn('controller.resources_released()', runtime)
        self.assertIn('retirement.disarm()', runtime)
        self.assertIn('configuration_current()', runtime)
        self.assertIn('controller.resource_owner()', runtime)
        self.assertIn('pub provider_availability:', runtime)
        self.assertIn('provider_availability: "unsupported"', runtime)
        for forbidden in ['FiveCodes', 'VirtualSim', 'ModemBinding', 'AccessPathKind', 'install_for_line', 'registry().install', 'outbound_authorized: true']:
            self.assertNotIn(forbidden, runtime)
        constructor = runtime.split('pub fn new(database:', 1)[1].split('fn metadata(', 1)[0]
        self.assertNotIn('.start(', constructor)
        self.assertNotIn('.connect(', constructor)

    def test_management_is_auth_protected_and_cannot_export_credentials_or_inject_callbacks(self):
        main = source('backend/src/main.rs')
        merge = main.index('.merge(api::app_voice::routes())')
        auth = main.index('api::auth::auth_middleware', merge)
        self.assertLess(merge, auth)
        api = source('backend/src/api/app_voice.rs')
        self.assertIn('DefaultBodyLimit::max(MAX_REQUEST_BYTES)', api)
        self.assertIn('HeaderValue::from_static("no-store")', api)
        self.assertIn('Result<Json<CreateAccountRequest>, JsonRejection>', api)
        self.assertNotIn('expose_to_backend', api)
        self.assertNotIn('encode_private', api)
        self.assertNotIn('on_callback', api)
        self.assertNotIn('format!("{error}', api)
        self.assertIn('provider_id != PROVIDER_ID', api)

    def test_startup_adds_private_schema_but_never_restores_active_app_sessions(self):
        database = source('backend/src/platform/db.rs')
        self.assertIn('account_store::initialize_schema(&conn)', database)
        state = source('backend/src/state.rs')
        self.assertIn('AccountManager::new(database.clone())', state)
        self.assertNotIn('app_accounts.start(', state)
        self.assertNotIn('app_accounts.install(', state)
        self.assertNotIn('account_store::install_all', database)

    def test_canonical_provider_id_is_shared_with_the_existing_product_contract(self):
        module = (PROVIDER / 'mod.rs').read_text(encoding='utf-8')
        store = (PROVIDER / 'account_store.rs').read_text(encoding='utf-8')
        self.assertIn('pub const PROVIDER_ID: &str = "app_voice"', module)
        self.assertIn('const PROVIDER: &str = super::PROVIDER_ID', store)
        self.assertIn("CHECK (provider_id = 'app_voice')", store)
        self.assertIn("CASE WHEN provider_id='app-voice' THEN 'app_voice'", store)
        self.assertIn('known_hyphenated_schema_migrates_atomically', (PROVIDER / 'account_store_tests.rs').read_text(encoding='utf-8'))

    def test_durable_generic_inbox_preserves_original_scope_without_claiming_sms(self):
        inbox = (PROVIDER / 'account_inbox.rs').read_text(encoding='utf-8')
        self.assertIn('UNIQUE(line_id,revision,run_nonce,source_kind,peer_epoch,sequence)', inbox)
        self.assertIn('TransactionBehavior::Immediate', inbox)
        self.assertIn('const MAX_PER_ACCOUNT: i64 = 128', inbox)
        self.assertIn('const MAX_TOTAL: i64 = 2048', inbox)
        self.assertIn('const MAX_TOTAL_BYTES: i64 = 16 * 1024 * 1024', inbox)
        self.assertIn('message.ticket().peer_incarnation()', inbox)
        self.assertIn('sender IS ?9 AND opaque_time IS ?10', inbox)
        self.assertIn('app_voice.inbox_stored', inbox)
        self.assertIn('"classification":"unclassified"', inbox)
        self.assertIn('receiver_object IS ?11 AND receiver_domain IS ?12 AND receiver_kind IS ?13', inbox)
        self.assertIn('if self.receiver.is_none() { return Err(INVALID); }', inbox)
        self.assertIn('old provider binding cannot be reconstructed', inbox)
        self.assertIn('original_incoming_binding(&delivery.owner, &delivery.scope)', (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8'))
        for forbidden in ['sms.received', 'INSERT INTO sms_messages', 'from_utf8_lossy', 'with_io_gate', 'impl Serialize for InboxScope', 'impl Serialize for CommittedInbox']:
            self.assertNotIn(forbidden, inbox)

    def test_consumer_reply_admission_follows_commit_and_retains_pressure_tickets(self):
        inbox = (PROVIDER / 'account_inbox.rs').read_text(encoding='utf-8')
        for kind in ['direct', 'batch']:
            begin = inbox.index(f'pub(super) async fn accept_{kind}')
            end = inbox.index('\n    }', begin)
            implementation = inbox[begin:end]
            self.assertLess(implementation.index(f'let committed = self.commit_{kind}'), implementation.index(f'backend.complete_{kind}'))
            self.assertIn('admission.load(Ordering::SeqCst)', implementation)
            self.assertIn('RuntimeError::Backpressure', implementation)
            self.assertIn('Completion::StatusUnrecorded', implementation)
        self.assertLess(inbox.index('tx.commit().map_err(|_| STORAGE)?;\n            Ok((id,Some(event)))'), inbox.index('database.publish_committed_app_event'))
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        self.assertIn('MAX_PENDING_MESSAGES: usize = 16', runtime)
        self.assertIn('MAX_PENDING_MESSAGE_BYTES: usize = 1024 * 1024', runtime)
        self.assertIn('Completion::Backpressure) => pending.push_back(delivery)', runtime)
        self.assertIn('Completion::StatusUnrecorded', runtime)
        self.assertIn('controller.phase() != Phase::Hosted', runtime)

    def test_inbox_api_exports_only_redacted_history_and_deletion_is_transactional(self):
        api = source('backend/src/api/app_voice.rs')
        self.assertIn('/api/non-3gpp/accounts/{line_id}/inbox', api)
        self.assertIn('!(1..=128).contains(&request.limit)', api)
        self.assertNotIn('private_message', api)
        store = (PROVIDER / 'account_store.rs').read_text(encoding='utf-8')
        self.assertIn('delete_private_for_account(&tx, id)', store)
        for workflow in ['.github/workflows/build-release.yml', '.github/workflows/beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::account_inbox::tests', source(workflow))
        self.assertIn('impl Drop for IncomingMessage', (PROVIDER / 'native_events.rs').read_text(encoding='utf-8'))

    def test_both_actions_have_nonzero_account_and_real_auth_http_filters(self):
        for workflow in ['.github/workflows/build-release.yml', '.github/workflows/beta-validation.yml']:
            text = source(workflow)
            for module in ['account_config', 'account_store', 'account_runtime']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests', text)
            self.assertIn('for filter in http_router_tests::app_voice_accounts', text)
            self.assertIn('DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"', text)
            self.assertIn('grep -q ": test$"', text)
        http = source('backend/src/api/app_voice_http_tests.rs')
        self.assertNotIn('#[ignore]', http)
        self.assertNotIn('else { return', http)
        self.assertIn('StatusCode::UNAUTHORIZED', http)
        self.assertIn('StatusCode::CONFLICT', http)


if __name__ == '__main__':
    unittest.main()
