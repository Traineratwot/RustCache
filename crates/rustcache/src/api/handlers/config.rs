//! `/api/config*` — get/replace config, file reload, process restart.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::state::ApiState;
use crate::config::persist::write_config_toml;
use crate::config::validate::{restart_fields_diff, validate_config};
use crate::config::Config;

/// The whole effective config (paths, ports, limits — no secrets).
pub async fn get_config(State(st): State<ApiState>) -> Json<serde_json::Value> {
    let cfg = st.config.get().await;
    // Config holds no secrets (paths, ports, limits) — safe to return as-is.
    Json(serde_json::to_value(cfg.as_ref().clone()).unwrap_or(serde_json::json!({})))
}

/// Replace the whole effective config (all TOML keys). Hot-applies exclusion +
/// cache-limit + log-retention changes; listener/path changes need a restart.
/// Never restarts the process itself — see `restart_process`.
pub async fn put_config(
    State(st): State<ApiState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut new_cfg: Config = serde_json::from_value(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid config: {e}")))?;
    // Keep CLI --data-dir override if the client omitted/blanked data_dir.
    let old = st.config.get().await;
    if new_cfg.data_dir.trim().is_empty() {
        new_cfg.data_dir = old.data_dir.clone();
    }
    validate_config(&new_cfg, &old).map_err(ApiError::FieldIssues)?;

    let restart = restart_fields_diff(&old, &new_cfg);

    write_config_toml(&st.config_path, &new_cfg).map_err(|e| ApiError::Internal(e.to_string()))?;

    // Hot-apply what we can without rebinding sockets / reopening stores.
    st.apply_config(new_cfg.clone())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    st.config.set(new_cfg.clone()).await;

    // Immediate log-retention pass so the UI reflects the new limits.
    match st
        .engine
        .logs()
        .cleanup(new_cfg.logs.max_rows, new_cfg.logs.max_age_days)
        .await
    {
        Ok((by_age, by_rows)) => {
            if by_age + by_rows > 0 {
                tracing::info!(by_age, by_rows, "request log cleanup (config)");
            }
        }
        Err(e) => tracing::warn!(error = %e, "request log cleanup failed"),
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "config": new_cfg.clone(),
        "restart_required": !restart.is_empty(),
        "restart_fields": restart,
    })))
}

/// Re-read `config.toml` from disk and hot-apply it.
pub async fn reload_config(
    State(st): State<ApiState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let cfg = st
        .reload_config()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(serde_json::json!({"ok": true, "config": cfg.clone()})))
}

#[derive(Deserialize)]
pub struct RestartQuery {
    dry_run: Option<String>,
}

fn is_truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1") | Some("true") | Some("yes") | Some("on"))
}

/// Re-exec the current process with the same argv (used by the UI restart button).
/// `?dry_run=1` (or `true`) only acknowledges — used by tests so the harness does not exit.
pub async fn restart_process(Query(q): Query<RestartQuery>) -> Json<serde_json::Value> {
    if is_truthy(q.dry_run.as_deref()) {
        return Json(serde_json::json!({"ok": true, "dry_run": true}));
    }
    tokio::spawn(async {
        // Give the HTTP response time to flush before we tear the process down.
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "restart: current_exe failed");
                std::process::exit(1);
            }
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let parent = std::process::id().to_string();
        match std::process::Command::new(&exe)
            .args(&args)
            .env("RUSTCACHE_RESTARTED_FROM", &parent)
            .spawn()
        {
            Ok(child) => {
                tracing::info!(child = child.id(), "restart: spawned replacement, exiting");
                std::process::exit(0);
            }
            Err(e) => {
                tracing::error!(error = %e, "restart: spawn failed");
                std::process::exit(1);
            }
        }
    });
    Json(serde_json::json!({"ok": true, "message": "restarting"}))
}
