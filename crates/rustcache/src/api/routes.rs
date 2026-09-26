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
        .route("/api/logs/stats", get(log_stats))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/config/reload", post(reload_config))
        .route("/api/config/restart", post(restart_process))
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

    // Report actual bind success. A TCP probe cannot tell two services sharing
    // a misconfigured port apart — both would show "running".
    let listeners = vec![
        json!({
            "name": "HTTP proxy",
            "bind": "0.0.0.0",
            "port": cfg.http.port,
            "running": st.listeners.http(),
        }),
        json!({
            "name": "HTTPS MITM",
            "bind": "0.0.0.0",
            "port": cfg.https.port,
            "running": st.listeners.https(),
        }),
        json!({
            "name": "SOCKS5",
            "bind": "0.0.0.0",
            "port": cfg.socks5.port,
            "running": st.listeners.socks5(),
        }),
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

#[derive(Deserialize)]
struct LogStatsQueryParams {
    since: Option<u64>,
    until: Option<u64>,
}

async fn log_stats(State(st): State<ApiState>, Query(q): Query<LogStatsQueryParams>) -> Response {
    let query = rustcache_core::stats::LogStatsQuery {
        since_ms: q.since,
        until_ms: q.until,
    };
    match st.engine.logs.stats(query).await {
        Ok(s) => Json(serde_json::to_value(s).unwrap_or(json!({}))).into_response(),
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

/// Fields that only take effect after a process restart.
fn restart_fields_diff(
    old: &crate::config::Config,
    new: &crate::config::Config,
) -> Vec<&'static str> {
    let mut v = Vec::new();
    if old.data_dir != new.data_dir {
        v.push("data_dir");
    }
    if old.http.port != new.http.port {
        v.push("http.port");
    }
    if old.https.port != new.https.port {
        v.push("https.port");
    }
    if old.socks5.port != new.socks5.port {
        v.push("socks5.port");
    }
    if old.api.bind != new.api.bind {
        v.push("api.bind");
    }
    if old.cache.dir != new.cache.dir {
        v.push("cache.dir");
    }
    if old.ca.dir != new.ca.dir {
        v.push("ca.dir");
    }
    if old.logs.db_path != new.logs.db_path {
        v.push("logs.db_path");
    }
    if old.pac.enabled != new.pac.enabled {
        v.push("pac.enabled");
    }
    if old.pac.bind != new.pac.bind {
        v.push("pac.bind");
    }
    v
}

/// One validation problem, keyed to a Settings form field.
#[derive(Debug, Clone, Serialize)]
struct FieldIssue {
    field: &'static str,
    message: String,
}

fn issue(field: &'static str, message: impl Into<String>) -> FieldIssue {
    FieldIssue {
        field,
        message: message.into(),
    }
}

/// Ports this process currently listens on (from the live/old config).
fn bound_ports(old: &crate::config::Config) -> Vec<u16> {
    let mut v = vec![old.http.port, old.https.port, old.socks5.port];
    if let Ok(sa) = old.api.bind.parse::<std::net::SocketAddr>() {
        v.push(sa.port());
    }
    if old.pac.enabled {
        if let Ok(sa) = old.pac.bind.parse::<std::net::SocketAddr>() {
            v.push(sa.port());
        }
    }
    v
}

/// True when nothing else holds the port. Ports we currently hold are OK —
/// they are released on restart, which is when the new value takes effect.
fn port_available(port: u16, ours: &[u16]) -> bool {
    if ours.contains(&port) {
        return true;
    }
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

fn bind_available(bind: &str, ours: &[u16]) -> bool {
    match bind.parse::<std::net::SocketAddr>() {
        Ok(sa) => {
            if ours.contains(&sa.port()) {
                return true;
            }
            std::net::TcpListener::bind(sa).is_ok()
        }
        Err(_) => false,
    }
}

fn probe_dir_writable(dir: &std::path::Path) -> Result<(), String> {
    let probe = dir.join(format!(".rustcache-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn nearest_existing(path: &std::path::Path) -> Option<&std::path::Path> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.as_os_str().is_empty() {
            cur = p.parent();
            continue;
        }
        if p.exists() {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

fn check_dir_field(issues: &mut Vec<FieldIssue>, field: &'static str, path: &std::path::Path) {
    if path.as_os_str().is_empty() {
        issues.push(issue(field, "path must not be empty"));
        return;
    }
    if path.exists() {
        if !path.is_dir() {
            issues.push(issue(
                field,
                format!("{} exists but is not a directory", path.display()),
            ));
            return;
        }
        if let Err(e) = probe_dir_writable(path) {
            issues.push(issue(
                field,
                format!("{} is not writable: {e}", path.display()),
            ));
        }
        return;
    }
    match nearest_existing(path) {
        None => issues.push(issue(
            field,
            format!("{}: no existing parent directory", path.display()),
        )),
        Some(anc) => {
            if !anc.is_dir() {
                issues.push(issue(
                    field,
                    format!("{} exists but is not a directory", anc.display()),
                ));
                return;
            }
            if let Err(e) = probe_dir_writable(anc) {
                issues.push(issue(
                    field,
                    format!(
                        "cannot create {}: parent {} is not writable: {e}",
                        path.display(),
                        anc.display()
                    ),
                ));
            }
        }
    }
}

fn check_file_field(issues: &mut Vec<FieldIssue>, field: &'static str, path: &std::path::Path) {
    if path.as_os_str().is_empty() {
        issues.push(issue(field, "path must not be empty"));
        return;
    }
    if path.exists() {
        if path.is_dir() {
            issues.push(issue(
                field,
                format!("{} is a directory, expected a file", path.display()),
            ));
            return;
        }
        if let Err(e) = std::fs::OpenOptions::new().append(true).open(path) {
            issues.push(issue(
                field,
                format!("{} cannot be opened for write: {e}", path.display()),
            ));
        }
        return;
    }
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => check_dir_field(issues, field, parent),
        _ => {}
    }
}

/// Validate a full config candidate against the live one. Collects every issue
/// (ranges, bind format, port availability, path type/permissions).
fn validate_config(
    new: &crate::config::Config,
    old: &crate::config::Config,
) -> Result<(), Vec<FieldIssue>> {
    let mut issues = Vec::new();

    if new.data_dir.trim().is_empty() {
        issues.push(issue("data_dir", "must not be empty"));
    }
    if new.http.port == 0 {
        issues.push(issue("http.port", "must be 1..=65535"));
    }
    if new.https.port == 0 {
        issues.push(issue("https.port", "must be 1..=65535"));
    }
    if new.socks5.port == 0 {
        issues.push(issue("socks5.port", "must be 1..=65535"));
    }
    if new.cache.dir.trim().is_empty() {
        issues.push(issue("cache.dir", "must not be empty"));
    }
    if new.cache.max_bytes == 0 {
        issues.push(issue("cache.max_bytes", "must be > 0"));
    }
    if new.cache.max_object_bytes == 0 {
        issues.push(issue("cache.max_object_bytes", "must be > 0"));
    }
    if new.cache.max_object_bytes > new.cache.max_bytes {
        issues.push(issue(
            "cache.max_object_bytes",
            "must be <= cache.max_bytes",
        ));
    }
    if new.ca.dir.trim().is_empty() {
        issues.push(issue("ca.dir", "must not be empty"));
    }
    if new.logs.db_path.trim().is_empty() {
        issues.push(issue("logs.db_path", "must not be empty"));
    }
    if !(1..=10_000_000).contains(&new.logs.max_rows) {
        issues.push(issue("logs.max_rows", "must be 1..=10000000"));
    }
    if !(1..=3650).contains(&new.logs.max_age_days) {
        issues.push(issue("logs.max_age_days", "must be 1..=3650"));
    }
    if !(10..=86_400).contains(&new.logs.cleanup_interval_secs) {
        issues.push(issue("logs.cleanup_interval_secs", "must be 10..=86400"));
    }

    // Bind format first — availability checks need a parsed addr.
    let api_bind_ok = if new.api.bind.trim().is_empty() {
        issues.push(issue("api.bind", "must not be empty"));
        false
    } else if new.api.bind.parse::<std::net::SocketAddr>().is_err() {
        issues.push(issue("api.bind", "must be host:port, e.g. 127.0.0.1:8080"));
        false
    } else {
        true
    };
    let pac_bind_ok = if new.pac.bind.trim().is_empty() {
        issues.push(issue("pac.bind", "must not be empty"));
        false
    } else if new.pac.bind.parse::<std::net::SocketAddr>().is_err() {
        issues.push(issue("pac.bind", "must be host:port, e.g. 0.0.0.0:8081"));
        false
    } else {
        true
    };

    // Internal conflicts: two listeners cannot share one port.
    {
        let mut used: Vec<(&'static str, u16)> = Vec::new();
        let mut claim = |issues: &mut Vec<FieldIssue>, field: &'static str, port: u16| {
            if port == 0 {
                return;
            }
            if let Some((other, _)) = used.iter().find(|(_, p)| *p == port) {
                issues.push(issue(
                    field,
                    format!("port {port} is already used by {other}"),
                ));
            } else {
                used.push((field, port));
            }
        };
        claim(&mut issues, "http.port", new.http.port);
        claim(&mut issues, "https.port", new.https.port);
        claim(&mut issues, "socks5.port", new.socks5.port);
        if api_bind_ok {
            if let Ok(sa) = new.api.bind.parse::<std::net::SocketAddr>() {
                claim(&mut issues, "api.bind", sa.port());
            }
        }
        if new.pac.enabled && pac_bind_ok {
            if let Ok(sa) = new.pac.bind.parse::<std::net::SocketAddr>() {
                claim(&mut issues, "pac.bind", sa.port());
            }
        }
    }

    // Port availability — only for values that change; current ports stay ours
    // until restart. Unchanged binds are already held by this process.
    let ours = bound_ports(old);
    if new.http.port != old.http.port && new.http.port != 0 && !port_available(new.http.port, &ours)
    {
        issues.push(issue(
            "http.port",
            format!("port {} is already in use", new.http.port),
        ));
    }
    if new.https.port != old.https.port
        && new.https.port != 0
        && !port_available(new.https.port, &ours)
    {
        issues.push(issue(
            "https.port",
            format!("port {} is already in use", new.https.port),
        ));
    }
    if new.socks5.port != old.socks5.port
        && new.socks5.port != 0
        && !port_available(new.socks5.port, &ours)
    {
        issues.push(issue(
            "socks5.port",
            format!("port {} is already in use", new.socks5.port),
        ));
    }
    if api_bind_ok && new.api.bind != old.api.bind && !bind_available(&new.api.bind, &ours) {
        issues.push(issue(
            "api.bind",
            format!("address {} is already in use", new.api.bind),
        ));
    }
    // PAC listener only binds when enabled.
    if new.pac.enabled
        && pac_bind_ok
        && new.pac.bind != old.pac.bind
        && !bind_available(&new.pac.bind, &ours)
    {
        issues.push(issue(
            "pac.bind",
            format!("address {} is already in use", new.pac.bind),
        ));
    }

    // Paths: type + create/write permissions.
    if !new.data_dir.trim().is_empty() {
        check_dir_field(&mut issues, "data_dir", &new.data_dir_path());
    }
    if !new.cache.dir.trim().is_empty() {
        check_dir_field(&mut issues, "cache.dir", &new.cache_dir());
    }
    if !new.ca.dir.trim().is_empty() {
        check_dir_field(&mut issues, "ca.dir", &new.ca_dir());
    }
    if !new.logs.db_path.trim().is_empty() {
        check_file_field(&mut issues, "logs.db_path", &new.logs_db_path());
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

fn issues_response(issues: &[FieldIssue]) -> Response {
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

/// Replace the whole effective config (all TOML keys). Hot-applies exclusion +
/// cache-limit + log-retention changes; listener/path changes need a restart.
/// Never restarts the process itself — see `restart_process`.
async fn put_config(State(st): State<ApiState>, Json(body): Json<Value>) -> Response {
    let mut new_cfg: crate::config::Config = match serde_json::from_value(body) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"ok": false, "error": format!("invalid config: {e}")})),
            )
                .into_response();
        }
    };
    // Keep CLI --data-dir override if the client omitted/blanked data_dir.
    let old = st.config.get().await;
    if new_cfg.data_dir.trim().is_empty() {
        new_cfg.data_dir = old.data_dir.clone();
    }
    if let Err(issues) = validate_config(&new_cfg, &old) {
        return issues_response(&issues);
    }

    let restart = restart_fields_diff(&old, &new_cfg);

    match std::fs::write(
        &st.config_path,
        toml::to_string_pretty(&new_cfg).unwrap_or_default(),
    ) {
        Ok(()) => {}
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": format!("write config: {e}")})),
            )
                .into_response();
        }
    }

    // Hot-apply what we can without rebinding sockets / reopening stores.
    if let Err(e) = st.apply_config(new_cfg.clone()).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response();
    }
    st.config.set(new_cfg.clone()).await;

    // Immediate log-retention pass so the UI reflects the new limits.
    match st
        .engine
        .logs
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

    Json(json!({
        "ok": true,
        "config": new_cfg.redacted(),
        "restart_required": !restart.is_empty(),
        "restart_fields": restart,
    }))
    .into_response()
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
struct RestartQuery {
    dry_run: Option<String>,
}

fn is_truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1") | Some("true") | Some("yes") | Some("on"))
}

/// Re-exec the current process with the same argv (used by the UI restart button).
/// `?dry_run=1` (or `true`) only acknowledges — used by tests so the harness does not exit.
async fn restart_process(Query(q): Query<RestartQuery>) -> Response {
    if is_truthy(q.dry_run.as_deref()) {
        return Json(json!({"ok": true, "dry_run": true})).into_response();
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
    Json(json!({"ok": true, "message": "restarting"})).into_response()
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
