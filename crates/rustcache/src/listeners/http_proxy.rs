//! HTTP caching proxy listener (port 3128).
//!
//! Absolute-form requests go through the cache path. CONNECT is a raw tunnel.

use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use rustcache_core::http::fetch::parse_url;
use rustcache_core::stats::ring::ReqRecord;

use crate::engine::{CacheEngine, Lookup, SharedEngine};

pub async fn serve(addr: std::net::SocketAddr, engine: SharedEngine) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "http proxy listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        let engine = engine.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, engine).await {
                tracing::debug!(%peer, error = %e, "http proxy connection error");
            }
        });
    }
}

/// Handle a single accepted connection (exported for integration tests).
pub async fn serve_connection(stream: TcpStream, engine: SharedEngine) -> anyhow::Result<()> {
    handle_conn(stream, engine).await
}

async fn handle_conn(mut stream: TcpStream, engine: SharedEngine) -> anyhow::Result<()> {
    let _ = stream.set_nodelay(true);
    loop {
        let req = match read_http_request(&mut stream).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        if req.method.eq_ignore_ascii_case("CONNECT") {
            handle_connect(&mut stream, &req, engine).await?;
            return Ok(());
        }
        let keep_alive = handle_absolute(&mut stream, &req, engine.clone()).await?;
        if !keep_alive {
            return Ok(());
        }
    }
}

