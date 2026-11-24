//! Authenticated, redacted application-account management, separate from SIM APIs.
//! No credential export, caller-supplied callbacks or cached readiness restore.
//! Incoming-call SDP/actions are bounded and original-owned; reserved outbound
//! endpoints cannot authorize SDK/SIM/IMS/CS sends. Native start is explicit.
use axum::{
    extract::{rejection::{JsonRejection, QueryRejection}, DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post}, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{
    api::models::ApiResponse,
    connectivity::app_voice::providers::jegotrip::{account_config::AccountConfig, account_runtime::AccountManager, app_login::{AppLoginManager, LoginRequest, JegotripLoginRequest, ConfigurationWriteRequest, AppVersionWriteRequest, DeviceIdentityWriteRequest}, PROVIDER_ID},
    state::AppState,
};

const MAX_REQUEST_BYTES: usize = 256 * 1024;
#[path="app_voice_relation_updates.rs"]
mod relation_updates;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAccountRequest { provider_id: String, config: AccountConfig }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceAccountRequest { expected_revision: u64, config: AccountConfig }
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionRequest { expected_revision: u64 }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerCallRequest { expected_revision: u64, sdp: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundCallRequest { expected_revision: u64, target: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundMessageRequest { expected_revision: u64, target: String, body: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxQuery { #[serde(default = "default_inbox_limit")] limit: usize }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImUserReadRequest{expected_revision:u64,nonce:String,consent:bool}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImUserReadRecipientRequest{expected_revision:u64,nonce:String,recipient:String,consent:bool}
impl Drop for ImUserReadRecipientRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.nonce);zeroize::Zeroize::zeroize(&mut self.recipient);}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImReconciliationRequest { expected_revision: u64, consent: bool }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImReadStatusTargetsQuery{expected_revision:u64}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImSendDataField{key:Vec<u8>,value:Vec<u8>}
impl Drop for ImSendDataField{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.key);zeroize::Zeroize::zeroize(&mut self.value);}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImSendRequest{expected_revision:u64,nonce:String,recipient:String,content:String,info_type:Option<String>,#[serde(default)]attachments:Vec<ImSendDataField>,#[serde(default)]parameters:Vec<ImSendDataField>,notifications:Option<Vec<ImSendDataField>>,notification_digest:Option<String>,consent:bool,write_consent:bool}
impl Drop for ImSendRequest{fn drop(&mut self){for s in [&mut self.nonce,&mut self.recipient,&mut self.content]{zeroize::Zeroize::zeroize(s);}if let Some(s)=&mut self.info_type{zeroize::Zeroize::zeroize(s);}if let Some(s)=&mut self.notification_digest{zeroize::Zeroize::zeroize(s);}}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImReadStatusRequest{expected_revision:u64,expected_index:String,nonce:String,consent:bool}
impl Drop for ImReadStatusRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.nonce);}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImReadStatusRecipientRequest{expected_revision:u64,expected_index:String,nonce:String,recipient:String,consent:bool}
impl Drop for ImReadStatusRecipientRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.nonce);zeroize::Zeroize::zeroize(&mut self.recipient);}}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserEntryLookupRequest{expected_revision:u64,recipient:String,consent:bool}
impl Drop for UserEntryLookupRequest{fn drop(&mut self){zeroize::Zeroize::zeroize(&mut self.recipient);}}
#[derive(Clone,Copy,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum DirectoryReadOperation{Relations,Properties}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryReadRequest{expected_revision:u64,operation:DirectoryReadOperation,group_id:Option<String>,since:String,consent:bool}
impl Drop for DirectoryReadRequest{fn drop(&mut self){if let Some(id)=&mut self.group_id{zeroize::Zeroize::zeroize(id);}zeroize::Zeroize::zeroize(&mut self.since);}}
fn default_inbox_limit() -> usize { 50 }

