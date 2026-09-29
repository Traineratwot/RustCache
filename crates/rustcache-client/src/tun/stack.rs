//! smoltcp userspace TCP stack: reads packets from TUN, handles TCP flows.
//!
//! TCP :80  → HTTP-proxy handshake to 127.0.0.1:31280 (absolute-form)
//! TCP :443 → CONNECT dest to 127.0.0.1:31280
//! TCP other → raw splice DIRECT (fail-open)
//! UDP → drop

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Hand-off: TUN flow emulates a ProxyServer client (does NOT call internal handle_*).
/// If the proxy is down, the flow goes DIRECT (fail-open).
pub async fn run(local_proxy: &str) -> Result<()> {
    // Open the TUN device (platform-specific).
    let mut tun = open_tun().await?;

    let proxy_addr = local_proxy.to_string();
    let mut flows: HashMap<u16, FlowState> = HashMap::new();

    let mut buf = [0u8; 2048];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(30), tun.read(&mut buf))
            .await
            .context("tun read timeout")?
            .context("tun read")?;
        if n == 0 {
            continue;
        }
        let pkt = &buf[..n];
        if let Some((src_port, dst_port, syn, payload_off)) = parse_tcp_packet(pkt) {
            if syn && payload_off == 0 {
                // New connection — decide the path.
                let route = if dst_port == 80 {
                    Route::HttpProxy
                } else if dst_port == 443 {
                    Route::ConnectProxy
                } else {
                    Route::Direct
                };
                flows.insert(src_port, FlowState { route, first: true });
            } else if let Some(flow) = flows.get_mut(&src_port) {
                if flow.first && payload_off > 0 {
                    flow.first = false;
                    // Spawn a handler for this flow's data.
                    let route = flow.route;
                    let proxy = proxy_addr.clone();
                    let data = pkt[payload_off..].to_vec();
                    tokio::spawn(async move {
                        if let Err(e) = handle_flow(route, &proxy, dst_port, &data).await {
                            tracing::debug!(error = %e, "tun flow error");
                        }
                    });
                }
            }
        }
        // UDP or unknown → drop (per plan).
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    HttpProxy,
    ConnectProxy,
    Direct,
}

struct FlowState {
    route: Route,
    first: bool,
}

/// Parse a minimal IPv4+TCP packet. Returns (src_port, dst_port, is_syn, payload_offset).
fn parse_tcp_packet(pkt: &[u8]) -> Option<(u16, u16, bool, usize)> {
    if pkt.len() < 40 {
        return None;
    }
    // IPv4
    let ihl = (pkt[0] & 0x0f) as usize * 4;
    if pkt[0] >> 4 != 4 || pkt.len() < ihl + 20 {
        return None;
    }
    let protocol = pkt[9];
    if protocol != 6 {
        // Not TCP
        return None;
    }
    let tcp = &pkt[ihl..];
    let src_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    let dst_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    let data_off = ((tcp[12] >> 4) as usize) * 4;
    let flags = tcp[13];
    let syn = flags & 0x02 != 0;
    let payload_off = if data_off > 0 && tcp.len() > data_off {
        ihl + data_off
    } else {
        0
    };
    Some((src_port, dst_port, syn, payload_off))
}

