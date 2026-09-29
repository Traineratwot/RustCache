//! Local-origin guard for the admin REST API.
//!
//! The API binds `127.0.0.1` and has no authentication, but "loopback" is not
//! the same as "unreachable from the web". Any page the user visits can issue
//! cross-site requests to `http://127.0.0.1:8080/...`; a plain
//! `POST /api/config/restart` carries no body and no preflight, so the browser
//! sends it and the proxy happily restarts. A hostile DNS name that resolves to
//! 127.0.0.1 (DNS rebinding) additionally defeats the browser's same-origin
//! read protection, exposing the whole config.
//!
//! Two cheap header checks close both holes without touching legitimate use:
//!
//! * `Origin`, when present, must be a loopback origin — a cross-site request
//!   always carries the initiating page's origin, so this rejects CSRF.
//! * `Host` must be loopback, a bare IP literal, or the configured `api.bind`
//!   host. Rebinding needs a *name* the attacker controls, and that name will
//!   never match — while `http://127.0.0.1:8080/` and a LAN IP still work.
//!
//! Applied to the admin router only. The LAN-facing PAC listener serves no
//! admin endpoints and is deliberately left open.

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::state::ApiState;

/// Host part of an authority, without port or IPv6 brackets.
fn host_part(authority: &str) -> &str {
    let authority = authority.trim();
    if let Some(inner) = authority.strip_prefix('[') {
        return inner.split(']').next().unwrap_or(inner);
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    }
}

fn is_loopback(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// `Origin` is acceptable when absent (same-origin navigations, curl, the
/// `null` origin of a local file) or when it points at loopback.
fn origin_allowed(origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let origin = origin.trim();
    if origin.is_empty() || origin.eq_ignore_ascii_case("null") {
        return true;
    }
    let authority = origin.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    !authority.is_empty() && is_loopback(host_part(authority))
}

/// `Host` is acceptable when it is loopback, a bare IP literal (which cannot be
/// rebound), or exactly the host this API was configured to bind.
fn host_allowed(host: Option<&str>, api_bind: &str) -> bool {
    let Some(host) = host.map(host_part) else {
        // HTTP/1.1 requires Host; a request without one is not from a browser.
        return true;
    };
    if is_loopback(host) || host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    host.eq_ignore_ascii_case(host_part(api_bind))
}

/// Reject cross-site and rebound requests to the admin API.
pub async fn local_origin_guard(State(st): State<ApiState>, req: Request, next: Next) -> Response {
    let origin = header_str(&req, header::ORIGIN);
    let host = header_str(&req, header::HOST);

    if !origin_allowed(origin.as_deref()) {
        tracing::warn!(origin = ?origin, path = %req.uri().path(), "rejected cross-site API request");
        return deny("cross-site requests to the admin API are not allowed");
    }
    let api_bind = st.config.get().await.api.bind.clone();
    if !host_allowed(host.as_deref(), &api_bind) {
        tracing::warn!(host = ?host, path = %req.uri().path(), "rejected API request with unexpected Host");
        return deny("unexpected Host header for the admin API");
    }
    next.run(req).await
}

fn header_str(req: &Request, name: header::HeaderName) -> Option<String> {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn deny(msg: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({"ok": false, "error": msg})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_part_handles_ports_and_ipv6() {
        assert_eq!(host_part("127.0.0.1:8080"), "127.0.0.1");
        assert_eq!(host_part("localhost"), "localhost");
        assert_eq!(host_part("[::1]:8080"), "::1");
        assert_eq!(host_part("proxy.lan:8080"), "proxy.lan");
    }

    #[test]
    fn origin_absent_or_loopback_is_allowed() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("null")));
        assert!(origin_allowed(Some("http://127.0.0.1:8080")));
        assert!(origin_allowed(Some("http://localhost:5173")));
        assert!(origin_allowed(Some("http://[::1]:8080")));
    }

    #[test]
    fn cross_site_origin_is_rejected() {
        assert!(!origin_allowed(Some("https://evil.test")));
        assert!(!origin_allowed(Some("http://attacker.example:8080")));
    }

    #[test]
    fn host_loopback_and_ip_literals_allowed() {
        assert!(host_allowed(Some("127.0.0.1:8080"), "127.0.0.1:8080"));
        assert!(host_allowed(Some("localhost:8080"), "127.0.0.1:8080"));
        assert!(host_allowed(Some("192.168.1.5:8080"), "0.0.0.0:8080"));
        assert!(host_allowed(None, "127.0.0.1:8080"));
    }

    #[test]
    fn rebinding_name_is_rejected() {
        assert!(!host_allowed(
            Some("rebind.evil.test:8080"),
            "127.0.0.1:8080"
        ));
        // …unless it is exactly the configured bind host.
        assert!(host_allowed(Some("proxy.lan:8080"), "proxy.lan:8080"));
    }
}
