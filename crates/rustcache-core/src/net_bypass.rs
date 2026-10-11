//! Outbound sockets that must bypass a local capture TUN.
//!
//! When a client-side capture TUN installs `0.0.0.0/1` + `128.0.0.0/1`, our
//! origin fetch would be re-captured and looped back into the local proxy
//! (502 on every request). Two things are required to keep the server on the
//! real internet:
//!
//! 1. **Source bind + policy routing** — bind the source address to the LAN IP
//!    so `ip rule from <lan_ip>/32 table <n>` steers packets around the TUN.
//! 2. **Real DNS** — while *our* capture TUN is up, its virtual DNS answers with
//!    fake IPs (`198.18.0.0/15`). Those are not routable on the real internet,
//!    so origin connect fails (or loops) even with (1). [`resolve_host`]
//!    detects them and re-queries real resolvers directly.
//!
//! `198.18.0.0/15` is only poisoned while `rustcache-tun0` is up. On an external
//! Clash/Mihomo fake-IP network the same range is a legitimate routable endpoint
//! set and must be used as-is.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, anyhow};

// Re-exported so existing `net_bypass::split_host_port` call sites keep working.
// The implementation lives in `rustcache-util` and is shared with the client.
pub use rustcache_util::split_host_port;

static LAN_IP: OnceLock<Option<Ipv4Addr>> = OnceLock::new();
static REAL_DNS: OnceLock<Vec<IpAddr>> = OnceLock::new();

/// tun2proxy virtual-DNS pool (default `198.18.0.0/15`). Poisoned only while
/// our capture TUN is up — see [`select_system_addrs`].
const FAKE_IP_NET: u32 = 0xC612_0000; // 198.18.0.0
const FAKE_IP_MASK: u32 = 0xFFFE_0000; // /15

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
        IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

/// Parse the IPv4 default gateway from `/proc/net/route` (no `ip` binary).
///
/// Columns: Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT
/// (Destination/Gateway are little-endian hex).
pub fn default_gateway() -> Option<Ipv4Addr> {
    let text = std::fs::read_to_string("/proc/net/route").ok()?;
    parse_default_gateway(&text)
}

fn parse_default_gateway(text: &str) -> Option<Ipv4Addr> {
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

/// True when OUR capture TUN interface is up. Fake-IP answers are only
/// poisoned while this is true; on a Clash/Mihomo network `198.18/15` is a
/// legitimate routable endpoint range.
pub fn capture_tun_present() -> bool {
    std::path::Path::new("/sys/class/net/rustcache-tun0").exists()
}

/// Decide what from the system lookup we may hand to a connect.
/// Returns `(usable, need_real_dns_fallback)`.
fn select_system_addrs(system: Vec<SocketAddr>, tun_present: bool) -> (Vec<SocketAddr>, bool) {
    if !tun_present {
        // Network DNS may legitimately answer with Clash/Mihomo fake-IPs
        // (198.18/15). Those are routable here — keep them.
        let empty = system.is_empty();
        return (system, empty);
    }
    let real: Vec<SocketAddr> = system
        .iter()
        .copied()
        .filter(|sa| !is_capture_fake_ip(sa.ip()))
        .collect();
    let need = real.is_empty();
    (real, need)
}

/// True when `ip` is a TUN virtual-DNS fake answer. Only meaningful while
/// [`capture_tun_present`] — see [`select_system_addrs`].
pub fn is_capture_fake_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => (u32::from(v4) & FAKE_IP_MASK) == FAKE_IP_NET,
        IpAddr::V6(v6) => {
            // tun2proxy's pool is IPv4; also treat Teredo/6to4 embeds of the pool
            // as fake so they cannot sneak through as "real" destinations.
            let seg = v6.segments();
            seg[0] == 0x2002
                && ((u32::from(Ipv4Addr::new(
                    (seg[1] >> 8) as u8,
                    seg[1] as u8,
                    (seg[2] >> 8) as u8,
                    seg[2] as u8,
                )) & FAKE_IP_MASK)
                    == FAKE_IP_NET)
        }
    }
}

