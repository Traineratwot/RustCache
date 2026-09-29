//! macOS TUN: best-effort utun (root). Not fully implemented in MVP.

use anyhow::Result;

use super::TunSnapshot;

/// Create TUN on macOS (best-effort, requires root).
pub async fn create_interface() -> Result<TunSnapshot> {
    anyhow::bail!("TUN on macOS (utun) is best-effort and not yet implemented")
}

/// Remove TUN on macOS.
pub async fn remove_interface(_snap: &TunSnapshot) -> Result<String> {
    Ok("tun not active on macOS".into())
}

/// Preserve DNS routes on macOS.
pub async fn preserve_dns_routes(_snap: &TunSnapshot) -> Result<Vec<String>> {
    Ok(Vec::new())
}
