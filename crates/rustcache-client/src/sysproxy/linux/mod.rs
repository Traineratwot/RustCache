//! Linux system-proxy handlers: GNOME (gsettings) and KDE (kioslaverc).
//!
//! Both are applied together: Chromium/Firefox read GNOME `gsettings` even in
//! a KDE session, while Qt/KDE apps read `kioslaverc`. Setting only one is why
//! "system proxy" looked dead in the browser.

pub mod gnome;
pub mod kde;

use anyhow::Result;
use serde_json::Value;

use crate::platform::PlatformInfo;
use crate::sysproxy::handler::SysProxyHandler;

use gnome::GnomeHandler;
use kde::KdeHandler;

/// Applies GNOME **and** KDE settings so every browser/DE picks the proxy up.
pub struct LinuxBothHandler;

impl SysProxyHandler for LinuxBothHandler {
    fn id(&self) -> &'static str {
        "linux"
    }

    fn snapshot(&self) -> Result<Value> {
        Ok(serde_json::json!({
            "gnome": GnomeHandler.snapshot()?,
            "kde": KdeHandler.snapshot()?,
        }))
    }

    fn apply(&self, host: &str, port: u16, bypass: &[String]) -> Result<String> {
        let mut notes = Vec::new();
        // Prefer whichever backend is available; report both attempts.
        match GnomeHandler.apply(host, port, bypass) {
            Ok(m) => notes.push(m),
            Err(e) => tracing::debug!(error = %e, "gnome proxy apply skipped"),
        }
        match KdeHandler.apply(host, port, bypass) {
            Ok(m) => notes.push(m),
            Err(e) => tracing::debug!(error = %e, "kde proxy apply skipped"),
        }
        if notes.is_empty() {
            anyhow::bail!(
                "no desktop proxy backend (gsettings/kwriteconfig). \
                 Set http_proxy/https_proxy={host}:{port} manually"
            );
        }
        Ok(notes.join("; "))
    }

    fn restore(&self, prev: &Value) -> Result<String> {
        let mut notes = Vec::new();
        if prev.get("gnome").is_some() || prev.get("kde").is_some() {
            if let Some(g) = prev.get("gnome") {
                notes.push(
                    GnomeHandler
                        .restore(g)
                        .unwrap_or_else(|e| format!("gnome: {e}")),
                );
            }
            if let Some(k) = prev.get("kde") {
                notes.push(
                    KdeHandler
                        .restore(k)
                        .unwrap_or_else(|e| format!("kde: {e}")),
                );
            }
        } else {
            // Legacy single-DE snapshot (flat object) — try both backends.
            notes.push(
                GnomeHandler
                    .restore(prev)
                    .unwrap_or_else(|e| format!("gnome: {e}")),
            );
            notes.push(
                KdeHandler
                    .restore(prev)
                    .unwrap_or_else(|e| format!("kde: {e}")),
            );
        }
        Ok(notes.join("; "))
    }

    fn status(&self) -> Result<String> {
        let g = GnomeHandler
            .status()
            .unwrap_or_else(|e| format!("gnome={e}"));
        let k = KdeHandler.status().unwrap_or_else(|e| format!("kde={e}"));
        Ok(format!("{g} | {k}"))
    }

    fn dry_run(&self, host: &str, port: u16, bypass: &[String]) -> Result<String> {
        Ok(format!(
            "# gnome\n{}\n# kde\n{}",
            GnomeHandler.dry_run(host, port, bypass)?,
            KdeHandler.dry_run(host, port, bypass)?
        ))
    }

    fn current_proxy(&self) -> Result<Option<(String, u16)>> {
        if let Some(p) = GnomeHandler.current_proxy()? {
            return Ok(Some(p));
        }
        KdeHandler.current_proxy()
    }
}

/// Pick the right handler for the detected DE.
///
/// Always [`LinuxBothHandler`] — browsers and KDE apps disagree on the source
/// of truth, so we set every backend that exists on the machine.
pub fn handler_for(_info: &PlatformInfo) -> Box<dyn SysProxyHandler> {
    Box::new(LinuxBothHandler)
}