/// Merged into main's protected routes BEFORE its existing auth middleware.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/jegotrip/config", get(jegotrip_configuration).put(jegotrip_configure).delete(jegotrip_clear_configuration))
        .route("/api/jegotrip/accounts", get(jegotrip_accounts))
        .route("/api/jegotrip/login/setup", get(app_login_setup))
        .route("/api/jegotrip/push/status",get(jegotrip_push_status))
        .route("/api/jegotrip/app-version", get(jegotrip_app_version).put(jegotrip_set_app_version))
        .route("/api/jegotrip/app-version/refresh", post(jegotrip_refresh_app_version))
        .route("/api/jegotrip/device-identities", get(jegotrip_device_identities).put(jegotrip_set_device_identity))
        .route("/api/jegotrip/device-identities/{id}", delete(jegotrip_delete_device_identity))
        .route("/api/jegotrip/login/attempts", get(app_login_attempts).post(jegotrip_login_submit))
        .route("/api/jegotrip/login/attempts/{attempt_id}", get(app_login_attempt))
        .route("/api/non-3gpp/app-login/setup", get(app_login_setup))
        .route("/api/non-3gpp/app-login/attempts", get(app_login_attempts).post(app_login_submit))
        .route("/api/non-3gpp/app-login/attempts/{attempt_id}", get(app_login_attempt))
        .route("/api/non-3gpp/accounts", get(list_accounts).post(create_account))
        .route("/api/non-3gpp/accounts/{line_id}", get(get_account).put(replace_account).delete(delete_account))
        .route("/api/non-3gpp/accounts/{line_id}/start", post(start_account))
        .route("/api/non-3gpp/accounts/{line_id}/stop", post(stop_account))
        .route("/api/non-3gpp/accounts/{line_id}/inbox", get(get_inbox))
        .route("/api/non-3gpp/accounts/{line_id}/messages", get(get_messages))
        .route("/api/non-3gpp/accounts/{line_id}/notifications", get(get_notifications))
        .route("/api/non-3gpp/accounts/{line_id}/call-notifications", get(get_call_notifications))
        .route("/api/non-3gpp/accounts/{line_id}/im-messages", get(get_im_messages))
        .route("/api/non-3gpp/accounts/{line_id}/im-outbox",get(get_im_send_history).post(send_im_message))
        .route("/api/non-3gpp/accounts/{line_id}/im-receipts", get(get_im_receipts))
        .route("/api/non-3gpp/accounts/{line_id}/im-read-status-targets",get(get_im_read_status_targets))
        .route("/api/non-3gpp/accounts/{line_id}/im-read-status-targets/{target_id}/set",post(set_im_read_status))
        .route("/api/non-3gpp/accounts/{line_id}/im-read-status-targets/{target_id}/set/recipient",post(set_im_read_status_via_recipient))
        .route("/api/non-3gpp/accounts/{line_id}/im-read-status-actions",get(get_im_read_status_history))
        .route("/api/non-3gpp/accounts/{line_id}/im-user-reads",get(get_im_user_reads))
        .route("/api/non-3gpp/accounts/{line_id}/im-user-reads/{receipt_id}/reconcile",post(reconcile_im_user_read))
        .route("/api/non-3gpp/accounts/{line_id}/user-entry/lookup",post(lookup_user_entry))
        .route("/api/non-3gpp/accounts/{line_id}/native-directory/read",post(read_native_directory))
        .route("/api/non-3gpp/accounts/{line_id}/relation-writes",get(relation_updates::relation_history).post(relation_updates::update_relations))
        .route("/api/non-3gpp/accounts/{line_id}/relation-intents",get(relation_updates::intent_history))
        .route("/api/non-3gpp/accounts/{line_id}/im-messages/{message_id}/mark-read",post(mark_im_user_read))
        .route("/api/non-3gpp/accounts/{line_id}/im-messages/{message_id}/mark-read/recipient",post(mark_im_user_read_recipient))
        .route("/api/non-3gpp/accounts/{line_id}/im-messages/{message_id}", delete(delete_im_message))
        .route("/api/non-3gpp/accounts/{line_id}/calls", get(get_calls))
        .route("/api/non-3gpp/accounts/{line_id}/calls/{call_id}", get(get_call))
        .route("/api/non-3gpp/accounts/{line_id}/calls/{call_id}/answer", post(answer_call))
        .route("/api/non-3gpp/accounts/{line_id}/calls/{call_id}/hangup", post(hangup_call))
        .route("/api/non-3gpp/accounts/{line_id}/calls/{call_id}/reject", post(hangup_call))
        .route("/api/non-3gpp/accounts/{line_id}/trunk", get(get_call_trunk_status))
        .route("/api/non-3gpp/accounts/{line_id}/outbound/calls", post(reserved_outbound_call))
        .route("/api/non-3gpp/accounts/{line_id}/outbound/messages", post(reserved_outbound_message))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}
fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    response
}
fn error_status(code: &str) -> StatusCode {
    match code {
        "app_login_communication_unresolved" | "app_login_attempt_exists" | "app_login_configuration_revision_conflict" | "app_login_migration_conflict"
            | "app_login_device_identity_conflict" | "app_login_device_identity_revision_conflict" | "app_login_device_identity_in_use" => StatusCode::CONFLICT,
        "app_login_busy" | "app_login_attempt_limit" | "app_login_account_limit" | "app_login_rate_limited" | "app_login_device_identity_limit" => StatusCode::TOO_MANY_REQUESTS,
        "app_login_storage_failed" => StatusCode::INTERNAL_SERVER_ERROR,
        "app_login_unavailable" | "app_login_deployment_missing" | "app_login_deployment_invalid"
            | "app_login_native_configuration_missing" | "app_login_shutting_down" | "app_login_entropy_failed"
            | "app_login_receiver_device_check_missing" | "app_login_communication_not_configured" => StatusCode::SERVICE_UNAVAILABLE,
        "app_login_communication_writes_disabled" => StatusCode::FORBIDDEN,
        "app_account_not_found" | "app_account_call_not_found" | "app_login_device_identity_not_found" => StatusCode::NOT_FOUND,
        "app_account_revision_conflict" | "app_account_stop_before_replace" | "app_account_call_operation_pending" => StatusCode::CONFLICT,
        "app_account_provider_unsupported" | "app_account_outbound_unsupported" => StatusCode::NOT_IMPLEMENTED,
        "app_account_storage_failed" | "app_account_inbox_storage_failed" | "app_account_sms_storage_failed"
            | "app_account_run_storage_failed" | "app_account_notification_storage_failed" | "app_account_call_storage_failed" | "database_unavailable" => StatusCode::INTERNAL_SERVER_ERROR,
        "app_account_limit" | "app_account_run_limit" | "app_account_entropy_unavailable" | "app_account_backend_unavailable"
            | "app_account_authentication_rejected" | "app_account_disconnected" | "app_account_hosting_rejected"
            | "app_account_operation_timeout" | "app_account_shutting_down" | "app_account_cleanup_unconfirmed"
            | "app_account_call_runtime_unavailable" | "app_account_call_cleanup_unconfirmed"
            | "app_account_call_trunk_delivery_unconfirmed" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_im_user_read_nonce_conflict" | "app_account_im_user_read_uncertain" | "app_account_im_user_read_target_unconfirmed" | "app_account_im_user_read_message_conflict" => StatusCode::CONFLICT,
        "app_account_im_user_read_consent_required" | "app_account_im_user_read_not_installed" | "app_account_im_user_read_recipient_not_installed" => StatusCode::FORBIDDEN,
        "app_account_im_user_read_recipient_unconfirmed" => StatusCode::CONFLICT,
        "app_account_im_read_status_consent_required" | "app_account_im_read_status_not_installed" => StatusCode::FORBIDDEN,
        "app_account_im_read_status_scope_invalid" | "app_account_im_read_status_uncertain" | "app_account_im_read_status_timeout" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_im_read_status_target_missing" => StatusCode::NOT_FOUND,
        "app_account_im_read_status_target_conflict" | "app_account_im_read_status_nonce_conflict" | "app_account_im_receipt_uncertain" => StatusCode::CONFLICT,
        "app_account_im_read_status_recipient_not_installed" => StatusCode::FORBIDDEN,
        "app_account_im_read_status_request_invalid" | "app_account_im_read_status_recipient_unsupported" => StatusCode::BAD_REQUEST,
        "app_account_im_read_status_recipient_unconfirmed" | "app_account_im_read_status_recipient_target_stale" | "app_account_im_read_status_resolution_inflight" | "app_account_im_read_status_resolution_finished" => StatusCode::CONFLICT,
        "app_account_im_read_status_full" | "app_account_im_read_status_recipient_capacity" => StatusCode::TOO_MANY_REQUESTS,
        "app_account_im_send_request_invalid" | "app_account_im_send_policy_invalid" => StatusCode::BAD_REQUEST,
        "app_account_im_send_not_installed" | "app_account_im_send_consent_required" | "app_account_im_send_recipient_not_installed" => StatusCode::FORBIDDEN,
        "app_account_im_send_scope_invalid" | "app_account_im_send_timeout" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_im_send_uncertain" | "app_account_im_send_nonce_conflict" | "app_account_im_send_recipient_unconfirmed" => StatusCode::CONFLICT,
        "app_account_im_send_full" | "app_account_im_send_capacity" => StatusCode::TOO_MANY_REQUESTS,
        "app_account_im_user_read_message_missing" => StatusCode::NOT_FOUND,
        "app_account_im_storage_failed" => StatusCode::INTERNAL_SERVER_ERROR,
        "app_account_im_user_read_full" => StatusCode::TOO_MANY_REQUESTS,
        "app_account_im_reconciliation_consent_required" | "app_account_im_reconciliation_not_installed" => StatusCode::FORBIDDEN,
        "app_account_im_reconciliation_not_found" => StatusCode::NOT_FOUND,
        "app_account_im_reconciliation_context_conflict" => StatusCode::CONFLICT,
        "app_account_im_reconciliation_timeout" | "app_account_im_reconciliation_reply_invalid" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_user_entry_consent_required" | "app_account_user_entry_not_installed" => StatusCode::FORBIDDEN,
        "app_account_user_entry_capacity" => StatusCode::TOO_MANY_REQUESTS,
        "app_account_user_entry_scope_invalid" | "app_account_user_entry_reference_unsupported" | "app_account_user_entry_transport_failed"
            | "app_account_user_entry_timeout" | "app_account_user_entry_reply_invalid" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_user_entry_ambiguous" | "app_account_user_entry_realm_mismatch" => StatusCode::CONFLICT,
        "app_account_relation_consent_required" | "app_account_relation_not_installed" => StatusCode::FORBIDDEN,
        "app_account_relation_uncertain" | "app_account_relation_nonce_conflict" | "app_account_relation_preparing" | "app_account_relation_nonce_retired" | "app_account_relation_legacy_intent" => StatusCode::CONFLICT,
        "app_account_relation_capacity" | "app_account_relation_full" => StatusCode::TOO_MANY_REQUESTS,
        "app_account_relation_scope_invalid" | "app_account_relation_timeout" | "app_account_relation_entropy_failed" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_relation_storage_failed" => StatusCode::INTERNAL_SERVER_ERROR,
        "app_account_directory_request_invalid" => StatusCode::BAD_REQUEST,
        "app_account_directory_consent_required" | "app_account_directory_not_installed" => StatusCode::FORBIDDEN,
        "app_account_directory_capacity" => StatusCode::TOO_MANY_REQUESTS,
        "app_account_directory_scope_invalid" | "app_account_directory_timeout" | "app_account_directory_reply_invalid" => StatusCode::SERVICE_UNAVAILABLE,
        "app_account_im_user_read_operation_timeout" | "app_account_im_scope_invalid" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    }
}
fn failure(code: &'static str) -> Response {
    no_store((error_status(code), Json(ApiResponse::<()>::error(code))).into_response())
}
fn success<T: Serialize>(status: StatusCode, result: Result<T, &'static str>) -> Response {
    match result {
        Ok(value) => no_store((status, Json(ApiResponse::success_with_message("ok", value))).into_response()),
        Err(code) => failure(code),
    }
}
fn now() -> u64 { chrono::Utc::now().timestamp().max(0) as u64 }

