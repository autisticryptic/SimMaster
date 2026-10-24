//! Independent application-account lifecycle, never a modem/SIM/IMS line.
//!
//! Storage/import/startup do not dial or restore cached readiness. Only an
//! explicit start with the exact supplied profile creates the native driver.
//! Per-account operation barriers preserve the old owning backend through
//! cancellation and cleanup retry; a revision change cannot rebind old work.
use super::{
    account_config::AccountConfig,
    account_inbox::{self, Completion, InboxMetadata, InboxScope},
    account_store::{self, AccountMetadata, StoredAccount},
    controller::{Callback, ControllerFailure, HostingController},
    runtime_backend::{BackendEvent, BackendEventPayload, NativeHostingBackend},
    runtime_connector::TcpRuntimeConnector,
    session::{BackendFailure, Phase},
};
use crate::platform::db::Database;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use std::{collections::{BTreeMap, VecDeque}, fmt, sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex}, time::{Duration, Instant}};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

pub trait AccountBackendFactory: Send + Sync {
    /// Construction only. Network work remains inside the original-owned
    /// HostingController::start, never config CRUD or process startup.
    fn build(&self, config: &AccountConfig)
        -> Result<(Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>), &'static str>;
}
struct ExplicitNativeFactory;
impl AccountBackendFactory for ExplicitNativeFactory {
    fn build(&self, config: &AccountConfig)
        -> Result<(Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>), &'static str> {
        let profile = config.runtime_profile()?;
        let connector = TcpRuntimeConnector::new(Duration::from_millis(config.profile.connect_timeout_ms))
            .map_err(|_| "app_account_configuration_invalid")?;
        Ok(NativeHostingBackend::new(profile, Arc::new(connector)))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub account: AccountMetadata,
    pub line_kind: &'static str,
    pub access: &'static str,
    pub phase: &'static str,
    pub io_cleanup_required: bool,
    pub incoming_call_ready: bool,
    pub incoming_message_ready: bool,
    pub outbound_authorized: bool,
    pub provider_availability: &'static str,
    pub manual_start_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<&'static str>,
}
fn view(metadata: AccountMetadata, phase: Phase, cleanup: bool, error: Option<&'static str>) -> AccountView {
    AccountView { account: metadata, line_kind: "app_account", access: "application_data",
        phase: phase_name(phase), io_cleanup_required: cleanup,
        // A hosted SDK acknowledgement is not verified SMS/call/media support.
        incoming_call_ready: false, incoming_message_ready: false, outbound_authorized: false,
        provider_availability: "unsupported", manual_start_required: true, last_error: error }
}
fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Unconfigured | Phase::CredentialsImported => "stopped",
        Phase::SdkLoginPending => "sdk_login_pending", Phase::SdkLoggedIn => "sdk_logged_in",
        Phase::HostingPending => "hosting_pending", Phase::UnhostingPending => "stopping",
        Phase::Hosted => "hosted", Phase::CredentialExpired => "credential_expired",
        Phase::AuthenticationFailed => "authentication_failed", Phase::Disconnected => "disconnected",
    }
}
fn failure_code(reason: BackendFailure) -> &'static str {
    match reason {
        BackendFailure::Unavailable => "app_account_backend_unavailable",
        BackendFailure::AuthenticationRejected => "app_account_authentication_rejected",
        BackendFailure::Expired => "app_account_credential_expired",
        BackendFailure::Disconnected => "app_account_disconnected",
        BackendFailure::HostingRejected => "app_account_hosting_rejected",
    }
}
fn controller_error(reason: ControllerFailure) -> &'static str {
    match reason {
        ControllerFailure::Backend(reason) => failure_code(reason),
        ControllerFailure::Timeout => "app_account_operation_timeout",
        // Do not echo SDK/native notification text or imported values.
        ControllerFailure::Credentials(_) => "app_account_lifecycle_invalid",
        ControllerFailure::Notification(_) => "app_account_protocol_invalid",
    }
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
pub(super) fn unix_now() -> u64 { chrono::Utc::now().timestamp().max(0) as u64 }
pub(super) fn local_nonce() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "app_account_entropy_unavailable")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

