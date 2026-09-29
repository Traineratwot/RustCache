//! System proxy apply / restore with crash-safe snapshots.
//!
//! Dispatches to a platform-specific [`SysProxyHandler`] chosen by
//! [`crate::platform::detect()`]. The snapshot envelope carries `"handler"`
//! so restore uses the same backend that produced the snapshot.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub mod handler;

use anyhow::Result;
use serde_json::Value;

use crate::platform::detect;
use crate::state::StateStore;
use handler::{SysProxyHandler, envelope, handler_id};

#[cfg(target_os = "linux")]
fn default_handler() -> Box<dyn SysProxyHandler> {
    linux::handler_for(&detect())
}

#[cfg(target_os = "macos")]
fn default_handler() -> Box<dyn SysProxyHandler> {
    Box::new(macos::MacosHandler)
}

#[cfg(target_os = "windows")]
fn default_handler() -> Box<dyn SysProxyHandler> {
    Box::new(windows::WindowsHandler)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn default_handler() -> Box<dyn SysProxyHandler> {
    // Unsupported OS — operations will fail with a clear message.
    struct Unsupported;
    impl SysProxyHandler for Unsupported {
        fn id(&self) -> &'static str {
            "unsupported"
        }
        fn snapshot(&self) -> Result<Value> {
            Ok(serde_json::json!({"available": false}))
        }
        fn apply(&self, _h: &str, _p: u16, _b: &[String]) -> Result<String> {
            anyhow::bail!("unsupported platform")
        }
        fn restore(&self, _prev: &Value) -> Result<String> {
            Ok("unsupported platform — nothing to restore".into())
        }
        fn status(&self) -> Result<String> {
            Ok("unsupported".into())
        }
        fn dry_run(&self, _h: &str, _p: u16, _b: &[String]) -> Result<String> {
            Ok("# unsupported platform".into())
        }
        fn current_proxy(&self) -> Result<Option<(String, u16)>> {
            Ok(None)
        }
    }
    Box::new(Unsupported)
}

/// Resolve handler from a snapshot envelope (preferred) or current detection.
fn handler_for_snapshot(prev: &Value) -> Box<dyn SysProxyHandler> {
    match handler_id(prev) {
        #[cfg(target_os = "linux")]
        Some("kde") => Box::new(linux::kde::KdeHandler),
        #[cfg(target_os = "linux")]
        Some("gnome") => Box::new(linux::gnome::GnomeHandler),
        #[cfg(target_os = "macos")]
        Some("macos") => Box::new(macos::MacosHandler),
        #[cfg(target_os = "windows")]
        Some("windows") => Box::new(windows::WindowsHandler),
        _ => default_handler(),
    }
}

/// Force system HTTP/HTTPS proxy to `listen` (e.g. `127.0.0.1:31280`).
/// Writes a restore snapshot first.
pub fn enable(
    listen_host: &str,
    listen_port: u16,
    bypass: &[String],
    store: &StateStore,
) -> Result<String> {
    let h = default_handler();
    let prev = envelope(h.id(), h.snapshot()?);
    store.mark_dirty(prev)?;
    let msg = h.apply(listen_host, listen_port, bypass)?;
    Ok(msg)
}

/// Restore previous system proxy settings and clear dirty flag.
pub fn disable(store: &StateStore) -> Result<String> {
    let snap = store.load()?;
    let h = handler_for_snapshot(&snap.platform);
    let msg = h.restore(&snap.platform)?;
    store.clear_dirty()?;
    Ok(msg)
}

/// Recover after a crash (dirty=true) without re-applying.
pub fn recover_if_dirty(store: &StateStore) -> Result<Option<String>> {
    if !store.is_dirty()? {
        return Ok(None);
    }
    let snap = store.load()?;
    let h = handler_for_snapshot(&snap.platform);
    let msg = h.restore(&snap.platform)?;
    store.clear_dirty()?;
    Ok(Some(msg))
}

pub fn status() -> Result<String> {
    default_handler().status()
}

/// Exact commands/registry writes that would run (`--dry-run`).
pub fn dry_run(listen_host: &str, listen_port: u16, bypass: &[String]) -> Result<String> {
    default_handler().dry_run(listen_host, listen_port, bypass)
}

/// Current effective HTTP proxy as `(host, port)`, if the system has one set.
pub fn current_http_proxy() -> Result<Option<(String, u16)>> {
    default_handler().current_proxy()
}
