//! Outbound sockets that must bypass a local capture TUN.
//!
//! When a client-side capture TUN installs `0.0.0.0/1` + `128.0.0.0/1`, our
//! origin fetch would be re-captured and looped back into the local proxy
//! (502 on every request). Binding the source address to the LAN IP lets
//! policy routing (`ip rule from <lan_ip>/32 table <n>`) send those packets
//! around the TUN via the real gateway.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::OnceLock;

static LAN_IP: OnceLock<Option<Ipv4Addr>> = OnceLock::new();

/// Cached LAN IPv4 the kernel would use to reach the default gateway.
/// Works even while capture TUN routes are installed: the connected route
/// to the gateway is more specific than `0.0.0.0/1`.
pub fn lan_ip() -> Option<Ipv4Addr> {
    *LAN_IP.get_or_init(detect_lan_ip)
}

fn detect_lan_ip() -> Option<Ipv4Addr> {
    let gw = default_gateway()?;
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // UDP connect is a route lookup only — no packet is sent.
    s.connect(SocketAddr::new(gw.into(), 53)).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// Parse the IPv4 default gateway from `/proc/net/route` (no `ip` binary).
pub fn default_gateway() -> Option<Ipv4Addr> {
    let text = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in text.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let dest = cols.next()?;
        let gw_hex = cols.next()?;
        let flags: u32 = cols.nth(5)?.parse().ok()?;
        if dest == "00000000" && flags & 0x3 == 0x3 {
            let gw = u32::from_str_radix(gw_hex, 16).ok()?;
            // /proc/net/route is little-endian hex.
            return Some(Ipv4Addr::from(gw.to_le_bytes()));
        }
    }
    None
}

/// Bind an outbound TCP socket to the LAN address so policy routing can
/// steer it around a capture TUN. Returns the unbound `TcpSocket` when no
/// LAN IP is known (kernel default path).
pub fn bind_lan(socket: tokio::net::TcpSocket) -> tokio::net::TcpSocket {
    if let Some(ip) = lan_ip() {
        let _ = socket.bind(SocketAddr::new(ip.into(), 0));
    }
    socket
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proc_does_not_panic() {
        let _ = default_gateway();
        let _ = lan_ip();
    }
}
