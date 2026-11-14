//! Explicit Jegotrip App authentication and optional normal business provisioning.
//! Database configuration is provider-specific; an App login is never native readiness.
mod config;
mod crypto;
mod http;
mod app_version;
mod device_identity;
mod device_trust;
mod device_sync;
mod trusted_reuse;
mod local_receiver;
mod push_status;
mod communication;
mod communication_profile;
#[path="communication_flow.rs"]
mod communication_flow;
mod store;
mod settings;
mod accounts;
#[cfg(test)] mod tests;
#[cfg(test)] mod device_identity_tests;

pub(crate) use config::DatabaseConfiguration;
pub(crate) use app_version::AppVersionView;
pub(crate) use device_identity::DeviceIdentityState;
pub(crate) use push_status::PushStatusView;
pub use accounts::JegotripAccountView;
pub use device_trust::DeviceTrustObservation;
pub use device_sync::{DeviceSyncObservation,DeviceSyncState};
pub use communication::{CommunicationOperation,CommunicationObservation,CommunicationAction};
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
    "app_login_version_discovery_failed", "app_login_device_untrusted", "app_login_business_not_enabled", "app_login_device_sync_context_missing",
    "app_login_device_identity_conflict", "app_login_device_identity_revision_conflict", "app_login_device_identity_not_found",
    "app_login_device_identity_limit", "app_login_device_identity_in_use",
    "app_login_verification_required", "app_login_migration_required", "app_login_existing_business_action_required",
    "app_login_session_revoked", "app_login_service_maintenance", "app_login_phone_binding_required",
    "app_login_identity_mismatch", "app_login_native_configuration_invalid", "app_login_business_unconfirmed",
    "app_login_account_creation_failed", "app_login_receiver_start_failed", "app_login_internal_failed",
    "app_login_communication_unresolved",
    "app_login_communication_not_configured", "app_login_communication_writes_disabled", "app_login_communication_unconfirmed",
    "app_login_receiver_device_check_missing",
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
pub enum LoginIntent { Authenticate, #[default] EnableHosting, AttachHosting, CheckDeviceTrust, ReuseHosting, EnableTrustedHosting, PrepareLocalReceiver, Communication }
impl LoginIntent {
    fn as_str(self)->&'static str {match self {Self::Authenticate=>"authenticate",Self::EnableHosting=>"enable_hosting",Self::AttachHosting=>"attach_hosting",Self::CheckDeviceTrust=>"check_device_trust",Self::ReuseHosting=>"reuse_hosting",Self::EnableTrustedHosting=>"enable_trusted_hosting",Self::PrepareLocalReceiver=>"prepare_local_receiver",Self::Communication=>"communication"}}
    fn parse(value:&str)->Option<Self> {match value {"authenticate"=>Some(Self::Authenticate),"enable_hosting"=>Some(Self::EnableHosting),"attach_hosting"=>Some(Self::AttachHosting),"check_device_trust"=>Some(Self::CheckDeviceTrust),"reuse_hosting"=>Some(Self::ReuseHosting),"enable_trusted_hosting"=>Some(Self::EnableTrustedHosting),"prepare_local_receiver"=>Some(Self::PrepareLocalReceiver),"communication"=>Some(Self::Communication),_=>None}}
    fn needs_native(self)->bool {matches!(self,Self::EnableHosting|Self::AttachHosting|Self::ReuseHosting|Self::EnableTrustedHosting|Self::PrepareLocalReceiver)}
    fn reuses_trusted_device(self)->bool {matches!(self,Self::ReuseHosting|Self::EnableTrustedHosting)}
    fn requires_original_device(self)->bool {self.reuses_trusted_device() || matches!(self,Self::PrepareLocalReceiver|Self::Communication)}
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
    /// Optional explicit App identity for a new login alias; not a trust token.
    /// Existing identities can only be changed through the revision-guarded API.
    #[serde(default)] pub device_id:Option<Secret>,
    #[serde(default)] pub communication_operation:Option<CommunicationOperation>,
    #[serde(default)] pub write_consent:bool,
    #[serde(default)] pub expected_configuration_revision:Option<u64>,
}
impl fmt::Debug for JegotripLoginRequest {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("JegotripLoginRequest(<redacted>)")}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigurationWriteRequest { pub expected_revision:u64, pub configuration:DatabaseConfiguration }
/// Operator override carried to the shared version store; the value is
/// re-validated there (digits and dots only, bounded length).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppVersionWriteRequest { pub version:String }
/// Administrative CAS. A null device_id generates a random UUID. Omitting an
/// account_id never clears/reassigns an existing canonical number binding.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceIdentityWriteRequest {
    pub id:Option<String>, pub expected_revision:u64, pub account_id:Option<String>, pub device_id:Option<Secret>,
}
impl fmt::Debug for DeviceIdentityWriteRequest {
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("DeviceIdentityWrite(<redacted>)")}
}
impl fmt::Debug for ConfigurationWriteRequest {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("JegotripConfigurationWrite(<redacted>)")}}
#[derive(Clone,Debug,Serialize)]
pub struct ConfigurationView {
    pub configuration_source:&'static str, pub revision:u64, pub initialized:bool,
    pub http_configured:bool, pub native_configured:bool,
    pub accepting_authenticate:bool, pub accepting_enable_hosting:bool, pub accepting_local_receiver:bool,
    #[serde(skip_serializing_if="Vec::is_empty")]
    pub communication_actions:Vec<CommunicationAction>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptView {
    pub attempt_id: String, pub phase: String, pub business_state: String,
    pub completed: bool, pub line_id: Option<String>, pub masked_number: Option<String>,
    pub error: Option<&'static str>, pub server_code: Option<u32>,
    pub created_at: i64, pub updated_at: i64,
    pub intent:LoginIntent, pub account_id:Option<String>,
    /// 本次原设备查询的历史状态，不作为托管/接收授权或永久授信缓存。
    pub device_trust:Option<DeviceTrustObservation>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub communication:Option<CommunicationObservation>,
}
#[derive(Debug, Serialize)]
pub struct LoginSetup {
    pub implemented: bool, pub deployment_present: bool, pub accepting: bool,
    pub manual_sdk_credentials_required: bool,
    pub accepting_authenticate:bool, pub native_configured:bool,
    pub configuration_source:&'static str, pub configuration_revision:u64,
    pub configuration_error:Option<&'static str>,
    pub accepting_local_receiver:bool,
    #[serde(skip_serializing_if="Vec::is_empty")]
    pub communication_actions:Vec<CommunicationAction>,
}
#[derive(Default)]
struct Admissions { aliases:BTreeSet<Vec<u8>>, devices:BTreeSet<String> }
struct ActiveGuard {
    manager:Weak<AppLoginManager>, alias:Vec<u8>, devices:BTreeSet<String>,
    snapshot:device_identity::Snapshot, _permit:OwnedSemaphorePermit,
}
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Some(m)=self.manager.upgrade() {
            let mut admissions=lock(&m.admissions);
            admissions.aliases.remove(&self.alias);
            for id in &self.devices {admissions.devices.remove(id);}
        }
    }
}

