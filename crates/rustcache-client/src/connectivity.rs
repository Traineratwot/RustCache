//! Connectivity test: Direct / Forced (local proxy) / System proxy legs.
//!
//! Each leg performs a plain HTTP GET and inspects the response for the
//! client marker (`X-RustCache-Client: 1`) and optional `X-RustCache-Status`.
//! On network failure a local echo fixture is used as fallback.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::config::ClientConfig;

const DEFAULT_URL: &str = "http://example.com/";
const DEFAULT_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LegKind {
    Direct,
    Forced,
    System,
}

impl LegKind {
    pub fn label(self) -> &'static str {
        match self {
            LegKind::Direct => "direct",
            LegKind::Forced => "forced",
            LegKind::System => "system",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LegResult {
    pub kind: LegKind,
    pub status: Option<u16>,
    pub marker: bool,
    pub rustcache_status: Option<String>,
    pub passed: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub url: String,
    pub legs: Vec<LegResult>,
    pub used_fixture: bool,
    pub passed: bool,
    pub lines: Vec<String>,
}

/// Local echo fixture: returns 200 and echoes request headers as body.
/// The response itself does NOT include the client marker (the marker is
/// injected by ProxyServer, not the origin).
struct Fixture {
    addr: SocketAddr,
    _handle: tokio::task::JoinHandle<()>,
}

async fn start_fixture() -> Result<Fixture> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut head = Vec::new();
                loop {
                    match tokio::time::timeout(Duration::from_secs(2), sock.read(&mut buf)).await {
                        Ok(Ok(0)) | Err(_) => return,
                        Ok(Ok(n)) => {
                            head.extend_from_slice(&buf[..n]);
                            if head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() > 8192 {
                                break;
                            }
                        }
                        Ok(Err(_)) => return,
                    }
                }
                let body = String::from_utf8_lossy(&head).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    Ok(Fixture {
        addr,
        _handle: handle,
    })
}

/// Simple HTTP GET over a connected stream; returns (status, headers+body).
async fn http_get(
    stream: &mut TcpStream,
    request_line_target: &str,
    host: &str,
    extra_marker: bool,
) -> Result<(u16, Vec<u8>)> {
    let marker = if extra_marker {
        format!(
            "{}: {}\r\n",
            crate::proxy::http_parse::CLIENT_MARKER,
            crate::proxy::http_parse::CLIENT_MARKER_VALUE
        )
    } else {
        String::new()
    };
    let req = format!(
        "GET {request_line_target} HTTP/1.1\r\nHost: {host}\r\n{marker}User-Agent: rustcache-client-test\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .context("read response timeout")?
            .context("read response")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 256 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((status, buf))
}

/// Split URL into (host, port, path, is_https).
fn parse_url(url: &str) -> Result<(String, u16, String, bool)> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        (false, url)
    };
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) =
        crate::proxy::relay::split_host_port_with(authority, if scheme { 443 } else { 80 })?;
    Ok((host, port, path, scheme))
}

async fn leg_direct(url: &str, timeout: Duration) -> LegResult {
    let (host, port, path, _https) = match parse_url(url) {
        Ok(v) => v,
        Err(e) => {
            return LegResult {
                kind: LegKind::Direct,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: format!("bad url: {e}"),
            };
        }
    };
    let addr = format!("{host}:{port}");
    let conn = tokio::time::timeout(timeout, TcpStream::connect(&addr)).await;
    let mut stream = match conn {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return LegResult {
                kind: LegKind::Direct,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: format!("connect: {e}"),
            };
        }
        Err(_) => {
            return LegResult {
                kind: LegKind::Direct,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: "timeout".into(),
            };
        }
    };
    match http_get(&mut stream, &path, &format!("{host}:{port}"), false).await {
        Ok((status, resp)) => {
            let marker = crate::proxy::http_parse::has_client_marker(&resp);
            let rs = crate::proxy::http_parse::rustcache_status(&resp);
            let ok = (200..400).contains(&status) && !marker && rs.is_none();
            let note = if ok {
                "OK".into()
            } else if marker {
                "unexpected client marker on direct leg".into()
            } else if rs.is_some() {
                "unexpected X-RustCache-* on direct leg".into()
            } else {
                format!("status {status}")
            };
            LegResult {
                kind: LegKind::Direct,
                status: Some(status),
                marker,
                rustcache_status: rs,
                passed: ok,
                note,
            }
        }
        Err(e) => LegResult {
            kind: LegKind::Direct,
            status: None,
            marker: false,
            rustcache_status: None,
            passed: false,
            note: format!("{e:#}"),
        },
    }
}

async fn leg_forced(
    url: &str,
    listen: &str,
    timeout: Duration,
    expect_marker: bool,
    informational: bool,
) -> LegResult {
    let (host, _port, _path, _https) = match parse_url(url) {
        Ok(v) => v,
        Err(e) => {
            return LegResult {
                kind: LegKind::Forced,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: format!("bad url: {e}"),
            };
        }
    };
    let conn = tokio::time::timeout(timeout, TcpStream::connect(listen)).await;
    let mut stream = match conn {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return LegResult {
                kind: LegKind::Forced,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: format!("local proxy: {e}"),
            };
        }
        Err(_) => {
            return LegResult {
                kind: LegKind::Forced,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: "local proxy timeout".into(),
            };
        }
    };
    // Absolute-form URI through the proxy.
    let abs = url.to_string();
    match http_get(&mut stream, &abs, &host, false).await {
        Ok((status, resp)) => {
            let marker = crate::proxy::http_parse::has_client_marker(&resp);
            let rs = crate::proxy::http_parse::rustcache_status(&resp);
            let ok = if informational {
                true
            } else {
                (200..400).contains(&status) && (!expect_marker || marker)
            };
            let note = if informational {
                format!("informational ({status})")
            } else if ok {
                "OK".into()
            } else {
                format!("status={status} marker={marker}")
            };
            LegResult {
                kind: LegKind::Forced,
                status: Some(status),
                marker,
                rustcache_status: rs,
                passed: ok,
                note,
            }
        }
        Err(e) => LegResult {
            kind: LegKind::Forced,
            status: None,
            marker: false,
            rustcache_status: None,
            passed: false,
            note: format!("{e:#}"),
        },
    }
}

