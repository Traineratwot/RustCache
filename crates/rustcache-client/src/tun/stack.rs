//! smoltcp userspace TCP stack over TUN with IP-rewriting.
//!
//! Accepts TCP connections destined for **any** IP (transparent), rewrites
//! dst→198.18.0.1 for smoltcp, rewrites src→original-dst on egress.
//!
//! TCP :80  → HTTP absolute-form to 127.0.0.1:31280 + client marker
//! TCP :443 → CONNECT to 127.0.0.1:31280, then splice
//! TCP other → DIRECT splice (fail-open)
//! UDP → drop

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context, Result};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address};

const TUN_IP: Ipv4Address = Ipv4Address::new(198, 18, 0, 1);
const TUN_PREFIX: u8 = 15;
const MTU: usize = 1500;
const MAX_LISTENERS_PER_PORT: usize = 16;
const SOCK_BUF: usize = 16384;

/// (client_ip, client_port) → (orig_dst_ip, orig_dst_port)
type FlowMap = Rc<RefCell<HashMap<(Ipv4Address, u16), (Ipv4Address, u16)>>>;

// ──────────────────────────── checksums ────────────────────────────

fn ip_checksum(hdr: &[u8]) -> u16 {
    let mut sum = 0u32;
    for i in (0..hdr.len()).step_by(2) {
        sum += if i + 1 < hdr.len() {
            u16::from_be_bytes([hdr[i], hdr[i + 1]]) as u32
        } else {
            (hdr[i] as u32) << 8
        };
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn fix_tcp_checksum(pkt: &mut [u8]) {
    if pkt.len() < 20 || pkt[0] >> 4 != 4 {
        return;
    }
    let ihl = (pkt[0] & 0x0f) as usize * 4;
    if pkt[9] != 6 || pkt.len() < ihl + 20 {
        return;
    }
    let tcp_len = pkt.len() - ihl;
    pkt[ihl + 16] = 0;
    pkt[ihl + 17] = 0;

    let mut sum = 0u32;
    // pseudo-header: src_ip + dst_ip + protocol + tcp_len
    sum += u16::from_be_bytes([pkt[12], pkt[13]]) as u32;
    sum += u16::from_be_bytes([pkt[14], pkt[15]]) as u32;
    sum += u16::from_be_bytes([pkt[16], pkt[17]]) as u32;
    sum += u16::from_be_bytes([pkt[18], pkt[19]]) as u32;
    sum += 6u32;
    sum += tcp_len as u32;
    // tcp header + data
    for i in (0..tcp_len).step_by(2) {
        sum += if i + 1 < tcp_len {
            u16::from_be_bytes([pkt[ihl + i], pkt[ihl + i + 1]]) as u32
        } else {
            (pkt[ihl + i] as u32) << 8
        };
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    let csum = !(sum as u16);
    pkt[ihl + 16] = (csum >> 8) as u8;
    pkt[ihl + 17] = (csum & 0xff) as u8;
}

/// Rewrite IPv4 dst (bytes 16..20), then fix IP + TCP checksums.
fn rewrite_dst(pkt: &mut [u8], new: Ipv4Address) {
    if pkt.len() < 20 || pkt[0] >> 4 != 4 {
        return;
    }
    let o = new.octets();
    pkt[16] = o[0];
    pkt[17] = o[1];
    pkt[18] = o[2];
    pkt[19] = o[3];
    pkt[10] = 0;
    pkt[11] = 0;
    let c = ip_checksum(&pkt[..20]);
    pkt[10] = (c >> 8) as u8;
    pkt[11] = (c & 0xff) as u8;
    fix_tcp_checksum(pkt);
}

/// Rewrite IPv4 src (bytes 12..16), then fix IP + TCP checksums.
fn rewrite_src(pkt: &mut [u8], new: Ipv4Address) {
    if pkt.len() < 20 || pkt[0] >> 4 != 4 {
        return;
    }
    let o = new.octets();
    pkt[12] = o[0];
    pkt[13] = o[1];
    pkt[14] = o[2];
    pkt[15] = o[3];
    pkt[10] = 0;
    pkt[11] = 0;
    let c = ip_checksum(&pkt[..20]);
    pkt[10] = (c >> 8) as u8;
    pkt[11] = (c & 0xff) as u8;
    fix_tcp_checksum(pkt);
}

/// Extract (src_ip, src_port, dst_ip, dst_port) from an IPv4+TCP packet.
fn parse_ipv4_tcp(pkt: &[u8]) -> Option<(Ipv4Address, u16, Ipv4Address, u16)> {
    if pkt.len() < 40 || pkt[0] >> 4 != 4 {
        return None;
    }
    let ihl = (pkt[0] & 0x0f) as usize * 4;
    if pkt[9] != 6 || pkt.len() < ihl + 20 {
        return None;
    }
    let src_ip = Ipv4Address::new(pkt[12], pkt[13], pkt[14], pkt[15]);
    let dst_ip = Ipv4Address::new(pkt[16], pkt[17], pkt[18], pkt[19]);
    let src_port = u16::from_be_bytes([pkt[ihl], pkt[ihl + 1]]);
    let dst_port = u16::from_be_bytes([pkt[ihl + 2], pkt[ihl + 3]]);
    Some((src_ip, src_port, dst_ip, dst_port))
}

// ──────────────────────────── smoltcp Device ────────────────────────────

struct TunRx {
    data: Vec<u8>,
}

impl RxToken for TunRx {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.data)
    }
}

struct TunTx {
    fd: RawFd,
    flows: FlowMap,
}

impl TxToken for TunTx {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        // Rewrite src back to original dst so the client accepts the packet.
        if let Some((_sip, _sp, dip, dp)) = parse_ipv4_tcp(&buf) {
            let orig = self.flows.borrow().get(&(dip, dp)).copied();
            if let Some((orig_ip, _orig_port)) = orig {
                rewrite_src(&mut buf, orig_ip);
            }
        }
        unsafe {
            libc::write(self.fd, buf.as_ptr() as *const libc::c_void, buf.len());
        }
        result
    }
}

