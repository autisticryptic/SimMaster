use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::db::Database;
use crate::models::ApiResponse;

#[derive(Debug, Default, Deserialize)]
pub struct NotificationQueueQuery {
    #[serde(default = "default_notification_queue_limit")]
    pub limit: i64,
}

fn default_notification_queue_limit() -> i64 {
    100
}

/// GET /api/notifications/queue
pub async fn get_notification_queue_handler(
    Query(query): Query<NotificationQueueQuery>,
    State(database): State<Arc<Database>>,
) -> (
    StatusCode,
    Json<ApiResponse<crate::db::NotificationQueueResponse>>,
) {
    match database.get_notification_queue(query.limit) {
        Ok(queue) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message("Success", queue)),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// POST /api/notifications/queue/{id}/retry
pub async fn retry_notification_queue_item_handler(
    Path(id): Path<i64>,
    State(database): State<Arc<Database>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match database.retry_notification_queue_item(id) {
        Ok(updated) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification queue item scheduled for retry",
                json!({ "updated": updated }),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// DELETE /api/notifications/queue/{id}
pub async fn delete_notification_queue_item_handler(
    Path(id): Path<i64>,
    State(database): State<Arc<Database>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match database.delete_notification_queue_item(id) {
        Ok(updated) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification queue item cancelled",
                json!({ "updated": updated }),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// POST /api/notifications/queue/retry-all
pub async fn retry_all_notification_queue_handler(
    State(database): State<Arc<Database>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match database.retry_all_notification_queue_items() {
        Ok(updated) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification queue items scheduled for retry",
                json!({ "updated": updated }),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}

/// POST /api/notifications/queue/clear
pub async fn clear_notification_queue_handler(
    State(database): State<Arc<Database>>,
) -> (StatusCode, Json<ApiResponse<serde_json::Value>>) {
    match database.clear_active_notification_queue() {
        Ok(updated) => (
            StatusCode::OK,
            Json(ApiResponse::success_with_message(
                "Notification queue cleared",
                json!({ "updated": updated }),
            )),
        ),
        Err(err) => (
            StatusCode::OK,
            Json(ApiResponse::error(format!("Failed: {}", err))),
        ),
    }
}