pub async fn jegotrip_configuration(State(manager):State<Arc<AppLoginManager>>)->Response {
    success(StatusCode::OK,manager.configuration())
}
pub async fn jegotrip_configure(State(manager):State<Arc<AppLoginManager>>,request:Result<Json<ConfigurationWriteRequest>,JsonRejection>)->Response {
    let Ok(Json(request))=request else{return failure("app_login_request_invalid");};
    match manager.configure(request) {
        Ok(view)=>success(StatusCode::OK,Ok(view)),
        // Invalid input to this write API is a client error, unlike a missing
        // or corrupt stored deployment read by setup/login. Keep the latter
        // 503 classification and the original initialization/CAS guards.
        Err(e)=>failure(if e.code=="app_login_deployment_invalid" {"app_login_request_invalid"} else {e.code}),
    }
}
pub async fn jegotrip_clear_configuration(State(manager):State<Arc<AppLoginManager>>,request:Result<Query<RevisionRequest>,QueryRejection>)->Response {
    let Ok(Query(request))=request else{return failure("app_login_request_invalid");};
    match manager.clear_configuration(request.expected_revision) {Ok(view)=>success(StatusCode::OK,Ok(view)),Err(e)=>failure(e.code)}
}
pub async fn jegotrip_accounts(State(manager):State<Arc<AppLoginManager>>)->Response {
    success(StatusCode::OK,manager.app_accounts())
}
/// Shared App-version state: value, provenance, age and which sources are
/// configured. No secret from the deployment is exported.
pub async fn jegotrip_app_version(State(manager):State<Arc<AppLoginManager>>)->Response {
    success(StatusCode::OK,manager.app_version_view().map_err(|e|e.code))
}
/// Force one remote lookup (App Store, then the official page) and persist it.
pub async fn jegotrip_refresh_app_version(State(manager):State<Arc<AppLoginManager>>)->Response {
    match manager.refresh_app_version().await {Ok(view)=>success(StatusCode::OK,Ok(view)),Err(e)=>failure(e.code)}
}
/// Explicit operator override, stored as source=manual.
pub async fn jegotrip_set_app_version(State(manager):State<Arc<AppLoginManager>>,request:Result<Json<AppVersionWriteRequest>,JsonRejection>)->Response {
    let Ok(Json(request))=request else{return failure("app_login_request_invalid");};
    match manager.set_app_version(&request.version) {Ok(view)=>success(StatusCode::OK,Ok(view)),Err(e)=>failure(e.code)}
}
/// Persisted App identities and honest number bindings, not trust credentials.
pub async fn jegotrip_device_identities(State(manager):State<Arc<AppLoginManager>>)->Response {
    success(StatusCode::OK,manager.device_identities().map_err(|e|e.code))
}
pub async fn jegotrip_set_device_identity(State(manager):State<Arc<AppLoginManager>>,request:Result<Json<DeviceIdentityWriteRequest>,JsonRejection>)->Response {
    let Ok(Json(request))=request else{return failure("app_login_request_invalid");};
    match manager.set_device_identity(request) {Ok(views)=>success(StatusCode::OK,Ok(views)),Err(e)=>failure(e.code)}
}
pub async fn jegotrip_delete_device_identity(State(manager):State<Arc<AppLoginManager>>,Path(id):Path<String>,
    request:Result<Query<RevisionRequest>,QueryRejection>)->Response {
    let Ok(Query(request))=request else{return failure("app_login_request_invalid");};
    match manager.delete_device_identity(&id,request.expected_revision) {Ok(views)=>success(StatusCode::OK,Ok(views)),Err(e)=>failure(e.code)}
}
pub async fn jegotrip_login_submit(State(manager):State<Arc<AppLoginManager>>,request:Result<Json<JegotripLoginRequest>,JsonRejection>)->Response {
    let Ok(Json(request))=request else{return failure("app_login_request_invalid");};
    match manager.submit_jegotrip(request) {Ok(view)=>success(StatusCode::ACCEPTED,Ok(view)),Err(e)=>failure(e.code)}
}