/// Bind an outbound TCP socket to the LAN address so policy routing can
/// steer it around a capture TUN. Returns the unbound `TcpSocket` when no
/// LAN IP is known (kernel default path).
///
/// **Never binds for loopback destinations** — a source address on another
/// interface cannot reach 127.0.0.1 (kernel returns EINVAL), which used to
/// break the client→RustCache hop once `lan_ip()` started working.
pub fn bind_lan(socket: tokio::net::TcpSocket) -> tokio::net::TcpSocket {
    bind_lan_to(socket, None)
}

/// Like [`bind_lan`], but skips the bind when `dest` is loopback.
pub fn bind_lan_to(socket: tokio::net::TcpSocket, dest: Option<IpAddr>) -> tokio::net::TcpSocket {
    if let Some(d) = dest {
        if d.is_loopback() {
            return socket;
        }
    }
    match lan_ip() {
        Some(ip) => {
            if let Err(e) = socket.bind(SocketAddr::new(ip.into(), 0)) {
                tracing::warn!(%ip, error = %e, "bind_lan failed — origin may loop via TUN");
            }
        }
        None => {
            tracing::warn!("bind_lan: no LAN IP detected — origin may loop via TUN");
        }
    }
    socket
}

/// One-shot per TUN state change: dump the bypass-relevant network state so a
/// log snippet shows whether policy routing / fake-IP DNS are actually in play.
pub fn log_bypass_context(where_: &str) {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0 = never, 1 = logged without TUN, 2 = logged with TUN. Re-logs on flip.
    static STATE: AtomicU8 = AtomicU8::new(0);
    let tun_present = std::path::Path::new("/sys/class/net/rustcache-tun0").exists();
    let want = if tun_present { 2u8 } else { 1u8 };
    if STATE.swap(want, Ordering::Relaxed) == want {
        return;
    }
    let lan = lan_ip();
    let gw = default_gateway();
    let resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
    let resolv_ns: Vec<&str> = resolv
        .lines()
        .filter(|l| l.trim_start().starts_with("nameserver"))
        .collect();
    let rules = std::process::Command::new("ip")
        .args(["rule", "show"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|e| format!("<ip rule: {e}>"));
    let table = std::process::Command::new("ip")
        .args(["route", "show", "table", "100"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|e| format!("<ip route table 100: {e}>"));
    tracing::info!(
        where_,
        ?lan,
        ?gw,
        tun_present,
        resolv = ?resolv_ns,
        ip_rules = %rules,
        table100 = %table,
        "bypass context"
    );
}

/// Log which interface/kernel path a packet to `dest` would take.
/// `src` is the LAN bind we applied, if any. Only logs while a capture TUN
/// is present — that is the only time the route choice is interesting.
pub fn log_route_decision(dest: IpAddr, src: Option<IpAddr>) {
    if !std::path::Path::new("/sys/class/net/rustcache-tun0").exists() {
        return;
    }
    let mut args = vec!["route".to_string(), "get".to_string(), dest.to_string()];
    if let Some(s) = src {
        args.push("from".into());
        args.push(s.to_string());
    }
    let out = std::process::Command::new("ip").args(&args).output();
    let text = match out {
        Ok(o) => format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => format!("<ip route get: {e}>"),
    };
    tracing::info!(%dest, ?src, route = %text.trim(), "route decision");
}

/// Resolve `host:port` without ever handing back *poisoned* TUN fake-IPs.
///
/// Tries the system resolver first (covers `/etc/hosts`, nsswitch, mDNS).
/// While our capture TUN is up, `198.18.0.0/15` answers are dropped and real
/// resolvers are queried from a LAN-bound socket so the packets skip the TUN.
/// Without a local TUN those same answers are a network fake-IP DNS and are
/// used as-is.
pub async fn resolve_host(host: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
    log_bypass_context("resolve_host");
    if let Ok(ip) = host.parse::<IpAddr>() {
        tracing::debug!(%host, %ip, "resolve_host: IP literal");
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let system: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map(|it| it.collect())
        .unwrap_or_default();
    let tun_present = capture_tun_present();
    let (usable, need_real_dns) = select_system_addrs(system.clone(), tun_present);
    let fake_n = system.len() - usable.len();
    // Loud only while a capture TUN is up — that is when this path matters.
    if tun_present || fake_n > 0 {
        tracing::info!(
            %host,
            port,
            tun_present,
            system = ?system.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            real_n = usable.len(),
            fake_n,
            "resolve_host: system lookup"
        );
    } else {
        tracing::debug!(%host, port, n = system.len(), "resolve_host: system lookup");
    }
    if !usable.is_empty() {
        return Ok(usable);
    }
    if !need_real_dns {
        // No TUN and system returned nothing — nothing to fall back from.
        return Err(anyhow!("resolve {host}: no addresses"));
    }

    match resolve_via_real_dns(host, port).await {
        Ok(addrs) if !addrs.is_empty() => {
            tracing::info!(
                %host,
                port,
                addrs = ?addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                "resolve_host: real DNS used (system had only TUN fake-IPs or failed)"
            );
            Ok(addrs)
        }
        Ok(_) => {
            tracing::warn!(%host, "resolve_host: real DNS returned no addresses");
            Err(anyhow!("resolve {host}: no addresses"))
        }
        Err(e) => {
            tracing::warn!(%host, error = %e, "resolve_host: real DNS failed");
            if system.is_empty() {
                Err(e)
            } else {
                // Capture TUN is up and system gave only poisoned fake-IPs —
                // still refuse to return them (they would loop via the TUN).
                Err(anyhow!(
                    "resolve {host}: only TUN fake-IPs and real DNS failed: {e:#}"
                ))
            }
        }
    }
}

/// Open a TCP connection to `addr` (`host:port`) that skips a capture TUN:
/// real DNS + LAN source bind. Each address is tried under `timeout`.
pub async fn connect_bypass(
    addr: &str,
    timeout: Duration,
) -> anyhow::Result<tokio::net::TcpStream> {
    let (host, port) = split_host_port(addr)?;
    let addrs = resolve_host(&host, port)
        .await
        .with_context(|| format!("resolve {addr}"))?;
    tracing::info!(%addr, ?addrs, "connect_bypass: resolved");
    if let Some(sa) = addrs.iter().find(|a| a.is_ipv4()) {
        log_route_decision(sa.ip(), lan_ip().map(IpAddr::V4));
    }
    let mut last_err = None;
    // Prefer IPv4 (LAN bind / policy routing is IPv4), then IPv6.
    let mut ordered = addrs;
    ordered.sort_by_key(|sa| if sa.is_ipv4() { 0 } else { 1 });
    for sa in ordered {
        let socket = if sa.is_ipv4() {
            bind_lan_to(
                tokio::net::TcpSocket::new_v4().map_err(|e| anyhow!("tcp socket: {e}"))?,
                Some(sa.ip()),
            )
        } else {
            tokio::net::TcpSocket::new_v6().map_err(|e| anyhow!("tcp socket: {e}"))?
        };
        socket.set_nodelay(true).ok();
        let local = socket
            .local_addr()
            .ok()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "?".into());
        match tokio::time::timeout(timeout, socket.connect(sa)).await {
            Ok(Ok(stream)) => {
                tracing::info!(%addr, %sa, %local, "connect_bypass: ok");
                return Ok(stream);
            }
            Ok(Err(e)) => {
                tracing::warn!(%addr, %sa, %local, error = %e, "connect_bypass: connect failed");
                last_err = Some(anyhow!("connect {sa}: {e}"));
            }
            Err(_) => {
                tracing::warn!(%addr, %sa, %local, "connect_bypass: connect timeout");
                last_err = Some(anyhow!("connect {sa}: timeout"));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("connect {addr}: no usable addresses")))
}

/// Real resolvers we can query when the system ones are poisoned by the TUN.
fn real_dns_servers() -> Vec<IpAddr> {
    REAL_DNS.get_or_init(discover_real_dns).clone()
}

fn discover_real_dns() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    let mut push = |ip: IpAddr| {
        // Skip TUN-local addresses (virtual-DNS pool / tun gateway). Those are
        // exactly what answers with fake-IPs; they are not real resolvers.
        if !is_capture_fake_ip(ip) && !is_tun_local_addr(ip) && !out.contains(&ip) {
            out.push(ip);
        }
    };

    // Files that survive tproxy-config's bind-mount over /etc/resolv.conf.
    for path in [
        "/run/systemd/resolve/resolv.conf",
        "/run/NetworkManager/resolv.conf",
        "/var/run/resolvconf/resolv.conf",
    ] {
        if let Ok(text) = std::fs::read_to_string(path) {
            for ns in parse_nameservers(&text) {
                push(ns);
            }
        }
    }

    // systemd-resolved stub: loopback, never hits the TUN.
    push(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 53)));

    // Live /etc/resolv.conf still helps when TUN is off or nameservers are real.
    // When TUN is up this file points at the TUN gateway — filtered above.
    if let Ok(text) = std::fs::read_to_string("/etc/resolv.conf") {
        for ns in parse_nameservers(&text) {
            push(ns);
        }
    }

    // Home routers are almost always a DNS forwarder.
    if let Some(gw) = default_gateway() {
        push(IpAddr::V4(gw));
    }

    // Public fallbacks (queried from a LAN-bound socket → policy routing).
    push(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));
    push(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
    out
}