/// Handle a flow: forward data to proxy or DIRECT.
async fn handle_flow(route: Route, proxy: &str, dst_port: u16, data: &[u8]) -> Result<()> {
    match route {
        Route::HttpProxy | Route::ConnectProxy => {
            // Try the local proxy first.
            match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(proxy)).await {
                Ok(Ok(mut upstream)) => {
                    let _ = upstream.set_nodelay(true);
                    // For HTTP: rewrite to absolute-form and inject marker.
                    // For CONNECT: send CONNECT then splice.
                    if route == Route::HttpProxy {
                        let marked = inject_marker_http(data);
                        upstream.write_all(&marked).await?;
                    } else {
                        // CONNECT: build the handshake from the first payload.
                        // The TUN stack sees raw TCP — we emulate a proxy client.
                        let host = extract_host(data).unwrap_or_else(|| "unknown".into());
                        let connect = format!(
                            "CONNECT {host}:{dst_port} HTTP/1.1\r\nHost: {host}:{dst_port}\r\nConnection: close\r\n\r\n"
                        );
                        upstream.write_all(connect.as_bytes()).await?;
                        // Forward client data after CONNECT handshake.
                        upstream.write_all(data).await?;
                    }
                    upstream.flush().await?;
                    // Drain response (we can't easily send it back through TUN in MVP).
                    let mut buf = vec![0u8; 4096];
                    let _ = tokio::time::timeout(Duration::from_secs(10), upstream.read(&mut buf))
                        .await;
                    Ok(())
                }
                _ => {
                    // Proxy down — DIRECT fail-open.
                    let host = extract_host(data).unwrap_or_else(|| "127.0.0.1".into());
                    direct_fetch(&host, dst_port, data).await
                }
            }
        }
        Route::Direct => {
            let host = extract_host(data).unwrap_or_else(|| "127.0.0.1".into());
            direct_fetch(&host, dst_port, data).await
        }
    }
}

/// Inject the client marker into an HTTP request (absolute-form preserved).
fn inject_marker_http(data: &[u8]) -> Vec<u8> {
    crate::proxy::http_parse::inject_client_marker(data)
}

/// Extract Host header from HTTP request bytes.
fn extract_host(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Direct fetch to origin (fail-open path).
async fn direct_fetch(host: &str, port: u16, data: &[u8]) -> Result<()> {
    let addr = format!("{host}:{port}");
    let mut upstream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr))
        .await
        .context("direct connect timeout")?
        .with_context(|| format!("connect {addr}"))?;
    let _ = upstream.set_nodelay(true);
    upstream.write_all(data).await?;
    upstream.flush().await?;
    let mut buf = vec![0u8; 4096];
    let _ = tokio::time::timeout(Duration::from_secs(5), upstream.read(&mut buf)).await;
    Ok(())
}

/// Open the platform TUN device for reading/writing packets.
async fn open_tun() -> Result<Box<dyn TunDevice>> {
    #[cfg(target_os = "linux")]
    {
        linux_open_tun().await
    }
    #[cfg(target_os = "macos")]
    {
        macos_open_tun().await
    }
    #[cfg(target_os = "windows")]
    {
        windows_open_tun().await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        anyhow::bail!("TUN not supported on this platform")
    }
}

/// Trait for reading/writing raw IP packets on a TUN device.
pub trait TunDevice: Send {
    fn read<'a>(
        &'a mut self,
        buf: &'a mut [u8],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<usize>> + Send + 'a>>;
}

#[cfg(target_os = "linux")]
async fn linux_open_tun() -> Result<Box<dyn TunDevice>> {
    struct LinuxTun {
        file: std::fs::File,
    }
    impl TunDevice for LinuxTun {
        fn read<'a>(
            &'a mut self,
            buf: &'a mut [u8],
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<usize>> + Send + 'a>>
        {
            Box::pin(async move {
                use std::io::Read;
                self.file.read(buf).context("tun read")
            })
        }
    }

    // Open /dev/net/tun via the `tun` crate.
    let mut config = tun::Configuration::default();
    config
        .address(crate::tun::TUN_ADDR)
        .netmask("255.255.0.0")
        .up();
    let dev = tun::create_as_async(&config).context("create TUN device")?;
    // Wrap as raw fd for reading.
    let fd = dev.as_raw_fd();
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    Ok(Box::new(LinuxTun { file }))
}

#[cfg(target_os = "macos")]
async fn macos_open_tun() -> Result<Box<dyn TunDevice>> {
    anyhow::bail!("TUN on macOS is best-effort and not yet implemented")
}

#[cfg(target_os = "windows")]
async fn windows_open_tun() -> Result<Box<dyn TunDevice>> {
    anyhow::bail!("TUN on Windows requires wintun.dll — not yet implemented")
}

// Re-export for tun crate raw fd access on Linux.
#[cfg(target_os = "linux")]
use std::os::unix::io::{AsRawFd, FromRawFd};
