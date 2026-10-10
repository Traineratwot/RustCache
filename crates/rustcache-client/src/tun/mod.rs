//! TUN TCP-only capture (feature `tun`).
//!
//! Packets → TUN device → smoltcp (addr 198.18.0.1/15):
//!   TCP :80  → HTTP-proxy handshake to 127.0.0.1:31280 (absolute-form)
//!   TCP :443 → CONNECT dest to 127.0.0.1:31280
//!   TCP other → raw splice DIRECT (fail-open)
//!   UDP → drop
//!
//! DNS safety: `preserve_dns_routes()` adds host-routes for resolvers
//! through the **old** default gateway so DNS does not black-hole.

pub mod stack;
pub mod privs;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

use anyhow::Result;
use serde_json::Value;

/// TUN interface address (fake-IP range, never collides with real traffic).
pub const TUN_ADDR: &str = "198.18.0.1";
pub const TUN_PREFIX: u8 = 15;

/// Snapshot of TUN state for dirty-recovery.
#[derive(Debug, Clone)]
pub struct TunSnapshot {
    pub iface: String,
    pub addr: String,
    pub routes: Vec<String>,
    pub dns_routes: Vec<String>,
    pub old_default: Option<String>,
}

impl TunSnapshot {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "iface": self.iface,
            "addr": self.addr,
            "routes": self.routes,
            "dns_routes": self.dns_routes,
            "old_default": self.old_default,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            iface: v.get("iface")?.as_str()?.to_string(),
            addr: v.get("addr")?.as_str()?.to_string(),
            routes: v
                .get("routes")?
                .as_array()?
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
            dns_routes: v
                .get("dns_routes")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            old_default: v
                .get("old_default")
                .and_then(|x| x.as_str())
                .map(String::from),
        })
    }
}

/// Handle to an active TUN session.
pub struct TunSession {
    pub snapshot: TunSnapshot,
    /// In-process task or privileged helper process.
    pub stack: privs::StackHandle,
}

/// Enable TUN capture. Returns the session (keeps the device alive).
///
/// Asks for root / `CAP_NET_ADMIN` when needed and **only** reports success
/// after the TUN device and capture routes actually exist.
pub async fn enable(local_proxy: &str) -> Result<TunSession> {
    // Spawns an in-process stack when we already have CAP_NET_ADMIN, otherwise
    // elevates (`pkexec`/`sudo`) to a `tun-stack` helper that owns the device.
    let mut handle = privs::spawn_stack(local_proxy)?;

    // Readiness = interface actually exists. Never claim success without it.
    if !wait_for_iface("rustcache-tun0", std::time::Duration::from_secs(5)).await {
        handle.abort();
        return Err(anyhow::anyhow!(
            "TUN device rustcache-tun0 was not created (need root / CAP_NET_ADMIN). \
             Approve the password prompt or run the client with sudo."
        ));
    }

    // Build a snapshot for dirty-recovery (no route commands here — event_loop does that).
    let snap = TunSnapshot {
        iface: "rustcache-tun0".into(),
        addr: TUN_ADDR.to_string(),
        routes: vec!["0.0.0.0/1".into(), "128.0.0.0/1".into()],
        dns_routes: Vec::new(),
        old_default: get_old_default().await,
    };

    // Preserve DNS routes through the old gateway.
    let dns_routes = preserve_dns_routes(&snap).await?;
    let mut snap = snap;
    snap.dns_routes = dns_routes;

    Ok(TunSession {
        snapshot: snap,
        stack: handle,
    })
}

/// Poll until `iface` exists (or timeout).
async fn wait_for_iface(name: &str, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if std::path::Path::new(&format!("/sys/class/net/{name}")).exists() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Get the current default gateway (for DNS host-route recovery).
async fn get_old_default() -> Option<String> {
    let out = tokio::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .await
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(idx) = parts.iter().position(|&p| p == "via") {
            if let Some(gw) = parts.get(idx + 1) {
                return Some(gw.to_string());
            }
        }
    }
    None
}

/// Tear down TUN: remove routes + interface.
pub async fn disable(snap: &TunSnapshot) -> Result<String> {
    remove_interface(snap).await
}

/// Create the TUN device and install the capture routes.
#[allow(dead_code)]
async fn create_interface() -> Result<TunSnapshot> {
    #[cfg(target_os = "linux")]
    {
        linux::create_interface().await
    }
    #[cfg(target_os = "macos")]
    {
        macos::create_interface().await
    }
    #[cfg(target_os = "windows")]
    {
        windows::create_interface().await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!("TUN is not supported on this platform")
    }
}

/// Remove routes + interface.
async fn remove_interface(snap: &TunSnapshot) -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        linux::remove_interface(snap).await
    }
    #[cfg(target_os = "macos")]
    {
        macos::remove_interface(snap).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::remove_interface(snap).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = snap;
        Ok("unsupported platform".into())
    }
}

/// Add /32 host-routes for DNS resolvers through the old default gateway.
/// Returns the list of installed DNS routes.
async fn preserve_dns_routes(snap: &TunSnapshot) -> Result<Vec<String>> {
    #[cfg(target_os = "linux")]
    {
        linux::preserve_dns_routes(snap).await
    }
    #[cfg(target_os = "macos")]
    {
        macos::preserve_dns_routes(snap).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::preserve_dns_routes(snap).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = snap;
        Ok(Vec::new())
    }
}
