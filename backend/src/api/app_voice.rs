//! Authenticated, redacted application-account management, separate from SIM APIs.
//! No credential export, caller-supplied callbacks or cached readiness restore.
//! Incoming-call SDP/actions are bounded and original-owned; reserved outbound
//! endpoints cannot authorize SDK/SIM/IMS/CS sends. Native start is explicit.
use axum::{
    extract::{rejection::{JsonRejection, QueryRejection}, DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post}, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{
    api::models::ApiResponse,
    connectivity::app_voice::providers::jegotrip::{account_config::AccountConfig, account_runtime::AccountManager, PROVIDER_ID},
    state::AppState,
};

const MAX_REQUEST_BYTES: usize = 256 * 1024;
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
fn default_inbox_limit() -> usize { 50 }

/// Merged into main's protected routes BEFORE its existing auth middleware.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/non-3gpp/accounts", get(list_accounts).post(create_account))
        .route("/api/non-3gpp/accounts/{line_id}", get(get_account).put(replace_account).delete(delete_account))
        .route("/api/non-3gpp/accounts/{line_id}/start", post(start_account))
        .route("/api/non-3gpp/accounts/{line_id}/stop", post(stop_account))
        .route("/api/non-3gpp/accounts/{line_id}/inbox", get(get_inbox))
        .route("/api/non-3gpp/accounts/{line_id}/messages", get(get_messages))
        .route("/api/non-3gpp/accounts/{line_id}/notifications", get(get_notifications))
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
        "app_account_not_found" | "app_account_call_not_found" => StatusCode::NOT_FOUND,
        "app_account_revision_conflict" | "app_account_stop_before_replace" | "app_account_call_operation_pending" => StatusCode::CONFLICT,
        "app_account_provider_unsupported" | "app_account_outbound_unsupported" => StatusCode::NOT_IMPLEMENTED,
        "app_account_storage_failed" | "app_account_inbox_storage_failed" | "app_account_sms_storage_failed"
            | "app_account_run_storage_failed" | "app_account_notification_storage_failed" | "app_account_call_storage_failed" | "database_unavailable" => StatusCode::INTERNAL_SERVER_ERROR,
        "app_account_limit" | "app_account_run_limit" | "app_account_entropy_unavailable" | "app_account_backend_unavailable"
            | "app_account_authentication_rejected" | "app_account_disconnected" | "app_account_hosting_rejected"
            | "app_account_operation_timeout" | "app_account_shutting_down" | "app_account_cleanup_unconfirmed"
            | "app_account_call_runtime_unavailable" | "app_account_call_cleanup_unconfirmed"
            | "app_account_call_trunk_delivery_unconfirmed" => StatusCode::SERVICE_UNAVAILABLE,
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
