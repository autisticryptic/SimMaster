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
    account_sms::{self, AccountSmsMessage},
    account_runs::{self, RunToken},
    account_notifications::NoticePlanner,
    account_calls::{CallService, CallView},
    controller::{Callback, ControllerFailure, HostingController},
    runtime_backend::{BackendEvent, BackendEventPayload, NativeHostingBackend},
    runtime_connector::TcpRuntimeConnector,
    session::{AttemptOwner, BackendFailure, Phase},
    rpc_pending::RpcConnectionScope,
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
    /// Runtime policy observation only; stopped metadata never decodes secrets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_on_disconnect: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<&'static str>,
    /// Supervisor observation only: 1-based fresh-start attempt of this run
    /// chain, and scheduled/exhausted/not_retryable/start_rejected/unsupervised.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_error: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<&'static str>,
}
fn view(metadata: AccountMetadata, phase: Phase, cleanup: bool, error: Option<&'static str>) -> AccountView {
    AccountView { account: metadata, line_kind: "app_account", access: "application_data",
        phase: phase_name(phase), io_cleanup_required: cleanup,
        // A hosted SDK acknowledgement is not verified SMS/call/media support.
        incoming_call_ready: false, incoming_message_ready: false, outbound_authorized: false,
        provider_availability: "unsupported", manual_start_required: true, cleanup_on_disconnect: None, cleanup_error: None,
        reconnect_attempt: None, reconnect_state: None, reconnect_error: None, last_error: error }
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

pub(super) struct RunQuiescence { token: Arc<RunToken> }
impl RunQuiescence {
    pub(super) fn token(&self) -> &RunToken { &self.token }
}
#[cfg(test)]
pub(super) fn test_quiescence(token: RunToken) -> RunQuiescence { RunQuiescence { token: Arc::new(token) } }

/// Runtime-only evidence that the exact native Smgw receiver and durable
/// consumer belong to the current hosted owner. Never restored from storage.
struct MessagingReadiness { owner: AttemptOwner, scope: RpcConnectionScope }
#[derive(Clone, Copy)]
struct TerminalObservation { phase: Phase, error: &'static str }

struct ManagedRun {
    line_id: String,
    database: Arc<Database>,
    revision: u64,
    inbox_nonce: String,
    run_token: Arc<RunToken>,
    finalized: AtomicBool,
    notifications: Option<Arc<dyn NoticePlanner>>,
    sms_supported: bool,
    credential_expiry: Option<u64>,
    cleanup_on_disconnect: bool,
    reconnect: Option<super::account_config::ReconnectPolicy>,
    /// 1 for an explicit start; carried and incremented only by the supervisor,
    /// reset after the run's own successful original HostingAck.
    reconnect_attempt: u32,
    hosted_once: AtomicBool,
    supervisor: std::sync::Weak<AccountManager>,
    reconnect_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    reconnect_state: Mutex<Option<&'static str>>,
    reconnect_error: Mutex<Option<&'static str>>,
    cleanup_work: AsyncMutex<()>,
    cleanup_changed: tokio::sync::Notify,
    cleanup_task: AsyncMutex<Option<tokio::task::JoinHandle<()>>>,
    manual_cleanup: AtomicBool,
    terminal_observation: Mutex<Option<TerminalObservation>>,
    cleanup_error: Mutex<Option<&'static str>>,
    messaging_readiness: Mutex<Option<MessagingReadiness>>,
    calls: Option<Arc<CallService>>,
    controller: AsyncMutex<HostingController>,
    backend: Arc<NativeHostingBackend>,
    accepting: AtomicBool,
    stopping: AtomicBool,
    error: Mutex<Option<&'static str>>,
    task: AsyncMutex<Option<tokio::task::JoinHandle<()>>>,
}
impl ManagedRun {
    fn configuration_current(&self) -> Result<(), &'static str> {
        let metadata = self.database.with_conn(|conn| account_store::metadata(conn, &self.line_id))?
            .ok_or("app_account_not_found")?;
        if metadata.revision != self.revision { return Err("app_account_revision_conflict"); }
        if !self.database.with_conn(|conn|account_runs::runtime_matches(conn,&self.run_token))? {
            return Err("app_account_runtime_owner_unconfirmed");
        }
        Ok(())
    }
    fn install_message_readiness(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) {
        let verified_receiver = self.backend.original_incoming_binding(owner, scope)
            .is_some_and(|binding| binding.receiver_kind == super::smgw_text::SMGW_RECEIVER);
        if self.sms_supported && self.accepting.load(Ordering::SeqCst) && verified_receiver {
            *lock(&self.messaging_readiness) = Some(MessagingReadiness { owner: owner.clone(), scope: scope.clone() });
        }
    }
    fn message_ready(&self, now: u64) -> bool {
        if !self.sms_supported || !self.accepting.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst)
            || self.finalized.load(Ordering::SeqCst) || self.credential_expiry.is_some_and(|at| at <= now)
            || self.configuration_current().is_err() { return false; }
        let consumer_live = self.task.try_lock().is_ok_and(|task| task.as_ref().is_some_and(|task| !task.is_finished()));
        if !consumer_live { return false; }
        let readiness = lock(&self.messaging_readiness);
        readiness.as_ref().is_some_and(|original| original.scope.is_open()
            && self.backend.original_incoming_binding(&original.owner, &original.scope)
                .is_some_and(|binding| binding.receiver_kind == super::smgw_text::SMGW_RECEIVER))
    }
    fn call_ready(&self, now: u64) -> bool {
        if !self.accepting.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst)
            || self.finalized.load(Ordering::SeqCst) || self.credential_expiry.is_some_and(|at| at<=now)
            || self.configuration_current().is_err() { return false; }
        self.task.try_lock().is_ok_and(|task| task.as_ref().is_some_and(|task| !task.is_finished()))
            && self.calls.as_ref().is_some_and(|calls| calls.ready())
    }
    /// User stop/start/shutdown owns the account: no supervised start may follow.
    fn cancel_reconnect(&self) {
        if let Some(task) = lock(&self.reconnect_task).take() { task.abort(); }
        let mut state = lock(&self.reconnect_state);
        if *state == Some("scheduled") { *state = None; }
    }
    fn retire(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        self.cleanup_changed.notify_waiters();
        *lock(&self.messaging_readiness) = None;
        if let Some(calls) = &self.calls { calls.retire(); }
        self.backend.retire_current();
    }
}
impl Drop for ManagedRun {
    fn drop(&mut self) {
        self.retire();
        if let Some(task) = self.task.get_mut().take() { task.abort(); }
        if let Some(task) = self.cleanup_task.get_mut().take() { task.abort(); }
        // A pending fresh start belongs to this run chain; never let it outlive
        // the retired run. (Aborting the currently running task is harmless.)
        if let Some(task) = lock(&self.reconnect_task).take() { task.abort(); }
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
    notifications: Option<Arc<dyn NoticePlanner>>,
    shutting_down: AtomicBool,
    shutdown_changed: tokio::sync::Notify,
    slots: Mutex<BTreeMap<String, Arc<AccountSlot>>>,
    /// Set only when an owning Arc explicitly enables supervision. Without it
    /// an opted-in policy is reported as unsupervised, never silently ignored.
    supervisor: Mutex<std::sync::Weak<AccountManager>>,
    /// Exclusive OS lock over this database for the process lifetime. Only its
    /// holder tags leases and may recover leases tagged by exited instances.
    claim: Mutex<Option<ProcessClaim>>,
}
struct ProcessClaim { _lock: std::fs::File, instance: String }
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct RecoveryReport { pub recovered_runs: usize, pub uncertain_notifications: usize, pub legacy_unconfirmed_leases: usize }
impl fmt::Debug for AccountManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AccountManager(<redacted>)") }
}
impl AccountManager {
    pub fn new(database: Arc<Database>) -> Self { Self::with_factory(database, Arc::new(ExplicitNativeFactory)) }
    pub(crate) fn with_notifications(database:Arc<Database>,notifications:Arc<dyn NoticePlanner>)->Self {
        let mut manager=Self::new(database);manager.notifications=Some(notifications);manager
    }
    #[cfg(test)]
    pub(crate) fn with_factory_and_notifications(database:Arc<Database>,factory:Arc<dyn AccountBackendFactory>,notifications:Arc<dyn NoticePlanner>)->Self {
        let mut manager=Self::with_factory(database,factory);manager.notifications=Some(notifications);manager
    }
    pub(crate) fn with_factory(database: Arc<Database>, factory: Arc<dyn AccountBackendFactory>) -> Self {
        Self { database, factory, notifications: None, shutting_down:AtomicBool::new(false), shutdown_changed:tokio::sync::Notify::new(), slots: Mutex::new(BTreeMap::new()),
            supervisor: Mutex::new(std::sync::Weak::new()), claim: Mutex::new(None) }
    }
    /// The process owner enables the bounded reconnect supervisor after wrapping
    /// the manager. No account is started by this call.
    pub(crate) fn enable_reconnect_supervisor(self: &Arc<Self>) { *lock(&self.supervisor) = Arc::downgrade(self); }
    /// Take the non-blocking exclusive flock beside data.db, then recover runs
    /// tagged by exited instances. Every tagging process held this same lock
    /// while alive, so acquiring it proves those holders exited; no PID/age
    /// inference. Untagged (legacy) leases stay `runtime_owner_unconfirmed`.
    /// Must run before this process starts accounts or the notification worker.
    pub(crate) fn claim_process_ownership(&self, lock_path: &std::path::Path) -> Result<RecoveryReport, &'static str> {
        use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};
        if lock(&self.claim).is_some() { return Err("app_account_process_claim_present"); }
        if lock(&self.slots).values().any(|slot| lock(&slot.run).is_some()) { return Err("app_account_claim_after_start"); }
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(lock_path).map_err(|_| "app_account_process_lock_failed")?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("app_account_process_lock_owned");
        }
        let instance = local_nonce()?;
        let (report, events) = self.database.with_conn(|conn| {
            let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate).map_err(|_| "app_account_run_storage_failed")?;
            let now = chrono::Utc::now().to_rfc3339(); let mut events = Vec::new();
            let leases = account_runs::orphaned_leases(&tx, &instance)?;
            for lease in &leases { events.extend(account_runs::release_orphan(&tx, lease, &now)?); }
            let legacy: i64 = tx.query_row("SELECT COUNT(*) FROM app_voice_runtime_leases WHERE holder IS NULL", [], |row| row.get(0))
                .map_err(|_| "app_account_run_storage_failed")?;
            let uncertain = super::account_notifications::recover_orphaned_sending(&tx)?;
            tx.commit().map_err(|_| "app_account_run_storage_failed")?;
            Ok((RecoveryReport { recovered_runs: leases.len(), uncertain_notifications: uncertain, legacy_unconfirmed_leases: legacy as usize }, events))
        })?;
        for event in events { self.database.publish_committed_app_event(event); }
        *lock(&self.claim) = Some(ProcessClaim { _lock: file, instance });
        Ok(report)
    }
    /// Internal proof that this process holds the original database OS lock.
    /// Never an API response, remote device lease or account-readiness grant.
    pub(crate) fn login_process_owner(&self) -> Option<String> {
        lock(&self.claim).as_ref().map(|claim| claim.instance.clone())
    }
    pub(crate) fn login_preflight_create(&self) -> Result<(), &'static str> {
        if self.shutting_down.load(Ordering::SeqCst) { return Err("app_account_shutting_down"); }
        let accounts = self.database.with_conn(account_store::list)?;
        if accounts.len() >= account_store::MAX_ACCOUNTS as usize { return Err("app_account_limit"); }
        Ok(())
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
        let Some(run) = run else {
            if self.database.with_conn(|conn|account_runs::ownership_pending(conn,&metadata.line_id))? {
                return Ok(view(metadata,Phase::Disconnected,true,Some("app_account_runtime_owner_unconfirmed")));
            }
            return Ok(view(metadata, Phase::Unconfigured, false, None));
        };
        if run.revision != metadata.revision { return Err("app_account_revision_conflict"); }
        if run.finalized.load(Ordering::SeqCst) && self.database.with_conn(|conn|account_runs::ownership_pending(conn,&metadata.line_id))? {
            return Ok(view(metadata,Phase::Disconnected,true,Some("app_account_runtime_owner_unconfirmed")));
        }
        let mut controller = run.controller.lock().await;
        controller.tick(now, Instant::now());
        let terminal = *lock(&run.terminal_observation);
        let observed_phase = terminal.map_or(controller.phase(), |terminal| terminal.phase);
        let error = terminal.map(|terminal| terminal.error).or_else(|| {
            if controller.phase() == Phase::CredentialExpired { Some("app_account_credential_expired") }
            else { *lock(&run.error) }
        });
        let mut snapshot = view(metadata, observed_phase, !controller.resources_released() || !run.finalized.load(Ordering::SeqCst), error);
        snapshot.cleanup_on_disconnect = Some(run.cleanup_on_disconnect);
        snapshot.cleanup_error = *lock(&run.cleanup_error);
        if run.reconnect.is_some() {
            snapshot.reconnect_attempt = Some(run.reconnect_attempt);
            snapshot.reconnect_state = *lock(&run.reconnect_state);
            snapshot.reconnect_error = *lock(&run.reconnect_error);
        }
        // SDK hosting success alone is not enough: only the exact supported
        // receiver + live original durable consumer publishes SMS readiness.
        if controller.phase() == Phase::Hosted && run.message_ready(now) {
            snapshot.incoming_message_ready = true;
            snapshot.provider_availability = "available";
        }
        if controller.phase() == Phase::Hosted && run.call_ready(now) {
            snapshot.incoming_call_ready = true;
            snapshot.provider_availability = "available";
        }
        Ok(snapshot)
    }
    /// Synchronous provider observation, not an IO/action grant. Configuration
    /// revision, local run lease, expiry and original physical scope are checked
    /// on every query; old successes and prior-process rows cannot restore it.
    pub(crate) fn messaging_availability(&self, line_id: &str) -> crate::connectivity::app_voice::registry::ProviderAvailability {
        use crate::connectivity::app_voice::registry::ProviderAvailability as Availability;
        if !account_store::valid_line_id(line_id) { return Availability::Unsupported; }
        let metadata = match self.metadata(line_id) {
            Ok(metadata) => metadata, Err("app_account_not_found") => return Availability::NotConfigured,
            Err(_) => return Availability::Unknown,
        };
        let run = lock(&self.slots).get(line_id).and_then(|slot| lock(&slot.run).clone());
        let Some(run) = run else { return Availability::NotConfigured; };
        if metadata.revision != run.revision || run.configuration_current().is_err() { return Availability::Unknown; }
        if !run.sms_supported { return Availability::Unsupported; }
        if run.message_ready(unix_now()) { Availability::Available } else { Availability::NotConfigured }
    }
    pub(crate) fn call_availability(&self, line_id: &str) -> crate::connectivity::app_voice::registry::ProviderAvailability {
        use crate::connectivity::app_voice::registry::ProviderAvailability as Availability;
        if !account_store::valid_line_id(line_id) { return Availability::Unsupported; }
        let metadata = match self.metadata(line_id) {
            Ok(metadata) => metadata, Err("app_account_not_found") => return Availability::NotConfigured,
            Err(_) => return Availability::Unknown,
        };
        let run = lock(&self.slots).get(line_id).and_then(|slot| lock(&slot.run).clone());
        let Some(run) = run else { return Availability::NotConfigured; };
        if metadata.revision != run.revision || run.configuration_current().is_err() { return Availability::Unknown; }
        if run.calls.is_none() { return Availability::Unsupported; }
        if run.call_ready(unix_now()) { Availability::Available } else { Availability::NotConfigured }
    }
    /// Aggregate observation only; callers needing Voice or Messaging must use
    /// the corresponding capability's current original-owner proof separately.
    pub(crate) fn inbound_availability(&self, line_id: &str) -> crate::connectivity::app_voice::registry::ProviderAvailability {
        use crate::connectivity::app_voice::registry::ProviderAvailability as Availability;
        let capabilities = [self.messaging_availability(line_id), self.call_availability(line_id)];
        if capabilities.contains(&Availability::Available) { Availability::Available }
        else if capabilities.contains(&Availability::Unknown) { Availability::Unknown }
        else if capabilities.contains(&Availability::NotConfigured) { Availability::NotConfigured }
        else { Availability::Unsupported }
    }
    pub fn reserve_outbound(&self, line_id: &str, revision: u64) -> Result<(), &'static str> {
        self.check_revision(line_id, revision)?;
        // Retained interface only. Neither receive readiness nor imported
        // credentials can authorize app-account billing or IMS/CS fallback.
        Err("app_account_outbound_unsupported")
    }
    pub async fn calls(&self, line_id: &str) -> Result<Vec<CallView>, &'static str> {
        let slot = self.slot(line_id)?; let run = lock(&slot.run).clone();
        let Some(run) = run else { return Ok(Vec::new()); }; run.configuration_current()?;
        run.calls.as_ref().map_or(Ok(Vec::new()), |calls| calls.list())
    }
    pub async fn call(&self, line_id: &str, call_id: &str) -> Result<CallView, &'static str> {
        let slot = self.slot(line_id)?; let run = lock(&slot.run).clone().ok_or("app_account_call_not_found")?;
        run.configuration_current()?; run.calls.as_ref().ok_or("app_account_call_runtime_unavailable")?.get(call_id)
    }
    async fn active_calls(&self, line_id: &str, revision: u64, now: u64) -> Result<Arc<CallService>, &'static str> {
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        self.check_revision(line_id, revision)?;
        let run = lock(&slot.run).clone().ok_or("app_account_call_runtime_unavailable")?;
        run.configuration_current()?; let mut controller = run.controller.lock().await; controller.tick(now, Instant::now());
        if !run.accepting.load(Ordering::SeqCst) || controller.phase() != Phase::Hosted { return Err("app_account_call_runtime_unavailable"); }
        run.calls.as_ref().cloned().ok_or("app_account_call_runtime_unavailable")
    }
    pub async fn answer_call(&self, line_id: &str, revision: u64, call_id: &str, sdp: Vec<u8>, now: u64) -> Result<CallView, &'static str> {
        let calls = self.active_calls(line_id, revision, now).await?;
        calls.accept(call_id, sdp).await?.await
    }
    pub async fn hangup_call(&self, line_id: &str, revision: u64, call_id: &str, now: u64) -> Result<CallView, &'static str> {
        let calls = self.active_calls(line_id, revision, now).await?;
        calls.hangup(call_id).await?.await
    }
    pub async fn call_trunk_status(&self, line_id: &str) -> Result<crate::services::trunk::runtime::TrunkRuntimeStatus, &'static str> {
        let slot = self.slot(line_id)?; let run = lock(&slot.run).clone().ok_or("app_account_call_runtime_unavailable")?;
        run.configuration_current()?; let calls = run.calls.as_ref().ok_or("app_account_call_runtime_unavailable")?;
        Ok(calls.trunk_status().await)
    }
    #[cfg(test)]
    pub(super) fn original_call_service(&self,line_id:&str)->Result<Arc<CallService>,&'static str>{
        let slot=self.slot(line_id)?;let run=lock(&slot.run).clone().ok_or("app_account_call_runtime_unavailable")?;
        run.configuration_current()?;run.calls.as_ref().cloned().ok_or("app_account_call_runtime_unavailable")
    }
    pub(crate) async fn call_operator(&self, line_id: &str) -> Result<crate::services::trunk::operator::OperatorLink, &'static str> {
        let slot = self.slot(line_id)?; let run = lock(&slot.run).clone().ok_or("app_account_call_runtime_unavailable")?;
        run.configuration_current()?; Ok(run.calls.as_ref().ok_or("app_account_call_runtime_unavailable")?.operator())
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
    pub fn messages(&self, line_id: &str, limit: usize) -> Result<Vec<AccountSmsMessage>, &'static str> {
        self.metadata(line_id)?;
        self.database.with_conn(|conn| account_sms::list(conn, line_id, limit))
    }
    pub fn notification_status(&self,line_id:&str,limit:usize)->Result<Vec<super::account_notifications::NoticeStatus>,&'static str>{
        self.metadata(line_id)?;
        self.database.with_conn(|conn|super::account_notifications::metadata(conn,line_id,limit))
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
    async fn require_released(&self,line_id:&str,slot: &AccountSlot) -> Result<(), &'static str> {
        let run = lock(&slot.run).clone();
        if let Some(run) = run {
            if !run.controller.lock().await.resources_released() || !run.finalized.load(Ordering::SeqCst) {
                return Err("app_account_stop_before_replace");
            }
            Self::join_automatic_cleanup(&run).await?;
        }
        if self.database.with_conn(|conn|account_runs::ownership_pending(conn,line_id))? {
            return Err("app_account_stop_before_replace");
        }
        Ok(())
    }
    pub async fn replace(&self, line_id: &str, expected_revision: u64, config: AccountConfig, now: u64)
        -> Result<AccountView, &'static str> {
        config.validate(now)?;
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        self.check_revision(line_id, expected_revision)?; self.require_released(line_id,&slot).await?;
        let metadata = self.database.with_conn(|conn| account_store::replace(conn, line_id, expected_revision, &config, now))?;
        if let Some(run) = lock(&slot.run).take() { run.retire(); }
        Ok(view(metadata, Phase::Unconfigured, false, None))
    }
    pub async fn delete(&self, line_id: &str, expected_revision: u64) -> Result<(), &'static str> {
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        self.check_revision(line_id, expected_revision)?; self.require_released(line_id,&slot).await?;
        if !self.database.with_conn(|conn| account_store::delete(conn, line_id, expected_revision))? {
            return Err("app_account_not_found");
        }
        if let Some(run) = lock(&slot.run).take() { run.retire(); }
        let mut slots = lock(&self.slots);
        if slots.get(line_id).is_some_and(|current| Arc::ptr_eq(current, &slot)) { slots.remove(line_id); }
        Ok(())
    }
    pub async fn start(&self, line_id: &str, expected_revision: u64, now: u64) -> Result<AccountView, &'static str> {
        self.start_with(line_id, expected_revision, now, None).await
    }
    /// `previous` is only the supervisor's own retired run: the fresh start is
    /// abandoned if the user stopped/replaced/deleted it, or another run exists.
    async fn start_with(&self, line_id: &str, expected_revision: u64, now: u64,
        previous: Option<(&Arc<ManagedRun>, u32)>) -> Result<AccountView, &'static str> {
        if self.shutting_down.load(Ordering::SeqCst){return Err("app_account_shutting_down");}
        let slot = self.slot(line_id)?; let _operation = slot.operation.lock().await;
        if self.shutting_down.load(Ordering::SeqCst){return Err("app_account_shutting_down");}
        if let Some((old, _)) = previous {
            let current = lock(&slot.run).clone();
            if old.manual_cleanup.load(Ordering::SeqCst) || !current.is_some_and(|run| Arc::ptr_eq(&run, old)) {
                return Err("app_account_reconnect_superseded");
            }
        } else if let Some(old) = lock(&slot.run).clone() {
            // An explicit user start owns this account again: cancel any
            // pending supervised start of the previous, finalized run.
            old.cancel_reconnect();
        }
        let metadata = self.check_revision(line_id, expected_revision)?;
        self.require_released(line_id,&slot).await?;
        let StoredAccount { metadata: stored, config } = self.database.with_conn(|conn| account_store::load(conn, line_id))?
            .ok_or("app_account_not_found")?;
        if stored.revision != metadata.revision { return Err("app_account_revision_conflict"); }
        config.validate(now)?;
        let inbox_nonce = local_nonce()?;
        let sms_supported = config.profile.incoming.as_ref().is_some_and(|binding| binding.receiver_kind == super::smgw_text::SMGW_RECEIVER);
        let credential_expiry = config.credentials.sdk.expires_at_unix_seconds;
        let cleanup_on_disconnect = config.cleanup_on_disconnect;
        let reconnect = config.reconnect;
        let reconnect_attempt = previous.map_or(1, |(_, attempt)| attempt);
        let call_settings = config.runtime_calls()?;
        let (backend, events) = self.factory.build(&config)?;
        let mut controller = HostingController::new(line_id.to_owned(), Some(backend.clone()));
        controller.import(config.credentials, now).map_err(controller_error)?;
        let holder = lock(&self.claim).as_ref().map(|claim| claim.instance.clone());
        let run_token=Arc::new(self.database.with_conn(|conn|account_runs::register_runtime_held(conn,line_id,metadata.revision,&inbox_nonce,holder.as_deref()))?);
        let calls = call_settings.map(|settings| CallService::new(self.database.clone(),run_token.clone(),backend.clone(),settings,credential_expiry));
        let run = Arc::new(ManagedRun { line_id: line_id.to_owned(), database: self.database.clone(), revision: metadata.revision,
            inbox_nonce, run_token, finalized: AtomicBool::new(false), notifications:self.notifications.clone(),
            sms_supported, credential_expiry, cleanup_on_disconnect,
            reconnect, reconnect_attempt, hosted_once: AtomicBool::new(false), supervisor: lock(&self.supervisor).clone(),
            reconnect_task: Mutex::new(None), reconnect_state: Mutex::new(None), reconnect_error: Mutex::new(None),
            cleanup_work: AsyncMutex::new(()), cleanup_changed: tokio::sync::Notify::new(), cleanup_task: AsyncMutex::new(None),
            manual_cleanup: AtomicBool::new(false), terminal_observation: Mutex::new(None), cleanup_error: Mutex::new(None),
            messaging_readiness: Mutex::new(None), calls, controller: AsyncMutex::new(controller), backend,
            accepting: AtomicBool::new(true), stopping: AtomicBool::new(false), error: Mutex::new(None), task: AsyncMutex::new(None) });
        // Retain the owning backend BEFORE any await that can create IO. A
        // cancelled HTTP future leaves this same run available for stop/retry.
        *lock(&slot.run) = Some(run.clone());
        let mut retirement = Retirement::new(run.clone());
        let weak = Arc::downgrade(&run);
        let mut task_slot = run.task.try_lock().expect("new original event consumer");
        *task_slot = Some(tokio::spawn(consume_events(weak, events)));
        drop(task_slot);
        if run.cleanup_on_disconnect {
            // This fresh slot has not escaped to a cleanup caller. Retain the
            // owning task without a suspension between spawn and publication.
            let mut cleanup = run.cleanup_task.try_lock().expect("new original cleanup owner");
            *cleanup = Some(tokio::spawn(automatic_cleanup(Arc::downgrade(&run))));
        }
        let mut controller = run.controller.lock().await;
        let started=tokio::select!{biased;
            _=self.shutdown_requested()=>{*lock(&run.error)=Some("app_account_shutting_down");return Err("app_account_shutting_down");},
            result=controller.start(now)=>result,
        };
        match started {
            Ok(_) => {
                if let Err(code) = self.check_revision(line_id, expected_revision) {
                    *lock(&run.error) = Some(code); return Err(code);
                }
                if self.shutting_down.load(Ordering::SeqCst){return Err("app_account_shutting_down");}
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
        let Some(run) = lock(&slot.run).clone() else {
            if self.database.with_conn(|conn|account_runs::ownership_pending(conn,line_id))? {return Err("app_account_cleanup_unconfirmed");}
            return Ok(view(metadata, Phase::Unconfigured, false, None));
        };
        let phase=self.cleanup_run(&run,now).await?;
        Ok(view(metadata,phase,!run.finalized.load(Ordering::SeqCst),None))
    }
    async fn cleanup_run(&self,run:&Arc<ManagedRun>,now:u64)->Result<Phase,&'static str>{
        run.manual_cleanup.store(true, Ordering::SeqCst);
        run.cancel_reconnect();
        *lock(&run.terminal_observation) = None;
        let result = Self::cleanup_original(run, now).await;
        Self::join_automatic_cleanup(run).await?;
        if result.is_ok() { *lock(&run.error) = None; *lock(&run.cleanup_error) = None; }
        result
    }
    async fn cleanup_original(run:&Arc<ManagedRun>,now:u64)->Result<Phase,&'static str>{
        // Manual stop, shutdown and the optional retirement worker serialize on
        // this ORIGINAL run, not a newly loaded account/configuration/owner.
        let _cleanup = run.cleanup_work.lock().await;
        if run.finalized.load(Ordering::SeqCst) { return Ok(run.controller.lock().await.phase()); }
        // Original retained resource owner, not current database configuration.
        run.stopping.store(true, Ordering::SeqCst);
        run.accepting.store(false, Ordering::SeqCst);
        run.cleanup_changed.notify_waiters();
        let mut retirement = Retirement::new(run.clone());
        let calls_error = if let Some(calls) = &run.calls { calls.close().await.err() } else { None };
        let calls_released = run.calls.as_ref().is_none_or(|calls| calls.resources_released());
        let mut controller = run.controller.lock().await;
        controller.tick(now, Instant::now());
        let result = controller.stop().await;
        let phase=controller.phase();
        let released=controller.resources_released() && calls_released;
        // Never join a consumer while holding the controller it may be waiting
        // for. Keep the actual JoinHandle in this run across caller cancellation.
        drop(controller);
        if released {
            let mut task_slot=run.task.lock().await;
            if let Some(task)=task_slot.as_mut() { task.abort(); let _=task.await; }
            *task_slot=None;
            drop(task_slot);
            let proof=RunQuiescence { token:run.run_token.clone() };
            if let Err(code)=run.database.with_conn(|conn|account_runs::release_and_compact(conn,&proof)) {
                *lock(&run.error)=Some(code);return Err(code);
            }
            run.finalized.store(true,Ordering::SeqCst);
            retirement.disarm();
        }
        match result {
            Ok(()) => {
                *lock(&run.error) = calls_error;
                if let Some(code) = calls_error { return Err(code); }
                if !released||!run.finalized.load(Ordering::SeqCst){return Err("app_account_cleanup_unconfirmed");}
                Ok(phase)
            }
            Err(reason) => { let code = controller_error(reason); *lock(&run.error) = Some(code); Err(code) }
        }
    }
    async fn join_automatic_cleanup(run: &ManagedRun) -> Result<(), &'static str> {
        let mut slot = run.cleanup_task.lock().await;
        if let Some(task) = slot.as_mut() {
            let result = task.await;
            slot.take(); // only after the individual original Join completed
            if result.is_err() { return Err("app_account_cleanup_unconfirmed"); }
        }
        Ok(())
    }
    async fn shutdown_requested(&self){
        loop {
            let changed=self.shutdown_changed.notified();tokio::pin!(changed);changed.as_mut().enable();
            if self.shutting_down.load(Ordering::SeqCst){return;}changed.await;
        }
    }
    /// Explicit process lifecycle owner, not a cached state/PID cleanup guess.
    /// Pending starts are cancelled, retained original actors/consumers joined.
    pub(crate) async fn shutdown_all(&self)->usize{
        use futures_util::{stream,StreamExt};
        self.shutting_down.store(true,Ordering::SeqCst);self.shutdown_changed.notify_waiters();
        let _retirement=ShutdownRetirement(self);
        let slots=lock(&self.slots).values().cloned().collect::<Vec<_>>();
        let results=stream::iter(slots).map(|slot|async move{
            let _operation=slot.operation.lock().await;
            let run=lock(&slot.run).clone();let Some(run)=run else{return false;};
            if self.cleanup_run(&run,unix_now()).await.is_err(){
                // A native negative result can coexist with released IO. A
                // second wait confirms the same owner; never resends a command.
                let _=self.cleanup_run(&run,unix_now()).await;return true;
            }
            false
        }).buffer_unordered(4).collect::<Vec<_>>().await;
        results.into_iter().filter(|failed|*failed).count()
    }
}
struct ShutdownRetirement<'a>(&'a AccountManager);
impl Drop for ShutdownRetirement<'_>{fn drop(&mut self){
    for slot in lock(&self.0.slots).values(){if let Some(run)=lock(&slot.run).as_ref(){run.retire();}}
}}
impl Drop for AccountManager {
    fn drop(&mut self) {
        for slot in lock(&self.slots).values() { if let Some(run) = lock(&slot.run).as_ref() { run.retire(); } }
    }
}

