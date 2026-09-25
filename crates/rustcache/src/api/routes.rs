//! Axum REST API routes.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::state::ApiState;

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/stats", get(stats))
        .route("/api/requests", get(requests))
        .route("/api/config", get(get_config))
        .route("/api/config/reload", post(reload_config))
        .route(
            "/api/exclusions",
            get(list_exclusions)
                .post(add_exclusion)
                .delete(clear_exclusions),
        )
        .route("/api/ca.crt", get(ca_crt))
        .route("/api/cache", get(cache_info).delete(purge_cache))
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({"ok": true}))
}

async fn stats(State(st): State<ApiState>) -> Json<Value> {
    let snap = st.engine.metrics().snapshot();
    Json(serde_json::to_value(snap).unwrap_or(json!({})))
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

async fn requests(State(st): State<ApiState>, Query(q): Query<LimitQuery>) -> Json<Value> {
    let limit = q.limit.unwrap_or(100).clamp(1, 100);
    let recs = st.engine.ring.snapshot(limit);
    Json(json!({"requests": recs}))
}

async fn get_config(State(st): State<ApiState>) -> Json<Value> {
    let cfg = st.config.get().await;
    Json(serde_json::to_value(cfg.redacted()).unwrap_or(json!({})))
}

async fn reload_config(State(st): State<ApiState>) -> Response {
    match st.reload_config().await {
        Ok(cfg) => Json(json!({"ok": true, "config": cfg.redacted()})).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct ExclusionBody {
    domain: Option<String>,
    cidr: Option<String>,
}

async fn list_exclusions(State(st): State<ApiState>) -> Json<Value> {
    let ms = st.exclusions().await;
    Json(json!({"exclusions": ms}))
}

async fn add_exclusion(State(st): State<ApiState>, Json(body): Json<ExclusionBody>) -> Response {
    let cfg = st.config.get().await;
    let mut domains = cfg.exclude.domains.clone();
    let mut cidrs = cfg.exclude.cidrs.clone();
    if let Some(d) = body.domain {
        if !domains.contains(&d) {
            domains.push(d);
        }
    }
    if let Some(c) = body.cidr {
        if !cidrs.contains(&c) {
            cidrs.push(c);
        }
    }
    st.set_exclusions(domains.clone(), cidrs.clone()).await;
    // persist to config file if possible
    let mut new_cfg = cfg;
    new_cfg.exclude.domains = domains;
    new_cfg.exclude.cidrs = cidrs;
    let _ = std::fs::write(
        &st.config_path,
        toml::to_string_pretty(&new_cfg).unwrap_or_default(),
    );
    st.config.set(new_cfg).await;
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct ExclusionDelete {
    domain: Option<String>,
    cidr: Option<String>,
    #[serde(default)]
    all: bool,
}

async fn clear_exclusions(
    State(st): State<ApiState>,
    Json(body): Json<ExclusionDelete>,
) -> Response {
    let cfg = st.config.get().await;
    let mut domains = cfg.exclude.domains.clone();
    let mut cidrs = cfg.exclude.cidrs.clone();
    if body.all {
        domains.clear();
        cidrs.clear();
    } else {
        if let Some(d) = body.domain {
            domains.retain(|x| x != &d);
        }
        if let Some(c) = body.cidr {
            cidrs.retain(|x| x != &c);
        }
    }
    st.set_exclusions(domains.clone(), cidrs.clone()).await;
    let mut new_cfg = cfg;
    new_cfg.exclude.domains = domains;
    new_cfg.exclude.cidrs = cidrs;
    let _ = std::fs::write(
        &st.config_path,
        toml::to_string_pretty(&new_cfg).unwrap_or_default(),
    );
    st.config.set(new_cfg).await;
    Json(json!({"ok": true})).into_response()
}

async fn ca_crt(State(st): State<ApiState>) -> Response {
    let mut resp = (
        [(axum::http::header::CONTENT_TYPE, "application/x-pem-file")],
        st.ca.cert_pem.clone(),
    )
        .into_response();
    if let Ok(val) = "attachment; filename=\"ca.crt\"".parse() {
        resp.headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, val);
    }
    resp
}

#[derive(Serialize)]
struct CacheInfo {
    bytes: u64,
    entries: u64,
}

async fn cache_info(State(st): State<ApiState>) -> Response {
    match st.engine.cache_size().await {
        Ok((bytes, entries)) => Json(CacheInfo { bytes, entries }).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn purge_cache(State(st): State<ApiState>) -> Response {
    match st.engine.purge_all().await {
        Ok(n) => Json(json!({"ok": true, "purged": n})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}
