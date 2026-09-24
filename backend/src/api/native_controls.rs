//! Authenticated, explicitly targeted native maintenance. No owner switching.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    api::models::ApiResponse,
    hardware::{
        cellular::backends::{self, native::NativeDevice},
        devices::quectel::maintenance::{self, MaintenanceAction},
    },
    state::AppState,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRequest {
    pub action: MaintenanceAction,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub action: MaintenanceAction,
    pub expected_revision: String,
    pub confirm_line_id: String,
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiResponse::<serde_json::Value>::error(message)),
    )
        .into_response()
}

async fn target(
    app: &AppState,
    line_id: &str,
    maintenance_write: bool,
) -> Result<Arc<NativeDevice>, Response> {
    let line = app
        .line_registry
        .get(line_id)
        .await
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "line_not_found"))?;
    let binding = line.binding();
    if !binding.present {
        return Err(error(StatusCode::CONFLICT, "native_line_absent"));
    }
    let device = backends::native_device(&binding.modem_path)
        .map_err(|e| error(StatusCode::CONFLICT, e.to_string()))?;
    if device.spec.line_id() != line_id {
        return Err(error(StatusCode::CONFLICT, "native_line_identity_mismatch"));
    }
    if maintenance_write {
        let profile = app.config_manager.get_line_profile(line_id);
        if profile.cellular_ims_connection_enabled
            || profile.data_connection_enabled
            || line.cellular_ims.status().await.registered
            || line.data_proxy.status().await.running
        {
            return Err(error(
                StatusCode::CONFLICT,
                "native_maintenance_disable_ims_and_data_first",
            ));
        }
    }
    Ok(device)
}

pub async fn quectel_diagnostics(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
) -> Response {
    let device = match target(&app, &line_id, false).await {
        Ok(d) => d,
        Err(r) => return r,
    };
    match maintenance::inspect(device).await {
        Ok(result) => Json(ApiResponse::success_with_message(
            "Native diagnostic queries completed",
            result,
        ))
        .into_response(),
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}

pub async fn quectel_plan(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(request): Json<PlanRequest>,
) -> Response {
    let device = match target(&app, &line_id, true).await {
        Ok(d) => d,
        Err(r) => return r,
    };
    match maintenance::plan(device, request.action).await {
        Ok(result) => Json(ApiResponse::success_with_message(
            "Review and explicitly confirm this exact line and revision",
            result,
        ))
        .into_response(),
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}

pub async fn quectel_apply(
    State(app): State<AppState>,
    Path(line_id): Path<String>,
    Json(request): Json<ApplyRequest>,
) -> Response {
    let device = match target(&app, &line_id, true).await {
        Ok(d) => d,
        Err(r) => return r,
    };
    match maintenance::apply(
        device,
        request.action,
        request.expected_revision,
        request.confirm_line_id,
    )
    .await
    {
        Ok(result) => {
            let status = if result.reconciliation_required {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(ApiResponse::success_with_message(
                    "Inspect status; request acceptance does not imply completed hardware change",
                    result,
                )),
            )
                .into_response()
        }
        Err(e) => error(StatusCode::CONFLICT, e.to_string()),
    }
}
