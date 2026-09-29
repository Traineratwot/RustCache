//! Minimal RustCache REST client (health, CA PEM, stats, exclusions).

use std::time::Duration;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct RustCacheApi {
    base: String,
    timeout: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct HealthReport {
    pub ok: bool,
    pub uptime_s: u64,
    pub http_running: bool,
    pub https_running: bool,
    pub socks_running: bool,
    pub http_port: u16,
    pub https_port: u16,
    pub socks_port: u16,
    pub api_port: u16,
}

/// Where a RustCache instance lives and which ports it actually bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredTarget {
    /// e.g. `http://192.168.1.10:8080`
    pub api_base: String,
    /// Host/IP without port.
    pub host: String,
    pub http_port: u16,
    pub https_port: u16,
    pub socks_port: u16,
    pub api_port: u16,
}

/// Normalize free-form user input (IP, host, host:port, URL) to an API base URL.
///
/// - `192.168.1.10`           → `http://192.168.1.10:8080`
/// - `192.168.1.10:8080`      → `http://192.168.1.10:8080`
/// - `http://10.0.0.5:8080`   → as-is
/// - `myhost.local`           → `http://myhost.local:8080`
pub fn normalize_api_base(input: &str, default_api_port: u16) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.is_empty() {
        return format!("http://127.0.0.1:{default_api_port}");
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return s.to_string();
    }
    // host or host:port (IPv6 in brackets)
    if s.starts_with('[') {
        if s.contains("]:") {
            return format!("http://{s}");
        }
        return format!("http://{s}:{default_api_port}");
    }
    let has_port = s
        .rsplit_once(':')
        .map(|(_, p)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or(false);
    if has_port {
        format!("http://{s}")
    } else {
        format!("http://{s}:{default_api_port}")
    }
}

/// Host part of an API base or bare input.
pub fn host_of_api_base(base: &str) -> String {
    let s = base
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let authority = s.split('/').next().unwrap_or(s);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest).to_string();
    }
    authority.split(':').next().unwrap_or(authority).to_string()
}

/// Live upstream (RustCache) coordinates — updated when the user enters a host.
#[derive(Debug, Clone)]
pub struct UpstreamCfg {
    pub api_base: String,
    pub host: String,
    pub http_port: u16,
    pub https_port: u16,
    pub socks_port: u16,
}

impl UpstreamCfg {
    pub fn from_api_base(base: &str) -> Self {
        Self {
            api_base: normalize_api_base(base, 8080),
            host: host_of_api_base(base),
            http_port: 3128,
            https_port: 3129,
            socks_port: 1080,
        }
    }

    pub fn from_discovered(d: &DiscoveredTarget) -> Self {
        Self {
            api_base: d.api_base.clone(),
            host: d.host.clone(),
            http_port: d.http_port,
            https_port: d.https_port,
            socks_port: d.socks_port,
        }
    }
}

pub type SharedUpstream = std::sync::Arc<parking_lot::RwLock<UpstreamCfg>>;

pub fn shared_upstream(cfg: UpstreamCfg) -> SharedUpstream {
    std::sync::Arc::new(parking_lot::RwLock::new(cfg))
}

impl RustCacheApi {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            timeout: Duration::from_millis(500),
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// GET /api/health — parse per-listener bind status.
    pub async fn health(&self) -> Result<HealthReport> {
        let body = self.get("/api/health").await?;
        let v: serde_json::Value = serde_json::from_str(&body).context("health json")?;
        let mut rep = HealthReport {
            ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
            uptime_s: v.get("uptime_s").and_then(|x| x.as_u64()).unwrap_or(0),
            ..Default::default()
        };
        if let Some(listeners) = v.get("listeners").and_then(|x| x.as_array()) {
            for l in listeners {
                let name = l.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let running = l.get("running").and_then(|x| x.as_bool()).unwrap_or(false);
                let port = l.get("port").and_then(|x| x.as_u64()).unwrap_or(0) as u16;
                match name {
                    "HTTP proxy" => {
                        rep.http_running = running;
                        if port > 0 {
                            rep.http_port = port;
                        }
                    }
                    "HTTPS MITM" => {
                        rep.https_running = running;
                        if port > 0 {
                            rep.https_port = port;
                        }
                    }
                    "SOCKS5" => {
                        rep.socks_running = running;
                        if port > 0 {
                            rep.socks_port = port;
                        }
                    }
                    "REST API" if port > 0 => {
                        rep.api_port = port;
                    }
                    _ => {}
                }
            }
        }
        Ok(rep)
    }

    /// GET /api/ca.crt — PEM bytes.
    pub async fn ca_pem(&self) -> Result<Vec<u8>> {
        self.get_bytes("/api/ca.crt").await
    }

    /// GET /api/stats — raw JSON.
    pub async fn stats(&self) -> Result<serde_json::Value> {
        let body = self.get("/api/stats").await?;
        Ok(serde_json::from_str(&body)?)
    }

