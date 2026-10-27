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
        self.assertIn('AccountManager::with_notifications(database.clone(),notification_sender.clone())', state)
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
        self.assertIn('"classification":classification', inbox)
        self.assertIn('smgw_text::classify(&receiver.kind, message)', inbox)
        self.assertIn('if classified.is_some() { "smgw_text" } else { "unclassified" }', inbox)
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
        self.assertLess(inbox.index('tx.commit().map_err(|_| STORAGE)?;\n            Ok((id,events))'), inbox.index('database.publish_committed_app_event'))
        self.assertLess(inbox.index('account_sms::promote_new(&tx'), inbox.index('tx.commit().map_err(|_| STORAGE)?;\n            Ok((id,events))'))
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

    def test_recovered_smgw_text_is_strict_and_does_not_turn_other_kinds_into_sms(self):
        text = (PROVIDER / 'smgw_text.rs').read_text(encoding='utf-8')
        self.assertIn('SMGW_RECEIVER: &[u8] = b"Sip.Request"', text)
        self.assertIn('receiver != SMGW_RECEIVER', text)
        self.assertIn('std::str::from_utf8(&message.body)', text)
        self.assertIn('message.from.as_deref()?', text)
        self.assertIn('!value.iter().all(u8::is_ascii_digit)', text)
        self.assertIn('DateTime::<Utc>::from_timestamp_millis', text)
        self.assertNotIn('from_utf8_lossy', text)
        self.assertNotIn('Utc::now()', text)
        self.assertNotIn('base64::', text)
        self.assertNotIn('pub text:', text)
        self.assertIn('SmgwText(<redacted>)', text)

    def test_sms_projection_is_atomic_explicit_application_transport_and_deletion_safe(self):
        sms = (PROVIDER / 'account_sms.rs').read_text(encoding='utf-8')
        self.assertIn('tx: &Transaction', sms)
        self.assertIn('AND receiver_kind=?4 AND body=?5 AND sender=?6 AND opaque_time IS ?7', sms)
        self.assertIn("'received',NULL,'app_voice'", sms)
        self.assertIn('"sms.received",Some(line),Some(super::PROVIDER_ID)', sms)
        self.assertIn('timestamp_source', sms)
        self.assertIn('app_voice_sms_deleted AFTER DELETE ON sms_messages', sms)
        self.assertIn("SET body=X'',sender=NULL,opaque_time=NULL", sms)
        self.assertIn('deleted=1,sent_at_millis=NULL', sms)
        promote = sms.split('pub(super) fn promote_new', 1)[1].split('/// Authorized read surface', 1)[0]
        self.assertNotIn('tx.commit()', promote)
        self.assertIn('TransactionBehavior::Immediate', sms)
        self.assertNotIn('insert_sms_with_transport', sms)
        self.assertNotIn('impl Serialize for SmgwText', sms)
        handlers = source('backend/src/api/handlers.rs')
        self.assertIn('"app_account_outbound_unsupported"', handlers)
        guarded = handlers.split('async fn send_sms_on_line_with_origin', 1)[1].split('let policy =', 1)[0]
        self.assertLess(guarded.index('account_store::valid_line_id'), guarded.index('VowifiScope::resolve'))
        self.assertIn('kind: "app_account"', handlers)

    def test_run_compaction_requires_joined_consumer_and_durable_original_fence(self):
        runs = (PROVIDER / 'account_runs.rs').read_text(encoding='utf-8')
        self.assertIn('proof:&RunQuiescence', runs)
        self.assertIn("SET state='released'", runs)
        self.assertIn('envelope_retained=0', runs)
        self.assertNotIn('DELETE FROM sms_messages', runs)
        self.assertNotIn('std::process', runs)
        self.assertNotIn('/proc/', runs)
        inbox = (PROVIDER / 'account_inbox.rs').read_text(encoding='utf-8')
        self.assertIn('account_runs::require_live(&tx,&self.line_id,self.revision,&self.nonce)', inbox)
        self.assertIn('MAX_IDENTITIES_PER_ACCOUNT: i64 = 512', inbox)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        stop = runtime.split('pub async fn stop(', 1)[1].split('impl Drop for AccountManager', 1)[0]
        self.assertLess(stop.index('drop(controller)'), stop.index('let _=task.await'))
        self.assertLess(stop.index('let _=task.await'), stop.index('RunQuiescence {'))
        self.assertLess(stop.index('release_and_compact'), stop.index('run.finalized.store(true'))
        self.assertIn('task: AsyncMutex<Option<tokio::task::JoinHandle<()>>>', runtime)
        self.assertIn('!run.finalized.load(Ordering::SeqCst)', runtime)

    def test_notification_plans_are_durable_bounded_and_claim_fenced(self):
        notices = (PROVIDER / 'account_notifications.rs').read_text(encoding='utf-8')
        for item in ['MAX_TARGETS:usize=8', 'MAX_JOBS:i64=2048', 'MAX_BYTES:i64=16*1024*1024', 'WORKERS:usize=4', 'MAX_ATTEMPTS:i64=5']:
            self.assertIn(item, notices)
        self.assertIn('UNIQUE(sms_id,rule_id,channel_id)', notices)
        self.assertIn("WHERE id=?5 AND claim=?6 AND state='sending'", notices)
        self.assertIn("state='uncertain'", notices)
        self.assertIn('shutdown.wait()', notices)
        self.assertIn('while running.join_next().await.is_some()', notices)
        self.assertNotIn('forward_sms(', notices)
        self.assertNotIn('line_registry', notices)
        sms = (PROVIDER / 'account_sms.rs').read_text(encoding='utf-8')
        promote = sms.split('pub(super) fn promote_new', 1)[1].split('struct WipingSms', 1)[0]
        self.assertIn('account_notifications::enqueue(tx,&snapshot.0,plan)', promote)
        self.assertNotIn('.await', promote)
        self.assertNotIn('tx.commit()', promote)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        self.assertIn('planner.capture()', runtime)
        self.assertIn('with_notifications(notifications)', runtime)
        self.assertIn('_=self.shutdown_requested()', runtime)

    def test_notification_service_uses_original_captured_destination_without_modem_or_requeue(self):
        adapter = source('backend/src/services/notify/app_voice_notifications.rs')
        self.assertIn('pair_fingerprint(rule, channel)', adapter)
        self.assertIn('fingerprint == work.fingerprint', adapter)
        self.assertIn('send_text_to_channel', adapter)
        self.assertIn('render_sms_template_bounded', adapter)
        self.assertNotIn('discover_modem', adapter)
        self.assertNotIn('forward_sms(', adapter)
        self.assertNotIn('enqueue_notification(', adapter)
        main = source('backend/src/main.rs')
        self.assertIn('NotificationWorker::spawn(', main)
        self.assertIn('accounts.shutdown_all()', main)
        self.assertIn('app_notification_worker.join()', main)

    def test_both_actions_have_nonzero_account_and_real_auth_http_filters(self):
        for workflow in ['.github/workflows/build-release.yml', '.github/workflows/beta-validation.yml']:
            text = source(workflow)
            for module in ['account_config', 'account_store', 'account_runtime', 'account_sms', 'smgw_text', 'account_runs', 'account_notifications']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests', text)
            self.assertIn('services::notify::notification::tests', text)
            self.assertIn('services::notify::notification::app_voice::tests', text)
            self.assertIn('for filter in http_router_tests::app_voice_accounts', text)
            self.assertIn('DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"', text)
            self.assertIn('grep -q ": test$"', text)
        http = source('backend/src/api/app_voice_http_tests.rs')
        self.assertNotIn('#[ignore]', http)
        self.assertNotIn('else { return', http)
        self.assertIn('StatusCode::UNAUTHORIZED', http)
        self.assertIn('StatusCode::CONFLICT', http)

    def test_runtime_exclusion_is_local_durable_and_not_a_pid_age_or_remote_lease(self):
        runs = (PROVIDER / 'account_runs.rs').read_text(encoding='utf-8')
        self.assertIn('CREATE TABLE IF NOT EXISTS app_voice_runtime_leases', runs)
        self.assertIn('line_id TEXT PRIMARY KEY NOT NULL', runs)
        self.assertIn('exclusive_runtime: bool', runs)
        self.assertIn('register_inner(conn,line,revision,nonce,true)', runs)
        self.assertIn('ownership_pending(&tx,line)?', runs)
        self.assertIn('revision=?2 AND run_nonce=?3', runs)
        self.assertNotIn('lease_until', runs)
        self.assertNotIn('std::process::id()', runs)
        self.assertNotIn('principal_key', runs)
        delete = runs.split('pub(super) fn delete_account_state', 1)[1]
        self.assertNotIn('DELETE FROM app_voice_runtime_leases', delete)

    def test_unowned_manager_cannot_start_stop_or_clear_original_runtime_state(self):
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        self.assertIn('app_account_runtime_owner_unconfirmed', runtime)
        self.assertIn('account_runs::runtime_matches(conn,&self.run_token)', runtime)
        self.assertIn('account_runs::register_runtime(conn,line_id,metadata.revision,&inbox_nonce)', runtime)
        start = runtime.split('pub async fn start(', 1)[1].split('pub async fn stop(', 1)[0]
        self.assertLess(start.index('self.require_released(line_id,&slot)'), start.index('self.factory.build(&config)'))
        self.assertLess(start.index('account_runs::register_runtime('), start.index('controller.start(now)'))
        tests = (PROVIDER / 'account_runtime_tests.rs').read_text(encoding='utf-8')
        self.assertIn('second_manager_cannot_start_stop_or_mutate_original_account', tests)
        self.assertIn('foreign_manager_stop_never_closes_hosted_original_actor', tests)
        self.assertIn('reopened_manager_keeps_legacy_live_run_unconfirmed', tests)

    def test_store_and_original_join_gc_preserve_exclusion_until_atomic_release(self):
        store = (PROVIDER / 'account_store.rs').read_text(encoding='utf-8')
        for name, end in [('replace', 'pub fn delete'), ('delete', '#[cfg(test)]')]:
            operation = store.split(f'pub fn {name}(', 1)[1].split(end, 1)[0]
            self.assertIn('super::account_runs::runtime_held(&tx,id)?', operation)
            self.assertIn('app_account_stop_before_replace', operation)
        runs = (PROVIDER / 'account_runs.rs').read_text(encoding='utf-8')
        release = runs.split('pub(super) fn release_and_compact', 1)[1].split('pub(super) fn delete_account_state', 1)[0]
        self.assertIn('held.as_ref().is_some_and', release)
        self.assertIn('token.exclusive_runtime', release)
        self.assertIn('if removed!=1{return Err(SCOPE);}', release)
        self.assertLess(release.index('DELETE FROM app_voice_runtime_leases'), release.rindex('tx.commit()'))
        tests = (PROVIDER / 'account_runs_tests.rs').read_text(encoding='utf-8')
        self.assertIn('failed_runtime_release_rolls_back_original_fence', tests)
        self.assertIn('stale_joined_proof_cannot_release_a_new_run', tests)
        self.assertIn('independent_sqlite_connections_atomically_admit_only_one', tests)
    def test_inbound_text_provider_uses_live_original_consumer_not_imported_or_cached_ready(self):
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        self.assertIn('struct MessagingReadiness { owner: AttemptOwner, scope: RpcConnectionScope }', runtime)
        self.assertIn('Callback::HostingAck { success: true }', runtime)
        self.assertIn('run.install_message_readiness(&owner, &scope)', runtime)
        self.assertIn('original.scope.is_open()', runtime)
        self.assertIn('binding.receiver_kind == super::smgw_text::SMGW_RECEIVER', runtime)
        self.assertIn('credential_expiry.is_some_and(|at| at <= now)', runtime)
        self.assertIn('!task.is_finished()', runtime)
        provider = (PROVIDER / 'message_provider.rs').read_text(encoding='utf-8')
        self.assertIn('Weak<AccountManager>', provider)
        self.assertIn('capabilities: vec![VoiceCapability::Voice, VoiceCapability::Messaging]', provider)
        self.assertIn('accounts.inbound_availability(line_id)', provider)
        self.assertIn('self.calls.as_ref().is_some_and(|calls| calls.ready())', runtime)
        self.assertIn('run.call_ready(unix_now())', runtime)
        self.assertNotIn('expose_to_backend', provider)
        self.assertIn('MessagingProvider::for_accounts(&app_state.app_accounts)', source('backend/src/main.rs'))
        for workflow in ['.github/workflows/build-release.yml', '.github/workflows/beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::message_provider::tests', source(workflow))

    def test_current_sms_receive_availability_is_independent_from_unsupported_paid_sends(self):
        handlers = source('backend/src/api/handlers.rs')
        self.assertIn('app.app_accounts.messaging_availability(&account.line_id)', handlers)
        self.assertIn('incoming_ready: Some(receiving), outbound_authorized: Some(false)', handlers)
        models = source('backend/src/api/models.rs')
        self.assertIn('pub incoming_ready: Option<bool>', models)
        self.assertIn('pub outbound_authorized: Option<bool>', models)
        tests = source('backend/src/api/app_voice_http_tests.rs')
        self.assertIn('channel["incoming_ready"],true', tests)
        self.assertIn('readiness["data"]["incoming_call_ready"],false', tests)
        self.assertIn('get_sms_messages_for_channel(10,0,Some("outgoing"),Some(&line)).unwrap().is_empty()', tests)
    def test_basic_call_settings_are_explicit_pure_and_original_receiver_bound(self):
        config = (PROVIDER / 'account_config.rs').read_text(encoding='utf-8')
        settings = (PROVIDER / 'call_settings.rs').read_text(encoding='utf-8')
        self.assertIn('pub call_media: Option<CallMediaConfig>', config)
        self.assertIn('self.profile.call_incoming.is_none() || self.profile.call_control.is_none()', config)
        self.assertIn('let _calls = self.runtime_calls()?', config)
        self.assertIn('!input.allow_plain_media', settings)
        self.assertIn('input.bind_ip.is_unspecified()', settings)
        self.assertIn('TrunkIncomingMode::BoundImmediate', settings)
        self.assertIn('trunk.vowifi_only || trunk.allow_home_cellular_calls', settings)
        self.assertNotIn('.connect(', settings)
        self.assertNotIn('.bind(', settings)
        self.assertNotIn('tokio::spawn', settings)
        for secret in ['CallMediaConfig', 'ProfileBytes']:
            self.assertNotIn(f'impl Serialize for {secret}', config)

    def test_basic_call_supervisor_waits_for_actual_original_media_and_keeps_join_ownership(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        operation = calls.split('async fn accept_original',1)[1].split('pub(super) async fn hangup',1)[0]
        self.assertLess(operation.index('CallOperation::Accept'),operation.index('MediaEndpointPlan::prepare_mpath'))
        self.assertLess(operation.index('ready=ready.await'),operation.index('Phase::Connected'))
        self.assertLess(operation.index('ready.original(&call.ticket'),operation.index('history::connected'))
        self.assertIn('socket.for_peer(&call.ticket,&call.offer,&answer',operation)
        self.assertIn('call.ticket.same_call(ticket)',calls)
        publication=calls.split('async fn publish_state',1)[1].split('async fn ring',1)[0]
        self.assertIn('call.state.try_lock()',publication)
        self.assertIn('from_millis(250)',publication)
        self.assertIn('tokio::task::yield_now().await',publication)
        self.assertNotIn('.revoke()',publication)
        self.assertNotIn('.close()',publication)
        self.assertIn('!call.ticket.remote_released()',calls)
        self.assertIn('self.expiry.is_some_and',calls)
        self.assertIn('account_runs::runtime_matches(conn,&self.run)',calls)
        self.assertIn('impl Drop for OperationWaiter',calls)
        self.assertIn('call.ticket.revoke()',calls)
        self.assertNotIn('CallOperation::Start',calls)
        self.assertIn('OperatorCommand::StartCall{call_id,..}=>current.operator.send_event(OperatorEvent::Unavailable',calls)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        cleanup = runtime.split('async fn cleanup_run',1)[1].split('async fn shutdown_requested',1)[0]
        self.assertLess(cleanup.index('calls.close().await'),cleanup.index('controller.stop().await'))
        self.assertIn('controller.resources_released() && calls_released',cleanup)
        for workflow in ['.github/workflows/build-release.yml','.github/workflows/beta-validation.yml']:
            for module in ['account_calls','account_call_history']:
                self.assertIn(f'connectivity::app_voice::providers::jegotrip::{module}::tests',source(workflow))
            self.assertIn('services::trunk::runtime::tests',source(workflow))
            self.assertIn('services::trunk::driver::tests',source(workflow))

    def test_call_history_cas_and_outbound_interfaces_never_inherit_original_grants(self):
        history = (PROVIDER / 'account_call_history.rs').read_text(encoding='utf-8')
        self.assertIn('app_call_token=?3 AND end_time IS NULL',history)
        self.assertIn('account_runs::require_live(&tx',history)
        self.assertIn('account_runs::runtime_matches(&tx',history)
        self.assertIn('TransactionBehavior::Immediate',history)
        self.assertIn('db.publish_committed_app_event',history)
        self.assertNotIn('impl Serialize for HistoryTicket',history)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        reserve = runtime.split('pub fn reserve_outbound',1)[1].split('pub async fn calls',1)[0]
        self.assertIn('self.check_revision(line_id, revision)?',reserve)
        self.assertIn('Err("app_account_outbound_unsupported")',reserve)
        self.assertNotIn('.call(',reserve)
        self.assertNotIn('.send(',reserve)
        api=source('backend/src/api/app_voice.rs')
        self.assertIn('request.sdp.len()>16_384',api)
        self.assertIn('manager.reserve_outbound(&line_id,request.expected_revision)',api)
        for path in ['calls/{call_id}/answer','calls/{call_id}/hangup','calls/{call_id}/reject','outbound/calls','outbound/messages']:
            self.assertIn(path,api)


    def test_specific_capability_availability_cannot_be_inherited_from_aggregate_sms_or_voice(self):
        registry = source('backend/src/connectivity/app_voice/registry.rs')
        self.assertIn('fn capability_availability', registry)
        self.assertIn('ProviderAvailability::Unknown', registry)
        self.assertIn('fn implemented_operations', registry)
        provider = (PROVIDER / 'message_provider.rs').read_text(encoding='utf-8')
        self.assertIn('VoiceCapability::Voice => accounts.call_availability(line_id)', provider)
        self.assertIn('VoiceCapability::Messaging => accounts.messaging_availability(line_id)', provider)
        self.assertIn('super::BASIC_INBOUND_OPERATIONS', provider)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        call = runtime.split('fn call_availability', 1)[1].split('fn inbound_availability', 1)[0]
        self.assertIn('metadata.revision != run.revision || run.configuration_current().is_err()', call)
        self.assertIn('run.call_ready(unix_now())', call)
        handlers = source('backend/src/api/handlers.rs')
        metadata = handlers.split('pub async fn get_jegotrip_credential_contract_handler()', 1)[1]
        self.assertIn('HeaderValue::from_static("no-store")', metadata)
        self.assertIn('implemented_operations: Vec::new()', metadata)
        self.assertIn('availability: "unknown".to_string()', metadata)

    def test_connected_waiter_rechecks_original_service_and_physical_media_at_actual_delivery(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        waiter = calls.split('impl Future for OperationWaiter', 1)[1].split('impl Drop for OperationWaiter', 1)[0]
        self.assertIn('this.service.upgrade()', waiter)
        self.assertIn('service.active(&this.call)', waiter)
        self.assertIn('state.phase == Phase::Connected', waiter)
        self.assertIn('media.scope().is_open()', waiter)
        self.assertIn('if !current || !media_open', waiter)
        self.assertIn('this.call.retire()', waiter)
        lifecycle = (PROVIDER / 'account_calls_lifecycle_tests.rs').read_text(encoding='utf-8')
        for regression in ['queued_connected_result_rechecks_permission_before_delivery',
                           'queued_connected_result_rechecks_original_revision_without_rebinding',
                           'queued_connected_result_rejects_closed_physical_media']:
            self.assertIn(regression, lifecycle)
        self.assertIn('let operation_guard = bounded(call.operation.lock()).await', lifecycle)
        self.assertIn('peer.reply(true).await; peer.eof().await', lifecycle)
        self.assertIn('account_runs::ownership_pending', lifecycle)


    def test_call_task_admission_precedes_state_mutation_and_retains_cleanup_owner(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        answer = calls.split('pub(super) async fn accept(', 1)[1].split('async fn accept_original', 1)[0]
        hangup = calls.split('pub(super) async fn hangup(', 1)[1].split('async fn finish', 1)[0]
        for operation, marker in [(answer, 'state.accept_queued=true'), (hangup, 'state.end_queued=true')]:
            self.assertLess(operation.index('self.task_admission().await'), operation.index(marker))
            submission = operation.split(marker, 1)[1].split('tasks.push(tokio::spawn', 1)[0]
            self.assertNotIn('.await', submission)
            self.assertIn('Ok(OperationWaiter', operation)
            self.assertNotIn('self.spawn_task', operation)
        tests = (PROVIDER / 'account_calls_admission_tests.rs').read_text(encoding='utf-8')
        for name in ['cancelled_answer_before_task_admission', 'cancelled_hangup_before_task_admission']:
            self.assertIn(name, tests)
        self.assertIn('assert!(!state.accept_queued && !state.end_queued)', tests)

    def test_original_task_joins_are_removed_individually_before_next_cancel_point(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        close = calls.split('pub(super) async fn close(', 1)[1].split('pub(super) fn resources_released', 1)[0]
        self.assertIn('while let Some(task)=tasks.first_mut()', close)
        self.assertLess(close.index('let _=task.await'), close.index('tasks.remove(0)'))
        self.assertNotIn('for task in tasks.iter_mut()', close)
        admission = calls.split('async fn task_admission', 1)[1].split('async fn spawn_task', 1)[0]
        self.assertLess(admission.index('(&mut tasks[index]).await'), admission.index('tasks.swap_remove(index)'))
        lifecycle = (PROVIDER / 'account_calls_lifecycle_tests.rs').read_text(encoding='utf-8')
        self.assertIn('extend([first,join])', lifecycle)
        self.assertIn('service.tasks.lock().await.len(),1', lifecycle)

    def test_batch_call_receipt_retries_only_nonadmitted_inform_under_one_deadline(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        ring = calls.split('async fn ring', 1)[1].split('pub(super) async fn accept(', 1)[0]
        self.assertLess(ring.index('let deadline ='), ring.index('let result = loop'))
        self.assertIn('tokio::time::timeout_at(deadline', ring)
        self.assertIn('Err(RuntimeError::CallNotReady)', ring)
        self.assertIn('Err(error) => return Err(call_error(error))', ring)
        self.assertIn('if !self.active(call)', ring)
        self.assertIn('call.ticket.revoked()', ring)
        tests = (PROVIDER / 'account_calls_admission_tests.rs').read_text(encoding='utf-8')
        for name in ['delayed_batch_receipt_precedes_one_inform', 'delayed_batch_receipt_does_not_retry_a_submitted',
                     'delayed_batch_receipt_expires_one_fixed_budget', 'cancelled_batch_receipt_wait_releases_original']:
            self.assertIn(name, tests)
        self.assertIn('pause_original_polls_for_test', tests)
        self.assertIn('payload.results[&1].result_stream', tests)
        self.assertIn('assert_eq!(payload.expose_to_transport(), native_up)', tests)


    def test_whole_original_call_watcher_covers_ringing_and_is_never_replaced_by_media(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        delivery = calls.split('pub(super) async fn delivery', 1)[1].split('async fn task_admission', 1)[0]
        self.assertLess(delivery.index('self.watch_original(&call)'), delivery.index('lock(&self.calls).insert'))
        watcher = calls.split('fn watch_original', 1)[1].split('pub(super) async fn delivery', 1)[0]
        self.assertIn('original.ticket.revoked()', watcher)
        self.assertIn('original.changed.notified()', watcher)
        self.assertIn('original.operation.lock().await', watcher)
        self.assertIn('service.finish(&original, true).await', watcher)
        answer = calls.split('async fn accept_original', 1)[1].split('pub(super) async fn hangup', 1)[0]
        self.assertIn('call.changed.notify_waiters()', answer)
        self.assertNotIn('call.watch.lock()', answer)
        self.assertNotIn('tokio::spawn', answer)
        retirement = (PROVIDER / 'account_calls_retirement_tests.rs').read_text(encoding='utf-8')
        for name in ['ringing_original_connection_loss', 'ringing_credential_expiry', 'ringing_revision_change',
                     'one_original_watcher_survives', 'failed_call_history_end_retains_original_run']:
            self.assertIn(name, retirement)
        close = calls.split('pub(super) async fn close', 1)[1].split('pub(super) fn resources_released', 1)[0]
        self.assertIn('if lock(&call.state).phase!=Phase::Ended{complete=false;}', close)

    def test_optional_original_cleanup_never_starts_replacement_or_takes_unowned_runtime(self):
        config = (PROVIDER / 'account_config.rs').read_text(encoding='utf-8')
        self.assertIn('#[serde(default)]\n    pub cleanup_on_disconnect: bool', config)
        self.assertIn('cleanup_on_disconnect: config.cleanup_on_disconnect', config)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        worker = runtime.split('async fn automatic_cleanup', 1)[1].split('const MAX_PENDING_MESSAGES', 1)[0]
        self.assertIn('run.cleanup_changed.notified()', worker)
        self.assertIn('timeout(Duration::from_secs(20)', worker)
        self.assertIn('AccountManager::cleanup_original(&run', worker)
        self.assertIn('run.finalized.load(Ordering::SeqCst)', worker)
        self.assertIn('run.terminal_observation', worker)
        self.assertIn('run.cleanup_error', worker)
        self.assertNotIn('.start(', worker)
        self.assertNotIn('.connect(', worker)
        self.assertNotIn('account_store::load', worker)
        cleanup = runtime.split('async fn cleanup_original', 1)[1].split('async fn join_automatic_cleanup', 1)[0]
        self.assertIn('run.cleanup_work.lock().await', cleanup)
        self.assertLess(cleanup.index('task.abort(); let _=task.await'), cleanup.index('account_runs::release_and_compact'))
        self.assertIn('controller.resources_released() && calls_released', cleanup)
        tests = (PROVIDER / 'account_auto_cleanup_tests.rs').read_text(encoding='utf-8')
        for name in ['opt_in_cleanup_keeps_consumer_join_fence', 'cancelled_manual_stop_does_not_detach',
                     'automatic_gc_failure_is_visible', 'persisted_cleanup_opt_in_never_reclaims']:
            self.assertIn(name, tests)
        self.assertIn('factory.build_count(), 1', tests)
        self.assertIn('count(&temp.database, "sms_messages"), 2', tests)

    def test_trunk_end_event_drain_precedes_driver_shutdown_and_is_original_consumer_bound(self):
        calls = (PROVIDER / 'account_calls.rs').read_text(encoding='utf-8')
        close = calls.split('pub(super) async fn close', 1)[1].split('pub(super) fn resources_released', 1)[0]
        self.assertLess(close.index('self.finish(&call,true).await'), close.index('drain_trunk_events'))
        self.assertLess(close.index('drain_trunk_events'), close.index('self.trunk.activate_profile'))
        self.assertIn('app_account_call_trunk_delivery_unconfirmed', close)
        operator = source('backend/src/services/trunk/operator.rs')
        self.assertIn('Arc::ptr_eq(&fence.consumer, &self.consumer)', operator)
        self.assertIn('consumer.closed()', operator)
        self.assertIn('trunk_operator_drain_timeout', operator)
        self.assertIn('impl Drop for TrunkEventReceiver', operator)
        self.assertIn('lagged_trunk_consumer_cannot_ack', operator)
        driver = source('backend/src/services/trunk/driver.rs')
        self.assertIn('subscribe_trunk_events()', driver)
        self.assertIn('trunk_operator_events_lost', driver)
        tests = (PROVIDER / 'account_calls_trunk_tests.rs').read_text(encoding='utf-8')
        self.assertIn('ringing_disconnect_sends_original_asterisk_cancel', tests)
        self.assertIn('assert!(cancel.starts_with(b"CANCEL sip:"))', tests)
        self.assertIn('assert!(!link.has_trunk_consumer())', tests)
        http = source('backend/src/api/app_voice_http_tests.rs')
        self.assertIn('authenticated_cleanup_opt_in_preserves_terminal_state_sms', http)
        self.assertIn('automatic_reconnect_implemented', http)


    def test_direct_retention_is_native_time_window_and_pending_is_not_capacity_evicted(self):
        history = (PROVIDER / 'direct_history.rs').read_text(encoding='utf-8')
        self.assertIn('Duration::from_secs(180)', history)
        self.assertIn('now.duration_since(self.rotated) > WINDOW', history)
        self.assertIn('now < self.observed', history)
        self.assertIn('self.current ^= 1', history)
        self.assertIn('refresh_duplicate', history)
        self.assertIn('Zeroizing<Vec<u8>>', history)
        protocol = (PROVIDER / 'arc_protocol.rs').read_text(encoding='utf-8')
        self.assertIn('self.direct_pending.len() >= super::rpc_stream::MAX_MAP_ENTRIES', protocol)
        self.assertIn('self.direct_seen.count() + self.direct_pending.len()', protocol)
        self.assertIn('request.call_id <= self.direct_last_id', protocol)
        frontier = protocol.split('fn direct_replay_frontier', 1)[1].split('pub fn complete_incoming', 1)[0]
        self.assertIn('self.direct_pending.keys().next()', frontier)
        self.assertIn('pending.saturating_sub(1)', frontier)
        self.assertIn('.min(self.direct_last_id)', frontier)
        for path in ['.github/workflows/build-release.yml', '.github/workflows/beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::direct_history::tests', source(path))

    def test_direct_compaction_requires_original_written_response_and_durable_watermark(self):
        actor = (PROVIDER / 'transport_actor.rs').read_text(encoding='utf-8')
        command = actor.split('async fn command(', 1)[1].split('async fn send_wire', 1)[0]
        self.assertLess(command.index('self.send_wire(&wire'), command.index('if is_response { receipt.direct'))
        self.assertIn('direct: None', actor)
        inbox = (PROVIDER / 'account_inbox.rs').read_text(encoding='utf-8')
        self.assertIn('receipt.direct_replay_frontier()', inbox)
        self.assertIn('proof.matches(&self.owner,&self.scope)', inbox)
        self.assertIn('app_voice_direct_replay_fences', inbox)
        self.assertIn('through_sequence=MAX(through_sequence,excluded.through_sequence)', inbox)
        self.assertIn("AND i.reply_state='direct_submitted'", inbox)
        self.assertIn('ORDER BY i.sequence DESC LIMIT -1 OFFSET 128', inbox)
        self.assertIn('app_account_inbox_direct_retired', inbox)
        self.assertLess(inbox.index('if let Some((id, matches)) = existing'), inbox.index('if kind == "direct"'))
        runs = (PROVIDER / 'account_runs.rs').read_text(encoding='utf-8')
        self.assertIn('DELETE FROM app_voice_direct_replay_fences WHERE line_id=?1 AND revision=?2 AND run_nonce=?3', runs)
        self.assertNotIn('DELETE FROM sms_messages', runs)

    def test_long_direct_regressions_use_same_actual_run_and_keep_failed_gc_ack_nonreplay(self):
        tests = (PROVIDER / 'account_long_reception_tests.rs').read_text(encoding='utf-8')
        self.assertIn('six_hundred_direct_sms_on_one_original_run', tests)
        self.assertIn('for sequence in 1i32..=600', tests)
        self.assertIn('assert_eq!(reply.call_id,rpc)', tests)
        self.assertIn('count(&temp.database,"sms_messages"),600', tests)
        self.assertIn('count(&temp.database,"app_voice_inbox"),128', tests)
        self.assertIn('connector.connection_calls(),1', tests)
        self.assertIn('inbox_nonce,original_nonce', tests)
        self.assertIn('submitted_direct_reply_gc_failure_retires_original_run', tests)
        self.assertIn('already submitted ACK may not be replayed', tests)
        sql = (PROVIDER / 'account_direct_retention_tests.rs').read_text(encoding='utf-8')
        self.assertIn('compacted_deleted_original_ticket_cannot_recreate', sql)
        self.assertIn('lower_pending_id_blocks_compaction', sql)
        self.assertIn('no_protocol_frontier_keeps_classified_envelopes', sql)


    def test_batch_retirement_requires_exact_ack_and_preserves_pending_gaps(self):
        scheduler = (PROVIDER / 'batch_scheduler.rs').read_text(encoding='utf-8')
        self.assertIn('acknowledged.extend(epoch.bucket.results.keys().copied())', scheduler)
        self.assertIn('words.ack_next_batch - 1 - bit', scheduler)
        self.assertIn('self.pending_server.keys().chain(self.working.results.keys())', scheduler)
        self.assertIn('self.history.values().flat_map(|epoch| epoch.bucket.results.keys())', scheduler)
        self.assertIn('self.dedup.base.saturating_sub(1)', scheduler)
        self.assertIn('sequence.saturating_sub(1)', scheduler)
        self.assertIn('through > self.retired_through || !acknowledged.is_empty()', scheduler)
        backend = (PROVIDER / 'offline_backend.rs').read_text(encoding='utf-8')
        self.assertLess(backend.index('scheduler.on_reply(&reply'), backend.index('delivery.retirement.take()'))
        self.assertIn('events.push(OfflineEvent::BatchRetirement(retirement))', backend)
        for filename in ['beta-validation.yml', 'build-release.yml']:
            workflow = (ROOT / '.github' / 'workflows' / filename).read_text(encoding='utf-8')
            self.assertIn('connectivity::app_voice::providers::jegotrip::batch_retention::tests', workflow)

    def test_batch_sparse_gc_is_original_leased_atomic_and_not_direct_or_unknown_eviction(self):
        retention = (PROVIDER / 'batch_retention.rs').read_text(encoding='utf-8')
        for guard in ['proof.matches(owner,scope)', 'account.revision!=revision', 'account_runs::require_live',
                      'app_voice_runtime_leases', 'proof.acknowledged()', 'MAX_SPARSE_PER_ACCOUNT: i64 = 2048',
                      'MAX_SPARSE_TOTAL: i64 = 8192', "i.source_kind='batch' AND i.reply_state='batch_queued'",
                      'app_voice_sms_projection', 'ORDER BY i.id DESC LIMIT -1 OFFSET 128']:
            self.assertIn(guard, retention)
        self.assertLess(retention.index('Transaction::new_unchecked'), retention.index('UPDATE app_voice_sms_projection'))
        self.assertLess(retention.index('INSERT OR IGNORE INTO app_voice_batch_replay_ids'), retention.index('DELETE FROM app_voice_inbox'))
        inbox = (PROVIDER / 'account_inbox.rs').read_text(encoding='utf-8')
        self.assertLess(inbox.index('if let Some((id, matches)) = existing'), inbox.index('batch_retention::retired'))
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        self.assertIn('BackendEventPayload::BatchRetirement(proof)', runtime)
        self.assertIn('super::batch_retention::apply', runtime)
        self.assertIn('*lock(&run.error)=Some(code);run.retire()', runtime)
        runs = (PROVIDER / 'account_runs.rs').read_text(encoding='utf-8')
        self.assertIn('batch_retention::delete_run(&tx,&token.line,token.revision,&token.nonce)', runs)

    def test_batch_long_reception_regressions_use_same_actor_and_ack_before_gc(self):
        sql = (PROVIDER / 'batch_retention_tests.rs').read_text(encoding='utf-8')
        for name in ['rolling_2300_batch_sms', 'sparse_original_deleted_ticket', 'failed_post_ack_compaction',
                     'foreign_closed_revision_or_missing_lease', 'unknown_and_pending_direct', 'sparse_retention_pressure']:
            self.assertIn(name, sql)
        tests = (PROVIDER / 'account_long_batch_tests.rs').read_text(encoding='utf-8')
        self.assertIn('receive(&mut peer,600', tests)
        self.assertIn('receive(&mut peer,129', tests)
        self.assertIn('original result bytes follow raw/SMS/event commit', tests)
        self.assertIn('already ACKed result must not be transmitted again', tests)
        self.assertIn('inbox_nonce,nonce', tests)
        self.assertIn('connector.connection_calls(),1', tests)
        self.assertIn('factory.build_count(),1', tests)
        self.assertIn('DROP TRIGGER fail_live_batch_gc', tests)


    def test_reconnect_supervisor_is_opt_in_post_cleanup_fresh_start_and_bounded(self):
        config = (PROVIDER / 'account_config.rs').read_text(encoding='utf-8')
        self.assertIn('if !self.cleanup_on_disconnect { return Err(INVALID); }', config)
        for bound in ['(1..=16).contains(&self.max_attempts)', '(1_000..=600_000).contains(&self.initial_delay_ms)', 'self.max_delay_ms > 3_600_000']:
            self.assertIn(bound, config)
        runtime = (PROVIDER / 'account_runtime.rs').read_text(encoding='utf-8')
        cleanup = runtime.split('async fn automatic_cleanup', 1)[1].split('fn reconnect_retryable', 1)[0]
        self.assertLess(cleanup.index('cleanup_original(&run'), cleanup.index('schedule_reconnect(&run)'))
        self.assertIn('return; }\n    // Only after the original run finalized', cleanup)
        retry = runtime.split('fn reconnect_retryable', 1)[1].split('fn schedule_reconnect', 1)[0]
        self.assertIn('"app_account_disconnected" | "app_account_backend_unavailable" | "app_account_operation_timeout"', retry)
        for forbidden in ['authentication', 'hosting_rejected', 'expired', 'revision', 'storage']:
            self.assertNotIn(forbidden, retry)
        schedule = runtime.split('fn schedule_reconnect', 1)[1].split('async fn reconnect_after', 1)[0]
        for guard in ['run.manual_cleanup.load', '!run.finalized.load', 't.phase == Phase::Disconnected', 'credential_expiry', 'next > policy.max_attempts', 'run.supervisor.upgrade()']:
            self.assertIn(guard, schedule)
        start = runtime.split('async fn start_with', 1)[1].split('let metadata = self.check_revision', 1)[0]
        self.assertIn('Arc::ptr_eq(&run, old)', start)
        self.assertIn('old.cancel_reconnect()', start)
        after = runtime.split('async fn reconnect_after', 1)[1].split('\nconst ', 1)[0]
        self.assertIn('old.revision', after)
        self.assertNotIn('import(', after)
        self.assertIn('run.cancel_reconnect();', runtime.split('async fn cleanup_run', 1)[1].split('async fn cleanup_original', 1)[0])
        self.assertIn('app_accounts.enable_reconnect_supervisor();', source('backend/src/state.rs'))
        tests = (PROVIDER / 'account_reconnect_tests.rs').read_text(encoding='utf-8')
        for name in ['disconnect_after_full_original_cleanup_starts_one_fresh_run', 'rejected_authentication_is_not_retried',
                     'failed_fresh_starts_are_bounded_then_exhausted', 'stop_during_scheduled_delay_cancels_supervised_start']:
            self.assertIn(name, tests)
        self.assertIn('assert_ne!(fresh.inbox_nonce, original_nonce', tests)

if __name__ == '__main__':
    unittest.main()
