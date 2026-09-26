//! `/api/health` — liveness plus per-listener bind status.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use super::port_of_bind;
use crate::api::state::ApiState;

/// Report uptime and which proxy listeners actually bound.
///
/// Bind success is used rather than a TCP probe: a probe cannot tell two
/// services sharing a misconfigured port apart — both would show "running".
pub async fn health(State(st): State<ApiState>) -> Json<Value> {
    let cfg = st.config.get().await;
    let uptime_s = st.started_at.elapsed().as_secs();

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
