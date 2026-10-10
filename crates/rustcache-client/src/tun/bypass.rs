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
///
/// Columns: Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT
/// (Destination/Gateway are little-endian hex).
pub fn default_gateway() -> Option<Ipv4Addr> {
    let text = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in text.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let _iface = cols.next()?;
        let dest = cols.next()?;
        let gw_hex = cols.next()?;
        let flags: u32 = cols.next()?.parse().ok()?;
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
        let iface = cols.next()?.to_string();
        let dest = cols.next()?;
        let _gw = cols.next()?;
        let flags: u32 = cols.next()?.parse().ok()?;
        if dest == "00000000" && flags & 0x3 == 0x3 && iface != "rustcache-tun0" {
            return Some(iface);
        }
    }
    None
}

/// True for tun2proxy virtual-DNS fake answers (default pool `198.18.0.0/15`).
pub fn is_capture_fake_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            const POOL: u32 = 0xC612_0000; // 198.18.0.0
            const MASK: u32 = 0xFFFE_0000; // /15
            (u32::from(v4) & MASK) == POOL
        }
        _ => false,
    }
}

/// Resolve `host:port` without returning TUN fake-IPs. System resolver first
/// (hosts/nsswitch); on failure or fake-only answers, query real resolvers
/// from a LAN-bound socket so the query itself skips the TUN.
pub async fn resolve_host(host: &str, port: u16) -> std::io::Result<Vec<std::net::SocketAddr>> {
    use std::net::{IpAddr, SocketAddr};

    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let system: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map(|it| it.collect())
        .unwrap_or_default();
    let real: Vec<SocketAddr> = system
        .iter()
        .copied()
        .filter(|sa| !is_capture_fake_ip(sa.ip()))
        .collect();
    if !real.is_empty() {
        return Ok(real);
    }

    match query_real_dns(host, port).await {
        Ok(addrs) if !addrs.is_empty() => Ok(addrs),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("resolve {host}: no addresses"),
        )),
        Err(e) if system.is_empty() => Err(e),
        Err(e) => Err(std::io::Error::new(
            e.kind(),
            format!("resolve {host}: only TUN fake-IPs and real DNS failed: {e}"),
        )),
    }
}

/// Minimal A/AAAA query to real resolvers (never the TUN virtual DNS).
async fn query_real_dns(host: &str, port: u16) -> std::io::Result<Vec<std::net::SocketAddr>> {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    let servers = real_dns_servers();
    let bind_ip = lan_ip()
        .map(IpAddr::V4)
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, "no DNS servers");
    for server in servers {
        let sock = match tokio::net::UdpSocket::bind(SocketAddr::new(bind_ip, 0)).await {
            Ok(s) => s,
            Err(e) => {
                last = e;
                continue;
            }
        };
        if sock.connect(SocketAddr::new(server, 53)).await.is_err() {
            continue;
        }
        let id = {
            use std::time::{SystemTime, UNIX_EPOCH};
            let ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            (ns ^ (std::process::id() as u128)) as u16
        };
        if let Ok(q) = encode_dns_query(id, host, 1) {
            if sock.send(&q).await.is_ok() {
                let mut buf = [0u8; 1500];
                match tokio::time::timeout(
                    std::time::Duration::from_millis(1500),
                    sock.recv(&mut buf),
                )
                .await
                {
                    Ok(Ok(n)) => {
                        let ips = parse_dns_a(&buf[..n], id);
                        if !ips.is_empty() {
                            return Ok(ips
                                .into_iter()
                                .map(|ip| SocketAddr::new(ip, port))
                                .collect());
                        }
                    }
                    Ok(Err(e)) => last = e,
                    Err(_) => {
                        last = std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("dns timeout from {server}"),
                        )
                    }
                }
            }
        }
    }
    Err(last)
}

fn real_dns_servers() -> Vec<std::net::IpAddr> {
    use std::net::IpAddr;
    let mut out = Vec::new();
    let mut push = |ip: IpAddr| {
        if !is_capture_fake_ip(ip) && !out.contains(&ip) {
            out.push(ip);
        }
    };
    for path in [
        "/run/systemd/resolve/resolv.conf",
        "/run/NetworkManager/resolv.conf",
        "/etc/resolv.conf",
    ] {
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                if let Some(rest) = line.trim().strip_prefix("nameserver") {
                    if let Ok(ip) = rest.trim().parse::<IpAddr>() {
                        push(ip);
                    }
                }
            }
        }
    }
    push(IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 53)));
    if let Some(gw) = default_gateway() {
        push(IpAddr::V4(gw));
    }
    push(IpAddr::V4(std::net::Ipv4Addr::new(1, 1, 1, 1)));
    push(IpAddr::V4(std::net::Ipv4Addr::new(8, 8, 8, 8)));
    out
}

/// Resolvers that must stay reachable via the real gateway while TUN is up.
/// Used to install /32 host-routes (more specific than 0.0.0.0/1).
pub fn dns_bypass_targets() -> Vec<std::net::IpAddr> {
    real_dns_servers()
}

fn encode_dns_query(id: u16, name: &str, qtype: u16) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("bad DNS label in `{name}`"),
            ));
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    Ok(buf)
}

fn parse_dns_a(msg: &[u8], id: u16) -> Vec<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr};
    if msg.len() < 12 || u16::from_be_bytes([msg[0], msg[1]]) != id {
        return Vec::new();
    }
    let qd = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let an = u16::from_be_bytes([msg[6], msg[7]]) as usize;
    let mut i = 12usize;
    for _ in 0..qd {
        if !skip_name(msg, &mut i) {
            return Vec::new();
        }
        i = i.saturating_add(4);
    }
    let mut out = Vec::new();
    for _ in 0..an {
        if !skip_name(msg, &mut i) || i + 10 > msg.len() {
            break;
        }
        let rtype = u16::from_be_bytes([msg[i], msg[i + 1]]);
        let rdlen = u16::from_be_bytes([msg[i + 8], msg[i + 9]]) as usize;
        i += 10;
        if i + rdlen > msg.len() {
            break;
        }
        if rtype == 1 && rdlen == 4 {
            out.push(IpAddr::V4(Ipv4Addr::new(
                msg[i],
                msg[i + 1],
                msg[i + 2],
                msg[i + 3],
            )));
        }
        i += rdlen;
    }
    out
}

fn skip_name(msg: &[u8], i: &mut usize) -> bool {
    let mut hops = 0;
    while *i < msg.len() {
        let len = msg[*i];
        if len == 0 {
            *i += 1;
            return true;
        }
        if len & 0xC0 == 0xC0 {
            *i += 2;
            return true;
        }
        if len > 63 {
            return false;
        }
        *i += 1 + len as usize;
        hops += 1;
        if hops > 64 {
            return false;
        }
    }
    false
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
