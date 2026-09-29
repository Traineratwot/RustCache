//! Windows TUN: wintun (admin). Not fully implemented in MVP.

use anyhow::Result;

use super::TunSnapshot;

/// Create TUN on Windows via wintun (requires admin + wintun.dll).
pub async fn create_interface() -> Result<TunSnapshot> {
    anyhow::bail!("TUN on Windows requires wintun.dll and admin — not yet implemented")
}

/// Remove TUN on Windows.
pub async fn remove_interface(_snap: &TunSnapshot) -> Result<String> {
    Ok("tun not active on Windows".into())
}

/// Preserve DNS routes on Windows.
pub async fn preserve_dns_routes(_snap: &TunSnapshot) -> Result<Vec<String>> {
    Ok(Vec::new())
}
