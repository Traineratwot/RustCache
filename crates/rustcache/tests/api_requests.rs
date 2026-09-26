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
use rustcache_core::stats::ReqRecord;
use tower::util::ServiceExt;

async fn make_state() -> (ApiState, std::path::PathBuf) {
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let ca_dir = std::env::temp_dir().join(format!("rc-req-ca-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = generate_ca(&ca_dir).unwrap();
    let mut cfg = Config::default();
    cfg.logs.db_path = dir.join("logs.db").to_string_lossy().into_owned();
    let state = ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: dir.join("config.toml"),
        started_at: std::time::Instant::now(),
    };
    (state, dir)
}

fn rec(ts: u64, method: &str, url: &str, host: &str, status: u16, outcome: &str) -> ReqRecord {
    ReqRecord {
        ts,
        method: method.into(),
        url: url.into(),
        host: host.into(),
        status,
        outcome: outcome.into(),
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
        "HIT",
    ));
    state.engine.record(rec(
        2000,
        "POST",
        "http://other.test/b",
        "other.test",
        404,
        "MISS",
    ));
    state.engine.record(rec(
        3000,
        "GET",
        "http://example.com/c",
        "example.com",
        500,
        "ERROR",
    ));
    state.engine.logs.flush().await.expect("flush");
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
        outcome: "HIT".into(),
        duration_ms: 10,
        resp_bytes: 100,
    };
    state.engine.record(hits.clone());
    hits.outcome = "HIT_REVALIDATED".into();
    hits.resp_bytes = 200;
    hits.ts = 2000;
    state.engine.record(hits.clone());
    hits.outcome = "MISS".into();
    hits.resp_bytes = 400;
    hits.ts = 3000;
    state.engine.record(hits.clone());
    hits.outcome = "REVALIDATED".into();
    hits.resp_bytes = 800;
    hits.ts = 4000;
    state.engine.record(hits);
    state.engine.logs.flush().await.expect("flush");

    let (_, v) = json_get(state.clone(), "/api/logs/stats").await;
    assert_eq!(v["total"], 4);
    assert_eq!(v["hits"], 2);
    assert_eq!(v["miss_like"], 2);
    assert!((v["hit_rate"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    assert_eq!(v["bytes_saved"], 300);
    assert_eq!(v["bytes_served"], 1500);
}