struct ManagedRun {
    line_id: String,
    database: Arc<Database>,
    revision: u64,
    inbox_nonce: String,
    controller: AsyncMutex<HostingController>,
    backend: Arc<NativeHostingBackend>,
    accepting: AtomicBool,
    stopping: AtomicBool,
    error: Mutex<Option<&'static str>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl ManagedRun {
    fn configuration_current(&self) -> Result<(), &'static str> {
        let metadata = self.database.with_conn(|conn| account_store::metadata(conn, &self.line_id))?
            .ok_or("app_account_not_found")?;
        if metadata.revision != self.revision { return Err("app_account_revision_conflict"); }
        Ok(())
    }
    fn retire(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        self.backend.retire_current();
    }
}
impl Drop for ManagedRun {
    fn drop(&mut self) {
        self.retire();
        if let Some(task) = lock(&self.task).take() { task.abort(); }
    }
}
struct Retirement { run: Arc<ManagedRun>, armed: bool }
impl Retirement {
    fn new(run: Arc<ManagedRun>) -> Self { Self { run, armed: true } }
    fn disarm(&mut self) { self.armed = false; }
}
impl Drop for Retirement { fn drop(&mut self) { if self.armed { self.run.retire(); } } }
#[derive(Default)]
struct AccountSlot {
    operation: AsyncMutex<()>,
    run: Mutex<Option<Arc<ManagedRun>>>,
}

