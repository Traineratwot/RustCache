//! Process startup: restart hand-off, bind-or-log, and listener bring-up.

use std::sync::Arc;

use rustcache_core::certs::leaf::LeafIssuer;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::api::{ApiState, pac_router};
use crate::config::Config;
use crate::engine::SharedEngine;
use crate::listeners;

/// After a UI-triggered self-restart the previous process may still hold the
/// listening sockets. Wait until it exits (Linux `/proc`) plus a short grace.
pub fn wait_for_restart_parent() {
    let Ok(v) = std::env::var("RUSTCACHE_RESTARTED_FROM") else {
        return;
    };
    let Ok(pid) = v.parse::<u32>() else {
        return;
    };
    tracing::info!(parent = pid, "waiting for previous process to exit");
    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            std::thread::sleep(std::time::Duration::from_millis(100));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    tracing::warn!(
        parent = pid,
        "previous process still running after wait; continuing"
    );
}

/// Block until the process should exit (SIGTERM / SIGINT).
///
/// Docker sends SIGTERM to PID 1; without a handler the kernel ignores it for
/// PID 1 and `docker stop` waits out `stop_grace_period` before SIGKILL.
pub async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler not installed");
                None
            }
        };
        tokio::select! {
            _ = async {
                match term.as_mut() {
                    Some(t) => { t.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Spawned listener tasks; abort them at shutdown.
pub struct ListenerTasks {
    http: JoinHandle<()>,
    https: JoinHandle<()>,
    socks5: JoinHandle<()>,
    pac: Option<JoinHandle<()>>,
}

impl ListenerTasks {
    /// Abort all listener tasks (shutdown path).
    pub fn abort(&self) {
        self.http.abort();
        self.https.abort();
        self.socks5.abort();
        if let Some(h) = &self.pac {
            h.abort();
        }
    }
}

/// Bind one TCP listener, or log the failure and set the status flag false.
///
/// Returns `None` on bind failure so the caller can skip spawning a serve task;
/// the process keeps running so the other listeners still come up.
async fn bind_or_log(name: &str, addr: &str, mark: impl FnOnce(bool)) -> Option<TcpListener> {
    match TcpListener::bind(addr).await {
        Ok(l) => {
            mark(true);
            Some(l)
        }
        Err(e) => {
            tracing::error!(error = %e, addr = %addr, "{name} listener bind failed");
            mark(false);
            None
        }
    }
}

/// Bind the proxy ports and spawn their serve tasks.
///
/// Ports are bound up front so a conflict (e.g. `http.port == https.port`) is
/// visible in health instead of one task dying silently. A bind failure logs an
/// error and marks the matching `ListenerStatus` flag false; the remaining
/// listeners still start.
pub async fn bring_up_listeners(
    cfg: &Config,
    state: &ApiState,
    engine: &SharedEngine,
    leaves: &Arc<LeafIssuer>,
) -> ListenerTasks {
    let http_bind = format!("0.0.0.0:{}", cfg.http.port);
    let https_bind = format!("0.0.0.0:{}", cfg.https.port);
    let socks_bind = format!("0.0.0.0:{}", cfg.socks5.port);

    let http_listener = bind_or_log("http", &http_bind, |up| state.listeners.set_http(up)).await;
    let https_listener = bind_or_log("mitm", &https_bind, |up| state.listeners.set_https(up)).await;
    let socks_listener =
        bind_or_log("socks5", &socks_bind, |up| state.listeners.set_socks5(up)).await;

    // Optional dedicated LAN listener that serves only the two PAC paths.
    let pac = if cfg.pac.enabled {
        let pac_bind = cfg.pac.bind.clone();
        let pac_state = state.clone();
        match bind_or_log("pac", &pac_bind, |up| state.listeners.set_pac(up)).await {
            Some(pac_listener) => {
                tracing::info!(addr = %pac_bind, "pac listener listening");
                Some(tokio::spawn(async move {
                    if let Err(e) = axum::serve(pac_listener, pac_router(pac_state)).await {
                        tracing::error!(error = %e, "pac listener failed");
                    }
                }))
            }
            None => None,
        }
    } else {
        state.listeners.set_pac(false);
        None
    };

    let engine_http = engine.clone();
    let engine_https = engine.clone();
    let engine_socks = engine.clone();
    let leaves_mitm = leaves.clone();

    let http = tokio::spawn(async move {
        if let Some(l) = http_listener {
            if let Err(e) = listeners::http_proxy::serve(l, engine_http).await {
                tracing::error!(error = %e, "http listener failed");
            }
        }
    });
    let https = tokio::spawn(async move {
        if let Some(l) = https_listener {
            if let Err(e) = listeners::mitm_proxy::serve(l, engine_https, leaves_mitm).await {
                tracing::error!(error = %e, "mitm listener failed");
            }
        }
    });
    let socks5 = tokio::spawn(async move {
        if let Some(l) = socks_listener {
            if let Err(e) = listeners::socks5::serve(l, engine_socks).await {
                tracing::error!(error = %e, "socks5 listener failed");
            }
        }
    });

    ListenerTasks {
        http,
        https,
        socks5,
        pac,
    }
}