/// One bounded cleanup attempt after this live manager's own run retires.
/// Not itself a reconnect/renewal worker or prior-process takeover proof; an
/// opted-in policy may only schedule a fresh start after this cleanup finalized.
async fn automatic_cleanup(weak: std::sync::Weak<ManagedRun>) {
    let Some(run) = weak.upgrade() else { return; };
    loop {
        let changed = run.cleanup_changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
        if !run.accepting.load(Ordering::SeqCst) || run.finalized.load(Ordering::SeqCst) { break; }
        changed.await;
    }
    if run.finalized.load(Ordering::SeqCst) { return; }
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let mut controller = run.controller.lock().await;
        controller.tick(unix_now(), Instant::now());
        let phase = match controller.phase() {
            Phase::CredentialExpired => Phase::CredentialExpired,
            Phase::AuthenticationFailed => Phase::AuthenticationFailed,
            _ => Phase::Disconnected,
        };
        let error = if phase == Phase::CredentialExpired { "app_account_credential_expired" }
            else { (*lock(&run.error)).unwrap_or("app_account_disconnected") };
        drop(controller);
        {
            let mut terminal = lock(&run.terminal_observation);
            if !run.manual_cleanup.load(Ordering::SeqCst) { *terminal = Some(TerminalObservation { phase, error }); }
        }
        let first = AccountManager::cleanup_original(&run, unix_now()).await;
        if first.is_err() && !run.finalized.load(Ordering::SeqCst) {
            // As with process shutdown, a native negative may precede confirmed
            // release. Retry only the same retained cleanup once, never start,
            // rebind or replay an already-attempted unregister/Release/logout.
            AccountManager::cleanup_original(&run, unix_now()).await
        } else { first }
    }).await.unwrap_or(Err("app_account_cleanup_unconfirmed"));
    // A negative teardown verdict remains visible separately from the terminal
    // cause, even when local IO release and durable finalization are confirmed.
    if let Err(code) = result { *lock(&run.cleanup_error) = Some(code); return; }
    // Only after the original run finalized (actual Joins + fence release):
    // never a second owner beside an uncleaned run.
    schedule_reconnect(&run);
}