    /// GET /api/exclusions — domain list (best-effort).
    pub async fn exclusions(&self) -> Vec<String> {
        match self.get("/api/exclusions").await {
            Ok(body) => serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("domains").and_then(|d| d.as_array()).map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    async fn get(&self, path: &str) -> Result<String> {
        let bytes = self.get_bytes(path).await?;
        String::from_utf8(bytes).context("utf-8 body")
    }

    async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let url = format!("{}{}", self.base, path);
        // Minimal HTTP/1.1 GET over std/tokio TCP — avoids pulling hyper into the client.
        let uri = parse_http_url(&url)?;
        let addr = format!("{}:{}", uri.host, uri.port);
        let mut stream = tokio::time::timeout(self.timeout, tokio::net::TcpStream::connect(&addr))
            .await
            .context("connect timeout")?
            .with_context(|| format!("connect {addr}"))?;
        stream.set_nodelay(true).ok();

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: rustcache-client\r\nAccept: */*\r\n\r\n",
            uri.path, uri.host
        );
        tokio::time::timeout(self.timeout, stream.write_all(req.as_bytes()))
            .await
            .context("write timeout")??;

        let mut buf = Vec::with_capacity(8 * 1024);
        let mut chunk = [0u8; 8 * 1024];
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.read(&mut chunk)).await {
                Ok(Ok(0)) => break,
                Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
                Ok(Err(e)) => return Err(e).context("read body"),
                Err(_) => break,
            }
        }

        split_http_body(&buf).map(|(_headers, body)| body.to_vec())
    }
}

/// Probe `http://host:port/api/health` (and a few fallback API ports) and
/// return the bound proxy ports. `input` is free-form host / URL.
pub async fn discover_target(input: &str) -> Result<DiscoveredTarget> {
    const DEFAULT_API_PORT: u16 = 8080;
    let base = normalize_api_base(input, DEFAULT_API_PORT);
    let host = host_of_api_base(&base);

    // Try the given base first, then common API ports on the same host.
    let mut candidates = vec![base.clone()];
    for p in [DEFAULT_API_PORT, 8081, 8082, 80, 8888] {
        let alt = normalize_api_base(&host, p);
        if !candidates.contains(&alt) {
            candidates.push(alt);
        }
    }

    let mut last_err = anyhow::anyhow!("no API answered on {host}");
    for cand in candidates {
        let api = RustCacheApi::new(&cand).with_timeout(Duration::from_millis(1200));
        match api.health().await {
            Ok(h) => {
                let http_port = if h.http_port > 0 { h.http_port } else { 3128 };
                let https_port = if h.https_port > 0 { h.https_port } else { 3129 };
                let socks_port = if h.socks_port > 0 { h.socks_port } else { 1080 };
                let api_port = h.api_port;
                return Ok(DiscoveredTarget {
                    api_base: cand,
                    host,
                    http_port,
                    https_port,
                    socks_port,
                    api_port,
                });
            }
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

struct HttpUri {
    host: String,
    port: u16,
    path: String,
}

fn parse_http_url(url: &str) -> Result<HttpUri> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .with_context(|| format!("url must be http(s): {url}"))?;
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = if let Some(h) = authority.strip_prefix('[') {
        let (h, rest) = h.split_once("]:").context("ipv6 authority")?;
        (h.to_string(), rest.parse::<u16>().context("port")?)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        (h.to_string(), p.parse::<u16>().context("port")?)
    } else {
        (
            authority.to_string(),
            if url.starts_with("https://") { 443 } else { 80 },
        )
    };
    Ok(HttpUri { host, port, path })
}

/// Split status-line+headers from body at first `\r\n\r\n`.
fn split_http_body(buf: &[u8]) -> Result<(&[u8], &[u8])> {
    let sep = b"\r\n\r\n";
    let idx = buf
        .windows(4)
        .position(|w| w == sep)
        .context("no HTTP header terminator")?;
    let (head, body) = buf.split_at(idx + 4);
    // strip 100-continue or chunked is out of scope for /api/* (they are identity)
    Ok((head, body))
}

/// Quick TCP reachability probe (used as breaker backup).
pub async fn tcp_alive(host: &str, port: u16, timeout: Duration) -> bool {
    let addr = format!("{host}:{port}");
    matches!(
        tokio::time::timeout(timeout, tokio::net::TcpStream::connect(&addr)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_urls() {
        let u = parse_http_url("http://127.0.0.1:8080/api/health").unwrap();
        assert_eq!(u.host, "127.0.0.1");
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/api/health");
    }

    #[test]
    fn split_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi";
        let (h, b) = split_http_body(raw).unwrap();
        assert!(h.starts_with(b"HTTP/1.1"));
        assert_eq!(b, b"hi");
    }

    #[test]
    fn normalize_host_forms() {
        assert_eq!(
            normalize_api_base("192.168.1.10", 8080),
            "http://192.168.1.10:8080"
        );
        assert_eq!(
            normalize_api_base("192.168.1.10:9000", 8080),
            "http://192.168.1.10:9000"
        );
        assert_eq!(
            normalize_api_base("http://10.0.0.5:8080", 8080),
            "http://10.0.0.5:8080"
        );
        assert_eq!(
            normalize_api_base("myhost.local", 8080),
            "http://myhost.local:8080"
        );
        assert_eq!(host_of_api_base("http://192.168.1.10:8080"), "192.168.1.10");
        assert_eq!(host_of_api_base("10.0.0.5"), "10.0.0.5");
    }
}
