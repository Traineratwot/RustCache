//! Axum REST API routes.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::pac::{generate_pac, PacParams};
use super::state::ApiState;
use crate::config::schema::PacMode;

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/stats", get(stats))
        .route("/api/requests", get(requests).delete(clear_requests))
        .route(
            "/api/logs/settings",
            get(get_log_settings).put(put_log_settings),
        )
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
        .route("/api/netinfo", get(netinfo))
        .route("/api/pac", get(pac_info))
        .route("/proxy.pac", get(proxy_pac))
        .route("/wpad.dat", get(proxy_pac))
        .with_state(state)
}

/// Router exposing only the two PAC paths (LAN-facing, no admin endpoints).
pub fn pac_router(state: ApiState) -> Router {
    Router::new()
        .route("/proxy.pac", get(proxy_pac))
        .route("/wpad.dat", get(proxy_pac))
        .with_state(state)
}

async fn health(State(st): State<ApiState>) -> Json<Value> {
    let cfg = st.config.get().await;
    let uptime_s = st.started_at.elapsed().as_secs();

    let listeners = vec![
        probe_listener("HTTP proxy", "0.0.0.0", cfg.http.port).await,
        probe_listener("HTTPS MITM", "0.0.0.0", cfg.https.port).await,
        probe_listener("SOCKS5", "0.0.0.0", cfg.socks5.port).await,
        json!({
            "name": "REST API",
            "bind": cfg.api.bind,
            "port": port_of_bind(&cfg.api.bind),
            "running": true,
        }),
    ];

    Json(json!({
        "ok": true,
        "uptime_s": uptime_s,
        "listeners": listeners,
    }))
}

fn port_of_bind(bind: &str) -> u16 {
    bind.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

async fn probe_listener(name: &str, host: &str, port: u16) -> Value {
    let addr = format!("{host}:{port}");
    let running = tokio::net::TcpStream::connect(&addr).await.is_ok();
    json!({
        "name": name,
        "bind": host,
        "port": port,
        "running": running,
    })
}

async fn stats(State(st): State<ApiState>) -> Json<Value> {
    let snap = st.engine.metrics().snapshot();
    Json(serde_json::to_value(snap).unwrap_or(json!({})))
}

#[derive(Deserialize)]
struct RequestsQuery {
    q: Option<String>,
    method: Option<String>,
    outcome: Option<String>,
    status_min: Option<u16>,
    status_max: Option<u16>,
    since: Option<u64>,
    until: Option<u64>,
    limit: Option<u32>,
    offset: Option<u32>,
}

async fn requests(State(st): State<ApiState>, Query(q): Query<RequestsQuery>) -> Response {
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
    match st.engine.logs.query(query).await {
        Ok(page) => Json(json!({
            "requests": page.requests,
            "total": page.total,
            "limit": limit,
            "offset": offset,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn clear_requests(State(st): State<ApiState>) -> Response {
    match st.engine.logs.clear().await {
        Ok(deleted) => Json(json!({"ok": true, "deleted": deleted})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
struct LogSettingsBody {
    max_rows: u64,
    max_age_days: u64,
    cleanup_interval_secs: u64,
}

async fn get_log_settings(State(st): State<ApiState>) -> Json<Value> {
    let cfg = st.config.get().await;
    Json(json!({
        "max_rows": cfg.logs.max_rows,
        "max_age_days": cfg.logs.max_age_days,
        "cleanup_interval_secs": cfg.logs.cleanup_interval_secs,
    }))
}

#[derive(Deserialize)]
struct LogSettingsUpdate {
    max_rows: Option<u64>,
    max_age_days: Option<u64>,
    cleanup_interval_secs: Option<u64>,
}

async fn put_log_settings(
    State(st): State<ApiState>,
    Json(body): Json<LogSettingsUpdate>,
) -> Response {
    let cfg = st.config.get().await;
    let mut new_cfg = cfg.clone();
    if let Some(v) = body.max_rows {
        if !(1..=10_000_000).contains(&v) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "max_rows must be 1..=10000000"})),
            )
                .into_response();
        }
        new_cfg.logs.max_rows = v;
    }
    if let Some(v) = body.max_age_days {
        if !(1..=3650).contains(&v) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "max_age_days must be 1..=3650"})),
            )
                .into_response();
        }
        new_cfg.logs.max_age_days = v;
    }
    if let Some(v) = body.cleanup_interval_secs {
        if !(10..=86_400).contains(&v) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": "cleanup_interval_secs must be 10..=86400"})),
            )
                .into_response();
        }
        new_cfg.logs.cleanup_interval_secs = v;
    }

    let settings = LogSettingsBody {
        max_rows: new_cfg.logs.max_rows,
        max_age_days: new_cfg.logs.max_age_days,
        cleanup_interval_secs: new_cfg.logs.cleanup_interval_secs,
    };

    let _ = std::fs::write(
        &st.config_path,
        toml::to_string_pretty(&new_cfg).unwrap_or_default(),
    );
    st.config.set(new_cfg).await;

    // Apply retention immediately so the UI reflects the new limits.
    match st
        .engine
        .logs
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

    Json(json!({"ok": true, "settings": settings})).into_response()
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

