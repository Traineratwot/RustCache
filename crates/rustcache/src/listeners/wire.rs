//! Shared HTTP/1.1 wire helpers used by both the plain HTTP and MITM listeners.
//!
//! Owns request parsing, response writing, and the diagnostic-header builder so
//! socket handlers stay focused on routing and cache logic. One writer and one
//! body-size cap for every listener — the plain and MITM paths must not diverge
//! on framing.

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum accepted request body size (32 MiB). Applies to every listener.
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Maximum accepted header block size (64 KiB).
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// A parsed HTTP/1.1 request head plus body.
pub struct HttpRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Read one HTTP/1.1 request from `stream`.
///
/// Returns `Ok(None)` on clean EOF before a request starts. Enforces the
/// shared header and body size caps so MITM and plain paths cannot diverge.
pub async fn read_http_request<S: AsyncReadExt + Unpin>(
    stream: &mut S,
) -> anyhow::Result<Option<HttpRequest>> {
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(anyhow::anyhow!("eof mid-headers"))
            };
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER_BYTES {
            return Err(anyhow::anyhow!("headers too large"));
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut rest = buf[header_end + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let start = lines.next().unwrap_or_default();
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let _version = parts.next().unwrap_or("HTTP/1.1");
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    while rest.len() < content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&tmp[..n]);
        if rest.len() > MAX_BODY_BYTES {
            return Err(anyhow::anyhow!("body too large"));
        }
    }
    let body = rest.into_iter().take(content_length).collect();
    Ok(Some(HttpRequest {
        method,
        target,
        headers,
        body,
    }))
}

/// Split an authority (CONNECT target or `host:port`) into `(host, port)`.
///
/// Handles IPv6 bracket form (`[::1]:443` → `("::1", Some("443"))`) which a
/// naive `split(':')` would break.
pub fn split_host_port(authority: &str) -> (&str, Option<&str>) {
    if let Some(rest) = authority.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, port_part)) => {
                let port = port_part.strip_prefix(':').filter(|p| !p.is_empty());
                (host, port)
            }
            None => (authority, None),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
            _ => (authority, None),
        }
    }
}

/// Host part of an authority (ignores port). Safe for IPv6 literals.
pub fn host_of(authority: &str) -> &str {
    split_host_port(authority).0
}

/// HTTP reason phrase for a status code. Unknown codes get an empty phrase
/// (HTTP allows this; a made-up phrase would be non-standard).
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        421 => "Misdirected Request",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}

/// Write one HTTP/1.1 response to `w`.
///
/// Single writer for every listener: strips `Transfer-Encoding` (bodies are
/// fully buffered), ensures a `Content-Length` is present, and when `close` is
/// set replaces any origin `Connection` value with `Connection: close`.
pub async fn write_http_response<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
    close: bool,
) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    let mut has_cl = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("transfer-encoding") {
            continue;
        }
        if k.eq_ignore_ascii_case("connection") {
            if close {
                continue;
            }
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str("\r\n");
            continue;
        }
        if k.eq_ignore_ascii_case("content-length") {
            has_cl = true;
        }
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    if !has_cl {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if close {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    w.write_all(out.as_bytes()).await?;
    if !body.is_empty() {
        w.write_all(body).await?;
    }
    w.flush().await?;
    Ok(())
}

/// Diagnostic headers RustCache adds to every response it writes to a client.
///
/// - `X-RustCache-Version` — build version
/// - `X-RustCache-Status` — cache outcome (`HIT` / `MISS` / `HIT_REVALIDATED` /
///   `REVALIDATED` / `BYPASS` / `ERROR`)
/// - `X-RustCache-Age` — seconds since the body was stored (cached paths only)
///
/// Origin-supplied `X-RustCache-*` headers are stripped so they cannot spoof ours.
pub fn cache_headers(
    headers: &[(String, String)],
    outcome: &str,
    age_secs: Option<u64>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = headers
        .iter()
        .filter(|(k, _)| !k.to_ascii_lowercase().starts_with("x-rustcache"))
        .cloned()
        .collect();
    out.push((
        "X-RustCache-Version".into(),
        rustcache_core::version().into(),
    ));
    out.push(("X-RustCache-Status".into(), outcome.into()));
    if let Some(age) = age_secs {
        out.push(("X-RustCache-Age".into(), age.to_string()));
    }
    out
}

/// Seconds elapsed since the entry was stored.
pub fn entry_age_secs(stored_at_ms: u64) -> u64 {
    rustcache_core::cache::meta::now_ms().saturating_sub(stored_at_ms) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hv<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn split_host_port_basic() {
        assert_eq!(
            split_host_port("example.com:443"),
            ("example.com", Some("443"))
        );
        assert_eq!(split_host_port("example.com"), ("example.com", None));
        assert_eq!(
            split_host_port("127.0.0.1:8080"),
            ("127.0.0.1", Some("8080"))
        );
    }

    #[test]
    fn split_host_port_ipv6() {
        assert_eq!(split_host_port("[::1]:443"), ("::1", Some("443")));
        assert_eq!(split_host_port("[::1]"), ("::1", None));
        assert_eq!(
            split_host_port("[2001:db8::1]:8443"),
            ("2001:db8::1", Some("8443"))
        );
    }

    #[test]
    fn host_of_extracts_without_port() {
        assert_eq!(host_of("[::1]:443"), "::1");
        assert_eq!(host_of("example.com:8443"), "example.com");
    }

    #[test]
    fn cache_headers_injects_version_status_age() {
        let origin = vec![
            ("Content-Type".into(), "text/plain".into()),
            ("X-RustCache-Status".into(), "SPOOFED".into()),
        ];
        let h = cache_headers(&origin, "HIT", Some(12));
        assert_eq!(
            hv(&h, "X-RustCache-Version"),
            Some(rustcache_core::version())
        );
        assert_eq!(hv(&h, "X-RustCache-Status"), Some("HIT"));
        assert_eq!(hv(&h, "X-RustCache-Age"), Some("12"));
        assert_eq!(hv(&h, "Content-Type"), Some("text/plain"));
        // origin-supplied spoof is stripped
        assert_eq!(
            h.iter()
                .filter(|(k, _)| k.eq_ignore_ascii_case("X-RustCache-Status"))
                .count(),
            1
        );
    }

    #[test]
    fn cache_headers_omits_age_when_none() {
        let h = cache_headers(&[], "BYPASS", None);
        assert_eq!(hv(&h, "X-RustCache-Status"), Some("BYPASS"));
        assert_eq!(hv(&h, "X-RustCache-Age"), None);
    }

    #[test]
    fn entry_age_secs_floor() {
        let now = rustcache_core::cache::meta::now_ms();
        assert_eq!(entry_age_secs(now), 0);
        assert_eq!(entry_age_secs(now.saturating_sub(2500)), 2);
    }
}