/// Error codes from which a FRESH login on the same stored revision may succeed.
/// Authentication/hosting rejection, expiry, revision/owner, storage, channel or
/// protocol failures need user action and are never retried automatically.
fn reconnect_retryable(code: &str) -> bool {
    matches!(code, "app_account_disconnected" | "app_account_backend_unavailable" | "app_account_operation_timeout")
}
fn schedule_reconnect(run: &Arc<ManagedRun>) {
    let Some(policy) = run.reconnect else { return; };
    if run.manual_cleanup.load(Ordering::SeqCst) || !run.finalized.load(Ordering::SeqCst) { return; }
    let terminal = *lock(&run.terminal_observation);
    let retryable = terminal.is_some_and(|t| t.phase == Phase::Disconnected && reconnect_retryable(t.error))
        && run.credential_expiry.is_none_or(|at| at > unix_now());
    if !retryable { *lock(&run.reconnect_state) = Some("not_retryable"); return; }
    // A run that reached its own HostingAck starts a new failure series.
    let next = if run.hosted_once.load(Ordering::SeqCst) { 1 } else { run.reconnect_attempt.saturating_add(1) };
    if next > policy.max_attempts { *lock(&run.reconnect_state) = Some("exhausted"); return; }
    let Some(manager) = run.supervisor.upgrade() else { *lock(&run.reconnect_state) = Some("unsupervised"); return; };
    if manager.shutting_down.load(Ordering::SeqCst) { return; }
    let delay = policy.delay(next);
    let (weak_manager, weak_run) = (Arc::downgrade(&manager), Arc::downgrade(run));
    let mut task = lock(&run.reconnect_task);
    *lock(&run.reconnect_state) = Some("scheduled");
    *task = Some(tokio::spawn(reconnect_after(weak_manager, weak_run, next, delay)));
}
async fn reconnect_after(manager: std::sync::Weak<AccountManager>, run: std::sync::Weak<ManagedRun>, attempt: u32, delay: Duration) {
    tokio::time::sleep(delay).await;
    let (Some(manager), Some(old)) = (manager.upgrade(), run.upgrade()) else { return; };
    if manager.shutting_down.load(Ordering::SeqCst) || old.manual_cleanup.load(Ordering::SeqCst) { return; }
    // Same stored revision as the retired original run; a replaced config is a
    // different user decision and fails the revision check, never rebinds.
    match manager.start_with(&old.line_id, old.revision, unix_now(), Some((&old, attempt))).await {
        Ok(_) => { *lock(&old.reconnect_state) = Some("restarted"); }
        Err("app_account_reconnect_superseded") => { *lock(&old.reconnect_state) = None; }
        Err(code) => { *lock(&old.reconnect_error) = Some(code); *lock(&old.reconnect_state) = Some("start_rejected"); }
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
    let notifications=run.notifications.as_ref().map(|planner|planner.capture()).transpose();
    let context = match InboxScope::original(&run.line_id, run.revision, &run.inbox_nonce, &delivery.owner, &delivery.scope)
        .and_then(|context| context.with_receiver(binding)).map(|context|context.with_notifications(notifications)) {
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
                        let hosted = matches!(callback.callback, Callback::HostingAck { success: true });
                        match controller.on_callback(&owner, callback.callback, unix_now()).await {
                            Ok(true) if hosted && controller.phase() == Phase::Hosted => {
                                run.hosted_once.store(true, Ordering::SeqCst);
                                run.install_message_readiness(&owner, &scope);
                                if let Some(calls) = &run.calls { if let Err(code) = calls.start(&owner,&scope).await { *lock(&run.error)=Some(code);run.retire(); } }
                            },
                            Ok(_) => {},
                            Err(reason) => { *lock(&run.error) = Some(controller_error(reason)); run.retire(); }
                        }
                    }
                    BackendEventPayload::Call(call)=>{
                        controller.tick(unix_now(), Instant::now());
                        if controller.phase() == Phase::Hosted && run.accepting.load(Ordering::SeqCst) {
                            if let Some(calls) = &run.calls {
                                if let Err(code) = calls.delivery(&call).await {
                                    *lock(&run.error)=Some(code);
                                    if call.ticket().is_open() { let _=run.backend.complete_call(&owner,&scope,call.ticket(),false,&[]).await; }
                                    run.retire();
                                }
                                continue;
                            }
                        }
                        *lock(&run.error)=Some("app_account_call_runtime_unavailable");
                        if run.backend.complete_call(&owner,&scope,call.ticket(),false,&[]).await.is_err(){run.retire();}
                    }
                    BackendEventPayload::Terminal(reason) => {
                        let _ = controller.on_callback(&owner, Callback::Failure(reason), unix_now()).await;
                        let mut error = lock(&run.error);
                        if reason != BackendFailure::Disconnected || error.is_none() { *error = Some(failure_code(reason)); }
                        drop(error); run.retire();
                    }
                    BackendEventPayload::BatchRetirement(proof) => {
                        controller.tick(unix_now(), Instant::now());
                        if controller.phase()!=Phase::Hosted || !run.accepting.load(Ordering::SeqCst) { continue; }
                        // Same original consumer FIFO as the durable commit and
                        // queued result. This event never emits another ACK.
                        if let Err(code)=super::batch_retention::apply(&run.database,&run.line_id,run.revision,
                            &run.inbox_nonce,&owner,&scope,&proof) {
                            *lock(&run.error)=Some(code);run.retire();
                        }
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
