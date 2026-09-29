//! Integration: the admin API rejects cross-site and DNS-rebound requests.
//!
//! The API has no authentication and binds loopback, so these header checks are
//! the only thing standing between a hostile web page and
//! `POST /api/config/restart`.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use rustcache::api::{ApiState, pac_router, router};
use rustcache::config::Config;
use rustcache::config::watch::LiveConfig;
use rustcache_core::certs::ca::generate_ca;
use rustcache_core::excl::ExclusionSet;
use tower::util::ServiceExt;

async fn make_state() -> ApiState {
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let ca = generate_ca(dir.join("ca")).unwrap();
    let cfg = Config {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Config::default()
    };
    ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: dir.join("config.toml"),
        started_at: std::time::Instant::now(),
        listeners: Default::default(),
    }
}

/// Send `req` through the admin router and return the status.
async fn status_of(state: ApiState, req: Request<Body>) -> StatusCode {
    router(state).oneshot(req).await.unwrap().status()
}

fn get(path: &str) -> Request<Body> {
    Request::builder()
        .uri(path)
        .header("host", "127.0.0.1:8080")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn same_origin_requests_pass() {
    install_crypto();
    let state = make_state().await;
    let req = Request::builder()
        .uri("/api/health")
        .header("host", "127.0.0.1:8080")
        .header("origin", "http://127.0.0.1:8080")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(state, req).await, StatusCode::OK);
}

#[tokio::test]
async fn requests_without_an_origin_pass() {
    install_crypto();
    let state = make_state().await;
    // curl, the packaged UI on a file:// page, and plain navigations.
    assert_eq!(status_of(state, get("/api/health")).await, StatusCode::OK);
}

#[tokio::test]
async fn vite_dev_server_origin_passes() {
    install_crypto();
    let state = make_state().await;
    let req = Request::builder()
        .uri("/api/health")
        .header("host", "127.0.0.1:8080")
        .header("origin", "http://localhost:5173")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(state, req).await, StatusCode::OK);
}

#[tokio::test]
async fn cross_site_origin_is_forbidden() {
    install_crypto();
    let state = make_state().await;
    let req = Request::builder()
        .uri("/api/health")
        .header("host", "127.0.0.1:8080")
        .header("origin", "https://evil.test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(state, req).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn cross_site_restart_is_forbidden() {
    install_crypto();
    let state = make_state().await;
    // The dangerous one: no body, no preflight, so a browser really does send it.
    let req = Request::builder()
        .method("POST")
        .uri("/api/config/restart?dry_run=1")
        .header("host", "127.0.0.1:8080")
        .header("origin", "https://evil.test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(state, req).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rebound_hostname_is_forbidden() {
    install_crypto();
    let state = make_state().await;
    // DNS rebinding: attacker-controlled name resolving to 127.0.0.1.
    let req = Request::builder()
        .uri("/api/config")
        .header("host", "rebind.evil.test:8080")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(state, req).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn lan_ip_host_still_works() {
    install_crypto();
    let state = make_state().await;
    // Someone who moved api.bind onto the LAN reaches it by IP; rebinding
    // cannot forge an IP-literal Host, so this stays allowed.
    let req = Request::builder()
        .uri("/api/health")
        .header("host", "192.168.1.5:8080")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status_of(state, req).await, StatusCode::OK);
}

#[tokio::test]
async fn pac_listener_is_not_guarded() {
    install_crypto();
    let state = make_state().await;
    // The LAN PAC listener must stay reachable from any client on the network.
    let req = Request::builder()
        .uri("/proxy.pac")
        .header("host", "proxy.lan:8081")
        .body(Body::empty())
        .unwrap();
    let resp = pac_router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