/// Local (LAN) IPv4 addresses of this machine, excluding loopback.
async fn netinfo() -> Json<Value> {
    Json(json!({ "local_ips": lan_ipv4s() }))
}

/// Sorted LAN IPv4 list: real LAN (192.168/10.x) before docker/bridge (172.16-31.x).
fn lan_ipv4s() -> Vec<String> {
    let mut ips: Vec<String> = Vec::new();
    if let Ok(ifas) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in ifas {
            if let std::net::IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() {
                    let s = v4.to_string();
                    if !ips.contains(&s) {
                        ips.push(s);
                    }
                }
            }
        }
    }
    ips.sort_by_key(|ip| {
        if ip.starts_with("192.168.") || ip.starts_with("10.") {
            0u8
        } else if ip.starts_with("172.") {
            1u8
        } else {
            2u8
        }
    });
    ips
}

/// Best LAN IPv4 for PAC `proxy_host` fallback (first of the sorted list).
pub fn best_lan_ipv4() -> Option<String> {
    lan_ipv4s().into_iter().next()
}

/// Host-part of a `Host` header (no port).
fn host_part(host_header: &str) -> String {
    match host_header.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h.to_string(),
        _ => host_header.to_string(),
    }
}

fn is_loopback_name(h: &str) -> bool {
    let h = h.trim().trim_start_matches('[').trim_end_matches(']');
    h.eq_ignore_ascii_case("localhost") || h == "127.0.0.1" || h == "::1"
}

/// Resolve the proxy host embedded in the PAC body.
fn resolve_proxy_host(host_header: Option<&str>) -> String {
    let host = host_header.map(host_part).unwrap_or_default();
    if host.is_empty() || is_loopback_name(&host) {
        best_lan_ipv4().unwrap_or_else(|| "127.0.0.1".into())
    } else {
        host
    }
}

async fn proxy_pac(State(st): State<ApiState>, headers: HeaderMap) -> Response {
    let cfg = st.config.get().await;
    let matchers = st.exclusions().await;
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok());
    let proxy_host = resolve_proxy_host(host);
    let body = generate_pac(&PacParams {
        proxy_host: &proxy_host,
        http_port: cfg.http.port,
        https_port: cfg.https.port,
        socks_port: cfg.socks5.port,
        mode: cfg.pac.mode,
        matchers: &matchers,
    });
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/x-ns-proxy-autoconfig",
            ),
            (
                axum::http::header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate",
            ),
            (axum::http::header::PRAGMA, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn pac_info(State(st): State<ApiState>) -> Json<Value> {
    let cfg = st.config.get().await;
    let port = port_of_bind(&cfg.pac.bind);
    let ips = lan_ipv4s();
    let hosts: Vec<String> = if ips.is_empty() {
        vec!["127.0.0.1".into()]
    } else {
        ips
    };
    let mut urls = Vec::new();
    for ip in &hosts {
        urls.push(format!("http://{ip}:{port}/proxy.pac"));
        urls.push(format!("http://{ip}:{port}/wpad.dat"));
    }
    Json(json!({
        "enabled": cfg.pac.enabled,
        "mode": mode_str(cfg.pac.mode),
        "bind": cfg.pac.bind,
        "port": port,
        "urls": urls,
    }))
}

fn mode_str(m: PacMode) -> &'static str {
    match m {
        PacMode::Http => "http",
        PacMode::Socks => "socks",
        PacMode::HttpSocks => "http+socks",
    }
}