/// One manager per application state, not a process-global credential registry.
/// Entries own no modem/worker/UICC, trunk or 3GPP cost-policy resources.
pub struct AccountManager {
    database: Arc<Database>,
    factory: Arc<dyn AccountBackendFactory>,
    slots: Mutex<BTreeMap<String, Arc<AccountSlot>>>,
}
impl fmt::Debug for AccountManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AccountManager(<redacted>)") }
}
impl AccountManager {
    pub fn new(database: Arc<Database>) -> Self { Self::with_factory(database, Arc::new(ExplicitNativeFactory)) }
    pub(crate) fn with_factory(database: Arc<Database>, factory: Arc<dyn AccountBackendFactory>) -> Self {
        Self { database, factory, slots: Mutex::new(BTreeMap::new()) }
    }
    fn metadata(&self, line_id: &str) -> Result<AccountMetadata, &'static str> {
        if !account_store::valid_line_id(line_id) { return Err("app_account_line_invalid"); }
        self.database.with_conn(|conn| account_store::metadata(conn, line_id))?.ok_or("app_account_not_found")
    }
    fn slot(&self, line_id: &str) -> Result<Arc<AccountSlot>, &'static str> {
        // Unknown IDs must not allocate unbounded runtime map entries.
        self.metadata(line_id)?;
        let mut slots = lock(&self.slots);
        if let Some(slot) = slots.get(line_id) { return Ok(slot.clone()); }
        if slots.len() >= account_store::MAX_ACCOUNTS as usize { return Err("app_account_limit"); }
        let slot = Arc::new(AccountSlot::default()); slots.insert(line_id.to_owned(), slot.clone()); Ok(slot)
    }
    fn check_revision(&self, line_id: &str, expected_revision: u64) -> Result<AccountMetadata, &'static str> {
        if expected_revision == 0 || expected_revision > i64::MAX as u64 { return Err("app_account_revision_invalid"); }
        let metadata = self.metadata(line_id)?;
        if metadata.revision != expected_revision { return Err("app_account_revision_conflict"); }
        Ok(metadata)
    }
    async fn snapshot_run(&self, metadata: AccountMetadata, run: Option<Arc<ManagedRun>>, now: u64)
        -> Result<AccountView, &'static str> {
        let Some(run) = run else { return Ok(view(metadata, Phase::Unconfigured, false, None)); };
        if run.revision != metadata.revision { return Err("app_account_revision_conflict"); }
        let mut controller = run.controller.lock().await;
        controller.tick(now, Instant::now());
        let error = if controller.phase() == Phase::CredentialExpired { Some("app_account_credential_expired") }
            else { *lock(&run.error) };
        Ok(view(metadata, controller.phase(), !controller.resources_released(), error))
    }
    pub async fn list(&self, now: u64) -> Result<Vec<AccountView>, &'static str> {
        let metadata = self.database.with_conn(account_store::list)?;
        let mut views = Vec::with_capacity(metadata.len());
        for account in metadata {
            let slot = lock(&self.slots).get(&account.line_id).cloned();
            let run = slot.as_ref().and_then(|slot| lock(&slot.run).clone());
            views.push(self.snapshot_run(account, run, now).await?);
        }
        Ok(views)
    }
    pub async fn get(&self, line_id: &str, now: u64) -> Result<AccountView, &'static str> {
        let slot = self.slot(line_id)?;
        let _operation = slot.operation.lock().await;
        let metadata = self.metadata(line_id)?; let run = lock(&slot.run).clone();
        self.snapshot_run(metadata, run, now).await
    }
    pub fn inbox(&self, line_id: &str, limit: usize) -> Result<Vec<InboxMetadata>, &'static str> {
        self.metadata(line_id)?;
        self.database.with_conn(|conn| account_inbox::metadata(conn, line_id, limit))
    }
    pub async fn create(&self, config: AccountConfig, now: u64) -> Result<AccountView, &'static str> {
        config.validate(now)?;
        // Local random ID never embeds a phone number/token/ICCID or claims to
        // be the provider's remote account/message identifier. Never upsert.
        for _ in 0..16 {
            let line_id = format!("app-account-{}", local_nonce()?);
            if self.database.with_conn(|conn| account_store::metadata(conn, &line_id))?.is_some() { continue; }
            let metadata = self.database.with_conn(|conn| account_store::create(conn, &line_id, &config, now))?;
            return Ok(view(metadata, Phase::Unconfigured, false, None));
        }
        Err("app_account_entropy_unavailable")
    }
    async fn require_released(slot: &AccountSlot) -> Result<(), &'static str> {
        let run = lock(&slot.run).clone();
        if let Some(run) = run {
            if !run.controller.lock().await.resources_released() { return Err("app_account_stop_before_replace"); }
        }
        Ok(())
    }
    pub async fn replace(&self, line_id: &str, expected_revision: u64, config: AccountConfig, now: u64)
        -> Result<AccountView, &'static str> {
        config.validate(now)?;
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        self.check_revision(line_id, expected_revision)?; Self::require_released(&slot).await?;
        let metadata = self.database.with_conn(|conn| account_store::replace(conn, line_id, expected_revision, &config, now))?;
        if let Some(run) = lock(&slot.run).take() { run.retire(); }
        Ok(view(metadata, Phase::Unconfigured, false, None))
    }
    pub async fn delete(&self, line_id: &str, expected_revision: u64) -> Result<(), &'static str> {
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        self.check_revision(line_id, expected_revision)?; Self::require_released(&slot).await?;
        if !self.database.with_conn(|conn| account_store::delete(conn, line_id, expected_revision))? {
            return Err("app_account_not_found");
        }
        if let Some(run) = lock(&slot.run).take() { run.retire(); }
        let mut slots = lock(&self.slots);
        if slots.get(line_id).is_some_and(|current| Arc::ptr_eq(current, &slot)) { slots.remove(line_id); }
        Ok(())
    }
    pub async fn start(&self, line_id: &str, expected_revision: u64, now: u64) -> Result<AccountView, &'static str> {
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        let metadata = self.check_revision(line_id, expected_revision)?;
        Self::require_released(&slot).await?;
        let StoredAccount { metadata: stored, config } = self.database.with_conn(|conn| account_store::load(conn, line_id))?
            .ok_or("app_account_not_found")?;
        if stored.revision != metadata.revision { return Err("app_account_revision_conflict"); }
        config.validate(now)?;
        let inbox_nonce = local_nonce()?;
        let (backend, events) = self.factory.build(&config)?;
        let mut controller = HostingController::new(line_id.to_owned(), Some(backend.clone()));
        controller.import(config.credentials, now).map_err(controller_error)?;
        let run = Arc::new(ManagedRun { line_id: line_id.to_owned(), database: self.database.clone(), revision: metadata.revision,
            inbox_nonce, controller: AsyncMutex::new(controller), backend,
            accepting: AtomicBool::new(true), stopping: AtomicBool::new(false), error: Mutex::new(None), task: Mutex::new(None) });
        // Retain the owning backend BEFORE any await that can create IO. A
        // cancelled HTTP future leaves this same run available for stop/retry.
        *lock(&slot.run) = Some(run.clone());
        let mut retirement = Retirement::new(run.clone());
        let weak = Arc::downgrade(&run);
        *lock(&run.task) = Some(tokio::spawn(consume_events(weak, events)));
        let mut controller = run.controller.lock().await;
        match controller.start(now).await {
            Ok(_) => {
                if let Err(code) = self.check_revision(line_id, expected_revision) {
                    *lock(&run.error) = Some(code); return Err(code);
                }
                retirement.disarm();
                Ok(view(metadata, controller.phase(), !controller.resources_released(), None))
            }
            Err(reason) => {
                let code = controller_error(reason); *lock(&run.error) = Some(code); Err(code)
            }
        }
    }
    pub async fn stop(&self, line_id: &str, expected_revision: u64, now: u64) -> Result<AccountView, &'static str> {
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        let metadata = self.check_revision(line_id, expected_revision)?;
        let Some(run) = lock(&slot.run).clone() else { return Ok(view(metadata, Phase::Unconfigured, false, None)); };
        // A private DB edit detected by the event/timer gate must still allow
        // cleanup of the retained OLD run. This command never imports the new
        // revision or substitutes its owner/connection for the original IO.
        run.stopping.store(true, Ordering::SeqCst);
        run.accepting.store(false, Ordering::SeqCst);
        let mut retirement = Retirement::new(run.clone());
        let mut controller = run.controller.lock().await;
        controller.tick(now, Instant::now());
        let result = controller.stop().await;
        if controller.resources_released() {
            retirement.disarm();
            if let Some(task) = lock(&run.task).take() { task.abort(); }
        }
        match result {
            Ok(()) => {
                *lock(&run.error) = None;
                Ok(view(metadata, controller.phase(), !controller.resources_released(), None))
            }
            Err(reason) => { let code = controller_error(reason); *lock(&run.error) = Some(code); Err(code) }
        }
    }
}
impl Drop for AccountManager {
    fn drop(&mut self) {
        for slot in lock(&self.slots).values() { if let Some(run) = lock(&slot.run).as_ref() { run.retire(); } }
    }
}

