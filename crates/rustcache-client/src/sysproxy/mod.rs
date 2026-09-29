//! System proxy apply / restore with crash-safe snapshots.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use anyhow::Result;
use serde_json::Value;

use crate::state::StateStore;

/// Force system HTTP/HTTPS proxy to `listen` (e.g. `127.0.0.1:31280`).
/// Writes a restore snapshot first.
pub fn enable(
    listen_host: &str,
    listen_port: u16,
    bypass: &[String],
    store: &StateStore,
) -> Result<String> {
    let prev = platform_snapshot()?;
    store.mark_dirty(prev.clone())?;
    let msg = platform_apply(listen_host, listen_port, bypass)?;
    Ok(msg)
}

/// Restore previous system proxy settings and clear dirty flag.
pub fn disable(store: &StateStore) -> Result<String> {
    let snap = store.load()?;
    let msg = platform_restore(&snap.platform)?;
    store.clear_dirty()?;
    Ok(msg)
}

/// Recover after a crash (dirty=true) without re-applying.
pub fn recover_if_dirty(store: &StateStore) -> Result<Option<String>> {
    if !store.is_dirty()? {
        return Ok(None);
    }
    let snap = store.load()?;
    let msg = platform_restore(&snap.platform)?;
    store.clear_dirty()?;
    Ok(Some(msg))
}

pub fn status() -> Result<String> {
    platform_status()
}

/// Exact commands/registry writes that would run (`--dry-run`).
pub fn dry_run(listen_host: &str, listen_port: u16, bypass: &[String]) -> Result<String> {
    platform_dry_run(listen_host, listen_port, bypass)
}

#[cfg(target_os = "linux")]
fn platform_snapshot() -> Result<Value> {
    linux::snapshot()
}
#[cfg(target_os = "linux")]
fn platform_apply(h: &str, p: u16, b: &[String]) -> Result<String> {
    linux::apply(h, p, b)
}
#[cfg(target_os = "linux")]
fn platform_restore(prev: &Value) -> Result<String> {
    linux::restore(prev)
}
#[cfg(target_os = "linux")]
fn platform_status() -> Result<String> {
    linux::status()
}
#[cfg(target_os = "linux")]
fn platform_dry_run(h: &str, p: u16, b: &[String]) -> Result<String> {
    linux::dry_run(h, p, b)
}

#[cfg(target_os = "macos")]
fn platform_snapshot() -> Result<Value> {
    macos::snapshot()
}
#[cfg(target_os = "macos")]
fn platform_apply(h: &str, p: u16, b: &[String]) -> Result<String> {
    macos::apply(h, p, b)
}
#[cfg(target_os = "macos")]
fn platform_restore(prev: &Value) -> Result<String> {
    macos::restore(prev)
}
#[cfg(target_os = "macos")]
fn platform_status() -> Result<String> {
    macos::status()
}
#[cfg(target_os = "macos")]
fn platform_dry_run(h: &str, p: u16, b: &[String]) -> Result<String> {
    macos::dry_run(h, p, b)
}

#[cfg(target_os = "windows")]
fn platform_snapshot() -> Result<Value> {
    windows::snapshot()
}
#[cfg(target_os = "windows")]
fn platform_apply(h: &str, p: u16, b: &[String]) -> Result<String> {
    windows::apply(h, p, b)
}
#[cfg(target_os = "windows")]
fn platform_restore(prev: &Value) -> Result<String> {
    windows::restore(prev)
}
#[cfg(target_os = "windows")]
fn platform_status() -> Result<String> {
    windows::status()
}
#[cfg(target_os = "windows")]
fn platform_dry_run(h: &str, p: u16, b: &[String]) -> Result<String> {
    windows::dry_run(h, p, b)
}
