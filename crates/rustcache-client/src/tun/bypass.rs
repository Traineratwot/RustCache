//! Outbound sockets that must bypass the capture TUN.
//!
//! When the TUN capture routes (0.0.0.0/1 + 128.0.0.0/1) are installed, every
//! packet the *origin fetch* side sends is re-captured by tun2proxy and sent
//! back into the local proxy — a routing loop that surfaces as 502 on every
//! request. The fix is policy routing:
//!
//!   ip route add default via <old_gw> dev <old_if> table 100
//!   ip rule add from <lan_ip>/32 table 100
//!
//! Packets sourced from the LAN address use table 100 (the real gateway) and
//! never enter the TUN. `bind_outbound` binds sockets to that LAN address.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::OnceLock;

/// Routing table that keeps the pre-TUN default route.
pub const BYPASS_TABLE: &str = "100";

/// Cached LAN IPv4 used as the source for TUN-bypassing sockets.
static LAN_IP: OnceLock<Option<Ipv4Addr>> = OnceLock::new();

/// Detect the IPv4 the kernel would use to reach the default gateway.
/// Works even when TUN routes are installed: the connected route to the
/// gateway is more specific than 0.0.0.0/1.
pub fn lan_ip() -> Option<Ipv4Addr> {
    *LAN_IP.get_or_init(detect_lan_ip)
}

fn detect_lan_ip() -> Option<Ipv4Addr> {
    let gw = default_gateway()?;
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // UDP "connect" is a route lookup only — no packet is sent.
    s.connect(SocketAddr::new(gw.into(), 53)).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// Parse the IPv4 default gateway from `/proc/net/route` (no `ip` binary needed).
pub fn default_gateway() -> Option<Ipv4Addr> {
    let text = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in text.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let dest = cols.next()?;
        let gw_hex = cols.next()?;
        let flags: u32 = cols.nth(5)?.parse().ok()?;
        // RTF_UP | RTF_GATEWAY
        if dest == "00000000" && flags & 0x3 == 0x3 {
            let gw = u32::from_str_radix(gw_hex, 16).ok()?;
            // /proc/net/route is little-endian hex.
            return Some(Ipv4Addr::from(gw.to_le_bytes()));
        }
    }
    None
}

/// Parse the default route's interface name from `/proc/net/route`.
pub fn default_iface() -> Option<String> {
    let text = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in text.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let dest = cols.next()?;
        let _gw = cols.next()?;
        let flags: u32 = cols.nth(5)?.parse().ok()?;
        let iface = cols.next()?.to_string();
        if dest == "00000000" && flags & 0x3 == 0x3 && iface != "rustcache-tun0" {
            return Some(iface);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_gateway_parses_proc() {
        // Just ensure it doesn't panic; the host may or may not have a default.
        let _ = default_gateway();
        let _ = default_iface();
    }
}
