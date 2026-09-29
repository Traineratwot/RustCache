//! Diagnostics: what is healthy, what is not, what to fix.

use std::time::Duration;

use crate::api::RustCacheApi;
use crate::config::ClientConfig;
use crate::state::StateStore;

pub struct DoctorReport {
    pub lines: Vec<String>,
    pub rustcache_ok: bool,
}

pub async fn run(cfg: &ClientConfig) -> DoctorReport {
    let mut lines = Vec::new();
    let mut rustcache_ok = false;

    let api = RustCacheApi::new(&cfg.rustcache_api).with_timeout(Duration::from_millis(800));
    lines.push(format!("api: {}", cfg.rustcache_api));
    match api.health().await {
        Ok(h) => {
            rustcache_ok = h.ok;
            lines.push(format!(
                "health: ok={} uptime={}s http={} https={} socks={}",
                h.ok, h.uptime_s, h.http_running, h.https_running, h.socks_running
            ));
            if !h.https_running {
                lines.push("warn: HTTPS MITM listener not running — CONNECT will go DIRECT".into());
            }
        }
        Err(e) => {
            lines.push(format!("health: FAIL — {e}"));
            lines.push("hint: is rustcache running? cargo run -p rustcache -- run".into());
        }
    }

    // CA
    match crate::ca::status(&api, &cfg.ca).await {
        Ok(s) => {
            lines.push(format!(
                "ca: installed={} fp={} detail={}",
                s.installed, s.fingerprint_sha256, s.detail
            ));
            if !s.installed {
                lines.push("hint: rustcache-client ca install".into());
            }
        }
        Err(e) => lines.push(format!("ca: status failed — {e}")),
    }

    // System proxy
    match crate::sysproxy::status() {
        Ok(s) => lines.push(format!("sysproxy: {s}")),
        Err(e) => lines.push(format!("sysproxy: status failed — {e}")),
    }

    // Dirty recovery state
    match StateStore::default_store() {
        Ok(store) => match store.is_dirty() {
            Ok(true) => lines.push(
                "warn: previous session left system proxy dirty — run `proxy off` or restart `run`"
                    .into(),
            ),
            Ok(false) => lines.push("state: clean".into()),
            Err(e) => lines.push(format!("state: {e}")),
        },
        Err(e) => lines.push(format!("state: {e}")),
    }

    // Listen bind check
    match cfg.listen_addr() {
        Ok((host, port)) => {
            let busy = crate::api::tcp_alive(&host, port, Duration::from_millis(200)).await;
            lines.push(format!(
                "listen: {host}:{port} ({})",
                if busy {
                    "already accepting connections"
                } else {
                    "free"
                }
            ));
        }
        Err(e) => lines.push(format!("listen: bad config — {e}")),
    }

    lines.push(format!(
        "config: mode={:?} fail_open={} breaker={}/{}ms",
        cfg.mode, cfg.fail_open, cfg.breaker_failures, cfg.breaker_open_ms
    ));

    DoctorReport {
        lines,
        rustcache_ok,
    }
}
