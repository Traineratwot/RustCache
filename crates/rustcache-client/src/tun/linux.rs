//! Linux TUN: create /dev/net/tun interface + routes + DNS host-routes.

use std::process::Command;

use anyhow::{Context, Result};

use super::TunSnapshot;

const IFACE: &str = "rustcache-tun0";

/// Create the TUN interface and install capture routes (0.0.0.0/1 + 128.0.0.0/1).
pub async fn create_interface() -> Result<TunSnapshot> {
    // The actual TUN device is created by `tun` crate in stack::open_tun.
    // Here we configure the routes.
    let addr = crate::tun::TUN_ADDR.to_string();
    let routes = vec!["0.0.0.0/1".to_string(), "128.0.0.0/1".to_string()];

    // Add routes via `ip route` (Linux).
    for r in &routes {
        let out = Command::new("ip")
            .args(["route", "add", r, "dev", IFACE])
            .output()
            .context("ip route add")?;
        if !out.status.success() {
            tracing::warn!(
                route = %r,
                stderr = %String::from_utf8_lossy(&out.stderr),
                "route add failed (may already exist)"
            );
        }
    }

    // Save the old default gateway for DNS route recovery.
    let old_default = get_default_gateway().await;

    Ok(TunSnapshot {
        iface: IFACE.to_string(),
        addr,
        routes,
        dns_routes: Vec::new(),
        old_default,
    })
}

/// Remove routes + interface.
pub async fn remove_interface(snap: &TunSnapshot) -> Result<String> {
    // Remove capture routes.
    for r in &snap.routes {
        let _ = Command::new("ip")
            .args(["route", "del", r, "dev", &snap.iface])
            .output();
    }
    // Remove DNS host-routes.
    for r in &snap.dns_routes {
        let _ = Command::new("ip").args(["route", "del", r]).output();
    }
    // Bring the interface down and delete it.
    let _ = Command::new("ip")
        .args(["link", "set", &snap.iface, "down"])
        .output();
    let _ = Command::new("ip")
        .args(["link", "del", &snap.iface])
        .output();
    Ok("tun routes removed".into())
}

/// Add /32 host-routes for DNS resolvers through the old default gateway.
pub async fn preserve_dns_routes(snap: &TunSnapshot) -> Result<Vec<String>> {
    let resolvers = read_resolvers();
    let Some(ref gw) = snap.old_default else {
        tracing::warn!("no old default gateway — cannot preserve DNS routes");
        return Ok(Vec::new());
    };
    let mut installed = Vec::new();
    for dns in &resolvers {
        let route = format!("{dns}/32");
        let out = Command::new("ip")
            .args(["route", "add", &route, "via", gw])
            .output();
        match out {
            Ok(o) if o.status.success() => installed.push(route),
            Ok(o) => {
                tracing::warn!(
                    dns = %dns,
                    stderr = %String::from_utf8_lossy(&o.stderr),
                    "DNS route add failed"
                );
            }
            Err(e) => tracing::warn!(error = %e, "DNS route add failed"),
        }
    }
    Ok(installed)
}

/// Get the current default gateway (for DNS host-route recovery).
async fn get_default_gateway() -> Option<String> {
    let out = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "default via 192.168.1.1 dev eth0 ..."
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

/// Read DNS resolvers from /etc/resolv.conf.
fn read_resolvers() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(text) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("nameserver") {
                let ip = rest.trim();
                if !ip.is_empty() && !v.contains(&ip.to_string()) {
                    v.push(ip.to_string());
                }
            }
        }
    }
    v
}
