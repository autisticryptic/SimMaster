//! Explicit Jegotrip App authentication and optional normal business provisioning.
//! Database configuration is provider-specific; an App login is never native readiness.
mod config;
mod crypto;
mod http;
mod store;
mod settings;
mod accounts;
#[cfg(test)] mod tests;

pub(crate) use config::DatabaseConfiguration;
pub use accounts::JegotripAccountView;
use super::{account_runtime::AccountManager, account_config::AccountConfig};
use futures_util::FutureExt;
use crate::platform::db::Database;
use http::{AppClient, TransportFactory, LiveFactory};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, fmt, path::PathBuf, sync::{Arc, Mutex, Weak, atomic::{AtomicBool, Ordering}}, time::Duration};
use tokio::sync::{Notify, Semaphore, OwnedSemaphorePermit};
use super::credentials::Secret;

const BUDGET: Duration = Duration::from_secs(120);
const ERRORS: &[&str] = &[
    "app_login_request_invalid", "app_login_deployment_missing", "app_login_deployment_invalid",
    "app_login_storage_failed", "app_login_attempt_exists", "app_login_attempt_limit", "app_login_account_limit",
    "app_login_configuration_revision_conflict", "app_login_migration_conflict", "app_login_native_configuration_missing",
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

#[derive(Clone,Copy,Debug,Default,Deserialize,Serialize,PartialEq,Eq)]
#[serde(rename_all="snake_case")]
pub enum LoginIntent { Authenticate, #[default] EnableHosting }
impl LoginIntent {
    fn as_str(self)->&'static str {match self {Self::Authenticate=>"authenticate",Self::EnableHosting=>"enable_hosting"}}
    fn parse(value:&str)->Option<Self> {match value {"authenticate"=>Some(Self::Authenticate),"enable_hosting"=>Some(Self::EnableHosting),_=>None}}
}

/// Historical API contract: explicit consent to the complete hosting operation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub attempt_id: String,
    pub username: Secret,
    pub password: Secret,
    pub country_code: Option<String>,
    pub enable_hosting: bool,
    pub start_receiver: bool,
}
impl fmt::Debug for LoginRequest { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("LoginRequest(<redacted>)") } }
impl LoginRequest {
    fn validate_identity(&self)->Result<(),Error> {
        let user=self.username.expose_to_backend().trim();let password=self.password.expose_to_backend();
        if !valid_attempt_id(&self.attempt_id) || user.is_empty() || user.len()>254 || user.chars().any(char::is_control)
            || password.is_empty() || password.len()>1024 || password.chars().any(char::is_control) {return Err(Error::new("app_login_request_invalid"));}
        if let Some(country)=&self.country_code {
            if country.is_empty() || country.len()>4 || !country.bytes().all(|b|b.is_ascii_digit())
                || !user.bytes().all(|b|b.is_ascii_digit()) || user.len()>20 {return Err(Error::new("app_login_request_invalid"));}
        }
        Ok(())
    }
    fn validate(&self) -> Result<(), Error> {
        if !self.enable_hosting {return Err(Error::new("app_login_request_invalid"));}
        self.validate_identity()
    }
    fn identity_key(&self) -> Vec<u8> {
        let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
        digest.update(self.country_code.as_deref().unwrap_or("").as_bytes()); digest.update(&[0]);
        digest.update(self.username.expose_to_backend().trim().to_ascii_lowercase().as_bytes());
        digest.finish().as_ref().to_vec()
    }
    fn masked_login_name(&self)->String {
        let first=self.username.expose_to_backend().trim().chars().next().unwrap_or('*');
        format!("{first}***")
    }
}
fn default_start_receiver() -> bool { true }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JegotripLoginRequest {
    pub attempt_id:String, pub username:Secret, pub password:Secret,
    pub country_code:Option<String>, #[serde(default)] pub intent:LoginIntent,
    pub consent:bool, #[serde(default="default_start_receiver")] pub start_receiver:bool,
}
impl fmt::Debug for JegotripLoginRequest {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("JegotripLoginRequest(<redacted>)")}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationWriteRequest { pub expected_revision:u64, pub configuration:DatabaseConfiguration }
impl fmt::Debug for ConfigurationWriteRequest {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("JegotripConfigurationWrite(<redacted>)")}}
#[derive(Clone,Debug,Serialize)]
pub struct ConfigurationView {
    pub configuration_source:&'static str, pub revision:u64, pub initialized:bool,
    pub http_configured:bool, pub native_configured:bool,
    pub accepting_authenticate:bool, pub accepting_enable_hosting:bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptView {
    pub attempt_id: String, pub phase: String, pub business_state: String,
    pub completed: bool, pub line_id: Option<String>, pub masked_number: Option<String>,
    pub error: Option<&'static str>, pub server_code: Option<u32>,
    pub created_at: i64, pub updated_at: i64,
    pub intent:LoginIntent, pub account_id:Option<String>,
}
#[derive(Debug, Serialize)]
pub struct LoginSetup {
    pub implemented: bool, pub deployment_present: bool, pub accepting: bool,
    pub manual_sdk_credentials_required: bool,
    pub accepting_authenticate:bool, pub native_configured:bool,
    pub configuration_source:&'static str, pub configuration_revision:u64,
    pub configuration_error:Option<&'static str>,
}
struct ActiveGuard { manager: Weak<AppLoginManager>, identity: Vec<u8>, _permit: OwnedSemaphorePermit }
impl Drop for ActiveGuard { fn drop(&mut self) { if let Some(m) = self.manager.upgrade() { lock(&m.identities).remove(&self.identity); } } }

pub struct AppLoginManager {
    database: Arc<Database>, accounts: Weak<AccountManager>, owner: Mutex<Option<String>>,
    deployment_path: Mutex<Option<PathBuf>>, factory: Arc<dyn TransportFactory>,
    active: Arc<Semaphore>, identities: Mutex<BTreeSet<Vec<u8>>>,
    tasks: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    finalization_errors: Mutex<BTreeSet<String>>, closed: AtomicBool, changed: Notify,
}
impl fmt::Debug for AppLoginManager { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppLoginManager(<redacted>)") } }
impl AppLoginManager {
    pub(crate) fn new(database: Arc<Database>, accounts: Weak<AccountManager>) -> Self {
        Self { database, accounts, owner: Mutex::new(None), deployment_path: Mutex::new(None), factory: Arc::new(LiveFactory),
            active: Arc::new(Semaphore::new(2)), identities: Mutex::new(BTreeSet::new()), tasks: Mutex::new(BTreeMap::new()),
            finalization_errors: Mutex::new(BTreeSet::new()), closed: AtomicBool::new(false), changed: Notify::new() }
    }
    /// Only after the original manager acquired the database OS lock. Migration
    /// retains ledger identities; no login/remote write is replayed at startup.
    pub(crate) fn initialize(&self, path: PathBuf) -> Result<(), &'static str> {
        let accounts = self.accounts.upgrade().ok_or("app_login_unavailable")?;
        let owner = accounts.login_process_owner().ok_or("app_login_unavailable")?;
        let mut current = lock(&self.owner);
        if current.is_some() { return Err("app_login_unavailable"); }
        self.database.with_conn(|conn| store::initialize(conn, &owner, now()))?;
        *lock(&self.deployment_path) = Some(path); *current = Some(owner); Ok(())
    }
    fn selected_configuration(&self)->Result<(&'static str,u64,Option<DatabaseConfiguration>),&'static str> {
        if lock(&self.owner).is_none() {return Ok(("unconfigured",0,None));}
        match self.database.with_conn(settings::load)? {
            settings::StoredConfiguration::Database {revision,configuration}=>Ok(("database",revision,configuration)),
            settings::StoredConfiguration::Missing=>{
                let path=lock(&self.deployment_path).clone();
                if let Some(path)=path.filter(|p|p.is_file()) {
                    let old=config::read_deployment(&path).map_err(|e|e.code)?;
                    Ok(("legacy_file",0,Some(old.into())))
                } else {Ok(("unconfigured",0,None))}
            }
        }
    }
    pub fn configuration(&self)->Result<ConfigurationView,&'static str> {
        let (source,revision,configuration)=self.selected_configuration()?;
        let initialized=lock(&self.owner).is_some();
        let open=initialized && !self.closed.load(Ordering::SeqCst);
        let http=configuration.is_some();let native=configuration.as_ref().is_some_and(|c|c.runtime.is_some());
        Ok(ConfigurationView {configuration_source:source,revision,initialized,http_configured:http,native_configured:native,
            accepting_authenticate:open&&http,accepting_enable_hosting:open&&http&&native})
    }
    pub(crate) fn configure(&self,request:ConfigurationWriteRequest)->Result<ConfigurationView,Error> {
        if lock(&self.owner).is_none() {return Err(Error::new("app_login_unavailable"));}
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_shutting_down"));}
        request.configuration.validate()?;
        // Builds trust/URL configuration only; no transport exchange at setup.
        let _=self.factory.build(&request.configuration.http)?;
        self.database.with_conn(|c|settings::save(c,request.expected_revision,Some(&request.configuration),now()).map_err(|e|e.code)).map_err(Error::new)?;
        self.configuration().map_err(Error::new)
    }
    pub(crate) fn clear_configuration(&self,expected:u64)->Result<ConfigurationView,Error> {
        if lock(&self.owner).is_none() {return Err(Error::new("app_login_unavailable"));}
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_shutting_down"));}
        self.database.with_conn(|c|settings::save(c,expected,None,now()).map_err(|e|e.code)).map_err(Error::new)?;
        self.configuration().map_err(Error::new)
    }
    pub fn setup(&self) -> LoginSetup {
        match self.configuration() {
            Ok(view)=>LoginSetup {implemented:true,deployment_present:view.http_configured,accepting:view.accepting_enable_hosting,
                manual_sdk_credentials_required:false,accepting_authenticate:view.accepting_authenticate,native_configured:view.native_configured,
                configuration_source:view.configuration_source,configuration_revision:view.revision,configuration_error:None},
            Err(code)=>LoginSetup {implemented:true,deployment_present:false,accepting:false,manual_sdk_credentials_required:false,
                accepting_authenticate:false,native_configured:false,configuration_source:"invalid",configuration_revision:0,configuration_error:Some(code)},
        }
    }
    pub fn app_accounts(&self)->Result<Vec<JegotripAccountView>,&'static str> {
        if lock(&self.owner).is_none() {return Ok(Vec::new());}
        self.database.with_conn(accounts::list)
    }
    pub fn list(&self) -> Result<Vec<AttemptView>, &'static str> {
        if lock(&self.owner).is_none() { return Ok(Vec::new()); }
        let mut views = self.database.with_conn(store::list)?;
        for view in &mut views { self.observe_finalization_error(view); } Ok(views)
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
        request.validate()?;self.submit_internal(request,LoginIntent::EnableHosting)
    }
    pub(crate) fn submit_jegotrip(self:&Arc<Self>,request:JegotripLoginRequest)->Result<AttemptView,Error> {
        if !request.consent || (request.intent==LoginIntent::Authenticate && request.start_receiver) {return Err(Error::new("app_login_request_invalid"));}
        let intent=request.intent;
        let common=LoginRequest {attempt_id:request.attempt_id,username:request.username,password:request.password,country_code:request.country_code,
            enable_hosting:intent==LoginIntent::EnableHosting,start_receiver:request.start_receiver};
        common.validate_identity()?;self.submit_internal(common,intent)
    }
    fn submit_internal(self:&Arc<Self>,request:LoginRequest,intent:LoginIntent)->Result<AttemptView,Error> {
        if self.closed.load(Ordering::SeqCst) { return Err(Error::new("app_login_shutting_down")); }
        let owner=lock(&self.owner).clone().ok_or(Error::new("app_login_unavailable"))?;
        if self.database.with_conn(|c|store::get(c,&request.attempt_id)).map_err(Error::new)?.is_some() {return Err(Error::new("app_login_attempt_exists"));}
        let (_,revision,configuration)=self.selected_configuration().map_err(Error::new)?;
        let configuration=configuration.ok_or(Error::new("app_login_deployment_missing"))?;
        if intent==LoginIntent::EnableHosting && configuration.runtime.is_none() {return Err(Error::new("app_login_native_configuration_missing"));}
        let client=self.factory.build(&configuration.http)?;
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        if intent==LoginIntent::EnableHosting {accounts.login_preflight_create().map_err(|_|Error::new("app_login_account_creation_failed"))?;}
        let permit=self.active.clone().try_acquire_owned().map_err(|_|Error::new("app_login_busy"))?;
        let identity=request.identity_key();
        if !lock(&self.identities).insert(identity.clone()) {return Err(Error::new("app_login_busy"));}
        let guard=ActiveGuard {manager:Arc::downgrade(self),identity,_permit:permit};
        let view=self.database.with_conn(|c|store::insert_with_intent(c,&request.attempt_id,&owner,intent,now())).map_err(Error::new)?;
        let id=request.attempt_id.clone();let manager=self.clone();
        // No await/cancellation gap between durable admission and spawn.
        let task=tokio::spawn(async move {
            let _guard=guard;
            let work=std::panic::AssertUnwindSafe(manager.run(&owner,request,intent,revision,configuration,client)).catch_unwind();
            let result=tokio::select! {
                biased;
                _=manager.wait_closed()=>Err(Error::new("app_login_interrupted")),
                result=tokio::time::timeout(BUDGET,work)=>match result {
                    Ok(Ok(result))=>result,Ok(Err(_))=>Err(Error::new("app_login_internal_failed")),Err(_)=>Err(Error::new("app_login_timeout")),
                },
            };
            if manager.database.with_conn(|c|store::finish(c,&id,&owner,result.err(),now())).is_err() {lock(&manager.finalization_errors).insert(id.clone());}
        });
        let mut tasks=lock(&self.tasks);tasks.retain(|_,t|!t.is_finished());tasks.insert(view.attempt_id.clone(),task);Ok(view)
    }
    async fn wait_closed(&self) {
        loop {
            let notified=self.changed.notified();tokio::pin!(notified);notified.as_mut().enable();
            if self.closed.load(Ordering::SeqCst) {return;}notified.await;
        }
    }
    fn step(&self,owner:&str,id:&str,phase:&str,business:Option<&str>)->Result<(),Error> {
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_interrupted"));}
        self.database.with_conn(|c|store::step(c,id,owner,phase,business,now())).map_err(Error::new)
    }
    async fn run(&self,owner:&str,mut request:LoginRequest,intent:LoginIntent,revision:u64,configuration:DatabaseConfiguration,mut client:AppClient)->Result<(),Error> {
        let id=request.attempt_id.clone();let id=id.as_str();let hint=request.masked_login_name();
        self.step(owner,id,"authenticating",None)?;
        client.login(&request).await?;
        request.password = Secret::from_owned(String::new());
        self.step(owner,id,"reading_bound_number",None)?;
        let bound=client.bound_number(&request).await?;
        request.username=Secret::from_owned(String::new());
        self.database.with_conn(|c|store::set_mask(c,id,owner,&bound.masked(),now())).map_err(Error::new)?;
        self.step(owner,id,"saving_app_account",None)?;
        let key=client.account_key(&bound)?;
        let account_id=self.database.with_conn(|c|accounts::record(c,&key,&hint,&bound.masked(),revision,now()).map_err(|e|e.code)).map_err(Error::new)?;
        self.database.with_conn(|c|store::set_account(c,id,owner,&account_id,now())).map_err(Error::new)?;
        // An App-only login does not consume SDK materials, enable a service,
        // retain the App token or fabricate a native account/configuration.
        if intent==LoginIntent::Authenticate {return Ok(());}
        self.step(owner,id,"fetching_credentials",None)?;
        let material=client.phone_credentials(&bound).await?;
        let runtime=configuration.runtime.ok_or(Error::new("app_login_native_configuration_missing"))?;
        let config:AccountConfig=runtime.bind(&bound.identity(),material)?;
        config.validate(now() as u64).map_err(|_|Error::new("app_login_native_configuration_invalid"))?;
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        accounts.login_preflight_create().map_err(|_|Error::new("app_login_account_creation_failed"))?;
        self.step(owner,id,"checking_business",None)?;
        if client.business_enabled().await? {
            self.step(owner,id,"creating_account",Some("already_enabled"))?;
        } else {
            self.step(owner,id,"enabling_business",Some("request_uncertain"))?;
            if let Err(error)=client.enable_business().await {
                if error.server_code.is_some() {self.step(owner,id,"enabling_business",Some("rejected"))?;}return Err(error);
            }
            self.step(owner,id,"confirming_business",Some("accepted_unconfirmed"))?;
            if !client.business_enabled().await? {return Err(Error::new("app_login_business_unconfirmed"));}
            self.step(owner,id,"creating_account",Some("enabled"))?;
        }
        // No distributed-transaction fiction and no implicit business closure.
        let account=accounts.create(config,now() as u64).await.map_err(|_|Error::new("app_login_account_creation_failed"))?;
        let line_id=account.account.line_id.clone();
        self.database.with_conn(|c|store::set_line(c,id,owner,&line_id,now())).map_err(Error::new)?;
        if request.start_receiver {
            self.step(owner,id,"starting_receiver",None)?;
            accounts.start(&line_id,account.account.revision,now() as u64).await.map_err(|_|Error::new("app_login_receiver_start_failed"))?;
        }
        Ok(())
    }
    pub(crate) async fn shutdown(&self) {
        self.closed.store(true,Ordering::SeqCst);self.active.close();self.changed.notify_waiters();
        let tasks=std::mem::take(&mut *lock(&self.tasks));for(_,task)in tasks {let _=task.await;}
    }
}
