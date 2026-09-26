//! Fetch responses from origin over HTTP or HTTPS.

use std::sync::Arc;

use anyhow::{Context, anyhow};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::Result;

/// Minimal origin fetcher used by the HTTP/HTTPS cache path.
/// Speaks HTTP/1.1 and returns status + headers + body (fully buffered up to max_object_bytes).
pub struct OriginFetcher {
    pub connect_timeout: std::time::Duration,
    pub user_agent: String,
}

impl Default for OriginFetcher {
    fn default() -> Self {
        Self {
            connect_timeout: std::time::Duration::from_secs(15),
            user_agent: format!("RustCache/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OriginResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl OriginFetcher {
    /// Fetch `url` with `method` and optional request headers + body.
    /// `tls` is used only when `url` scheme is `https` (SNI = host); `http://` stays plaintext
    /// even if a connector is supplied.
    pub async fn fetch(
        &self,
        method: &str,
        url: &str,
        req_headers: &[(String, String)],
        body: Option<&[u8]>,
        tls: Option<Arc<tokio_rustls::TlsConnector>>,
        max_body: u64,
    ) -> Result<OriginResponse> {
        let parsed = parse_url(url)?;
        let port = parsed
            .port
            .unwrap_or(if parsed.scheme == "https" { 443 } else { 80 });
        let addr = format!("{}:{}", parsed.host, port);

        let stream = tokio::time::timeout(self.connect_timeout, TcpStream::connect(&addr))
            .await
            .map_err(|_| anyhow!("connect timeout to {addr}"))?
            .with_context(|| format!("connect {addr}"))?;
        let _ = stream.set_nodelay(true);

        let mut headers: Vec<(String, String)> = Vec::new();
        let mut saw_host = false;
        let mut saw_ua = false;
        let mut saw_conn = false;
        for (k, v) in req_headers {
            let lk = k.to_ascii_lowercase();
            if lk == "proxy-connection" || lk == "proxy-authorization" {
                continue;
            }
            if lk == "host" {
                saw_host = true;
            }
            if lk == "user-agent" {
                saw_ua = true;
            }
            if lk == "connection" {
                saw_conn = true;
            }
            headers.push((k.clone(), v.clone()));
        }
        if !saw_host {
            headers.push((
                "Host".into(),
                if parsed.port.is_some() && port != 80 && port != 443 {
                    format!("{}:{}", parsed.host, port)
                } else {
                    parsed.host.clone()
                },
            ));
        }
        if !saw_ua {
            headers.push(("User-Agent".into(), self.user_agent.clone()));
        }
        if !saw_conn {
            headers.push(("Connection".into(), "close".into()));
        }

        let path = if parsed.path_query.is_empty() {
            "/"
        } else {
            &parsed.path_query
        };
        let mut head = format!("{method} {path} HTTP/1.1\r\n");
        for (k, v) in &headers {
            head.push_str(k);
            head.push_str(": ");
            head.push_str(v);
            head.push_str("\r\n");
        }
        if let Some(b) = body {
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            {
                head.push_str(&format!("Content-Length: {}\r\n", b.len()));
            }
        }
        head.push_str("\r\n");
        let payload = head.into_bytes();

        // TLS only for https:// — a connector present for an http:// URL must not
        // wrap the socket (that yields InvalidContentType against port 80/8080).
        if parsed.scheme == "https" {
            let Some(tls) = tls else {
                return Err(crate::Error::Tls(format!(
                    "https fetch requires a TLS connector: {url}"
                )));
            };
            let domain = rustls::pki_types::ServerName::try_from(parsed.host.clone())
                .map_err(|_| crate::Error::Tls(format!("invalid SNI: {}", parsed.host)))?;
            let mut tls_stream = tls
                .connect(domain, stream)
                .await
                .map_err(|e| crate::Error::Tls(e.to_string()))?;
            tls_stream.write_all(&payload).await?;
            if let Some(b) = body {
                tls_stream.write_all(b).await?;
            }
            tls_stream.flush().await?;
            let resp = read_http1_response(&mut tls_stream, max_body, method).await?;
            Ok(resp)
        } else {
            let mut stream = stream;
            stream.write_all(&payload).await?;
            if let Some(b) = body {
                stream.write_all(b).await?;
            }
            stream.flush().await?;
            let resp = read_http1_response(&mut stream, max_body, method).await?;
            Ok(resp)
        }
    }
}

pub struct ParsedUrl {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path_query: String,
}

pub fn parse_url(url: &str) -> Result<ParsedUrl> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| crate::Error::Protocol(format!("bad url: {url}")))?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path_query) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h.to_ascii_lowercase(), p.parse::<u16>().ok())
        }
        _ => (authority.to_ascii_lowercase(), None),
    };
    let path_query = if path_query.is_empty() {
        "/".to_string()
    } else if path_query.starts_with('?') {
        format!("/{path_query}")
    } else {
        path_query.to_string()
    };
    Ok(ParsedUrl {
        scheme,
        host,
        port,
        path_query,
    })
}

