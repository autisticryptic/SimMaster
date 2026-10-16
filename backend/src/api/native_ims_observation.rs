//! Explicit authenticated modem-native status probing; cached GET never does IO.
use axum::{extract::{Path, State}, http::StatusCode, response::{IntoResponse, Response}, Json};
use serde::Deserialize;
use crate::{api::models::ApiResponse, hardware::cellular::native_ims::NativeImsProbeError,
    services::native_ims_observation as observation, state::AppState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {}

fn failure(status: StatusCode, code: &'static str) -> Response {
    (status, Json(ApiResponse::<serde_json::Value>::error(code))).into_response()
}

pub async fn status(State(app): State<AppState>, Path(line_id): Path<String>) -> Response {
    match observation::status(&app.line_registry, &app.config_manager, &app.shutdown, &line_id).await {
        Some(status) => Json(ApiResponse::success_with_message("Cached modem observation; no device query", status)).into_response(),
        None => failure(StatusCode::NOT_FOUND, "line_not_found"),
    }
}

pub async fn probe(State(app): State<AppState>, Path(line_id): Path<String>, Json(_): Json<ProbeRequest>) -> Response {
    if app.line_registry.get(&line_id).await.is_none() {
        return failure(StatusCode::NOT_FOUND, "line_not_found");
    }
    match observation::probe(app.line_registry.clone(), app.config_manager.clone(), app.shutdown.clone(), &line_id).await {
        Ok(status) => Json(ApiResponse::success_with_message("One read-only modem observation completed; not host IMS readiness", status)).into_response(),
        Err(error) => failure(match error {
            NativeImsProbeError::Unsupported | NativeImsProbeError::FirmwareNotQualified => StatusCode::NOT_IMPLEMENTED,
            NativeImsProbeError::Busy | NativeImsProbeError::BindingChanged => StatusCode::CONFLICT,
            NativeImsProbeError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            NativeImsProbeError::MalformedResponse => StatusCode::BAD_GATEWAY,
        }, error.code()),
    }
}
