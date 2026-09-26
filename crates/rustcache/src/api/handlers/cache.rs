//! `/api/cache` — on-disk cache size and purge.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::api::state::ApiState;

#[derive(Serialize)]
pub struct CacheInfo {
    bytes: u64,
    entries: u64,
}

/// On-disk cache usage (bytes and entry count).
pub async fn cache_info(State(st): State<ApiState>) -> Result<Json<CacheInfo>, ApiError> {
    let (bytes, entries) = st
        .engine
        .cache_size()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(CacheInfo { bytes, entries }))
}

/// Drop every cached entry; reports how many were purged.
pub async fn purge_cache(State(st): State<ApiState>) -> Result<Json<serde_json::Value>, ApiError> {
    let n = st
        .engine
        .purge_all()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(serde_json::json!({"ok": true, "purged": n})))
}
