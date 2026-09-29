//! Minimal HTTP/1.x head parser (request-line + headers we care about).

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub struct HttpHead {
    pub method: String,
    pub target: String,
    pub version: String,
    pub host: Option<String>,
    /// Raw head bytes (request-line + headers + CRLFCRLF) to forward verbatim when needed.
    pub raw: Vec<u8>,
}

/// Read until `\r\n\r\n` (bounded), parse, return head + remaining stream.
pub async fn parse_head(mut stream: TcpStream) -> Result<(HttpHead, TcpStream)> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    let max = 64 * 1024;
    loop {
        let n = stream.read(&mut chunk).await.context("read head")?;
        if n == 0 {
            bail!("eof before head terminator");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > max {
            bail!("head too large");
        }
    }
    let head = parse_head_bytes(&buf)?;
    Ok((head, stream))
}

/// Parse an already-buffered head (includes trailing `\r\n\r\n`).
pub fn parse_head_bytes(buf: &[u8]) -> Result<HttpHead> {
    let sep = b"\r\n\r\n";
    let end = buf
        .windows(4)
        .position(|w| w == sep)
        .map(|i| i + 4)
        .unwrap_or(buf.len());
    let head_raw = &buf[..end];
    let text = String::from_utf8_lossy(head_raw);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().context("empty head")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().context("no method")?.to_string();
    let target = parts.next().context("no target")?.to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();

    let mut host = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                host = Some(v.trim().to_string());
            }
        }
    }

    // If absolute-form has a host and Host header is missing, derive it.
    if host.is_none() {
        host = host_from_target(&target);
    }

    Ok(HttpHead {
        method,
        target,
        version,
        host,
        raw: head_raw.to_vec(),
    })
}

/// Extract host[:port] from absolute URI or CONNECT target.
pub fn host_from_target(target: &str) -> Option<String> {
    if let Some(rest) = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
    {
        let authority = rest.split('/').next().unwrap_or(rest);
        return Some(authority.to_string());
    }
    // CONNECT form host:port
    if !target.is_empty() && !target.starts_with('/') {
        return Some(target.to_string());
    }
    None
}

/// Host header value without port.
pub fn host_only(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.split(':').next().unwrap_or(host)
}

/// Whether `host` matches a bypass pattern (`localhost`, `127.0.0.1`, `*.local`).
pub fn matches_bypass(host: &str, patterns: &[String]) -> bool {
    let h = host_only(host).to_ascii_lowercase();
    for p in patterns {
        let p = p.trim().to_ascii_lowercase();
        if p.is_empty() {
            continue;
        }
        if let Some(suffix) = p.strip_prefix("*.") {
            if h == suffix || h.ends_with(&format!(".{suffix}")) {
                return true;
            }
        } else if h == p {
            return true;
        }
    }
    false
}

/// Strip hop-by-hop headers and rewrite request-line to origin-form for direct origin fetch.
pub fn rewrite_for_origin(head: &HttpHead, host: &str) -> Result<Vec<u8>> {
    let text = String::from_utf8_lossy(&head.raw);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().context("no request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let target = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("HTTP/1.1");

    let path = if target.starts_with("http://") || target.starts_with("https://") {
        match target.split_once("://") {
            Some((_, rest)) => {
                let after = rest.split_once('/').map(|(_, p)| format!("/{p}"));
                after.unwrap_or_else(|| "/".to_string())
            }
            None => "/".to_string(),
        }
    } else {
        target.to_string()
    };

    let mut out = format!("{method} {path} {version}\r\n");
    let mut has_host = false;
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((k, _)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim();
        let lower = key.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "proxy-connection" | "proxy-authorization" | "connection" | "keep-alive"
        ) {
            continue;
        }
        if lower == "host" {
            has_host = true;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    if !has_host {
        out.push_str(&format!("Host: {host}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    Ok(out.into_bytes())
}

/// Write a simple error response to the client.
pub async fn write_error(stream: &mut TcpStream, status: u16, msg: &str) -> Result<()> {
    let body = msg.as_bytes();
    let resp = format!(
        "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

/// Write `200 Connection Established`.
pub async fn write_established(stream: &mut TcpStream) -> Result<()> {
    stream
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connect() {
        let raw = b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n";
        let h = parse_head_bytes(raw).unwrap();
        assert_eq!(h.method, "CONNECT");
        assert_eq!(h.target, "example.com:443");
        assert_eq!(h.host.as_deref(), Some("example.com:443"));
    }

    #[test]
    fn parse_absolute_get() {
        let raw = b"GET http://example.com/a?b=1 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let h = parse_head_bytes(raw).unwrap();
        assert_eq!(h.method, "GET");
        assert_eq!(h.target, "http://example.com/a?b=1");
    }

    #[test]
    fn bypass_patterns() {
        let p = vec!["localhost".into(), "*.local".into()];
        assert!(matches_bypass("localhost", &p));
        assert!(matches_bypass("foo.local", &p));
        assert!(!matches_bypass("example.com", &p));
    }

    #[test]
    fn rewrite_strips_proxy_headers() {
        let raw = b"GET http://example.com/x HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: keep-alive\r\nAccept: */*\r\n\r\n";
        let h = parse_head_bytes(raw).unwrap();
        let out = rewrite_for_origin(&h, "example.com").unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("GET /x HTTP/1.1\r\n"));
        assert!(!s.to_ascii_lowercase().contains("proxy-connection"));
        assert!(s.contains("Host: example.com"));
    }
}