struct TunDev {
    fd: RawFd,
    flows: FlowMap,
}

impl Device for TunDev {
    type RxToken<'a> = TunRx;
    type TxToken<'a> = TunTx;

    fn receive(
        &mut self,
        _ts: SmolInstant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut buf = vec![0u8; MTU];
        let n = unsafe {
            libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
        };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);

        // Track original dst and rewrite for smoltcp.
        if let Some((sip, sp, dip, dp)) = parse_ipv4_tcp(&buf) {
            self.flows.borrow_mut().insert((sip, sp), (dip, dp));
            rewrite_dst(&mut buf, TUN_IP);
        }

        Some((
            TunRx { data: buf },
            TunTx {
                fd: self.fd,
                flows: self.flows.clone(),
            },
        ))
    }

    fn transmit(&mut self, _ts: SmolInstant) -> Option<Self::TxToken<'_>> {
        Some(TunTx {
            fd: self.fd,
            flows: self.flows.clone(),
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = MTU;
        caps.medium = Medium::Ip;
        caps
    }
}

// ──────────────────────────── connection bridge ────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Http,
    Connect,
    Direct,
}

struct Conn {
    kind: Kind,
    orig_dst: Ipv4Address,
    orig_port: u16,
    proxy: Option<TcpStream>,
    to_upstream: Vec<u8>,
    from_upstream: Vec<u8>,
    connect_sent: bool,
    connect_done: bool,
}

/// Insert client marker into an HTTP request that is already in absolute-form.
#[allow(dead_code)]
fn mark_http(data: &[u8]) -> Vec<u8> {
    crate::proxy::http_parse::inject_client_marker(data)
}