async fn read_http1_response<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    max_body: u64,
    method: &str,
) -> Result<OriginResponse> {
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 8192];
    // Read until header terminator
    let header_end = loop {
        if let Some(i) = find_header_end(&buf) {
            break i;
        }
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(crate::Error::Protocol("eof before headers".into()));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 64 * 1024 {
            return Err(crate::Error::Protocol("headers too large".into()));
        }
    };

    let header_bytes = buf[..header_end].to_vec();
    let mut rest = buf[header_end + 4..].to_vec();

    let text = String::from_utf8_lossy(&header_bytes).to_string();
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| crate::Error::Protocol("empty status line".into()))?;
    let status = parse_status(status_line)?;

    let mut headers = Vec::new();
    let mut content_length: Option<u64> = None;
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let v = v.trim().to_string();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.parse().ok();
            } else if k.eq_ignore_ascii_case("transfer-encoding")
                && v.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
            headers.push((k, v));
        }
    }

    // HEAD responses carry Content-Length but no body.
    let body = if method.eq_ignore_ascii_case("HEAD") || status == 304 || status == 204 {
        Vec::new()
    } else if chunked {
        read_chunked(stream, &mut rest, max_body).await?
    } else if let Some(len) = content_length {
        read_exact_len(stream, &mut rest, len, max_body).await?
    } else {
        // read to EOF
        let mut body = rest;
        while body.len() as u64 <= max_body {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
            if body.len() as u64 > max_body {
                return Err(crate::Error::Cache("object too large".into()));
            }
        }
        body
    };

    Ok(OriginResponse {
        status,
        headers,
        body,
    })
}

fn parse_status(line: &str) -> Result<u16> {
    // HTTP/1.1 200 OK
    let mut parts = line.split_whitespace();
    let _ver = parts.next();
    let code = parts
        .next()
        .ok_or_else(|| crate::Error::Protocol(format!("bad status line: {line}")))?;
    code.parse()
        .map_err(|_| crate::Error::Protocol(format!("bad status code: {code}")))
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn read_exact_len<S: AsyncRead + Unpin>(
    stream: &mut S,
    rest: &mut Vec<u8>,
    len: u64,
    max_body: u64,
) -> Result<Vec<u8>> {
    if len > max_body {
        return Err(crate::Error::Cache("object too large".into()));
    }
    let mut tmp = [0u8; 8192];
    while (rest.len() as u64) < len {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&tmp[..n]);
        if rest.len() as u64 > max_body {
            return Err(crate::Error::Cache("object too large".into()));
        }
    }
    if (rest.len() as u64) < len {
        return Err(crate::Error::Protocol("eof before body complete".into()));
    }
    Ok(rest[..len as usize].to_vec())
}

async fn read_chunked<S: AsyncRead + Unpin>(
    stream: &mut S,
    rest: &mut Vec<u8>,
    max_body: u64,
) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        // need a full chunk-size line
        while !rest.windows(2).any(|w| w == b"\r\n") {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(crate::Error::Protocol("eof in chunked".into()));
            }
            rest.extend_from_slice(&tmp[..n]);
        }
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| crate::Error::Protocol("missing chunk size line".into()))?;
        let size_line = String::from_utf8_lossy(&rest[..line_end]).to_string();
        rest.drain(..line_end + 2);
        let size_str = size_line.split(';').next().unwrap_or("").trim();
        let size = u64::from_str_radix(size_str, 16)
            .map_err(|_| crate::Error::Protocol(format!("bad chunk size: {size_str}")))?;
        if size == 0 {
            // consume trailing \r\n after optional trailers until blank line
            loop {
                while rest.len() < 2 {
                    let n = stream.read(&mut tmp).await?;
                    if n == 0 {
                        return Ok(body);
                    }
                    rest.extend_from_slice(&tmp[..n]);
                }
                if rest.starts_with(b"\r\n") {
                    return Ok(body);
                }
                // trailer line — drop to next \r\n
                while !rest.windows(2).any(|w| w == b"\r\n") {
                    let n = stream.read(&mut tmp).await?;
                    if n == 0 {
                        return Ok(body);
                    }
                    rest.extend_from_slice(&tmp[..n]);
                }
                let e = rest
                    .windows(2)
                    .position(|w| w == b"\r\n")
                    .ok_or_else(|| crate::Error::Protocol("missing trailer line".into()))?;
                rest.drain(..e + 2);
            }
        }
        // read chunk data + trailing CRLF
        let need = size as usize + 2;
        while rest.len() < need {
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(crate::Error::Protocol("eof in chunk data".into()));
            }
            rest.extend_from_slice(&tmp[..n]);
        }
        body.extend_from_slice(&rest[..size as usize]);
        if body.len() as u64 > max_body {
            return Err(crate::Error::Cache("object too large".into()));
        }
        rest.drain(..need);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_basic() {
        let u = parse_url("http://example.com/a?b=1").unwrap();
        assert_eq!(u.scheme, "http");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, None);
        assert_eq!(u.path_query, "/a?b=1");
    }

    #[test]
    fn parse_url_with_port() {
        let u = parse_url("https://example.com:8443/").unwrap();
        assert_eq!(u.port, Some(8443));
    }
}
