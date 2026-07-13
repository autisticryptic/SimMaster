//! Web API layer: HTTP handlers, request/response models, and authentication.
//!
//! This groups the outward-facing surface of the service:
//!   - `handlers`: axum route handlers for every `/api/*` endpoint
//!   - `models`: request/response DTOs shared across handlers
//!   - `auth`: password/session authentication + the auth middleware

pub mod auth;
pub mod handlers;
pub mod models;