const MAX_PENDING_MESSAGES: usize = 16;
const MAX_PENDING_MESSAGE_BYTES: usize = 1024 * 1024;
enum PendingMessage {
    Direct(super::arc_protocol::IncomingProtocolMessage),
    Batch(super::offline_backend::ScheduledIncomingMessage),
}
impl PendingMessage {
    fn byte_length(&self) -> usize {
        let message = match self { Self::Direct(message) => message.message(), Self::Batch(message) => message.message() };
        message.body.len() + message.from.as_ref().map_or(0, Vec::len) + message.time.as_ref().map_or(0, Vec::len)
    }
}
struct PendingDelivery { owner: super::session::AttemptOwner, scope: super::rpc_pending::RpcConnectionScope, message: PendingMessage }
async fn handle_message(run: &ManagedRun, controller: &mut HostingController, delivery: PendingDelivery,
    pending: &mut VecDeque<PendingDelivery>) {
    if !run.accepting.load(Ordering::SeqCst) { return; }
    if controller.phase() != Phase::Hosted { *lock(&run.error) = Some("app_account_inbox_not_hosted"); run.retire(); return; }
    if controller.resource_owner().as_ref() != Some(&delivery.owner)
        || !run.backend.original_scope(&delivery.owner).is_some_and(|scope| scope.matches(&delivery.scope)) { return; }
    let bytes = pending.iter().map(|message| message.message.byte_length()).sum::<usize>();
    if pending.len() >= MAX_PENDING_MESSAGES || bytes.saturating_add(delivery.message.byte_length()) > MAX_PENDING_MESSAGE_BYTES {
        *lock(&run.error) = Some("app_account_inbox_backpressure"); run.retire(); return;
    }
    let binding = match run.backend.original_incoming_binding(&delivery.owner, &delivery.scope) {
        Some(binding) => binding, None => { *lock(&run.error) = Some("app_account_inbox_scope_invalid"); run.retire(); return; }
    };
    let context = match InboxScope::original(&run.line_id, run.revision, &run.inbox_nonce, &delivery.owner, &delivery.scope)
        .and_then(|context| context.with_receiver(binding)) {
        Ok(context) => context, Err(code) => { *lock(&run.error) = Some(code); run.retire(); return; }
    };
    let result = match &delivery.message {
        PendingMessage::Direct(message) => context.accept_direct(&run.database, &run.backend, message, &run.accepting).await,
        PendingMessage::Batch(message) => context.accept_batch(&run.database, &run.backend, message, &run.accepting).await,
    };
    match result {
        Ok(Completion::Recorded | Completion::Retired) => {},
        // ONLY command-admission pressure is retryable. The borrowed original
        // message/ticket remains here and duplicate commit emits no new event.
        Ok(Completion::Backpressure) => pending.push_back(delivery),
        // Data already committed and the original response was accepted. A
        // failed SQL status update cannot replay an already-consumed ticket.
        Ok(Completion::StatusUnrecorded) => { *lock(&run.error) = Some("app_account_inbox_reply_status_unrecorded"); run.retire(); },
        Err(code) => {
            if run.accepting.load(Ordering::SeqCst) { *lock(&run.error) = Some(code); run.retire(); }
        },
    }
}