/// Rewrite origin-form to absolute-form + marker.
fn origin_to_absolute(data: &[u8], host: &str, port: u16) -> Vec<u8> {
    let text = String::from_utf8_lossy(data);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("HTTP/1.1");

    let abs_path = if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else if port == 80 {
        format!("http://{host}{path}")
    } else {
        format!("http://{host}:{port}{path}")
    };

    let mut out = format!("{method} {abs_path} {version}\r\n");
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((k, _)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("proxy-connection") {
                continue;
            }
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("X-RustCache-Client: 1\r\nConnection: close\r\n\r\n");
    out.into_bytes()
}

/// IP address to string without IPv6 concerns (we only handle IPv4 here).
fn ip_str(ip: Ipv4Address) -> String {
    let o = ip.octets();
    format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3])
}

/// Host-route each DNS resolver via the real default gateway so lookups do
/// not black-hole into the TUN. Must run with CAP_NET_ADMIN (the helper).
fn preserve_dns_via_gateway() {
    let gw = default_gateway();
    let Some(gw) = gw else {
        tracing::warn!("no default gateway — cannot preserve DNS routes");
        return;
    };
    for dns in resolvers() {
        let route = format!("{dns}/32");
        let out = std::process::Command::new("ip")
            .args(["route", "add", &route, "via", &gw])
            .output();
        match out {
            Ok(o) if o.status.success() => {
                tracing::info!(%route, %gw, "DNS host-route via gateway");
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                // Already exists is fine.
                if !err.contains("File exists") {
                    tracing::warn!(%route, stderr = %err.trim(), "DNS route add failed");
                }
            }
            Err(e) => tracing::warn!(%route, error = %e, "DNS route add failed"),
        }
    }
}

