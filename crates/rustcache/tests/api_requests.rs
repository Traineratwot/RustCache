//! Integration: request-log query/clear and log settings endpoints.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use rustcache::api::{router, ApiState};
use rustcache::config::watch::LiveConfig;
use rustcache::config::Config;
use rustcache_core::certs::ca::generate_ca;
use rustcache_core::excl::ExclusionSet;
use rustcache_core::stats::{Outcome, ReqRecord};
use tower::util::ServiceExt;

async fn make_state() -> (ApiState, std::path::PathBuf) {
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let ca = generate_ca(dir.join("ca")).unwrap();
    let mut cfg = Config::default();
    // Keep path validation hermetic — do not touch the real home data dir.
    cfg.data_dir = dir.to_string_lossy().into_owned();
    cfg.logs.db_path = "logs.db".into();
    let state = ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: dir.join("config.toml"),
        started_at: std::time::Instant::now(),
        listeners: Default::default(),
    };
    (state, dir)
}

fn rec(ts: u64, method: &str, url: &str, host: &str, status: u16, outcome: Outcome) -> ReqRecord {
    ReqRecord {
        ts,
        method: method.into(),
        url: url.into(),
        host: host.into(),
        status,
        outcome,
        duration_ms: 1,
        resp_bytes: 2,
    }
}

async fn seed(state: &ApiState) {
    state.engine.record(rec(
        1000,
        "GET",
        "http://example.com/a",
        "example.com",
        200,
        Outcome::Hit,
    ));
    state.engine.record(rec(
        2000,
        "POST",
        "http://other.test/b",
        "other.test",
        404,
        Outcome::Miss,
    ));
    state.engine.record(rec(
        3000,
        "GET",
        "http://example.com/c",
        "example.com",
        500,
        Outcome::Error,
    ));
    state.engine.logs().flush().await.expect("flush");
}

async fn json_get(state: ApiState, uri: &str) -> (StatusCode, serde_json::Value) {
    let app = router(state);
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    (status, v)
}

async fn json_send(
    state: ApiState,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let app = router(state);
    let mut builder = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let req = builder.body(body).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    (status, v)
}

#[tokio::test]
async fn requests_default_page_shape() {
    install_crypto();
    let (state, _dir) = make_state().await;
    seed(&state).await;

    let (status, v) = json_get(state.clone(), "/api/requests").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["total"], 3);
    assert_eq!(v["limit"], 50);
    assert_eq!(v["offset"], 0);
    let arr = v["requests"].as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["ts"], 3000);
}

#[tokio::test]
async fn requests_filters() {
    install_crypto();
    let (state, _dir) = make_state().await;
    seed(&state).await;

    let (_, v) = json_get(state.clone(), "/api/requests?q=example").await;
    assert_eq!(v["total"], 2);

    let (_, v) = json_get(state.clone(), "/api/requests?method=POST").await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["requests"][0]["outcome"], "MISS");

    let (_, v) = json_get(state.clone(), "/api/requests?outcome=ERROR").await;
    assert_eq!(v["total"], 1);

    let (_, v) = json_get(state.clone(), "/api/requests?status_min=400&status_max=499").await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["requests"][0]["status"], 404);

    let (_, v) = json_get(state.clone(), "/api/requests?since=1500&until=2500").await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["requests"][0]["ts"], 2000);
}

