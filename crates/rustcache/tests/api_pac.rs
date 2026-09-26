//! Integration smoke: PAC endpoints return the right content type and body.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use rustcache::api::{ApiState, pac_router, router};
use rustcache::config::Config;
use rustcache::config::schema::{PacConfig, PacMode};
use rustcache::config::watch::LiveConfig;
use rustcache_core::certs::ca::generate_ca;
use rustcache_core::excl::ExclusionSet;
use tower::util::ServiceExt;

async fn make_state(domains: Vec<String>, cidrs: Vec<String>) -> ApiState {
    let (engine, dir) = spawn_engine(ExclusionSet::from_specs(&domains, &cidrs)).await;
    let ca = generate_ca(dir.join("ca")).unwrap();
    let mut cfg = Config::default();
    cfg.exclude.domains = domains;
    cfg.exclude.cidrs = cidrs;
    ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: dir.join("config.toml"),
        started_at: std::time::Instant::now(),
        listeners: Default::default(),
    }
}

async fn get_pac_response(
    state: ApiState,
    path: &str,
    host: &str,
) -> (StatusCode, Vec<(String, String)>, String) {
    let app = router(state);
    let req = Request::builder()
        .uri(path)
        .header("host", host)
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, String::from_utf8_lossy(&bytes).to_string())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn proxy_pac_and_wpad_identical() {
    install_crypto();
    let state = make_state(
        vec!["bank.example".into(), "*.local".into()],
        vec!["10.0.0.0/8".into()],
    )
    .await;

    let (s1, h1, b1) = get_pac_response(state.clone(), "/proxy.pac", "192.168.1.5").await;
    assert_eq!(s1, StatusCode::OK);
    assert_eq!(
        header(&h1, "content-type"),
        Some("application/x-ns-proxy-autoconfig")
    );
    assert!(header(&h1, "cache-control").unwrap().contains("no-store"));
    assert!(b1.contains("FindProxyForURL"));
    assert!(b1.contains("bank.example"));
    assert!(b1.contains("*.local"));
    assert!(b1.contains("10.0.0.0"));

    let (s2, _h2, b2) = get_pac_response(state, "/wpad.dat", "192.168.1.5").await;
    assert_eq!(s2, StatusCode::OK);
    assert_eq!(b1, b2, "wpad.dat body must match proxy.pac");
}

#[tokio::test]
async fn pac_uses_request_host_as_proxy() {
    install_crypto();
    let state = make_state(vec![], vec![]).await;
    let (_s, _h, body) = get_pac_response(state, "/proxy.pac", "10.1.2.3").await;
    assert!(body.contains("PROXY 10.1.2.3:"), "body={body}");
}

#[tokio::test]
async fn pac_falls_back_to_lan_on_localhost() {
    install_crypto();
    let state = make_state(vec![], vec![]).await;
    let (_s, _h, body) = get_pac_response(state, "/proxy.pac", "localhost").await;
    assert!(body.contains("FindProxyForURL"));
    assert!(
        !body.contains("PROXY localhost:"),
        "must not use localhost: {body}"
    );
}

#[tokio::test]
async fn pac_info_json() {
    install_crypto();
    let state = make_state(vec![], vec![]).await;
    let app = router(state);
    let req = Request::builder()
        .uri("/api/pac")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["enabled"], true);
    assert_eq!(v["mode"], "http+socks");
    assert!(v["port"].as_u64().is_some());
    let urls = v["urls"].as_array().unwrap();
    assert!(!urls.is_empty());
    assert!(urls[0].as_str().unwrap().contains("/proxy.pac"));
}

#[tokio::test]
async fn pac_only_router_serves_pac_paths() {
    install_crypto();
    let state = make_state(vec![], vec![]).await;
    let app = pac_router(state);

    let req = Request::builder()
        .uri("/proxy.pac")
        .header("host", "192.168.0.10")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // admin endpoints must NOT be on the PAC router
    let req = Request::builder()
        .uri("/api/pac")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pac_mode_http_only() {
    install_crypto();
    let (engine, _dir) = spawn_engine(ExclusionSet::default()).await;
    let ca_dir = std::env::temp_dir().join(format!("rc-pac-ca2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ca_dir);
    let ca = generate_ca(&ca_dir).unwrap();
    let mut cfg = Config::default();
    cfg.pac = PacConfig {
        enabled: true,
        bind: "0.0.0.0:8081".into(),
        mode: PacMode::Http,
        ..Default::default()
    };
    let state = ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: ca_dir.join("config.toml"),
        started_at: std::time::Instant::now(),
        listeners: Default::default(),
    };
    let (_s, _h, body) = get_pac_response(state, "/proxy.pac", "192.168.1.1").await;
    assert!(body.contains("PROXY 192.168.1.1:"));
    assert!(!body.contains("SOCKS5"));
}

#[tokio::test]
async fn preferred_ip_overrides_host_header() {
    install_crypto();
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let ca = generate_ca(dir.join("ca")).unwrap();
    let mut cfg = Config::default();
    cfg.pac = PacConfig {
        preferred_ip: "10.9.9.9".into(),
        ..Default::default()
    };
    let state = ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: dir.join("config.toml"),
        started_at: std::time::Instant::now(),
        listeners: Default::default(),
    };
    let (_s, _h, body) = get_pac_response(state, "/proxy.pac", "192.168.1.5").await;
    assert!(body.contains("PROXY 10.9.9.9:"), "body={body}");
    assert!(
        !body.contains("PROXY 192.168.1.5:"),
        "preferred must override Host: {body}"
    );
}

#[tokio::test]
async fn pac_info_lists_preferred_ip_first() {
    install_crypto();
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let ca = generate_ca(dir.join("ca")).unwrap();
    let mut cfg = Config::default();
    cfg.pac = PacConfig {
        preferred_ip: "10.9.9.9".into(),
        ..Default::default()
    };
    let state = ApiState {
        engine,
        config: LiveConfig::new(cfg),
        ca: Arc::new(ca),
        config_path: dir.join("config.toml"),
        started_at: std::time::Instant::now(),
        listeners: Default::default(),
    };
    let app = router(state);
    let req = Request::builder()
        .uri("/api/pac")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["preferred_ip"], "10.9.9.9");
    let urls = v["urls"].as_array().unwrap();
    assert!(
        urls[0].as_str().unwrap().contains("10.9.9.9"),
        "urls={urls:?}"
    );
    assert!(urls[0].as_str().unwrap().contains("/proxy.pac"));
}
