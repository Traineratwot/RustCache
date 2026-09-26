//! `/api/logs/*` — request-log stats and retention settings.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::state::ApiState;
use crate::config::persist::write_config_toml;
use crate::config::validate::validate_log_settings;

#[derive(Deserialize)]
pub struct LogStatsQueryParams {
    since: Option<u64>,
    until: Option<u64>,
}

/// Aggregated request-log stats (hit rate, bytes saved, durations).
pub async fn log_stats(
    State(st): State<ApiState>,
    Query(q): Query<LogStatsQueryParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let query = rustcache_core::stats::LogStatsQuery {
        since_ms: q.since,
        until_ms: q.until,
    };
    let s = st
        .engine
        .logs()
        .stats(query)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(
        serde_json::to_value(s).unwrap_or(serde_json::json!({})),
    ))
}

#[derive(Serialize)]
pub struct LogSettingsBody {
    max_rows: u64,
    max_age_days: u64,
    cleanup_interval_secs: u64,
}

/// Current request-log retention settings.
pub async fn get_log_settings(State(st): State<ApiState>) -> Json<serde_json::Value> {
    let cfg = st.config.get().await;
    Json(serde_json::json!({
        "max_rows": cfg.logs.max_rows,
        "max_age_days": cfg.logs.max_age_days,
        "cleanup_interval_secs": cfg.logs.cleanup_interval_secs,
    }))
}

#[derive(Deserialize)]
pub struct LogSettingsUpdate {
    max_rows: Option<u64>,
    max_age_days: Option<u64>,
    cleanup_interval_secs: Option<u64>,
}

/// Replace retention settings (persisted to config.toml) and apply them now.
pub async fn put_log_settings(
    State(st): State<ApiState>,
    Json(body): Json<LogSettingsUpdate>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Err(issue) =
        validate_log_settings(body.max_rows, body.max_age_days, body.cleanup_interval_secs)
    {
        return Err(ApiError::BadRequest(format!(
            "{} {}",
            issue.field, issue.message
        )));
    }
    let cfg = st.config.get().await;
    let mut new_cfg = cfg.as_ref().clone();
    if let Some(v) = body.max_rows {
        new_cfg.logs.max_rows = v;
    }
    if let Some(v) = body.max_age_days {
        new_cfg.logs.max_age_days = v;
    }
    if let Some(v) = body.cleanup_interval_secs {
        new_cfg.logs.cleanup_interval_secs = v;
    }

    let settings = LogSettingsBody {
        max_rows: new_cfg.logs.max_rows,
        max_age_days: new_cfg.logs.max_age_days,
        cleanup_interval_secs: new_cfg.logs.cleanup_interval_secs,
    };

    if let Err(e) = write_config_toml(&st.config_path, &new_cfg) {
        tracing::warn!(error = %e, "config write failed");
    }
    st.config.set(new_cfg).await;

    // Apply retention immediately so the UI reflects the new limits.
    match st
        .engine
        .logs()
        .cleanup(settings.max_rows, settings.max_age_days)
        .await
    {
        Ok((by_age, by_rows)) => {
            if by_age + by_rows > 0 {
                tracing::info!(by_age, by_rows, "request log cleanup (settings)");
            }
        }
        Err(e) => tracing::warn!(error = %e, "request log cleanup failed"),
    }

    Ok(Json(serde_json::json!({"ok": true, "settings": settings})))
}
