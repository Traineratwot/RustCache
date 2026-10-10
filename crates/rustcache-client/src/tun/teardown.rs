//! Force-restore the network after TUN capture.
//!
//! `tproxy-setup` (inside tun2proxy) leaves three things behind:
//!
//! 1. Capture routes `0.0.0.0/1` + `128.0.0.0/1` via `rustcache-tun0`
//! 2. Policy rules / table from our bypass routing
//! 3. A **bind-mount over `/etc/resolv.conf`** pointing at the TUN gateway
//!
//! (3) is what kills the internet after disable: even with the routes gone,
//! DNS still resolves via the dead TUN address. The helper's `tproxy_remove`
//! only runs on clean exit — if the helper is SIGKILLed or the process dies,
//! nothing restores. This module is the belt-and-braces path: run it as root
//! (`tun-teardown`) to always get back to a working network.

use std::process::Command;

use anyhow::Result;

const IFACE: &str = super::stack::TUN_IFACE;
const BYPASS_TABLE: &str = super::bypass::BYPASS_TABLE;

fn find_ip_bin() -> String {
    find_util_bin("ip")
}

fn find_util_bin(name: &str) -> String {
    for dir in ["/usr/sbin", "/usr/bin", "/sbin", "/bin"] {
        let p = format!("{dir}/{name}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    name.to_string()
}

/// Run every restore step. Requires root (CAP_NET_ADMIN + mount). Safe to run
/// repeatedly — each step tolerates "already gone".
pub fn force_restore() -> Result<Vec<String>> {
    let mut done = Vec::new();
    let ip = find_ip_bin();

    // 0. Stop any leftover privileged helper — it re-creates the device and
    //    re-installs routes while it lives. Unprivileged kill gets EPERM.
    let pkill = find_util_bin("pkill");
    match run(&pkill, &["-f", "rustcache-client tun-stack"]) {
        Ok(true) => done.push("tun-stack helper killed".into()),
        Ok(false) => {}
        Err(e) => done.push(format!("pkill helper failed: {e:#}")),
    }

    // 1. Unmount the resolv.conf bind-mount FIRST — this is what breaks DNS.
    match unmount_resolv_conf() {
        Ok(true) => done.push("resolv.conf unmounted".into()),
        Ok(false) => {}
        Err(e) => done.push(format!("resolv.conf umount failed: {e:#}")),
    }

    // 2. Delete the TUN device — its routes disappear with it.
    match run(&ip, &["link", "del", IFACE]) {
        Ok(true) => done.push(format!("link {IFACE} deleted")),
        Ok(false) => {}
        Err(e) => done.push(format!("link del failed: {e:#}")),
    }

    // 3. Belt: remove capture routes in case the device was already gone
    //    but the routes lingered (kernel keeps them if the link is renamed).
    for r in ["0.0.0.0/1", "128.0.0.0/1"] {
        let _ = run(&ip, &["route", "del", r, "dev", IFACE]);
    }

    // 4. Our bypass policy routing + DNS host-routes + DNS destination rules.
    for args in [
        vec!["rule", "del", "priority", "100"],
        vec!["rule", "del", "priority", "90"],
    ] {
        let _ = run(&ip, &args);
    }
    for dns in super::bypass::dns_bypass_targets() {
        if dns.is_loopback() {
            continue;
        }
        let dest = match dns {
            std::net::IpAddr::V4(v4) => format!("{v4}/32"),
            std::net::IpAddr::V6(v6) => format!("{v6}/128"),
        };
        for _ in 0..3 {
            let _ = run(&ip, &["rule", "del", "priority", "50", "to", &dest]);
        }
        let _ = run(&ip, &["route", "del", &dest]);
    }
    let _ = run(&ip, &["route", "flush", "table", BYPASS_TABLE]);
    done.push("bypass routing removed".into());

    // 5. Flush DNS caches so the new resolv.conf takes effect immediately.
    let _ = run("resolvectl", &["flush-caches"]);
    let _ = run("systemd-resolve", &["--flush-caches"]);

    if done.is_empty() {
        done.push("nothing to clean".into());
    }
    Ok(done)
}

/// `true` when a resolv.conf bind-mount is present and was removed.
fn unmount_resolv_conf() -> Result<bool> {
    // `findmnt` reports the mount only when the bind is active.
    let mounted = Command::new("findmnt")
        .args(["--noheadings", "/etc/resolv.conf"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !mounted {
        // Fallback: try umount anyway (busybox / no findmnt).
        let out = Command::new("umount").arg("/etc/resolv.conf").output();
        return Ok(matches!(out, Ok(o) if o.status.success()));
    }

    // The mount is often remounted RO by tproxy-config — make it RW first
    // so umount is not rejected, then unmount.
    let _ = Command::new("mount")
        .args(["-o", "remount,rw", "/etc/resolv.conf"])
        .output();
    let out = Command::new("umount").arg("/etc/resolv.conf").output()?;
    if out.status.success() {
        Ok(true)
    } else {
        Err(anyhow::anyhow!(
            "umount /etc/resolv.conf: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn run(cmd: &str, args: &[&str]) -> Result<bool> {
    let out = Command::new(cmd).args(args).output()?;
    Ok(out.status.success())
}

#[cfg(test)]
mod tests {
    // force_restore needs root; only exercise the no-root helpers here.

    #[test]
    fn unmount_resolv_conf_is_idempotent() {
        // Must not panic when findmnt is missing or nothing is mounted.
        let _ = super::unmount_resolv_conf();
    }
}
