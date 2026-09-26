//! `/api/pac`, `/proxy.pac`, `/wpad.dat` — PAC discovery and body serving.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use super::netinfo::{best_lan_ipv4, lan_ipv4s};
use super::port_of_bind;
use crate::api::pac::{PacParams, generate_pac};
use crate::api::state::ApiState;
use crate::config::schema::PacMode;

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

/// Serve the generated PAC body (content-type `application/x-ns-proxy-autoconfig`).
///
/// The embedded proxy host prefers the request's `Host` header and falls back
/// to the best LAN IPv4 so remote clients can reach the listeners.
pub async fn proxy_pac(State(st): State<ApiState>, headers: HeaderMap) -> Response {
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

/// PAC status for the Connect page: mode, bind, and candidate proxy URLs.
pub async fn pac_info(State(st): State<ApiState>) -> Json<serde_json::Value> {
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
    Json(serde_json::json!({
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
