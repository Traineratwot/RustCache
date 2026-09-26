//! Shared JSON error responses for the REST API.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::config::validate::FieldIssue;

/// Shared JSON error responses for the REST API.
///
/// Each variant renders a fixed JSON shape the web client already understands:
/// * `BadRequest` — 400 `{"ok": false, "error": "..."}`
/// * `Internal` — 500 `{"error": "..."}`
/// * `FieldIssues` — 400 `{"ok": false, "error": "...", "errors": [...]}`
pub enum ApiError {
    /// 400 — the request was rejected without per-field detail.
    BadRequest(String),
    /// 500 — storage, write, or apply failure.
    Internal(String),
    /// 400 — per-field config validation problems (Settings form highlights).
    FieldIssues(Vec<FieldIssue>),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": msg})),
            )
                .into_response(),
            ApiError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": msg})),
            )
                .into_response(),
            ApiError::FieldIssues(issues) => {
                let summary = issues
                    .iter()
                    .map(|i| format!("{}: {}", i.field, i.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"ok": false, "error": summary, "errors": issues})),
                )
                    .into_response()
            }
        }
    }
}