pub struct HttpRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Read one HTTP/1.1 request. Returns None on clean EOF before a request starts.
pub async fn read_http_request(stream: &mut TcpStream) -> anyhow::Result<Option<HttpRequest>> {
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
        if buf.len() > 64 * 1024 {
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
        if rest.len() > 32 * 1024 * 1024 {
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

async fn handle_connect(
    stream: &mut TcpStream,
    req: &HttpRequest,
    engine: SharedEngine,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let target = req.target.clone();
    let host = target.split(':').next().unwrap_or(&target).to_string();
    engine.metrics().add_tunnel();

    match TcpStream::connect(&target).await {
        Ok(mut upstream) => {
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
            let (a, b) = tokio::io::copy_bidirectional(stream, &mut upstream).await?;
            engine.metrics().add_served(a + b);
            engine.record(ReqRecord {
                ts: rustcache_core::cache::meta::now_ms(),
                method: "CONNECT".into(),
                url: target.clone(),
                host,
                status: 200,
                outcome: "TUNNEL".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: a + b,
            });
            Ok(())
        }
        Err(e) => {
            engine.metrics().add_error();
            let msg = format!(
                "HTTP/1.1 502 Bad Gateway\r\nContent-Length: {}\r\n\r\n{}",
                e.to_string().len(),
                e
            );
            stream.write_all(msg.as_bytes()).await?;
            engine.record(ReqRecord {
                ts: rustcache_core::cache::meta::now_ms(),
                method: "CONNECT".into(),
                url: target,
                host,
                status: 502,
                outcome: "ERROR".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: 0,
            });
            Ok(())
        }
    }
}

async fn handle_absolute(
    stream: &mut TcpStream,
    req: &HttpRequest,
    engine: SharedEngine,
) -> anyhow::Result<bool> {
    let started = Instant::now();
    let url = req.target.clone();
    let host = parse_url(&url).map(|u| u.host).unwrap_or_default();
    let method = req.method.to_uppercase();
    let is_get_head = method == "GET" || method == "HEAD";
    let private_req = CacheEngine::request_is_private(&req.headers);

    // Exclusions or private (Authorization) requests → bypass (no cache)
    if engine.is_excluded_url(&url).await || private_req {
        engine.metrics().add_bypass();
        let resp = engine
            .fetcher
            .fetch(
                &method,
                &url,
                &req.headers,
                if req.body.is_empty() {
                    None
                } else {
                    Some(req.body.as_slice())
                },
                None,
                engine.max_object_bytes,
            )
            .await;
        match resp {
            Ok(r) => {
                write_response(stream, r.status, &r.headers, &r.body).await?;
                engine.metrics().add_served(r.body.len() as u64);
                engine.record(ReqRecord {
                    ts: rustcache_core::cache::meta::now_ms(),
                    method,
                    url,
                    host,
                    status: r.status,
                    outcome: "BYPASS".into(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    resp_bytes: r.body.len() as u64,
                });
                Ok(connection_keep_alive(req))
            }
            Err(e) => {
                engine.metrics().add_error();
                write_error(stream, 502, &e.to_string()).await?;
                Ok(false)
            }
        }
    } else if !is_get_head {
        // Non-GET/HEAD: no cache, pass through
        engine.metrics().add_bypass();
        let resp = engine
            .fetcher
            .fetch(
                &method,
                &url,
                &req.headers,
                if req.body.is_empty() {
                    None
                } else {
                    Some(req.body.as_slice())
                },
                None,
                engine.max_object_bytes,
            )
            .await;
        match resp {
            Ok(r) => {
                write_response(stream, r.status, &r.headers, &r.body).await?;
                engine.metrics().add_served(r.body.len() as u64);
                engine.record(ReqRecord {
                    ts: rustcache_core::cache::meta::now_ms(),
                    method,
                    url,
                    host,
                    status: r.status,
                    outcome: "BYPASS".into(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    resp_bytes: r.body.len() as u64,
                });
                Ok(connection_keep_alive(req))
            }
            Err(e) => {
                engine.metrics().add_error();
                write_error(stream, 502, &e.to_string()).await?;
                Ok(false)
            }
        }
    } else {
        // Cache path
        serve_cached(stream, req, &engine, &url, &host, &method, started).await
    }
}

async fn serve_cached(
    stream: &mut TcpStream,
    req: &HttpRequest,
    engine: &SharedEngine,
    url: &str,
    host: &str,
    method: &str,
    started: Instant,
) -> anyhow::Result<bool> {
    let lookup = engine.lookup(url).await;
    match lookup {
        Lookup::Hit(entry) => {
            engine.metrics().add_hit(entry.body.len() as u64);
            let body: &[u8] = if method.eq_ignore_ascii_case("HEAD") {
                &[]
            } else {
                &entry.body
            };
            write_response(stream, entry.meta.status, &entry.meta.headers, body).await?;
            engine.metrics().add_served(body.len() as u64);
            engine.record(ReqRecord {
                ts: rustcache_core::cache::meta::now_ms(),
                method: method.into(),
                url: url.into(),
                host: host.into(),
                status: entry.meta.status,
                outcome: "HIT".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: entry.body.len() as u64,
            });
            Ok(connection_keep_alive(req))
        }
        Lookup::Revalidate { entry } => match engine
            .fetch_and_store(method, url, &req.headers, None, None, Some(&entry))
            .await
        {
            Ok(new_entry) => {
                let outcome =
                    if new_entry.meta.etag == entry.meta.etag && new_entry.body == entry.body {
                        "HIT_REVALIDATED"
                    } else {
                        "REVALIDATED"
                    };
                if outcome == "HIT_REVALIDATED" {
                    engine.metrics().add_hit(new_entry.body.len() as u64);
                } else {
                    engine.metrics().add_miss();
                }
                let body: &[u8] = if method.eq_ignore_ascii_case("HEAD") {
                    &[]
                } else {
                    &new_entry.body
                };
                write_response(stream, new_entry.meta.status, &new_entry.meta.headers, body)
                    .await?;
                engine.metrics().add_served(body.len() as u64);
                engine.record(ReqRecord {
                    ts: rustcache_core::cache::meta::now_ms(),
                    method: method.into(),
                    url: url.into(),
                    host: host.into(),
                    status: new_entry.meta.status,
                    outcome: outcome.into(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    resp_bytes: new_entry.body.len() as u64,
                });
                Ok(connection_keep_alive(req))
            }
            Err(e) => {
                engine.metrics().add_error();
                write_error(stream, 502, &e.to_string()).await?;
                Ok(false)
            }
        },
        Lookup::Miss => match engine
            .fetch_and_store(method, url, &req.headers, None, None, None)
            .await
        {
            Ok(entry) => {
                engine.metrics().add_miss();
                let body: &[u8] = if method.eq_ignore_ascii_case("HEAD") {
                    &[]
                } else {
                    &entry.body
                };
                write_response(stream, entry.meta.status, &entry.meta.headers, body).await?;
                engine.metrics().add_served(body.len() as u64);
                engine.record(ReqRecord {
                    ts: rustcache_core::cache::meta::now_ms(),
                    method: method.into(),
                    url: url.into(),
                    host: host.into(),
                    status: entry.meta.status,
                    outcome: "MISS".into(),
                    duration_ms: started.elapsed().as_millis() as u64,
                    resp_bytes: body.len() as u64,
                });
                Ok(connection_keep_alive(req))
            }
            Err(e) => {
                engine.metrics().add_error();
                write_error(stream, 502, &e.to_string()).await?;
                Ok(false)
            }
        },
    }
}

fn connection_keep_alive(req: &HttpRequest) -> bool {
    for (k, v) in &req.headers {
        if k.eq_ignore_ascii_case("connection") {
            return v.to_ascii_lowercase().contains("keep-alive");
        }
    }
    // HTTP/1.1 default keep-alive; we close to keep the cache path simple and
    // avoid leftover body frames after origin `Connection: close`.
    false
}

pub async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    let mut has_cl = false;
    let mut has_conn = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("transfer-encoding") {
            continue;
        }
        if k.eq_ignore_ascii_case("content-length") {
            has_cl = true;
        }
        if k.eq_ignore_ascii_case("connection") {
            has_conn = true;
            out.push_str("Connection: close\r\n");
            continue;
        }
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    if !has_cl {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if !has_conn {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn write_error(stream: &mut TcpStream, status: u16, msg: &str) -> anyhow::Result<()> {
    write_response(stream, status, &[], msg.as_bytes()).await
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

/// Used by MITM path to serve a cached request over an arbitrary AsyncWrite stream.
pub async fn write_response_to<W: AsyncWriteExt + Unpin>(
    stream: &mut W,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> anyhow::Result<()> {
    let reason = reason_phrase(status);
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    let mut has_cl = false;
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("transfer-encoding") {
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
    out.push_str("\r\n");
    stream.write_all(out.as_bytes()).await?;
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}
