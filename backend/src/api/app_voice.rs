//! Authenticated, redacted application-account management, separate from SIM APIs.
//! No credential export, caller-supplied callbacks, SMS/media commands or cached
//! readiness restoration. Native start is explicit and revision-bound.
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
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}
fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    response
}
fn error_status(code: &str) -> StatusCode {
    match code {
        "app_account_not_found" => StatusCode::NOT_FOUND,
        "app_account_revision_conflict" | "app_account_stop_before_replace" => StatusCode::CONFLICT,
        "app_account_provider_unsupported" => StatusCode::NOT_IMPLEMENTED,
        "app_account_storage_failed" | "app_account_inbox_storage_failed" | "app_account_sms_storage_failed"
            | "app_account_run_storage_failed" | "database_unavailable" => StatusCode::INTERNAL_SERVER_ERROR,
        "app_account_limit" | "app_account_run_limit" | "app_account_entropy_unavailable" | "app_account_backend_unavailable"
            | "app_account_authentication_rejected" | "app_account_disconnected" | "app_account_hosting_rejected"
            | "app_account_operation_timeout" => StatusCode::SERVICE_UNAVAILABLE,
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