/// True for addresses that belong to the capture TUN itself (gateway / pool).
fn is_tun_local_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let n = u32::from(v4);
            // tun2proxy TUN gateway / interface (10.0.0.0/30) and link-local of
            // the fake-IP pool are not reachable as real DNS.
            (n & 0xFFFF_FFFC) == 0x0A00_0000 // 10.0.0.0/30
                || (n & 0xFFFE_0000) == 0xC612_0000 // 198.18.0.0/15
        }
        _ => false,
    }
}

fn parse_nameservers(text: &str) -> Vec<IpAddr> {
    let mut v = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("nameserver") else {
            continue;
        };
        if let Ok(ip) = rest.trim().parse::<IpAddr>() {
            if !v.contains(&ip) {
                v.push(ip);
            }
        }
    }
    v
}

async fn resolve_via_real_dns(host: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
    let servers = real_dns_servers();
    tracing::info!(%host, ?servers, "resolve_via_real_dns: trying real resolvers");
    let mut last_err = anyhow!("no DNS servers configured");
    for server in servers {
        match query_dns_a(server, host).await {
            Ok(ips) if !ips.is_empty() => {
                tracing::info!(%host, %server, ?ips, "resolve_via_real_dns: got answers");
                return Ok(ips
                    .into_iter()
                    .map(|ip| SocketAddr::new(ip, port))
                    .collect());
            }
            Ok(ips) => {
                tracing::warn!(%host, %server, ?ips, "resolve_via_real_dns: empty/fake answers");
                last_err = anyhow!("no usable A/AAAA records for {host} at {server}");
            }
            Err(e) => {
                tracing::warn!(%host, %server, error = %e, "resolve_via_real_dns: query failed");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// Minimal DNS client (A + AAAA). UDP first; when the answer is a TUN fake-IP
/// the query re-entered the capture path, so we retry over **DNS/TCP** —
/// tun2proxy virtual DNS only hijacks UDP/53, TCP reaches the real resolver.
async fn query_dns_a(server: IpAddr, name: &str) -> anyhow::Result<Vec<IpAddr>> {
    // Loopback resolvers (127.0.0.53) must not use a LAN-bound socket — the
    // kernel rejects that bind/connect pair (EINVAL).
    let bind_ip = if server.is_loopback() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        lan_ip()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    };
    let sock = tokio::net::UdpSocket::bind(SocketAddr::new(bind_ip, 0))
        .await
        .context("dns udp bind")?;
    if let Ok(local) = sock.local_addr() {
        tracing::debug!(%name, %server, %local, "query_dns_a: udp socket");
    }
    sock.connect(SocketAddr::new(server, 53))
        .await
        .with_context(|| format!("dns udp connect {server}"))?;

    let id = dns_txid();
    let mut ips = Vec::new();
    let mut saw_fake = 0usize;
    for qtype in [1u16, 28u16] {
        // A, AAAA
        let q = encode_dns_query(id, name, qtype)?;
        sock.send(&q)
            .await
            .with_context(|| format!("dns send {server}"))?;
        let mut buf = [0u8; 1500];
        match tokio::time::timeout(Duration::from_millis(1500), sock.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                let answers = parse_dns_answers(&buf[..n], id, qtype);
                saw_fake += answers.iter().filter(|ip| is_capture_fake_ip(**ip)).count();
                ips.extend(answers);
            }
            Ok(Err(e)) => {
                tracing::warn!(%name, %server, error = %e, "query_dns_a: udp recv error");
                break;
            }
            Err(_) => {
                tracing::warn!(%name, %server, "query_dns_a: udp timeout — trying DNS/TCP");
                break;
            }
        }
        if qtype == 1 && !ips.is_empty() {
            // Prefer A; still try AAAA only when A is empty (v4-only callers).
            break;
        }
    }
    // TUN virtual DNS can still answer if the query leaked into the TUN —
    // never hand those fake-IPs to a connect while our TUN is up. Without a
    // local TUN they are a network fake-IP DNS and must be kept.
    if capture_tun_present() {
        let before = ips.len();
        ips.retain(|ip| !is_capture_fake_ip(*ip));
        if before != ips.len() {
            tracing::warn!(
                %name,
                %server,
                dropped = before - ips.len(),
                "query_dns_a: UDP got TUN fake-IPs — retrying DNS/TCP"
            );
            return query_dns_tcp(server, name).await;
        }
    }
    if ips.is_empty() && saw_fake == 0 {
        // Empty UDP answer (or timeout path already returned Err). Try TCP too.
        return query_dns_tcp(server, name).await;
    }
    Ok(ips)
}

/// DNS over TCP (RFC 7766): 2-byte length prefix + DNS message. tun2proxy's
/// virtual DNS only hijacks UDP/53, so this reaches the real resolver even
/// when UDP is being re-captured.
async fn query_dns_tcp(server: IpAddr, name: &str) -> anyhow::Result<Vec<IpAddr>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let bind_ip = if server.is_loopback() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        lan_ip()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    };
    let socket = if server.is_ipv4() {
        bind_lan_to(
            tokio::net::TcpSocket::new_v4().map_err(|e| anyhow!("tcp socket: {e}"))?,
            Some(server),
        )
    } else {
        tokio::net::TcpSocket::new_v6().map_err(|e| anyhow!("tcp socket: {e}"))?
    };
    let _ = socket.set_nodelay(true);
    let _ = bind_ip; // already applied via bind_lan_to for non-loopback
    let mut stream = tokio::time::timeout(
        Duration::from_millis(2000),
        socket.connect(SocketAddr::new(server, 53)),
    )
    .await
    .map_err(|_| anyhow!("dns tcp connect timeout {server}"))?
    .with_context(|| format!("dns tcp connect {server}"))?;

    let id = dns_txid();
    let mut ips = Vec::new();
    for qtype in [1u16, 28u16] {
        let q = encode_dns_query(id, name, qtype)?;
        let mut msg = Vec::with_capacity(q.len() + 2);
        msg.extend_from_slice(&(q.len() as u16).to_be_bytes());
        msg.extend_from_slice(&q);
        stream.write_all(&msg).await.context("dns tcp send")?;
        stream.flush().await.ok();

        let mut lenb = [0u8; 2];
        tokio::time::timeout(Duration::from_millis(2000), stream.read_exact(&mut lenb))
            .await
            .map_err(|_| anyhow!("dns tcp timeout {server}"))?
            .context("dns tcp len")?;
        let len = u16::from_be_bytes(lenb) as usize;
        if len == 0 || len > 4096 {
            return Err(anyhow!("dns tcp bad length {len} from {server}"));
        }
        let mut body = vec![0u8; len];
        tokio::time::timeout(Duration::from_millis(2000), stream.read_exact(&mut body))
            .await
            .map_err(|_| anyhow!("dns tcp timeout {server}"))?
            .context("dns tcp body")?;
        ips.extend(parse_dns_answers(&body, id, qtype));
        if qtype == 1 && !ips.is_empty() {
            break;
        }
    }
    if capture_tun_present() {
        ips.retain(|ip| !is_capture_fake_ip(*ip));
    }
    tracing::info!(%name, %server, ?ips, "query_dns_tcp: answers");
    Ok(ips)
}

