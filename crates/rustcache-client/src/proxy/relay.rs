//! Byte relays: via RustCache proxy or DIRECT to origin.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::proxy::http_parse::{HttpHead, rewrite_for_origin, write_error, write_established};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Outcome of trying the RustCache hop first.
pub enum Hop {
    /// Connected to RustCache; caller should speak proxy protocol.
    Upstream(TcpStream),
    /// RustCache is unreachable; fall back to DIRECT (client stream untouched).
    Down,
}

/// Try to reach the RustCache proxy port. `Down` on failure — caller falls back.
pub async fn try_upstream(proxy_host: &str, proxy_port: u16) -> Hop {
    let addr = format!("{proxy_host}:{proxy_port}");
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => {
            let _ = s.set_nodelay(true);
            Hop::Upstream(s)
        }
        _ => Hop::Down,
    }
}

/// CONNECT through RustCache, then splice. `upstream` must already be connected.
pub async fn connect_via_connected(
    mut client: TcpStream,
    mut upstream: TcpStream,
    target: &str,
) -> Result<()> {
    let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nConnection: close\r\n\r\n");
    upstream.write_all(req.as_bytes()).await?;
    upstream.flush().await?;

    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    loop {
        let n = tokio::time::timeout(CONNECT_TIMEOUT, upstream.read(&mut chunk))
            .await
            .context("upstream head timeout")?
            .context("upstream read")?;
        if n == 0 {
            bail!("upstream closed during CONNECT");
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            bail!("upstream CONNECT head too large");
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let status_ok = text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200");
    if !status_ok {
        let first = text.lines().next().unwrap_or("HTTP/1.1 502 Bad Gateway");
        let resp = format!("{first}\r\nConnection: close\r\n\r\n");
        client.write_all(resp.as_bytes()).await?;
        return Ok(());
    }

    write_established(&mut client).await?;
    let (a, b) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    tracing::debug!(a, b, "connect splice done");
    Ok(())
}

/// CONNECT DIRECT to origin (fail-open path).
pub async fn connect_direct(mut client: TcpStream, target: &str) -> Result<()> {
    let (host, port) = split_host_port(target)?;
    let addr = format!("{host}:{port}");
    let mut upstream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
        .await
        .context("direct connect timeout")?
        .with_context(|| format!("connect {addr}"))?;
    let _ = upstream.set_nodelay(true);
    write_established(&mut client).await?;
    let (a, b) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    tracing::debug!(a, b, "direct splice done");
    Ok(())
}

/// Forward plain HTTP request to RustCache (absolute-URI as-is). `upstream` pre-connected.
pub async fn http_via_connected(
    mut client: TcpStream,
    mut upstream: TcpStream,
    head: &HttpHead,
) -> Result<()> {
    let _ = upstream.set_nodelay(true);
    upstream.write_all(&head.raw).await?;
    upstream.flush().await?;
    let (a, b) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    tracing::debug!(a, b, "http via proxy done");
    Ok(())
}

/// Fetch plain HTTP DIRECT from origin and relay the response.
pub async fn http_direct(mut client: TcpStream, head: &HttpHead) -> Result<()> {
    let host_hdr = crate::proxy::http_parse::host_from_target(&head.target)
        .or_else(|| head.host.clone())
        .unwrap_or_default();
    if host_hdr.is_empty() {
        write_error(&mut client, 400, "missing Host").await?;
        return Ok(());
    }
    let (host, port) = split_host_port_with(&host_hdr, 80).unwrap_or((host_hdr.clone(), 80));
    let addr = format!("{host}:{port}");
    let mut upstream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr)).await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            write_error(&mut client, 502, &format!("origin connect: {e}")).await?;
            return Ok(());
        }
        Err(_) => {
            write_error(&mut client, 504, "origin timeout").await?;
            return Ok(());
        }
    };
    let _ = upstream.set_nodelay(true);

    let rewritten = rewrite_for_origin(head, &host_hdr)?;
    upstream.write_all(&rewritten).await?;
    upstream.flush().await?;
    let (a, b) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    tracing::debug!(a, b, "http direct done");
    Ok(())
}

/// Split `host:port` (IPv6 in brackets). `default_port` used when missing.
pub fn split_host_port_with(target: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some(rest) = target.strip_prefix('[') {
        let (h, p) = rest.split_once("]:").context("bad [v6]:port")?;
        return Ok((h.to_string(), p.parse().context("port")?));
    }
    if let Some((h, p)) = target.rsplit_once(':') {
        if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            return Ok((h.to_string(), p.parse().context("port")?));
        }
    }
    Ok((target.to_string(), default_port))
}

/// Split `host:port` for CONNECT — default HTTPS port 443.
pub fn split_host_port(target: &str) -> Result<(String, u16)> {
    split_host_port_with(target, 443)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_defaults() {
        assert_eq!(
            split_host_port("example.com:443").unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(
            split_host_port("example.com").unwrap(),
            ("example.com".into(), 443)
        );
        assert_eq!(split_host_port("[::1]:8080").unwrap(), ("::1".into(), 8080));
        // plain HTTP default is 80, not 443
        assert_eq!(
            split_host_port_with("example.com", 80).unwrap(),
            ("example.com".into(), 80)
        );
    }
}
