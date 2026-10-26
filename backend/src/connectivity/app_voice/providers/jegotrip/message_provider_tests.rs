//! Actions-only original native TLS/hosting and per-account SMS readiness tests.
//! No Android SDK, real account/service/device or local compiled execution.
use super::*;
use super::super::{
    account_config::AccountConfig,
    account_runtime::{AccountBackendFactory, AccountView},
    account_test_support::{config, peer, QueuedFactory, TempDatabase},
    rpc_envelope::decode_request,
    runtime_backend::{BackendEvent, NativeHostingBackend, tests::{next_non_poll, Peer}},
    session::{STATUS_REGISTER, STATUS_UNREGISTER},
    smgw_text::SMGW_RECEIVER,
    tls_engine::tests::Certificates,
};
use crate::connectivity::app_voice::registry::ProviderRegistry;
use std::{future::Future, sync::{atomic::{AtomicBool, Ordering}, Mutex}, time::Duration};
use tokio::sync::{mpsc, oneshot};

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), future).await.expect("bounded native readiness fixture")
}
#[derive(Default)]
struct RecordingFactory { queue: QueuedFactory, backend: Mutex<Option<Arc<NativeHostingBackend>>> }
impl AccountBackendFactory for RecordingFactory {
    fn build(&self, config: &AccountConfig) -> Result<(Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>), &'static str> {
        let (backend, events) = self.queue.build(config)?;
        *self.backend.lock().unwrap() = Some(backend.clone());
        Ok((backend, events))
    }
}
enum Stop { Normal, Retired }
async fn finish(mut peer: Peer, stop: Stop) {
    if matches!(stop, Stop::Normal) {
        peer.swap(Some(STATUS_UNREGISTER), true).await;
        let message = next_non_poll(&mut peer).await;
        let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
        peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await;
    }
    while let Some(message) = peer.receive().await {
        // Original kernel-accepted polls may precede retirement. They never
        // become a new operation or an availability grant after scope closure.
        let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
    }
}
async fn hosted(accounts: &AccountManager, line: &str) -> AccountView {
    bounded(async {
        loop {
            let view = accounts.get(line, 100).await.unwrap();
            if view.phase == "hosted" { return view; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await
}
struct Running {
    temp: TempDatabase, accounts: Arc<AccountManager>, provider: MessagingProvider,
    factory: Arc<RecordingFactory>, line: String, revision: u64,
    stop: Option<oneshot::Sender<Stop>>, serving: tokio::task::JoinHandle<()>, dropped: Arc<AtomicBool>,
}
impl Running {
    async fn start(kind: &[u8], expiry: Option<u64>) -> Self {
        let cert = Certificates::generate("DNS:fixture.invalid");
        let temp = TempDatabase::new(); let factory = Arc::new(RecordingFactory::default());
        let (mut peer, _, dropped) = peer(&cert, &factory.queue);
        let accounts = Arc::new(AccountManager::with_factory(temp.database.clone(), factory.clone()));
        let provider = MessagingProvider::for_accounts(&accounts);
        let mut configuration = config(&cert.root);
        configuration.profile.incoming.as_mut().unwrap().receiver_kind = kind.to_vec();
        configuration.credentials.sdk.expires_at_unix_seconds = expiry;
        let line = accounts.create(configuration, 100).await.unwrap().account.line_id;
        assert_eq!(provider.availability(&line), ProviderAvailability::NotConfigured);
        assert!(!accounts.get(&line, 100).await.unwrap().incoming_message_ready);
        let (stop, stopped) = oneshot::channel();
        let serving = tokio::spawn(async move {
            peer.authenticate().await; peer.swap(Some(STATUS_REGISTER), true).await;
            finish(peer, stopped.await.unwrap()).await;
        });
        accounts.start(&line, 1, 100).await.unwrap(); hosted(&accounts, &line).await;
        Self { temp, accounts, provider, factory, line, revision: 1, stop: Some(stop), serving, dropped }
    }
    async fn stop(mut self, stop: Stop) {
        let retired = matches!(stop, Stop::Retired);
        self.stop.take().unwrap().send(stop).ok().unwrap();
        let stopped = match bounded(self.accounts.stop(&self.line, self.revision, 100)).await {
            Ok(stopped) => stopped,
            // Retiring IO before the controller sees its terminal callback can
            // make unregister report Disconnected. Do not erase that verdict:
            // first prove this original owner really joined/compacted, then
            // repeat ONLY its cleanup wait, not a new SDK operation/session.
            Err("app_account_disconnected") if retired => {
                let released = self.temp.database.with_conn(|conn| super::super::account_runs::ownership_pending(conn, &self.line)).unwrap();
                assert!(!released, "only original actor+consumer Join proof can retire the database fence");
                assert!(self.dropped.load(Ordering::SeqCst), "original connector IO must actually be released");
                match bounded(self.accounts.get(&self.line, 100)).await {
                    Ok(first) => {
                        assert!(!first.io_cleanup_required);
                        assert!(!first.incoming_message_ready && !first.incoming_call_ready && !first.outbound_authorized);
                    }
                    Err("app_account_revision_conflict") if self.revision == 2 => {},
                    Err(error) => panic!("unexpected post-Join snapshot {error}"),
                }
                bounded(self.accounts.stop(&self.line, self.revision, 100)).await.unwrap()
            }
            Err(error) => panic!("unexpected original cleanup verdict {error}"),
        };
        assert!(!stopped.io_cleanup_required);
        assert!(!stopped.incoming_message_ready && !stopped.incoming_call_ready && !stopped.outbound_authorized);
        assert_ne!(self.provider.availability(&self.line), ProviderAvailability::Available);
        bounded(self.serving).await.unwrap(); assert!(self.dropped.load(Ordering::SeqCst));
    }
}

#[test]
fn construction_and_descriptor_only_advertise_inbound_implemented_text_not_voice_or_paid_actions() {
    let temp = TempDatabase::new(); let accounts = Arc::new(AccountManager::new(temp.database.clone()));
    let provider = MessagingProvider::for_accounts(&accounts);
    assert_eq!(provider.descriptor().id, PROVIDER_ID);
    assert_eq!(provider.descriptor().capabilities, vec![VoiceCapability::Messaging]);
    assert_eq!(provider.availability("modem-fixture"), ProviderAvailability::Unsupported);
    assert_eq!(provider.availability("wlan-fixture"), ProviderAvailability::Unsupported);
    assert_eq!(format!("{provider:?}"), "MessagingProvider(<redacted>)");
    assert!(ProviderRegistry::new().is_empty());
    drop(accounts);
    assert_eq!(provider.availability("app-account-0123456789abcdef0123456789abcdef"), ProviderAvailability::NotConfigured);
}

#[tokio::test]
async fn exact_native_smgw_hosting_and_live_durable_consumer_publish_only_sms_readiness() {
    let running = Running::start(SMGW_RECEIVER, None).await;
    let view = running.accounts.get(&running.line, 100).await.unwrap();
    assert!(view.incoming_message_ready); assert!(!view.incoming_call_ready && !view.outbound_authorized);
    assert_eq!(view.provider_availability, "available"); assert_eq!(view.access, "application_data");
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Available);
    let registry = ProviderRegistry::new(); registry.install(Box::new(MessagingProvider::for_accounts(&running.accounts))).unwrap();
    assert_eq!(registry.availability(PROVIDER_ID, &running.line), ProviderAvailability::Available);
    assert_eq!(registry.availability("google_voice", &running.line), ProviderAvailability::Unsupported);
    assert_eq!(registry.availability(PROVIDER_ID, "modem-fixture"), ProviderAvailability::Unsupported);
    running.stop(Stop::Normal).await;
}

#[tokio::test]
async fn same_sdk_success_on_generic_receiver_does_not_grant_text_or_voice_readiness() {
    let running = Running::start(b"fixture.kind", None).await;
    let view = running.accounts.get(&running.line, 100).await.unwrap();
    assert_eq!(view.phase, "hosted"); assert!(!view.incoming_message_ready && !view.incoming_call_ready && !view.outbound_authorized);
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Unsupported);
    running.stop(Stop::Normal).await;
}

#[tokio::test]
async fn another_manager_cannot_restore_readiness_from_same_database_live_run_or_history() {
    let running = Running::start(SMGW_RECEIVER, None).await;
    let other = Arc::new(AccountManager::new(running.temp.database.clone()));
    let other_provider = MessagingProvider::for_accounts(&other);
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Available);
    assert_eq!(other_provider.availability(&running.line), ProviderAvailability::NotConfigured);
    let other_view = other.get(&running.line, 100).await.unwrap();
    assert!(!other_view.incoming_message_ready); assert_eq!(other_view.last_error, Some("app_account_runtime_owner_unconfirmed"));
    assert_eq!(other.start(&running.line, 1, 100).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Available);
    running.stop(Stop::Normal).await;
}

#[tokio::test]
async fn closing_original_native_scope_removes_availability_before_any_cached_hosted_view() {
    let running = Running::start(SMGW_RECEIVER, None).await;
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Available);
    running.factory.backend.lock().unwrap().as_ref().unwrap().retire_current();
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::NotConfigured);
    assert!(!running.accounts.get(&running.line, 100).await.unwrap().incoming_message_ready);
    running.stop(Stop::Retired).await;
}