fn dns_txid() -> u16 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (ns ^ (std::process::id() as u128)) as u16
}

/// RFC 1035 §4.1.1 question encoding.
fn encode_dns_query(id: u16, name: &str, qtype: u16) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&id.to_be_bytes());
    buf.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(anyhow!("bad DNS label in `{name}`"));
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // IN
    Ok(buf)
}

/// RFC 1035 §4.1.3 answer section: collect A/AAAA RDATA.
fn parse_dns_answers(msg: &[u8], id: u16, qtype: u16) -> Vec<IpAddr> {
    if msg.len() < 12 {
        return Vec::new();
    }
    let rid = u16::from_be_bytes([msg[0], msg[1]]);
    if rid != id {
        return Vec::new();
    }
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let ancount = u16::from_be_bytes([msg[6], msg[7]]) as usize;
    let mut i = 12usize;
    for _ in 0..qdcount {
        if !skip_dns_name(msg, &mut i) {
            return Vec::new();
        }
        i = i.saturating_add(4);
    }
    let mut out = Vec::new();
    for _ in 0..ancount {
        if !skip_dns_name(msg, &mut i) {
            break;
        }
        if i + 10 > msg.len() {
            break;
        }
        let rtype = u16::from_be_bytes([msg[i], msg[i + 1]]);
        let _class = u16::from_be_bytes([msg[i + 2], msg[i + 3]]);
        let _ttl = u32::from_be_bytes([msg[i + 4], msg[i + 5], msg[i + 6], msg[i + 7]]);
        let rdlen = u16::from_be_bytes([msg[i + 8], msg[i + 9]]) as usize;
        i += 10;
        if i + rdlen > msg.len() {
            break;
        }
        let rdata = &msg[i..i + rdlen];
        if rtype == 1 && rdlen == 4 {
            out.push(IpAddr::V4(Ipv4Addr::new(
                rdata[0], rdata[1], rdata[2], rdata[3],
            )));
        } else if rtype == 28 && rdlen == 16 {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(rdata);
            out.push(IpAddr::V6(Ipv6Addr::from(octets)));
        }
        // Honour qtype as a hint but keep both families available.
        let _ = qtype;
        i += rdlen;
    }
    out
}