pub async fn jegotrip_push_status(State(manager):State<Arc<AppLoginManager>>)->Response {
    success(StatusCode::OK,manager.push_status().await.map_err(|e|e.code))
}
pub async fn app_login_setup(State(manager): State<Arc<AppLoginManager>>) -> Response {
    success(StatusCode::OK, Ok(manager.setup()))
}
pub async fn app_login_attempts(State(manager): State<Arc<AppLoginManager>>) -> Response {
    success(StatusCode::OK, manager.list())
}
pub async fn app_login_attempt(State(manager): State<Arc<AppLoginManager>>, Path(id): Path<String>) -> Response {
    success(StatusCode::OK, manager.get(&id))
}
pub async fn app_login_submit(State(manager): State<Arc<AppLoginManager>>, request: Result<Json<LoginRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_login_request_invalid"); };
    match manager.submit(request) {
        Ok(view) => success(StatusCode::ACCEPTED, Ok(view)),
        Err(error) => failure(error.code),
    }
}

pub async fn list_accounts(State(manager): State<Arc<AccountManager>>) -> Response {
    success(StatusCode::OK, manager.list(now()).await)
}
pub async fn get_account(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>) -> Response {
    success(StatusCode::OK, manager.get(&line_id, now()).await)
}
pub async fn create_account(State(manager): State<Arc<AccountManager>>, request: Result<Json<CreateAccountRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    if request.provider_id != PROVIDER_ID { return failure("app_account_provider_unsupported"); }
    success(StatusCode::CREATED, manager.create(request.config, now()).await)
}
pub async fn replace_account(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Json<ReplaceAccountRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    success(StatusCode::OK, manager.replace(&line_id, request.expected_revision, request.config, now()).await)
}
pub async fn delete_account(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Query<RevisionRequest>, QueryRejection>) -> Response {
    let Ok(Query(request)) = request else { return failure("app_account_request_invalid"); };
    success(StatusCode::OK, manager.delete(&line_id, request.expected_revision).await)
}
pub async fn get_inbox(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Query<InboxQuery>, QueryRejection>) -> Response {
    let Ok(Query(request)) = request else { return failure("app_account_request_invalid"); };
    if !(1..=128).contains(&request.limit) { return failure("app_account_request_invalid"); }
    // Local ids/unclassified status only. Raw body/sender/time and protocol
    // sequences/nonces stay private; this is not a public SMS feed.
    success(StatusCode::OK, manager.inbox(&line_id, request.limit))
}

