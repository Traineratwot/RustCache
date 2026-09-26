//! HTTP caching proxy listener (port 3128).
//!
//! Absolute-form requests go through the shared cache path; CONNECT is a raw
//! tunnel. Every request is answered once and the connection closes — real
//! keep-alive is out of scope (see `force_close` semantics on the writer).

use std::time::Instant;

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use rustcache_core::http::fetch::parse_url;
use rustcache_core::stats::{Outcome, ReqRecord};

use crate::engine::{upstream_tls_connector, SharedEngine};
use crate::listeners::serve::{resolve_cached, RequestContext};
use crate::listeners::wire::{
    host_of, read_http_request as read_request, write_http_response, HttpRequest,
};

// Re-export for integration tests and sibling listeners.
pub use super::wire::{cache_headers, entry_age_secs, read_http_request};

/// Accept loop on a pre-bound listener (bind happens in `main` so failures are visible).
pub async fn serve(listener: TcpListener, engine: SharedEngine) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
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
    let req = match read_request(&mut stream).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    if req.method.eq_ignore_ascii_case("CONNECT") {
        return handle_connect(&mut stream, &req, engine).await;
    }
    handle_absolute(&mut stream, req, engine).await
}

async fn handle_connect(
    stream: &mut TcpStream,
    req: &HttpRequest,
    engine: SharedEngine,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let target = req.target.clone();
    let host = host_of(&target).to_string();
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
                outcome: Outcome::Tunnel,
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
                outcome: Outcome::Error,
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: 0,
            });
            Ok(())
        }
    }
}

async fn handle_absolute(
    stream: &mut TcpStream,
    req: HttpRequest,
    engine: SharedEngine,
) -> anyhow::Result<()> {
    let url = req.target.clone();
    let host = parse_url(&url).map(|u| u.host).unwrap_or_default();
    let ctx = RequestContext {
        method: req.method.to_uppercase(),
        url,
        host,
        headers: req.headers,
        body: req.body,
        started: Instant::now(),
    };
    let out = resolve_cached(&engine, &ctx, Some(upstream_tls_connector())).await?;
    write_http_response(stream, out.status, &out.headers, &out.body, true).await
}