pub struct AppLoginManager {
    database: Arc<Database>, accounts: Weak<AccountManager>, owner: Mutex<Option<String>>,
    deployment_path: Mutex<Option<PathBuf>>, factory: Arc<dyn TransportFactory>,
    active: Arc<Semaphore>, admissions:Mutex<Admissions>,
    tasks: Mutex<BTreeMap<String, tokio::task::JoinHandle<()>>>,
    finalization_errors: Mutex<BTreeSet<String>>, closed: AtomicBool, changed: Notify,
}
impl fmt::Debug for AppLoginManager { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AppLoginManager(<redacted>)") } }
impl AppLoginManager {
    pub(crate) fn new(database: Arc<Database>, accounts: Weak<AccountManager>) -> Self {
        Self { database, accounts, owner: Mutex::new(None), deployment_path: Mutex::new(None), factory: Arc::new(LiveFactory),
            active: Arc::new(Semaphore::new(2)), admissions:Mutex::new(Admissions::default()), tasks: Mutex::new(BTreeMap::new()),
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
        let existing_device=configuration.as_ref().is_some_and(|c|c.http.endpoints.verification_trustful_device.is_some());
        Ok(ConfigurationView {configuration_source:source,revision,initialized,http_configured:http,native_configured:native,
            accepting_authenticate:open&&http,accepting_enable_hosting:open&&http&&native,
            accepting_local_receiver:open&&http&&native&&existing_device,
            communication_actions:if open&&source=="database"{configuration.as_ref().and_then(|c|c.http.communication.as_ref()).map(|v|v.available()).unwrap_or_default()}else{Vec::new()}})
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
                configuration_source:view.configuration_source,configuration_revision:view.revision,configuration_error:None,
                accepting_local_receiver:view.accepting_local_receiver,communication_actions:view.communication_actions},
            Err(code)=>LoginSetup {implemented:true,deployment_present:false,accepting:false,manual_sdk_credentials_required:false,
                accepting_authenticate:false,native_configured:false,configuration_source:"invalid",configuration_revision:0,configuration_error:Some(code),
                accepting_local_receiver:false,communication_actions:Vec::new()},
        }
    }
    pub fn app_accounts(&self)->Result<Vec<JegotripAccountView>,&'static str> {
        if lock(&self.owner).is_none() {return Ok(Vec::new());}
        self.database.with_conn(accounts::list)
    }
    /// Pure inspection: no provider login, enrolment, start, callback or business
    /// operation. Historical records never make an idle receiver look ready.
    pub(crate) async fn push_status(&self)->Result<PushStatusView,Error> {
        let initialized=lock(&self.owner).is_some();
        let configuration=self.selected_configuration().map_err(Error::new)?.2;
        let template=push_status::PushTemplateStatus::inspect(configuration.as_ref().and_then(|c|c.runtime.as_ref()));
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        let native_accounts=accounts.list(now() as u64).await.map_err(Error::new)?;
        let stored=self.database.with_conn(push_status::stored_observations).map_err(Error::new)?;
        Ok(PushStatusView::assembled(initialized,now(),template,native_accounts,stored))
    }
    /// Authenticated metadata only. No alias hash, username, password, App token
    /// or native SDK material is reachable from this view.
    pub(crate) fn device_identities(&self)->Result<DeviceIdentityState,Error> {
        let initialized=lock(&self.owner).is_some();
        let configuration=self.selected_configuration().map_err(Error::new)?.2;
        let mode=configuration.as_ref().map(|c|c.http.device_identity_mode).unwrap_or_default();
        let admissions=lock(&self.admissions);
        let mut identities=if initialized {self.database.with_conn(device_identity::list).map_err(Error::new)?} else {Vec::new()};
        for view in &mut identities {view.in_use|=admissions.devices.contains(&view.id);}
        Ok(DeviceIdentityState {initialized,configured_mode:mode,max_identities:device_identity::MAX_IDENTITIES,identities})
    }
    fn device_admin_preflight(&self)->Result<(),Error> {
        if lock(&self.owner).is_none() {return Err(Error::new("app_login_unavailable"));}
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_shutting_down"));}
        Ok(())
    }
    pub(crate) fn set_device_identity(&self,request:DeviceIdentityWriteRequest)->Result<DeviceIdentityState,Error> {
        self.device_admin_preflight()?;
        let admissions=lock(&self.admissions);
        if !admissions.aliases.is_empty() {return Err(Error::new("app_login_device_identity_in_use"));}
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_shutting_down"));}
        let value=request.device_id.as_ref().map(|v|v.expose_to_backend().trim());
        self.database.with_conn(|conn| {
            let key=match request.account_id.as_deref() {
                Some(account)=>Some(accounts::identity_key(conn,account)?.ok_or("app_account_not_found")?),None=>None,
            };
            device_identity::set(conn,request.id.as_deref(),request.expected_revision,key.as_deref(),value,now()).map_err(|e|e.code)
        }).map_err(Error::new)?;
        drop(admissions);
        self.device_identities()
    }
    pub(crate) fn delete_device_identity(&self,id:&str,expected:u64)->Result<DeviceIdentityState,Error> {
        self.device_admin_preflight()?;
        let admissions=lock(&self.admissions);
        if !admissions.aliases.is_empty() {return Err(Error::new("app_login_device_identity_in_use"));}
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_shutting_down"));}
        self.database.with_conn(|conn|device_identity::remove(conn,id,expected,now()).map_err(|e|e.code)).map_err(Error::new)?;
        drop(admissions);
        self.device_identities()
    }
    /// Shared-state version view, separate from private deployment context.
    pub(crate) fn app_version_view(&self)->Result<AppVersionView,Error> {
        let (_,_,configuration)=self.selected_configuration().map_err(Error::new)?;
        let http=configuration.as_ref().map(|c|&c.http);
        let stored=self.database.with_conn(|conn|Ok::<_,&'static str>(app_version::load(conn))).ok().flatten();
        Ok(app_version::view(stored,now(),
            http.is_some_and(|h|h.app_version_file.is_some()),
            http.is_some_and(|h|h.app_version_web_url.is_some()),
            http.is_some_and(|h|h.app_version_ios_app_id.is_some()),
            http.is_some_and(|h|h.app_version.is_some()),
            http.is_some_and(|h|h.app_version_store)))
    }
    /// Force one remote lookup now (App Store, then the official page) and
    /// persist the result. Never guesses: a failed lookup is an explicit error.
    pub(crate) async fn refresh_app_version(&self)->Result<AppVersionView,Error> {
        let (_,_,configuration)=self.selected_configuration().map_err(Error::new)?;
        let configuration=configuration.ok_or(Error::new("app_login_deployment_missing"))?;
        let factory=self.factory.build(&configuration.http)?;
        let client=factory.with_version_store(Arc::new(app_version::DatabaseVersionStore::new(self.database.clone())));
        if client.refresh_app_version().await.is_none() { return Err(Error::new("app_login_version_discovery_failed")); }
        self.app_version_view()
    }
    /// Operator override. Stored with source=manual so the provenance stays
    /// honest and a later automatic re-check can replace it.
    pub(crate) fn set_app_version(&self,version:&str)->Result<AppVersionView,Error> {
        let stored=app_version::manual(version.trim(),now())?;
        self.database.with_conn(|conn|app_version::save(conn,&stored)).map_err(Error::new)?;
        self.app_version_view()
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
        request.validate()?;self.submit_internal(request,LoginIntent::EnableHosting,None)
    }
    pub(crate) fn submit_jegotrip(self:&Arc<Self>,request:JegotripLoginRequest)->Result<AttemptView,Error> {
        if !request.consent || (matches!(request.intent,LoginIntent::Authenticate|LoginIntent::CheckDeviceTrust|LoginIntent::Communication) && request.start_receiver) {return Err(Error::new("app_login_request_invalid"));}
        if (request.intent==LoginIntent::Communication)!=request.communication_operation.is_some()
            ||(request.intent==LoginIntent::Communication&&(request.expected_configuration_revision.is_none()||request.communication_operation.as_ref().is_some_and(|v|v.action().writes())&&!request.write_consent))
            ||(request.intent!=LoginIntent::Communication&&(request.write_consent||request.expected_configuration_revision.is_some())){return Err(Error::new("app_login_request_invalid"));}
        let intent=request.intent;
        let common=LoginRequest {attempt_id:request.attempt_id,username:request.username,password:request.password,country_code:request.country_code,
            enable_hosting:matches!(intent,LoginIntent::EnableHosting|LoginIntent::EnableTrustedHosting),start_receiver:request.start_receiver};
        let preferred=request.device_id;
        if preferred.as_ref().is_some_and(|v| !device_identity::valid_device_id(v.expose_to_backend().trim())) {return Err(Error::new("app_login_request_invalid"));}
        if (intent.reuses_trusted_device() || matches!(intent,LoginIntent::PrepareLocalReceiver|LoginIntent::Communication)) && !preferred.as_ref().is_some_and(|v|device_identity::valid_phone_app_uuid(v.expose_to_backend().trim())) {
            return Err(Error::new("app_login_request_invalid"));
        }
        common.validate_identity()?;
        self.submit_internal_operation(common,intent,preferred,request.communication_operation,request.expected_configuration_revision)
    }
    fn submit_internal(self:&Arc<Self>,request:LoginRequest,intent:LoginIntent,preferred:Option<Secret>)->Result<AttemptView,Error> {
        self.submit_internal_operation(request,intent,preferred,None,None)
    }
    fn submit_internal_operation(self:&Arc<Self>,request:LoginRequest,intent:LoginIntent,preferred:Option<Secret>,operation:Option<CommunicationOperation>,expected:Option<u64>)->Result<AttemptView,Error> {
        if self.closed.load(Ordering::SeqCst) { return Err(Error::new("app_login_shutting_down")); }
        let owner=lock(&self.owner).clone().ok_or(Error::new("app_login_unavailable"))?;
        if self.database.with_conn(|c|store::get(c,&request.attempt_id)).map_err(Error::new)?.is_some() {return Err(Error::new("app_login_attempt_exists"));}
        let (source,revision,configuration)=self.selected_configuration().map_err(Error::new)?;
        let configuration=configuration.ok_or(Error::new("app_login_deployment_missing"))?;
        if intent==LoginIntent::Communication&&source!="database"{return Err(Error::new("app_login_communication_not_configured"));}
        if intent.needs_native() && configuration.runtime.is_none() {return Err(Error::new("app_login_native_configuration_missing"));}
        if intent==LoginIntent::Communication && operation.is_none(){return Err(Error::new("app_login_request_invalid"));}
        let mut client=self.factory.build(&configuration.http)?;
        if let Some(expected)=expected{if expected!=revision{return Err(Error::new("app_login_configuration_revision_conflict"));}}
        if let Some(operation)=operation{client=client.with_communication(operation)?;}
        else if intent==LoginIntent::PrepareLocalReceiver {client=client.with_local_receiver()?;}
        else if intent.reuses_trusted_device() { client=client.with_trusted_reuse()?; }
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        if intent.needs_native() {accounts.login_preflight_create().map_err(|_|Error::new("app_login_account_creation_failed"))?;}
        let permit=self.active.clone().try_acquire_owned().map_err(|_|Error::new("app_login_busy"))?;
        let alias=client.login_alias_key(&request)?;
        let mut admissions=lock(&self.admissions);
        if self.closed.load(Ordering::SeqCst) {return Err(Error::new("app_login_shutting_down"));}
        if admissions.aliases.contains(&alias) {return Err(Error::new("app_login_busy"));}
        let explicit=preferred.as_ref().map(|v|v.expose_to_backend().trim()).or_else(||
            (configuration.http.device_identity_mode==config::DeviceIdentityMode::Static).then(||configuration.http.device_id.expose_to_backend()));
        let (view,snapshot)=self.database.with_conn(|conn| {
            let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|"app_login_storage_failed")?;
            let snapshot=if intent.requires_original_device() {
                device_identity::resolve_reuse(&tx,&alias,explicit.ok_or("app_login_request_invalid")?,now()).map_err(|e|e.code)?
            } else {device_identity::resolve(&tx,&alias,explicit,Some(configuration.http.device_id.expose_to_backend()),now()).map_err(|e|e.code)?};
            if admissions.devices.contains(&snapshot.id) {return Err("app_login_busy");}
            if client.communication_operation().is_some_and(|operation|operation.action().writes())&&store::communication_write_unresolved(&tx,&snapshot.id)?{return Err("app_login_communication_unresolved");}
            let view=store::insert_admitted(&tx,&request.attempt_id,&owner,intent,now())?;
            tx.commit().map_err(|_|"app_login_storage_failed")?;
            Ok((view,snapshot))
        }).map_err(Error::new)?;
        client=client.with_device_id(snapshot.device_id.expose_to_backend());
        let client=client.with_version_store(Arc::new(app_version::DatabaseVersionStore::new(self.database.clone())));
        admissions.aliases.insert(alias.clone());admissions.devices.insert(snapshot.id.clone());
        let guard=ActiveGuard {manager:Arc::downgrade(self),alias,devices:BTreeSet::from([snapshot.id.clone()]),snapshot,_permit:permit};
        let id=request.attempt_id.clone();let manager=self.clone();
        // No await/cancellation gap between durable admission and spawn.
        let task=tokio::spawn(async move {
            let mut guard=guard;
            let work=std::panic::AssertUnwindSafe(manager.run(&owner,request,intent,revision,configuration,client,&mut guard)).catch_unwind();
            let result=tokio::select! {
                biased;
                _=manager.wait_closed()=>Err(Error::new("app_login_interrupted")),
                result=tokio::time::timeout(BUDGET,work)=>match result {
                    Ok(Ok(result))=>result,Ok(Err(_))=>Err(Error::new("app_login_internal_failed")),Err(_)=>Err(Error::new("app_login_timeout")),
                },
            };
            if manager.database.with_conn(|c|store::finish(c,&id,&owner,result.err(),now())).is_err() {lock(&manager.finalization_errors).insert(id.clone());}
        });
        let mut tasks=lock(&self.tasks);tasks.retain(|_,t|!t.is_finished());tasks.insert(view.attempt_id.clone(),task);
        drop(tasks);drop(admissions);Ok(view)
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
    fn confirm_device_identity(&self,guard:&mut ActiveGuard,key:&[u8],strict_reuse:bool)->Result<bool,Error> {
        let mut admissions=lock(&self.admissions);
        let selected=self.database.with_conn(|conn| {
            let tx=rusqlite::Transaction::new_unchecked(conn,rusqlite::TransactionBehavior::Immediate).map_err(|_|"app_login_storage_failed")?;
            let selected=if strict_reuse {device_identity::confirm_reuse(&tx,&guard.alias,&guard.snapshot,key,now())}
                else {device_identity::confirm(&tx,&guard.alias,&guard.snapshot,key,now())}.map_err(|e|e.code)?;
            if selected.id!=guard.snapshot.id && admissions.devices.contains(&selected.id) {return Err("app_login_busy");}
            tx.commit().map_err(|_|"app_login_storage_failed")?;Ok(selected)
        }).map_err(Error::new)?;
        let changed=guard.snapshot.id!=selected.id;
        admissions.devices.insert(selected.id.clone());guard.devices.insert(selected.id.clone());guard.snapshot=selected;
        Ok(changed)
    }
    async fn run(&self,owner:&str,mut request:LoginRequest,intent:LoginIntent,revision:u64,configuration:DatabaseConfiguration,mut client:AppClient,guard:&mut ActiveGuard)->Result<(),Error> {
        // First run of a fresh installation: learn a real App version from the
        // configured source and persist it BEFORE any login call, so the very
        // first request already carries a value the server accepts.
        client.ensure_app_version().await;
        // The original LoginActivity enrols this device before signing in; the
        // login body then carries the returned enrollId. Best effort only.
        if !intent.reuses_trusted_device() {
            if intent!=LoginIntent::CheckDeviceTrust {client.ensure_device_enrollment().await;}
        }
        eprintln!("app_login_version_in_use {}",client.version_provenance().await);
        let outcome=self.run_with_version_gate(owner,&request,intent,revision,&configuration,&mut client,guard).await;
        request.password = Secret::from_owned(String::new());
        request.username = Secret::from_owned(String::new());
        outcome
    }
    /// The server refuses an outdated client with a single business code. When
    /// that happens the version is re-learned once (App Store / official page),
    /// persisted, and the WHOLE flow is replayed so every body, query and header
    /// is rebuilt with the new value. One retry only: a repeated gate is final.
    /// Trusted reuse stops this retry as soon as device/business writing began;
    /// a later 524 cannot authorize replay of a possibly applied operation.
    async fn run_with_version_gate(&self,owner:&str,request:&LoginRequest,intent:LoginIntent,revision:u64,configuration:&DatabaseConfiguration,client:&mut AppClient,guard:&mut ActiveGuard)->Result<(),Error> {
        let error=match self.run_flow(owner,request,intent,revision,configuration,client,guard).await { Ok(())=>return Ok(()), Err(error)=>error };
        if error.server_code!=Some(http::VERSION_GATE_CODE) || client.trusted_reuse_write_started() { return Err(error); }
        let before=client.version_value().await;
        let Some((value,source))=client.refresh_app_version().await else {
            eprintln!("app_login_version_refresh_unavailable"); return Err(error);
        };
        if before.as_deref()==Some(value.as_str()) {
            eprintln!("app_login_version_gate_unchanged source={source}"); return Err(error);
        }
        eprintln!("app_login_version_gate_retry source={source}");
        client.reset_session();
        self.step(owner,&request.attempt_id,"authenticating",None)?;
        self.run_flow(owner,request,intent,revision,configuration,client,guard).await
    }
    async fn run_flow(&self,owner:&str,request:&LoginRequest,intent:LoginIntent,revision:u64,configuration:&DatabaseConfiguration,client:&mut AppClient,guard:&mut ActiveGuard)->Result<(),Error> {
        let id=request.attempt_id.clone();let id=id.as_str();let hint=request.masked_login_name();
        self.step(owner,id,"authenticating",None)?;
        client.login(&request).await?;
        self.step(owner,id,"reading_bound_number",None)?;
        let mut bound=client.bound_number(&request).await?;
        let key=client.account_key(&bound)?;
        if self.confirm_device_identity(guard,&key,intent.requires_original_device())? {
            // A newly seen alias discovered an existing number's identity.
            // Re-authenticate ONCE with it, before any token/trust/business call;
            // never switch a device beneath an existing App session.
            client.use_device_identity(guard.snapshot.device_id.expose_to_backend());
            if !intent.reuses_trusted_device() {
                if intent!=LoginIntent::CheckDeviceTrust {client.ensure_device_enrollment().await;}
            }
            client.login(&request).await?;
            let confirmed=client.bound_number(&request).await?;
            if client.account_key(&confirmed)?!=key {return Err(Error::new("app_login_identity_mismatch"));}
            self.confirm_device_identity(guard,&key,intent.requires_original_device())?;
            bound=confirmed;
        }
        self.database.with_conn(|c|store::set_mask(c,id,owner,&bound.masked(),now())).map_err(Error::new)?;
        self.step(owner,id,"saving_app_account",None)?;
        let account_id=self.database.with_conn(|c|accounts::record(c,&key,&hint,&bound.masked(),revision,now()).map_err(|e|e.code)).map_err(Error::new)?;
        self.database.with_conn(|c|store::set_account(c,id,owner,&account_id,now())).map_err(Error::new)?;
        // An App-only login does not consume SDK materials, enable a service,
        // retain the App token or fabricate a native account/configuration.
        if intent==LoginIntent::Authenticate {return Ok(());}
        if intent==LoginIntent::Communication{return self.run_communication(owner,id,revision,client,guard,&bound).await;}
        if intent==LoginIntent::CheckDeviceTrust {
            // 手机授信迁移后的显式只读核验：不 enroll、不取电话凭据、
            // 不创建原生账号、不写业务、不把历史观察当作授权。
            self.step(owner,id,"checking_device_trust",None)?;
            let security=client.security_verify_device().await;
            let existing=client.verify_trusted_device().await;
            // Retain finite fields of this one response, not only its switch.
            // Comparing diagnostics must not borrow a code from another call.
            let status=client.call_status().await;
            let business=status.map(|s|s.hosting_enabled);
            let observation=DeviceTrustObservation::capture(&guard.snapshot,now(),security,existing,business)
                .with_diagnostic_status(status);
            self.database.with_conn(|c|store::set_device_trust(c,id,owner,&observation,now())).map_err(Error::new)?;
            return Ok(());
        }
        if self.database.with_conn(|c|device_identity::native_ownership_pending(c,&key)).map_err(Error::new)? {
            return Err(Error::new("app_login_device_identity_in_use"));
        }
        if intent==LoginIntent::PrepareLocalReceiver {
            return self.run_local_receiver(owner,request,configuration,client,guard,&bound).await;
        }
        if intent.reuses_trusted_device() {
            return self.run_trusted_reuse(owner,request,intent,configuration,client,guard,&bound).await;
        }
        self.step(owner,id,"fetching_credentials",None)?;
        let material=client.phone_credentials(&bound).await?;
        // The App asks the server whether THIS device is trusted before it
        // touches the business switch. The answer is reported verbatim; a
        // missing configuration or a transport error is never read as trust,
        // and the business call below remains the only decider. When the
        // installation also configured the App's own enrolment/verification
        // endpoints, those run first, exactly as the original client does, and
        // each answer is logged instead of being treated as a grant.
        let enroll=client.enroll_device().await;
        let security=client.security_verify_device().await;
        for (label,outcome) in [("enroll",&enroll),("security_verify",&security)] {
            match outcome {
                Ok(Some(decision))=>eprintln!("app_login_device_trust step={label} authentication_code={:?}",decision.authentication_code),
                Ok(None)=>eprintln!("app_login_device_trust step={label} not_configured"),
                Err(error)=>eprintln!("app_login_device_trust step={label} failed code={} server_code={:?}",error.code,error.server_code),
            }
        }
        let existing=client.verify_trusted_device().await;
        match &existing {
            Ok(Some(decision))=>eprintln!("app_login_trust_decision authentication_code={:?}",decision.authentication_code),
            Ok(None)=>eprintln!("app_login_trust_decision not_configured"),
            Err(error)=>eprintln!("app_login_trust_decision_failed code={} server_code={:?}",error.code,error.server_code),
        }
        let runtime=configuration.runtime.clone().ok_or(Error::new("app_login_native_configuration_missing"))?;
        let config:AccountConfig=runtime.bind(&bound.identity(),material)?;
        config.validate(now() as u64).map_err(|_|Error::new("app_login_native_configuration_invalid"))?;
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        accounts.login_preflight_create().map_err(|_|Error::new("app_login_account_creation_failed"))?;
        self.step(owner,id,"checking_business",None)?;
        let business=client.business_enabled().await;
        let observation=DeviceTrustObservation::capture(&guard.snapshot,now(),security,existing,business);
        self.database.with_conn(|c|store::set_device_trust(c,id,owner,&observation,now())).map_err(Error::new)?;
        let enabled=business?;
        if intent==LoginIntent::AttachHosting {
            // Taking over an EXISTING hosting service. The server only lets the
            // account's verified device enable it (506 otherwise, verified on
            // the real service), so this intent never writes the business
            // switch: it requires the operator to have enabled hosting in the
            // official App first, and fails closed when the server says it is
            // still off.
            if !enabled { return Err(Error::new("app_login_business_not_enabled")); }
            self.step(owner,id,"creating_account",Some("already_enabled"))?;
        } else if enabled {
            self.step(owner,id,"creating_account",Some("already_enabled"))?;
        } else {
            self.step(owner,id,"enabling_business",Some("request_uncertain"))?;
            if let Err(error)=client.enable_business().await {
                if error.server_code.is_some() {self.step(owner,id,"enabling_business",Some("rejected"))?;}
                // 506 is the server's own "untrusted device" refusal; keep it a
                // distinct, actionable code instead of a generic rejection.
                if error.server_code==Some(506) {return Err(Error {code:"app_login_device_untrusted",server_code:error.server_code});}
                return Err(error);
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
        let tasks={
            // The same admission lock covers spawn + handle registration. A
            // concurrent shutdown cannot miss a newly admitted provisioning task.
            let _admissions=lock(&self.admissions);
            self.closed.store(true,Ordering::SeqCst);self.active.close();self.changed.notify_waiters();
            std::mem::take(&mut *lock(&self.tasks))
        };
        for(_,task)in tasks {let _=task.await;}
    }
}