async fn consume_events(run: std::sync::Weak<ManagedRun>, mut events: mpsc::Receiver<BackendEvent>) {
    let mut pending = VecDeque::new();
    let mut timer = tokio::time::interval(Duration::from_millis(250));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(run) = run.upgrade() else { return; };
                let Some(event) = event else {
                    if run.accepting.load(Ordering::SeqCst) { *lock(&run.error) = Some("app_account_disconnected"); run.retire(); }
                    return;
                };
                // Stop still drains this bounded queue, without callbacks or
                // consumer ACKs, while the native driver completes its own IO.
                let terminal = matches!(&event.payload, BackendEventPayload::Terminal(_));
                if !run.accepting.load(Ordering::SeqCst) && (!terminal || run.stopping.load(Ordering::SeqCst)) { continue; }
                let mut controller = run.controller.lock().await;
                if !run.accepting.load(Ordering::SeqCst) && (!terminal || run.stopping.load(Ordering::SeqCst)) { continue; }
                if !terminal {
                    if let Err(code) = run.configuration_current() {
                        if let Some(owner) = controller.resource_owner() {
                            let _ = controller.on_callback(&owner, Callback::Failure(BackendFailure::Disconnected), unix_now()).await;
                        }
                        *lock(&run.error) = Some(code); run.retire(); continue;
                    }
                }
                let scope = event.connection_scope();
                if controller.resource_owner().as_ref() != Some(event.owner())
                    || !run.backend.original_scope(event.owner()).is_some_and(|original| original.matches(&scope)) { continue; }
                let owner = event.owner().clone();
                match event.payload {
                    BackendEventPayload::Callback(callback) => {
                        if matches!(callback.callback, Callback::Channels { .. }) {
                            *lock(&run.error) = Some("app_account_channels_unverified"); run.retire(); continue;
                        }
                        if let Err(reason) = controller.on_callback(&owner, callback.callback, unix_now()).await {
                            *lock(&run.error) = Some(controller_error(reason)); run.retire();
                        }
                    }
                    BackendEventPayload::Terminal(reason) => {
                        let _ = controller.on_callback(&owner, Callback::Failure(reason), unix_now()).await;
                        let mut error = lock(&run.error);
                        if reason != BackendFailure::Disconnected || error.is_none() { *error = Some(failure_code(reason)); }
                        drop(error); run.retire();
                    }
                    BackendEventPayload::BatchMessage(message) => {
                        controller.tick(unix_now(), Instant::now());
                        handle_message(&run, &mut controller, PendingDelivery { owner, scope, message: PendingMessage::Batch(message) }, &mut pending).await;
                    }
                    BackendEventPayload::DirectMessage(message) => {
                        controller.tick(unix_now(), Instant::now());
                        handle_message(&run, &mut controller, PendingDelivery { owner, scope, message: PendingMessage::Direct(message) }, &mut pending).await;
                    }
                }
            }
            _ = timer.tick() => {
                let Some(run) = run.upgrade() else { return; };
                if !run.accepting.load(Ordering::SeqCst) { pending.clear(); continue; }
                let mut controller = run.controller.lock().await;
                if run.accepting.load(Ordering::SeqCst) {
                    if let Err(code) = run.configuration_current() {
                        if let Some(owner) = controller.resource_owner() {
                            let _ = controller.on_callback(&owner, Callback::Failure(BackendFailure::Disconnected), unix_now()).await;
                        }
                        *lock(&run.error) = Some(code); run.retire();
                    } else { controller.tick(unix_now(), Instant::now()); }
                    if run.accepting.load(Ordering::SeqCst) {
                        if let Some(message) = pending.pop_front() { handle_message(&run, &mut controller, message, &mut pending).await; }
                    } else { pending.clear(); }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "account_runtime_tests.rs"]
mod tests;
