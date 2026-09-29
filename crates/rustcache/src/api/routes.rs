//! Axum REST API route wiring (handlers live in `super::handlers`).

use axum::Router;
use axum::routing::{get, post};

use super::guard::local_origin_guard;
use super::handlers;
use super::state::ApiState;

/// Full admin REST API router (binds 127.0.0.1 by default).
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/health", get(handlers::health::health))
        .route("/api/stats", get(handlers::stats::stats))
        .route(
            "/api/requests",
            get(handlers::requests::requests).delete(handlers::requests::clear_requests),
        )
        .route(
            "/api/logs/settings",
            get(handlers::logs::get_log_settings).put(handlers::logs::put_log_settings),
        )
        .route("/api/logs/stats", get(handlers::logs::log_stats))
        .route(
            "/api/config",
            get(handlers::config::get_config).put(handlers::config::put_config),
        )
        .route("/api/config/reload", post(handlers::config::reload_config))
        .route(
            "/api/config/restart",
            post(handlers::config::restart_process),
        )
        .route(
            "/api/exclusions",
            get(handlers::exclusions::list_exclusions)
                .post(handlers::exclusions::add_exclusion)
                .delete(handlers::exclusions::clear_exclusions),
        )
        .route("/api/ca.crt", get(handlers::ca::ca_crt))
        .route(
            "/api/cache",
            get(handlers::cache::cache_info).delete(handlers::cache::purge_cache),
        )
        .route("/api/netinfo", get(handlers::netinfo::netinfo))
        .route("/api/pac", get(handlers::pac::pac_info))
        .route("/proxy.pac", get(handlers::pac::proxy_pac))
        .route("/wpad.dat", get(handlers::pac::proxy_pac))
        // Rejects cross-site and DNS-rebound requests — see `super::guard`.
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            local_origin_guard,
        ))
        .with_state(state)
}

/// Router exposing only the two PAC paths (LAN-facing, no admin endpoints).
pub fn pac_router(state: ApiState) -> Router {
    Router::new()
        .route("/proxy.pac", get(handlers::pac::proxy_pac))
        .route("/wpad.dat", get(handlers::pac::proxy_pac))
        .with_state(state)
}