fn default_gateway() -> Option<String> {
    let out = std::process::Command::new("ip")
        .args(["route", "show", "default"])
        .output()
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

fn resolvers() -> Vec<String> {
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

// ──────────────────────────── event loop ────────────────────────────

/// Run the TUN TCP stack (blocking; call from `spawn_blocking`).
///
/// `ready` is signalled once with `Ok(())` after the device + routes exist,
/// or `Err(msg)` if setup failed — so [`super::enable`] does not report
/// success for a device that was never created.
pub fn event_loop(
    local_proxy: &str,
    mut ready: Option<std::sync::mpsc::Sender<std::result::Result<(), String>>>,
) -> Result<()> {
    let mut report = |r: std::result::Result<(), String>| {
        if let Some(tx) = ready.take() {
            let _ = tx.send(r);
        }
    };
    // 1. Create TUN device (sync — we're in spawn_blocking).
    let mut config = tun::Configuration::default();
    config
        .tun_name("rustcache-tun0")
        .address("198.18.0.1")
        .netmask("255.255.0.0")
        .up();
    let tun_dev = match tun::create(&config) {
        Ok(d) => d,
        Err(e) => {
            let msg = format!("{e:#}");
            report(Err(msg.clone()));
            return Err(anyhow::anyhow!("create TUN device: {msg}"));
        }
    };
    let fd = tun_dev.as_raw_fd();

    // 2. Add capture routes (device exists now).
    for route in ["0.0.0.0/1", "128.0.0.0/1"] {
        let out = std::process::Command::new("ip")
            .args(["route", "add", route, "dev", "rustcache-tun0"])
            .output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => tracing::warn!(
                route,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "route add failed (may already exist)"
            ),
            Err(e) => tracing::warn!(route, error = %e, "route add failed"),
        }
    }

    // 3. Keep DNS off the TUN (host-routes via the real default gateway).
    //    Must run here — this process has CAP_NET_ADMIN, the GUI does not.
    preserve_dns_via_gateway();

    // Device + routes are up — tell enable() it may report success.
    report(Ok(()));

    // Prevent tun_dev from closing the fd — TunDev owns it now.
    std::mem::forget(tun_dev);

    let flows: FlowMap = Rc::new(RefCell::new(HashMap::new()));
    let mut device = TunDev {
        fd,
        flows: flows.clone(),
    };

    // smoltcp interface
    let mut iface = Interface::new(
        Config::new(HardwareAddress::Ip),
        &mut device,
        SmolInstant::now(),
    );
    iface.update_ip_addrs(|addrs| {
        let o = TUN_IP.octets();
        let _ = addrs.push(IpCidr::new(
            IpAddress::v4(o[0], o[1], o[2], o[3]),
            TUN_PREFIX,
        ));
    });

    // Socket set: listeners for :80 and :443 + connection sockets
    let mut sockets = SocketSet::new(vec![]);
    let mut listeners_80: Vec<SocketHandle> = Vec::new();
    let mut listeners_443: Vec<SocketHandle> = Vec::new();

    for _ in 0..MAX_LISTENERS_PER_PORT {
        let rx = tcp::SocketBuffer::new(vec![0; SOCK_BUF]);
        let tx = tcp::SocketBuffer::new(vec![0; SOCK_BUF]);
        let mut s = tcp::Socket::new(rx, tx);
        s.listen(80).context("listen :80")?;
        listeners_80.push(sockets.add(s));
    }
    for _ in 0..MAX_LISTENERS_PER_PORT {
        let rx = tcp::SocketBuffer::new(vec![0; SOCK_BUF]);
        let tx = tcp::SocketBuffer::new(vec![0; SOCK_BUF]);
        let mut s = tcp::Socket::new(rx, tx);
        s.listen(443).context("listen :443")?;
        listeners_443.push(sockets.add(s));
    }

    // Active connections
    let mut conns: HashMap<SocketHandle, Conn> = HashMap::new();

    loop {
        let ts = SmolInstant::now();
        iface.poll(ts, &mut device, &mut sockets);

        // ── detect new connections from listeners ──
        for (handle, kind) in listeners_80
            .iter()
            .map(|h| (*h, Kind::Http))
            .chain(listeners_443.iter().map(|h| (*h, Kind::Connect)))
        {
            if conns.contains_key(&handle) {
                continue;
            }
            let sock = sockets.get_mut::<tcp::Socket>(handle);
            if !sock.is_active() {
                continue;
            }
            let remote = match sock.remote_endpoint() {
                Some(r) => r,
                None => continue,
            };
            let (client_ip, client_port) = match remote.addr {
                IpAddress::Ipv4(a) => (a, remote.port),
                #[allow(unreachable_patterns)]
                _ => continue,
            };
            let (orig_ip, orig_port) = flows
                .borrow()
                .get(&(client_ip, client_port))
                .copied()
                .unwrap_or((TUN_IP, if kind == Kind::Http { 80 } else { 443 }));

            // Connect to local proxy (or DIRECT for non-80/443).
            let proxy = if kind == Kind::Direct {
                TcpStream::connect((ip_str(orig_ip).as_str(), orig_port)).ok()
            } else {
                TcpStream::connect(local_proxy).ok()
            };

            conns.insert(
                handle,
                Conn {
                    kind,
                    orig_dst: orig_ip,
                    orig_port,
                    proxy,
                    to_upstream: Vec::new(),
                    from_upstream: Vec::new(),
                    connect_sent: false,
                    connect_done: false,
                },
            );
        }

        // ── pump data ──
        let mut dead: Vec<SocketHandle> = Vec::new();
        for (handle, conn) in conns.iter_mut() {
            let sock = sockets.get_mut::<tcp::Socket>(*handle);

            // smoltcp → upstream buffer
            if sock.may_recv() {
                let _ = sock.recv(|data| {
                    if !data.is_empty() {
                        conn.to_upstream.extend_from_slice(data);
                    }
                    (data.len(), ())
                });
            }

            // Prepare upstream data depending on kind
            if !conn.to_upstream.is_empty() {
                match conn.kind {
                    Kind::Http => {
                        // Rewrite origin-form → absolute-form + marker.
                        let host = ip_str(conn.orig_dst);
                        let rewritten =
                            origin_to_absolute(&conn.to_upstream, &host, conn.orig_port);
                        conn.to_upstream.clear();
                        if let Some(ref mut p) = conn.proxy {
                            let _ = p.write_all(&rewritten);
                            let _ = p.flush();
                        }
                    }
                    Kind::Connect => {
                        if !conn.connect_sent {
                            let host = ip_str(conn.orig_dst);
                            let connect = format!(
                                "CONNECT {host}:{} HTTP/1.1\r\nHost: {host}:{}\r\nConnection: close\r\n\r\n",
                                conn.orig_port, conn.orig_port
                            );
                            if let Some(ref mut p) = conn.proxy {
                                let _ = p.write_all(connect.as_bytes());
                                let _ = p.flush();
                            }
                            conn.connect_sent = true;
                        } else {
                            if let Some(ref mut p) = conn.proxy {
                                let _ = p.write_all(&conn.to_upstream);
                                let _ = p.flush();
                            }
                            conn.to_upstream.clear();
                        }
                    }
                    Kind::Direct => {
                        if let Some(ref mut p) = conn.proxy {
                            let _ = p.write_all(&conn.to_upstream);
                            let _ = p.flush();
                        }
                        conn.to_upstream.clear();
                    }
                }
            }

            // upstream → smoltcp buffer
            if let Some(ref mut p) = conn.proxy {
                let _ = p.set_nonblocking(true);
                let mut buf = [0u8; SOCK_BUF];
                loop {
                    match p.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            // For CONNECT: first response is "200 Connection Established"
                            if conn.kind == Kind::Connect && !conn.connect_done {
                                conn.from_upstream.extend_from_slice(&buf[..n]);
                                // Check if we got the full CONNECT response.
                                if conn
                                    .from_upstream
                                    .windows(4)
                                    .any(|w| w == b"\r\n\r\n")
                                {
                                    conn.connect_done = true;
                                    // Skip the CONNECT response — client expects raw TLS.
                                    conn.from_upstream.clear();
                                }
                                break;
                            }
                            conn.from_upstream.extend_from_slice(&buf[..n]);
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
                let _ = p.set_nonblocking(false);
            }

            // smoltcp ← upstream buffer
            if sock.can_send() && !conn.from_upstream.is_empty() {
                let n = conn.from_upstream.len().min(SOCK_BUF);
                let _ = sock.send_slice(&conn.from_upstream[..n]);
                conn.from_upstream.drain(..n);
            }

            // Detect close
            if !sock.is_active() {
                dead.push(*handle);
            }
        }

        // ── cleanup dead connections ──
        for handle in dead {
            conns.remove(&handle);
            let sock = sockets.get_mut::<tcp::Socket>(handle);
            // Re-arm listener.
            let port = sock.local_endpoint().map(|e| e.port).unwrap_or(80);
            let _ = sock.listen(port);
        }

        // Sleep until smoltcp has work or 5 ms passes.
        let delay = iface
            .poll_delay(ts, &sockets)
            .unwrap_or_else(|| smoltcp::time::Duration::from_millis(5));
        let ms = delay.total_millis().clamp(1, 50) as u64;
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Async entry point: spawn the blocking event loop and keep it alive.
///
/// `ready` receives a single setup result from [`event_loop`].
pub async fn run(
    local_proxy: &str,
    ready: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) -> Result<()> {
    let proxy = local_proxy.to_string();
    let handle = tokio::task::spawn_blocking(move || {
        let ready = Some(ready);
        if let Err(e) = event_loop(&proxy, ready) {
            tracing::error!(error = %e, "tun stack stopped");
            return Err(e);
        }
        Ok(())
    });
    // Keep the task alive (TunSession._task holds the JoinHandle).
    match handle.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(anyhow::anyhow!("tun stack task: {e}")),
    }
}

// Silence unused-import warnings for types used only in cfg branches.
#[allow(unused_imports)]
use std::os::unix::io::AsRawFd as _AsRawFd;
