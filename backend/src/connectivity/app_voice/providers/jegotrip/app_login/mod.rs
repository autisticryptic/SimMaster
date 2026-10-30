//! Explicit App-account login. No mobile-number verification SDK, telemetry or
//! paid outbound fallback. App business acceptance is not native readiness.
mod config;
mod crypto;
mod http;
mod store;
#[cfg(test)] mod tests;

use super::{account_runtime::AccountManager, account_config::AccountConfig};
use futures_util::FutureExt;
use crate::platform::db::Database;
use config::Deployment;
use http::{AppClient, TransportFactory, LiveFactory};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, fmt, path::PathBuf, sync::{Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::sync::{Notify, Semaphore, OwnedSemaphorePermit};
use super::credentials::Secret;

const BUDGET: Duration = Duration::from_secs(120);
const ERRORS: &[&str] = &[
    "app_login_request_invalid", "app_login_deployment_missing", "app_login_deployment_invalid",
    "app_login_storage_failed", "app_login_attempt_exists", "app_login_attempt_limit",
    "app_login_busy", "app_login_unavailable", "app_login_shutting_down", "app_login_interrupted",
    "app_login_timeout", "app_login_entropy_failed", "app_login_transport_failed",
    "app_login_response_invalid", "app_login_rate_limited", "app_login_server_rejected",
    "app_login_verification_required", "app_login_migration_required", "app_login_existing_business_action_required",
    "app_login_session_revoked", "app_login_service_maintenance", "app_login_phone_binding_required",
    "app_login_identity_mismatch", "app_login_native_configuration_invalid", "app_login_business_unconfirmed",
    "app_login_account_creation_failed", "app_login_receiver_start_failed", "app_login_internal_failed",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Error { pub code: &'static str, pub server_code: Option<u32> }
impl Error { fn new(code: &'static str) -> Self { Self { code, server_code: None } } }
fn stored_code(value: &str) -> Option<&'static str> { ERRORS.iter().copied().find(|code| *code == value) }
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }
fn now() -> i64 { chrono::Utc::now().timestamp().max(0) }
pub(crate) fn valid_attempt_id(s: &str) -> bool { s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub attempt_id: String,
    pub username: Secret,
    pub password: Secret,
    /// None selects the original generic-account form, not an inferred country.
    pub country_code: Option<String>,
    pub enable_hosting: bool,
    pub start_receiver: bool,
}
impl fmt::Debug for LoginRequest { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("LoginRequest(<redacted>)") } }
impl LoginRequest {
    fn validate(&self) -> Result<(), Error> {
        let user = self.username.expose_to_backend().trim(); let password = self.password.expose_to_backend();
        if !valid_attempt_id(&self.attempt_id) || !self.enable_hosting || user.is_empty() || user.len() > 254
            || user.chars().any(char::is_control) || password.is_empty() || password.len() > 1024
            || password.chars().any(char::is_control) { return Err(Error::new("app_login_request_invalid")); }
        if let Some(country) = &self.country_code {
            if country.is_empty() || country.len() > 4 || !country.bytes().all(|b| b.is_ascii_digit())
                || !user.bytes().all(|b| b.is_ascii_digit()) || user.len() > 20 { return Err(Error::new("app_login_request_invalid")); }
        }
        Ok(())
    }
    fn identity_key(&self) -> Vec<u8> {
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        digest.update(self.country_code.as_deref().unwrap_or("").as_bytes()); digest.update(&[0]);
        digest.update(self.username.expose_to_backend().trim().to_ascii_lowercase().as_bytes());
        digest.finish().as_ref().to_vec()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptView {
    pub attempt_id: String,
    pub phase: String,
    pub business_state: String,
    pub completed: bool,
    pub line_id: Option<String>,
    pub masked_number: Option<String>,
    pub error: Option<&'static str>,
    pub server_code: Option<u32>,
    pub created_at: i64,
    pub updated_at: i64,
}
#[derive(Debug, Serialize)]
pub struct LoginSetup {
    pub implemented: bool,
    /// File presence only; actual private configuration is checked on submission.
    pub deployment_present: bool,
    pub accepting: bool,
    pub manual_sdk_credentials_required: bool,
}
struct ActiveGuard { manager: Weak<AppLoginManager>, identity: Vec<u8>, _permit: OwnedSemaphorePermit }
impl Drop for ActiveGuard { fn drop(&mut self) { if let Some(m) = self.manager.upgrade() { lock(&m.identities).remove(&self.identity); } } }

pub struct AppLoginManager {
    database: Arc<Database>,
    accounts: Weak<AccountManager>,
    owner: Mutex<Option<String>>,
    deployment_path: Mutex<Option<PathBuf>>,
    factory: Arc<dyn TransportFactory>,
    active: Arc<Semaphore>,
    identities: Mutex<BTreeSet<Vec<u8>>>,
    tasks: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    finalization_errors: Mutex<BTreeSet<String>>,
    closed: AtomicBool,
    changed: Notify,
}
impl fmt::Debug for AppLoginManager { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppLoginManager(<redacted>)") } }
impl AppLoginManager {
    pub(crate) fn new(database: Arc<Database>, accounts: Weak<AccountManager>) -> Self {
        Self { database, accounts, owner: Mutex::new(None), deployment_path: Mutex::new(None), factory: Arc::new(LiveFactory),
            active: Arc::new(Semaphore::new(2)), identities: Mutex::new(BTreeSet::new()), tasks: Mutex::new(BTreeMap::new()),
            finalization_errors: Mutex::new(BTreeSet::new()), closed: AtomicBool::new(false), changed: Notify::new() }
    }
    /// Only after the original AccountManager acquired the database OS lock.
    /// An exited attempt becomes terminal/uncertain; it is NEVER replayed.
    pub(crate) fn initialize(&self, path: PathBuf) -> Result<(), &'static str> {
        let accounts = self.accounts.upgrade().ok_or("app_login_unavailable")?;
        let owner = accounts.login_process_owner().ok_or("app_login_unavailable")?;
        let mut current = lock(&self.owner);
        if current.is_some() { return Err("app_login_unavailable"); }
        self.database.with_conn(|conn| store::initialize(conn, &owner, now()))?;
        *lock(&self.deployment_path) = Some(path); *current = Some(owner); Ok(())
    }
    pub fn setup(&self) -> LoginSetup {
        let present = lock(&self.deployment_path).as_ref().is_some_and(|p| p.is_file());
        LoginSetup { implemented: true, deployment_present: present,
            accepting: present && lock(&self.owner).is_some() && !self.closed.load(Ordering::SeqCst), manual_sdk_credentials_required: false }
    }
    pub fn list(&self) -> Result<Vec<AttemptView>, &'static str> {
        if lock(&self.owner).is_none() { return Ok(Vec::new()); }
        let mut views = self.database.with_conn(store::list)?;
        for view in &mut views { self.observe_finalization_error(view); }
        Ok(views)
    }
    fn observe_finalization_error(&self, view: &mut AttemptView) {
        if lock(&self.finalization_errors).contains(&view.attempt_id) { view.error = Some("app_login_storage_failed"); }
    }
    pub fn get(&self, id: &str) -> Result<AttemptView, &'static str> {
        if !valid_attempt_id(id) { return Err("app_login_request_invalid"); }
        if lock(&self.owner).is_none() { return Err("app_login_unavailable"); }
        let mut view = self.database.with_conn(|conn| store::get(conn, id))?.ok_or("app_account_not_found")?;
        self.observe_finalization_error(&mut view); Ok(view)
    }
    pub(crate) fn submit(self: &Arc<Self>, request: LoginRequest) -> Result<AttemptView, Error> {
        request.validate()?;
        if self.closed.load(Ordering::SeqCst) { return Err(Error::new("app_login_shutting_down")); }
        let owner = lock(&self.owner).clone().ok_or(Error::new("app_login_unavailable"))?;
        if self.database.with_conn(|c| store::get(c, &request.attempt_id)).map_err(|_| Error::new("app_login_storage_failed"))?.is_some() {
            return Err(Error::new("app_login_attempt_exists"));
        }
        let path = lock(&self.deployment_path).clone().ok_or(Error::new("app_login_deployment_missing"))?;
        let deployment = config::read_deployment(&path)?;
        let client = self.factory.build(&deployment.http)?;
        let accounts = self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        accounts.login_preflight_create().map_err(|_| Error::new("app_login_account_creation_failed"))?;
        let permit = self.active.clone().try_acquire_owned().map_err(|_| Error::new("app_login_busy"))?;
        let identity = request.identity_key();
        if !lock(&self.identities).insert(identity.clone()) { return Err(Error::new("app_login_busy")); }
        let guard = ActiveGuard { manager: Arc::downgrade(self), identity, _permit: permit };
        let view = self.database.with_conn(|c| store::insert(c, &request.attempt_id, &owner, now()))
            .map_err(|code| Error::new(stored_code(code).unwrap_or("app_login_storage_failed")))?;
        let id = request.attempt_id.clone(); let manager = self.clone();
        // There is no await/cancellation gap between durable admission and spawn.
        let task = tokio::spawn(async move {
            let _guard = guard;
            let work = std::panic::AssertUnwindSafe(manager.run(&owner, request, deployment, client)).catch_unwind();
            let result = tokio::select! {
                biased;
                _ = manager.wait_closed() => Err(Error::new("app_login_interrupted")),
                result = tokio::time::timeout(BUDGET, work) => match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => Err(Error::new("app_login_internal_failed")),
                    Err(_) => Err(Error::new("app_login_timeout")),
                },
            };
            if manager.database.with_conn(|c| store::finish(c, &id, &owner, result.err(), now())).is_err() {
                // Observable in this owner, without claiming the durable row was
                // committed or replaying a possibly accepted business request.
                lock(&manager.finalization_errors).insert(id.clone());
            }
        });
        let mut tasks = lock(&self.tasks); tasks.retain(|_, t| !t.is_finished()); tasks.insert(view.attempt_id.clone(), task);
        Ok(view)
    }
    async fn wait_closed(&self) {
        loop {
            let notified = self.changed.notified(); tokio::pin!(notified); notified.as_mut().enable();
            if self.closed.load(Ordering::SeqCst) { return; } notified.await;
        }
    }
    fn step(&self, owner: &str, id: &str, phase: &str, business: Option<&str>) -> Result<(), Error> {
        if self.closed.load(Ordering::SeqCst) { return Err(Error::new("app_login_interrupted")); }
        self.database.with_conn(|c| store::step(c, id, owner, phase, business, now())).map_err(|_| Error::new("app_login_storage_failed"))
    }
    async fn run(&self, owner: &str, mut request: LoginRequest, deployment: Deployment, mut client: AppClient) -> Result<(), Error> {
        let id = request.attempt_id.clone();
        let id = id.as_str();
        self.step(owner, id, "authenticating", None)?;
        client.login(&request).await?;
        request.password = Secret::from_owned(String::new());
        self.step(owner, id, "reading_bound_number", None)?;
        let bound = client.bound_number(&request).await?;
        request.username = Secret::from_owned(String::new());
        self.database.with_conn(|c| store::set_mask(c, id, owner, &bound.masked(), now())).map_err(|_| Error::new("app_login_storage_failed"))?;
        self.step(owner, id, "fetching_credentials", None)?;
        let material = client.phone_credentials(&bound).await?;
        let config: AccountConfig = deployment.runtime.bind(&bound.identity(), material)?;
        config.validate(now() as u64).map_err(|_| Error::new("app_login_native_configuration_invalid"))?;
        let accounts = self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        accounts.login_preflight_create().map_err(|_| Error::new("app_login_account_creation_failed"))?;
        self.step(owner, id, "checking_business", None)?;
        // authentication_code is an App UI check, not an authority minted by us.
        // Submit the user's normal authenticated business request and honor the
        // SERVER result. Do not call or forge MobileAuth/security-phone checks.
        if client.business_enabled().await? {
            self.step(owner, id, "creating_account", Some("already_enabled"))?;
        } else {
            self.step(owner, id, "enabling_business", Some("request_uncertain"))?;
            if let Err(error) = client.enable_business().await {
                if error.server_code.is_some() {
                    self.step(owner, id, "enabling_business", Some("rejected"))?;
                }
                return Err(error);
            }
            self.step(owner, id, "confirming_business", Some("accepted_unconfirmed"))?;
            if !client.business_enabled().await? { return Err(Error::new("app_login_business_unconfirmed")); }
            self.step(owner, id, "creating_account", Some("enabled"))?;
        }
        // Remote business configuration and SQLite creation are not a distributed
        // transaction. A failure preserves the accepted/uncertain business state;
        // never close the user's business as an implicit compensation.
        let account = accounts.create(config, now() as u64).await.map_err(|_| Error::new("app_login_account_creation_failed"))?;
        let line_id = account.account.line_id.clone();
        self.database.with_conn(|c| store::set_line(c, id, owner, &line_id, now())).map_err(|_| Error::new("app_login_storage_failed"))?;
        if request.start_receiver {
            self.step(owner, id, "starting_receiver", None)?;
            accounts.start(&line_id, account.account.revision, now() as u64).await.map_err(|_| Error::new("app_login_receiver_start_failed"))?;
        }
        Ok(())
    }
    pub(crate) async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst); self.active.close(); self.changed.notify_waiters();
        let tasks = std::mem::take(&mut *lock(&self.tasks));
        for (_, task) in tasks { let _ = task.await; }
    }
}