pub async fn get_messages(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Query<InboxQuery>, QueryRejection>) -> Response {
    let Ok(Query(request)) = request else { return failure("app_account_request_invalid"); };
    if !(1..=128).contains(&request.limit) { return failure("app_account_request_invalid"); }
    // Only exact recovered Smgw text projections. Generic raw bytes, original
    // routing/credentials and guessed time/remote ids never enter this API.
    success(StatusCode::OK, manager.messages(&line_id, request.limit))
}

pub async fn get_notifications(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,
    request:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};
    if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.notification_status(&line_id,request.limit))
}

pub async fn read_native_directory(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Json<DirectoryReadRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_directory_request_invalid");};
    if request.since.is_empty()||request.since.len()>20||request.group_id.as_ref().is_some_and(|id|id.len()>508){return failure("app_account_directory_request_invalid");}
    let Ok(since)=request.since.parse::<i64>()else{return failure("app_account_directory_request_invalid");};
    if since< -1||since.to_string()!=request.since{return failure("app_account_directory_request_invalid");}
    let prepared=manager.prepare_native_directory(&line_id,request.expected_revision,request.group_id.as_deref(),since,matches!(request.operation,DirectoryReadOperation::Properties),request.consent);
    // Preparation reserved/captured the original native budget and query;
    // destroy HTTP temporary fields before the first manager/network await.
    drop(request);
    let prepared=match prepared{Ok(value)=>value,Err(code)=>return failure(code)};
    success(StatusCode::OK,manager.finish_native_directory(prepared).await)
}
pub async fn get_im_messages(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};
    if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.im_messages(&line_id,request.limit))
}
pub async fn get_im_send_history(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<InboxQuery>,QueryRejection>)->Response{let Ok(Query(request))=request else{return failure("app_account_request_invalid");};if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}success(StatusCode::OK,manager.im_send_history(&line_id,request.limit))}
fn im_send_failure(code:&'static str,not_admitted:bool,nonce:&str)->Response{
    // Exact request correlation DATA. Never export the user nonce/recipient,
    // original identity, scope or token; generic failures have no stage claim.
    let mut hash=ring::digest::Context::new(&ring::digest::SHA256);
    hash.update(b"original-im-send-ui-nonce-v1\0");hash.update(nonce.as_bytes());
    let nonce_tag:String=hash.finish().as_ref().iter().map(|b|format!("{b:02x}")).collect();
    no_store((error_status(code),Json(serde_json::json!({"status":"error","message":code,"im_send_admission":if not_admitted{"not_admitted"}else{"admitted_or_unknown"},"nonce_tag":nonce_tag}))).into_response())
}
pub async fn send_im_message(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Json<ImSendRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_request_invalid");};
    if request.nonce.len()!=32||!request.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return failure("app_account_request_invalid");}
    if request.recipient.len()>512||request.content.len()>65536||request.attachments.len()>128||request.parameters.len()>128||request.notifications.as_ref().is_some_and(|v|v.len()>128)||request.notification_digest.as_ref().is_some_and(|value|value.len()>65536){return im_send_failure("app_account_request_invalid",true,&request.nonce);}
    let fields=|v:&[ImSendDataField]|v.iter().map(|p|(p.key.clone(),p.value.clone())).collect::<Vec<_>>();
    let body={
        let attachments=zeroize::Zeroizing::new(fields(&request.attachments));
        let parameters=zeroize::Zeroizing::new(fields(&request.parameters));
        let notifications=request.notifications.as_ref().map(|v|zeroize::Zeroizing::new(fields(v)));
        crate::connectivity::app_voice::providers::jegotrip::im_inbox::ImSendMessage::components(request.info_type.as_deref(),&request.content,&attachments,&parameters,notifications.as_ref().map(|v|v.as_slice()))
    };
    let body=match body{Ok(body)=>body,Err(code)=>return im_send_failure(code,true,&request.nonce)};
    let body=if let Some(value)=&request.notification_digest{match body.with_notification_digest(value){Ok(body)=>body,Err(code)=>return im_send_failure(code,true,&request.nonce)}}else{body};
    let revision=request.expected_revision;let consent=request.consent;let write_consent=request.write_consent;
    let recipient=zeroize::Zeroizing::new(request.recipient.clone());let nonce=zeroize::Zeroizing::new(request.nonce.clone());
    // Erase the deserialized payload and temporary field copies BEFORE the
    // manager's first await; only the budgeted ImSendMessage is retained.
    drop(request);
    match manager.send_im_message_with_admission(&line_id,revision,&recipient,body,&nonce,consent,write_consent).await{
        Ok(view)=>success(StatusCode::OK,Ok(view)),
        Err(error)=>im_send_failure(error.code(),error.not_admitted(),&nonce),
    }
}
pub async fn get_im_receipts(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};
    if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.im_received_receipts(&line_id,request.limit))
}
pub async fn get_im_read_status_targets(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<ImReadStatusTargetsQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};success(StatusCode::OK,manager.im_read_status_targets(&line_id,request.expected_revision).await)
}
pub async fn get_im_read_status_history(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}success(StatusCode::OK,manager.im_read_status_history(&line_id,request.limit))
}
pub async fn set_im_read_status(State(manager):State<Arc<AccountManager>>,Path((line_id,target_id)):Path<(String,String)>,request:Result<Json<ImReadStatusRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_request_invalid");};let Ok(id)=target_id.parse::<i64>()else{return failure("app_account_request_invalid");};let Ok(index)=request.expected_index.parse::<i64>()else{return failure("app_account_request_invalid");};
    if id<=0||id.to_string()!=target_id||index<0||index.to_string()!=request.expected_index||request.nonce.len()!=32||!request.nonce.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c)){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.set_im_read_status(&line_id,request.expected_revision,id,&request.expected_index,&request.nonce,request.consent).await)
}
pub async fn set_im_read_status_via_recipient(State(manager):State<Arc<AccountManager>>,Path((line_id,target_id)):Path<(String,String)>,request:Result<Json<ImReadStatusRecipientRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_request_invalid");};let Ok(id)=target_id.parse::<i64>()else{return failure("app_account_request_invalid");};let Ok(index)=request.expected_index.parse::<i64>()else{return failure("app_account_request_invalid");};
    if id<=0||id.to_string()!=target_id||index<0||index.to_string()!=request.expected_index||request.nonce.len()!=32||!request.nonce.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c))||!crate::connectivity::app_voice::providers::jegotrip::valid_user_read_recipient(&request.recipient){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.set_im_read_status_via_recipient(&line_id,request.expected_revision,id,&request.expected_index,&request.nonce,&request.recipient,request.consent).await)
}
pub async fn get_im_user_reads(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}success(StatusCode::OK,manager.im_user_read_history(&line_id,request.limit))
}
pub async fn mark_im_user_read(State(manager):State<Arc<AccountManager>>,Path((line_id,message_id)):Path<(String,String)>,request:Result<Json<ImUserReadRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_request_invalid");};let Ok(id)=message_id.parse::<i64>()else{return failure("app_account_request_invalid");};
    if id<=0||id.to_string()!=message_id||request.nonce.len()!=32||!request.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.mark_im_user_read(&line_id,request.expected_revision,id,&request.nonce,request.consent).await)
}
pub async fn mark_im_user_read_recipient(State(manager):State<Arc<AccountManager>>,Path((line_id,message_id)):Path<(String,String)>,request:Result<Json<ImUserReadRecipientRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_request_invalid");};let Ok(id)=message_id.parse::<i64>()else{return failure("app_account_request_invalid");};
    if id<=0||id.to_string()!=message_id||request.nonce.len()!=32||!request.nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||!crate::connectivity::app_voice::providers::jegotrip::valid_user_read_recipient(&request.recipient){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.mark_im_user_read_via_recipient(&line_id,request.expected_revision,id,&request.nonce,&request.recipient,request.consent).await)
}
pub async fn reconcile_im_user_read(State(manager): State<Arc<AccountManager>>, Path((line_id, receipt_id)): Path<(String,String)>,
    request: Result<Json<ImReconciliationRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    let Ok(id) = receipt_id.parse::<i64>() else { return failure("app_account_request_invalid"); };
    if id <= 0 || id.to_string() != receipt_id { return failure("app_account_request_invalid"); }
    success(StatusCode::OK, manager.reconcile_im_user_read(&line_id, request.expected_revision, id, request.consent).await)
}
pub async fn lookup_user_entry(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Json<UserEntryLookupRequest>,JsonRejection>)->Response{
    let Ok(Json(request))=request else{return failure("app_account_request_invalid");};
    if !crate::connectivity::app_voice::providers::jegotrip::valid_user_entry_recipient(&request.recipient){return failure("app_account_request_invalid");}
    let prepared=manager.prepare_user_entry(&line_id,request.expected_revision,&request.recipient,request.consent);
    // The original run/input/deadline is captured synchronously. Wipe the
    // temporary HTTP recipient before controller acquisition or any network IO.
    drop(request);
    let prepared=match prepared{Ok(value)=>value,Err(code)=>return failure(code)};
    success(StatusCode::OK,manager.finish_user_entry(prepared).await)
}
pub async fn delete_im_message(State(manager):State<Arc<AccountManager>>,Path((line_id,message_id)):Path<(String,String)>)->Response{
    let Ok(id)=message_id.parse::<i64>()else{return failure("app_account_request_invalid");};
    if id<=0||id.to_string()!=message_id{return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.delete_im_message(&line_id,id))
}