#[tokio::test]
async fn original_configuration_revision_conflict_never_uses_cached_sms_readiness() {
    let mut running = Running::start(SMGW_RECEIVER, None).await;
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Available);
    // Public Store CAS refuses active replacement; this explicit SQL injection
    // is solely an Actions race fixture for a concurrently changed database.
    running.temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1", [&running.line])
        .map_err(|_| "test_revision_injection_failed")).unwrap();
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Unknown);
    assert_eq!(running.accounts.get(&running.line, 100).await.err(), Some("app_account_revision_conflict"));
    running.revision = 2;
    running.factory.backend.lock().unwrap().as_ref().unwrap().retire_current();
    running.stop(Stop::Retired).await;
}

#[tokio::test]
async fn imported_credential_expiry_closes_original_readiness_without_waiting_for_timer_or_new_login() {
    let expires = super::super::account_runtime::unix_now() + 600;
    let running = Running::start(SMGW_RECEIVER, Some(expires)).await;
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::Available);
    let expired = running.accounts.get(&running.line, expires).await.unwrap();
    assert_eq!(expired.phase, "credential_expired"); assert!(!expired.incoming_message_ready && !expired.incoming_call_ready);
    assert_eq!(running.provider.availability(&running.line), ProviderAvailability::NotConfigured);
    running.stop(Stop::Retired).await;
}
