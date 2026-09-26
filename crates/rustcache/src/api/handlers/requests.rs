//! `/api/requests` — request-log query and clear.

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::state::ApiState;

#[derive(Deserialize)]
pub struct RequestsQuery {
    q: Option<String>,
    method: Option<String>,
    outcome: Option<rustcache_core::stats::Outcome>,
    status_min: Option<u16>,
    status_max: Option<u16>,
    since: Option<u64>,
    until: Option<u64>,
    limit: Option<u32>,
    offset: Option<u32>,
}

/// Filterable page of logged requests.
pub async fn requests(
    State(st): State<ApiState>,
    Query(q): Query<RequestsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let query = rustcache_core::stats::LogQuery {
        q: q.q,
        method: q.method,
        outcome: q.outcome,
        status_min: q.status_min,
        status_max: q.status_max,
        since_ms: q.since,
        until_ms: q.until,
        limit: q.limit.unwrap_or(50).clamp(1, 500),
        offset: q.offset.unwrap_or(0),
    };
    let limit = query.limit;
    let offset = query.offset;
    let page = st
        .engine
        .logs()
        .query(query)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({
        "requests": page.requests,
        "total": page.total,
        "limit": limit,
        "offset": offset,
    })))
}

/// Drop every logged request; reports how many rows were deleted.
pub async fn clear_requests(
    State(st): State<ApiState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let deleted = st
        .engine
        .logs()
        .clear()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({"ok": true, "deleted": deleted})))
}