async fn leg_system(
    url: &str,
    proxy: Option<(String, u16)>,
    timeout: Duration,
    expect_marker: bool,
    informational: bool,
) -> LegResult {
    let Some((ph, pp)) = proxy else {
        return LegResult {
            kind: LegKind::System,
            status: None,
            marker: false,
            rustcache_status: None,
            passed: informational, // skipped is OK when informational
            note: "no system proxy set — skipped".into(),
        };
    };
    let (host, _port, _path, _https) = match parse_url(url) {
        Ok(v) => v,
        Err(e) => {
            return LegResult {
                kind: LegKind::System,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: format!("bad url: {e}"),
            };
        }
    };
    let addr = format!("{ph}:{pp}");
    let conn = tokio::time::timeout(timeout, TcpStream::connect(&addr)).await;
    let mut stream = match conn {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            return LegResult {
                kind: LegKind::System,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: format!("system proxy: {e}"),
            };
        }
        Err(_) => {
            return LegResult {
                kind: LegKind::System,
                status: None,
                marker: false,
                rustcache_status: None,
                passed: false,
                note: "system proxy timeout".into(),
            };
        }
    };
    let abs = url.to_string();
    match http_get(&mut stream, &abs, &host, false).await {
        Ok((status, resp)) => {
            let marker = crate::proxy::http_parse::has_client_marker(&resp);
            let rs = crate::proxy::http_parse::rustcache_status(&resp);
            let ok = if informational {
                true
            } else {
                (200..400).contains(&status) && (!expect_marker || marker)
            };
            let note = if informational {
                format!("informational ({status})")
            } else if ok {
                "OK".into()
            } else {
                format!("status={status} marker={marker}")
            };
            LegResult {
                kind: LegKind::System,
                status: Some(status),
                marker,
                rustcache_status: rs,
                passed: ok,
                note,
            }
        }
        Err(e) => LegResult {
            kind: LegKind::System,
            status: None,
            marker: false,
            rustcache_status: None,
            passed: false,
            note: format!("{e:#}"),
        },
    }
}

/// Run the full connectivity test suite.
pub async fn run(cfg: &ClientConfig, url: Option<&str>, timeout_secs: Option<u64>) -> TestReport {
    let url = url.unwrap_or(DEFAULT_URL).to_string();
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let (host, _port, _path, _https) =
        parse_url(&url).unwrap_or_else(|_| ("example.com".into(), 80, "/".into(), false));

    // Probe: try the real URL first; if unreachable, fall back to local fixture.
    let mut used_fixture = false;
    let effective_url = {
        let probe = tokio::time::timeout(timeout, TcpStream::connect(&format!("{host}:80"))).await;
        match probe {
            Ok(Ok(_)) => url.clone(),
            _ => {
                used_fixture = true;
                let fixture = match start_fixture().await {
                    Ok(f) => f,
                    Err(e) => {
                        let mut lines = vec![format!("url: {url}")];
                        lines.push(format!("error: cannot start fixture: {e:#}"));
                        return TestReport {
                            url,
                            legs: Vec::new(),
                            used_fixture: false,
                            passed: false,
                            lines,
                        };
                    }
                };
                format!("http://{}/", fixture.addr)
            }
        }
    };

    // When capture is on, the system leg is mandatory; otherwise informational.
    let capture_on = cfg.mode != crate::config::CaptureMode::Off;
    let expect_marker = capture_on;

    let direct = leg_direct(&effective_url, timeout).await;
    let forced = leg_forced(&effective_url, &cfg.listen, timeout, true, false).await;
    let sys_proxy = crate::sysproxy::current_http_proxy().ok().flatten();
    let system = leg_system(
        &effective_url,
        sys_proxy,
        timeout,
        expect_marker,
        !capture_on,
    )
    .await;

    let legs = vec![direct, forced, system];
    let passed = legs.iter().all(|l| l.passed);

    let mut lines = Vec::new();
    lines.push(format!("url: {effective_url}"));
    if used_fixture {
        lines.push("note: origin unreachable — using local fixture".into());
    }
    for l in &legs {
        let status_s = l
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".into());
        let marker_s = if l.marker { "yes" } else { "no" };
        let rs = l.rustcache_status.as_deref().unwrap_or("—");
        lines.push(format!(
            "{}: {:>3} marker={:<3} x-rustcache={:<6} {}",
            l.kind.label(),
            status_s,
            marker_s,
            rs,
            if l.passed { "OK" } else { &l.note }
        ));
    }
    lines.push(format!("overall: {}", if passed { "OK" } else { "FAIL" }));

    TestReport {
        url: effective_url,
        legs,
        used_fixture,
        passed,
        lines,
    }
}
