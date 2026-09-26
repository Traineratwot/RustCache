//! HTTP handlers for the REST API — one module per resource.

pub mod ca;
pub mod cache;
pub mod config;
pub mod exclusions;
pub mod health;
pub mod logs;
pub mod netinfo;
pub mod pac;
pub mod requests;
pub mod stats;

/// TCP port part of a `host:port` bind string (0 when missing or unparsable).
pub fn port_of_bind(bind: &str) -> u16 {
    bind.rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}