/// Advance `i` past a (possibly compressed) DNS name.
fn skip_dns_name(msg: &[u8], i: &mut usize) -> bool {
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
    fn parse_proc_does_not_panic() {
        let _ = default_gateway();
        let _ = lan_ip();
    }

    #[test]
    fn default_gateway_reads_real_proc_format() {
        // Real /proc/net/route shape: Iface Destination Gateway Flags ...
        // Gateway 0132A8C0 = 192.168.50.1 (little-endian hex).
        let text = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
enp7s0\t00000000\t0132A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0
docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
";
        assert_eq!(
            parse_default_gateway(text),
            Some(Ipv4Addr::new(192, 168, 50, 1))
        );
    }

    #[test]
    fn default_gateway_ignores_onlink_and_tun() {
        let text = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
rustcache-tun0\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0
enp7s0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0
";
        assert_eq!(parse_default_gateway(text), None);
    }

    #[test]
    fn fake_ip_detection() {
        assert!(is_capture_fake_ip("198.18.0.1".parse().unwrap()));
        assert!(is_capture_fake_ip("198.19.255.255".parse().unwrap()));
        assert!(!is_capture_fake_ip("198.20.0.1".parse().unwrap()));
        assert!(!is_capture_fake_ip("93.184.216.34".parse().unwrap()));
        assert!(!is_capture_fake_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_capture_fake_ip("::1".parse().unwrap()));
    }

    #[test]
    fn system_fake_ip_is_usable_when_no_capture_tun() {
        // Clash/Mihomo DNS hands out 198.18.0.29 — must NOT be discarded
        // (remote server with network fake-IP DNS used to 502 every request).
        let system = vec!["198.18.0.29:443".parse().unwrap()];
        let (usable, need_real_dns) = select_system_addrs(system.clone(), false);
        assert_eq!(usable, system);
        assert!(!need_real_dns);
    }

    #[test]
    fn system_fake_ip_is_dropped_when_capture_tun_present() {
        // OUR tun2proxy virtual DNS poisons answers — must be stripped and
        // real DNS must be tried (this is the loop protection).
        let system = vec!["198.18.0.29:443".parse().unwrap()];
        let (usable, need_real_dns) = select_system_addrs(system, true);
        assert!(usable.is_empty());
        assert!(need_real_dns);
    }

    #[test]
    fn real_system_answers_preferred_even_with_capture_tun() {
        let system = vec![
            "8.47.69.5:80".parse().unwrap(),
            "198.18.0.29:80".parse().unwrap(),
        ];
        let (usable, need_real_dns) = select_system_addrs(system, true);
        assert_eq!(usable, vec!["8.47.69.5:80".parse().unwrap()]);
        assert!(!need_real_dns);
    }

    #[test]
    fn real_dns_fallback_only_when_system_empty_or_only_poisoned() {
        assert!(select_system_addrs(vec![], false).1); // empty → fallback
        assert!(!select_system_addrs(vec!["1.2.3.4:80".parse().unwrap()], false).1);
        assert!(select_system_addrs(vec!["198.18.0.29:80".parse().unwrap()], true).1);
    }

    #[test]
    fn split_host_port_forms() {
        assert_eq!(
            split_host_port("example.com:443").unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            split_host_port("127.0.0.1:80").unwrap(),
            ("127.0.0.1".into(), 80)
        );
        assert_eq!(split_host_port("[::1]:8080").unwrap(), ("::1".into(), 8080));
        assert!(split_host_port("example.com").is_err());
        assert!(split_host_port(":80").is_err());
    }

    #[test]
    fn dns_query_roundtrip_parses_a_record() {
        let q = encode_dns_query(0x1234, "example.com", 1).unwrap();
        assert_eq!(&q[0..2], &[0x12, 0x34]);
        // Build a synthetic answer: name ptr to question, type A, class IN, ttl, 1.2.3.4
        let mut resp = Vec::new();
        resp.extend_from_slice(&0x1234u16.to_be_bytes());
        resp.extend_from_slice(&0x8180u16.to_be_bytes());
        resp.extend_from_slice(&1u16.to_be_bytes()); // qd
        resp.extend_from_slice(&1u16.to_be_bytes()); // an
        resp.extend_from_slice(&0u16.to_be_bytes());
        resp.extend_from_slice(&0u16.to_be_bytes());
        // question (copy of query body)
        resp.extend_from_slice(&q[12..]);
        // answer: compression ptr to offset 12, type A, class IN, ttl 60, rdlen 4
        resp.extend_from_slice(&0xC00Cu16.to_be_bytes());
        resp.extend_from_slice(&1u16.to_be_bytes());
        resp.extend_from_slice(&1u16.to_be_bytes());
        resp.extend_from_slice(&60u32.to_be_bytes());
        resp.extend_from_slice(&4u16.to_be_bytes());
        resp.extend_from_slice(&[1, 2, 3, 4]);
        let ips = parse_dns_answers(&resp, 0x1234, 1);
        assert_eq!(ips, vec![IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))]);
    }

    #[test]
    fn dns_rejects_wrong_txid() {
        let q = encode_dns_query(1, "a.b", 1).unwrap();
        let mut resp = q.clone();
        resp[0] = 0xFF;
        assert!(parse_dns_answers(&resp, 1, 1).is_empty());
    }
}