pub async fn get_call_notifications(State(manager):State<Arc<AccountManager>>,Path(line_id):Path<String>,request:Result<Query<InboxQuery>,QueryRejection>)->Response{
    let Ok(Query(request))=request else{return failure("app_account_request_invalid");};
    if !(1..=128).contains(&request.limit){return failure("app_account_request_invalid");}
    success(StatusCode::OK,manager.call_notification_status(&line_id,request.limit))
}
fn call_id_valid(value: &str) -> bool { value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) }
pub async fn get_calls(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>) -> Response {
    success(StatusCode::OK, manager.calls(&line_id).await)
}
pub async fn get_call(State(manager): State<Arc<AccountManager>>, Path((line_id, call_id)): Path<(String,String)>) -> Response {
    if !call_id_valid(&call_id) { return failure("app_account_request_invalid"); }
    success(StatusCode::OK, manager.call(&line_id,&call_id).await)
}
pub async fn answer_call(State(manager): State<Arc<AccountManager>>, Path((line_id, call_id)): Path<(String,String)>,
    request: Result<Json<AnswerCallRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    if !call_id_valid(&call_id) || request.sdp.is_empty() || request.sdp.len()>16_384 { return failure("app_account_request_invalid"); }
    success(StatusCode::OK, manager.answer_call(&line_id,request.expected_revision,&call_id,request.sdp.into_bytes(),now()).await)
}
pub async fn hangup_call(State(manager): State<Arc<AccountManager>>, Path((line_id, call_id)): Path<(String,String)>,
    request: Result<Json<RevisionRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    if !call_id_valid(&call_id) { return failure("app_account_request_invalid"); }
    success(StatusCode::OK, manager.hangup_call(&line_id,request.expected_revision,&call_id,now()).await)
}
pub async fn get_call_trunk_status(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>) -> Response {
    success(StatusCode::OK, manager.call_trunk_status(&line_id).await)
}
pub async fn reserved_outbound_call(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Json<OutboundCallRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    if request.target.trim().is_empty() || request.target.len()>128 { return failure("app_account_request_invalid"); }
    success(StatusCode::OK, manager.reserve_outbound(&line_id,request.expected_revision))
}
pub async fn reserved_outbound_message(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Json<OutboundMessageRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    if request.target.trim().is_empty() || request.target.len()>128 || request.body.is_empty() || request.body.len()>16_384 { return failure("app_account_request_invalid"); }
    success(StatusCode::OK, manager.reserve_outbound(&line_id,request.expected_revision))
}

pub async fn start_account(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Json<RevisionRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    // Accepted is an actual original RPC submission, not SDK/hosting completion.
    success(StatusCode::ACCEPTED, manager.start(&line_id, request.expected_revision, now()).await)
}
pub async fn stop_account(State(manager): State<Arc<AccountManager>>, Path(line_id): Path<String>,
    request: Result<Json<RevisionRequest>, JsonRejection>) -> Response {
    let Ok(Json(request)) = request else { return failure("app_account_request_invalid"); };
    success(StatusCode::OK, manager.stop(&line_id, request.expected_revision, now()).await)
}