#[tokio::test]
async fn requests_pagination() {
    install_crypto();
    let (state, _dir) = make_state().await;
    seed(&state).await;

    let (_, v) = json_get(state.clone(), "/api/requests?limit=2&offset=0").await;
    assert_eq!(v["total"], 3);
    assert_eq!(v["requests"].as_array().unwrap().len(), 2);

    let (_, v) = json_get(state.clone(), "/api/requests?limit=2&offset=2").await;
    assert_eq!(v["total"], 3);
    assert_eq!(v["requests"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn clear_requests_deletes_all() {
    install_crypto();
    let (state, _dir) = make_state().await;
    seed(&state).await;

    let (status, v) = json_send(state.clone(), "DELETE", "/api/requests", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert_eq!(v["deleted"], 3);

    let (_, v) = json_get(state.clone(), "/api/requests").await;
    assert_eq!(v["total"], 0);
}

#[tokio::test]
async fn log_settings_get_and_put() {
    install_crypto();
    let (state, _dir) = make_state().await;

    let (status, v) = json_get(state.clone(), "/api/logs/settings").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["max_rows"], 10_000);
    assert_eq!(v["max_age_days"], 7);
    assert_eq!(v["cleanup_interval_secs"], 300);

    let (status, v) = json_send(
        state.clone(),
        "PUT",
        "/api/logs/settings",
        Some(serde_json::json!({
            "max_rows": 50,
            "max_age_days": 3,
            "cleanup_interval_secs": 60
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert_eq!(v["settings"]["max_rows"], 50);

    let (_, v) = json_get(state.clone(), "/api/logs/settings").await;
    assert_eq!(v["max_rows"], 50);
    assert_eq!(v["cleanup_interval_secs"], 60);
}

#[tokio::test]
async fn log_settings_rejects_out_of_range() {
    install_crypto();
    let (state, _dir) = make_state().await;

    let (status, _) = json_send(
        state.clone(),
        "PUT",
        "/api/logs/settings",
        Some(serde_json::json!({"max_rows": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = json_send(
        state.clone(),
        "PUT",
        "/api/logs/settings",
        Some(serde_json::json!({"max_age_days": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = json_send(
        state.clone(),
        "PUT",
        "/api/logs/settings",
        Some(serde_json::json!({"cleanup_interval_secs": 5})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn log_stats_shape() {
    install_crypto();
    let (state, _dir) = make_state().await;
    seed(&state).await;

    let (status, v) = json_get(state.clone(), "/api/logs/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["total"], 3);
    assert!(v["hits"].is_number());
    assert!(v["miss_like"].is_number());
    assert!(v["hit_rate"].is_number());
    assert!(v["bytes_served"].is_number());
    assert!(v["bytes_saved"].is_number());
    assert!(v["avg_duration_ms"].is_number());
    assert!(v["max_duration_ms"].is_number());
    assert!(v["bucket_ms"].is_number());
    assert!(v["by_outcome"].is_array());
    assert!(v["top_hosts"].is_array());
    assert!(v["series"].is_array());

    let by = v["by_outcome"].as_array().unwrap();
    assert_eq!(by.len(), 8);
    let hit = by.iter().find(|r| r["outcome"] == "HIT").unwrap();
    assert_eq!(hit["count"], 1);
    let miss = by.iter().find(|r| r["outcome"] == "MISS").unwrap();
    assert_eq!(miss["count"], 1);
    let err = by.iter().find(|r| r["outcome"] == "ERROR").unwrap();
    assert_eq!(err["count"], 1);
}

#[tokio::test]
async fn log_stats_empty_db() {
    install_crypto();
    let (state, _dir) = make_state().await;

    let (status, v) = json_get(state.clone(), "/api/logs/stats").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["total"], 0);
    assert_eq!(v["hit_rate"], 0.0);
    assert_eq!(v["by_outcome"].as_array().unwrap().len(), 8);
    assert!(v["top_hosts"].as_array().unwrap().is_empty());
    assert!(v["series"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn log_stats_since_until() {
    install_crypto();
    let (state, _dir) = make_state().await;
    seed(&state).await;

    let (_, v) = json_get(state.clone(), "/api/logs/stats?since=2000&until=2000").await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["since_ms"], 2000);
    assert_eq!(v["until_ms"], 2000);
}

#[tokio::test]
async fn log_stats_hit_rate_bytes() {
    install_crypto();
    let (state, _dir) = make_state().await;

    let mut hits = ReqRecord {
        ts: 1000,
        method: "GET".into(),
        url: "http://a/".into(),
        host: "a".into(),
        status: 200,
        outcome: Outcome::Hit,
        duration_ms: 10,
        resp_bytes: 100,
    };
    state.engine.record(hits.clone());
    hits.outcome = Outcome::HitRevalidated;
    hits.resp_bytes = 200;
    hits.ts = 2000;
    state.engine.record(hits.clone());
    hits.outcome = Outcome::Miss;
    hits.resp_bytes = 400;
    hits.ts = 3000;
    state.engine.record(hits.clone());
    hits.outcome = Outcome::Revalidated;
    hits.resp_bytes = 800;
    hits.ts = 4000;
    state.engine.record(hits);
    state.engine.logs().flush().await.expect("flush");

    let (_, v) = json_get(state.clone(), "/api/logs/stats").await;
    assert_eq!(v["total"], 4);
    assert_eq!(v["hits"], 2);
    assert_eq!(v["miss_like"], 2);
    assert!((v["hit_rate"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(v["bytes_saved"], 300);
    assert_eq!(v["bytes_served"], 1500);
}

fn full_config_json(data_dir: &str) -> serde_json::Value {
    serde_json::json!({
        "data_dir": data_dir,
        "http": {"port": 3128},
        "https": {"port": 3129},
        "socks5": {"port": 1080},
        "api": {"bind": "127.0.0.1:8080"},
        "cache": {"dir": "cache", "max_bytes": 2147483648u64, "max_object_bytes": 52428800u64},
        "exclude": {"domains": [], "cidrs": []},
        "ca": {"dir": "ca"},
        "pac": {"enabled": true, "bind": "0.0.0.0:8081", "mode": "http+socks"},
        "logs": {"db_path": "logs.db", "max_rows": 10000, "max_age_days": 7, "cleanup_interval_secs": 300}
    })
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .expect("ephemeral port")
}

#[tokio::test]
async fn config_put_full_roundtrip() {
    install_crypto();
    let (state, dir) = make_state().await;
    let data_dir = dir.to_string_lossy().into_owned();
    let new_port = free_port();

    let mut body = full_config_json(&data_dir);
    body["http"]["port"] = serde_json::json!(new_port);
    body["cache"]["max_bytes"] = serde_json::json!(123456789u64);
    body["logs"]["max_rows"] = serde_json::json!(500);
    body["pac"]["mode"] = serde_json::json!("socks");

    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "resp: {v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["config"]["http"]["port"], new_port);
    assert_eq!(v["config"]["cache"]["max_bytes"], 123456789u64);
    assert_eq!(v["config"]["logs"]["max_rows"], 500);
    assert_eq!(v["config"]["pac"]["mode"], "socks");
    assert_eq!(v["restart_required"], true);
    assert!(v["restart_fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x == "http.port"));

    // persisted to config.toml
    let text = std::fs::read_to_string(dir.join("config.toml")).unwrap();
    assert!(text.contains(&new_port.to_string()));

    // GET reflects the same values
    let (_, g) = json_get(state.clone(), "/api/config").await;
    assert_eq!(g["http"]["port"], new_port);
    assert_eq!(g["pac"]["mode"], "socks");

    // live engine cache limits were hot-applied
    assert_eq!(state.engine.max_bytes(), 123456789u64);
}

#[tokio::test]
async fn config_put_hot_apply_no_restart() {
    install_crypto();
    let (state, _dir) = make_state().await;

    // Start from the live config so only hot fields change.
    let (_, mut body) = json_get(state.clone(), "/api/config").await;
    body["cache"]["max_bytes"] = serde_json::json!(999_000_000u64);
    body["cache"]["max_object_bytes"] = serde_json::json!(1_000_000u64);
    body["exclude"]["domains"] = serde_json::json!(["*.local"]);
    body["logs"]["max_rows"] = serde_json::json!(77);

    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["restart_required"], false, "resp: {v}");
    assert_eq!(state.engine.max_bytes(), 999_000_000u64);
    assert_eq!(state.engine.max_object_bytes(), 1_000_000u64);

    let excl = state.exclusions().await;
    assert!(excl.iter().any(|m| matches!(
        m,
        rustcache_core::excl::Matcher::Wildcard { value } if value == "local"
    )));
}

#[tokio::test]
async fn config_put_rejects_invalid() {
    install_crypto();
    let (state, dir) = make_state().await;
    let data_dir = dir.to_string_lossy().into_owned();

    let mut body = full_config_json(&data_dir);
    body["http"]["port"] = serde_json::json!(0);
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["ok"], false);
    assert!(v["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["field"] == "http.port"));

    let mut body = full_config_json(&data_dir);
    body["cache"]["max_object_bytes"] = serde_json::json!(u64::MAX);
    body["cache"]["max_bytes"] = serde_json::json!(1u64);
    let (status, _) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut body = full_config_json(&data_dir);
    body["api"]["bind"] = serde_json::json!("not-an-addr");
    let (status, _) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mut body = full_config_json(&data_dir);
    body["logs"]["max_rows"] = serde_json::json!(0);
    let (status, _) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn config_put_rejects_busy_port() {
    install_crypto();
    let (state, dir) = make_state().await;
    let data_dir = dir.to_string_lossy().into_owned();

    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = held.local_addr().unwrap().port();

    let mut body = full_config_json(&data_dir);
    body["http"]["port"] = serde_json::json!(busy);
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {v}");
    assert_eq!(v["ok"], false);
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["field"] == "http.port"),
        "resp: {v}"
    );

    // An unused port is accepted.
    let free = free_port();
    let mut body = full_config_json(&data_dir);
    body["http"]["port"] = serde_json::json!(free);
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "resp: {v}");
}

#[tokio::test]
async fn config_put_rejects_bad_path() {
    install_crypto();
    let (state, dir) = make_state().await;
    let data_dir = dir.to_string_lossy().into_owned();

    // A regular file where a directory is required.
    let blocker = dir.join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();

    let mut body = full_config_json(&data_dir);
    body["cache"]["dir"] = serde_json::json!(blocker.join("cache").to_string_lossy());
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {v}");
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["field"] == "cache.dir"),
        "resp: {v}"
    );

    // data_dir pointing at a file must also fail.
    let mut body = full_config_json(&data_dir);
    body["data_dir"] = serde_json::json!(blocker.to_string_lossy());
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {v}");
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["field"] == "data_dir"),
        "resp: {v}"
    );

    // logs.db_path may not be a directory.
    let mut body = full_config_json(&data_dir);
    body["logs"]["db_path"] = serde_json::json!(dir.join("ca").to_string_lossy());
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {v}");
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["field"] == "logs.db_path"),
        "resp: {v}"
    );
}

#[tokio::test]
async fn config_put_rejects_port_conflict() {
    install_crypto();
    let (state, dir) = make_state().await;
    let data_dir = dir.to_string_lossy().into_owned();

    // http and https cannot share a port — one bind would silently fail.
    let mut body = full_config_json(&data_dir);
    let p = free_port();
    body["http"]["port"] = serde_json::json!(p);
    body["https"]["port"] = serde_json::json!(p);
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {v}");
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["field"] == "https.port"
                && e["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("http.port")),
        "resp: {v}"
    );

    // socks5 vs api.bind is also a conflict.
    let mut body = full_config_json(&data_dir);
    let p = free_port();
    body["socks5"]["port"] = serde_json::json!(p);
    body["api"]["bind"] = serde_json::json!(format!("127.0.0.1:{p}"));
    let (status, v) = json_send(state.clone(), "PUT", "/api/config", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "resp: {v}");
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["field"] == "api.bind"),
        "resp: {v}"
    );
}

#[tokio::test]
async fn config_restart_dry_run() {
    install_crypto();
    let (state, _dir) = make_state().await;

    // dry_run must not re-exec / exit the test process.
    let (status, v) = json_send(state.clone(), "POST", "/api/config/restart?dry_run=1", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["ok"], true);
    assert_eq!(v["dry_run"], true);
}
