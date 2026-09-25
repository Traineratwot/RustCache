//! Shared integration fixtures: local origin, echo server, engine/proxy helpers.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use rustcache::engine::{CacheEngine, SharedEngine};
use rustcache_core::cache::disk::DiskCache;
use rustcache_core::cache::mem::MemCache;
use rustcache_core::excl::ExclusionSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Mutable response knobs for the fake origin.
#[derive(Default)]
pub struct OriginState {
    pub hits: AtomicUsize,
    pub requests: RwLock<Vec<(String, String, Option<String>)>>, // method, path, if-none-match
    pub body: RwLock<Vec<u8>>,
    pub cache_control: RwLock<String>,
    pub etag: RwLock<Option<String>>,
    pub delay_ms: AtomicU64,
}

impl OriginState {
    pub fn new(body: &str, cache_control: &str) -> Arc<Self> {
        Arc::new(Self {
            hits: AtomicUsize::new(0),
            requests: RwLock::new(Vec::new()),
            body: RwLock::new(body.as_bytes().to_vec()),
            cache_control: RwLock::new(cache_control.to_string()),
            etag: RwLock::new(None),
            delay_ms: AtomicU64::new(0),
        })
    }

    pub fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

/// Minimal HTTP/1.1 origin on an ephemeral port.
pub async fn spawn_origin(state: Arc<OriginState>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let st = state.clone();
            tokio::spawn(async move {
                let _ = handle_origin_conn(&mut sock, st).await;
            });
        }
    });
    addr
}

async fn handle_origin_conn(sock: &mut TcpStream, st: Arc<OriginState>) -> anyhow::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        buf.clear();
        let header_end = loop {
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let mut lines = head.split("\r\n");
        let start = lines.next().unwrap_or_default();
        let mut parts = start.split_whitespace();
        let method = parts.next().unwrap_or("GET").to_string();
        let path = parts.next().unwrap_or("/").to_string();
        let mut inm = None;
        let mut content_length = 0usize;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim().to_string();
                if k == "if-none-match" {
                    inm = Some(v);
                } else if k == "content-length" {
                    content_length = v.parse().unwrap_or(0);
                }
            }
        }
        // drain body if any
        let mut rest = buf[header_end + 4..].to_vec();
        while rest.len() < content_length {
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                break;
            }
            rest.extend_from_slice(&tmp[..n]);
        }

        st.hits.fetch_add(1, Ordering::SeqCst);
        st.requests
            .write()
            .unwrap()
            .push((method.clone(), path.clone(), inm.clone()));

        let delay = st.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }

        let etag = st.etag.read().unwrap().clone();
        let cc = st.cache_control.read().unwrap().clone();
        let body = st.body.read().unwrap().clone();

        if method.eq_ignore_ascii_case("HEAD") {
            write_origin(sock, 200, &cc, etag.as_deref(), b"").await?;
            continue;
        }

        if let (Some(tag), Some(req_tag)) = (etag.as_deref(), inm.as_deref()) {
            if req_tag == tag {
                let resp = format!(
                    "HTTP/1.1 304 Not Modified\r\nETag: {tag}\r\nCache-Control: {cc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                sock.write_all(resp.as_bytes()).await?;
                sock.flush().await?;
                continue;
            }
        }

        write_origin(sock, 200, &cc, etag.as_deref(), &body).await?;
    }
}

async fn write_origin(
    sock: &mut TcpStream,
    status: u16,
    cache_control: &str,
    etag: Option<&str>,
    body: &[u8],
) -> anyhow::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {status} OK\r\nCache-Control: {cache_control}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(t) = etag {
        head.push_str(&format!("ETag: {t}\r\n"));
    }
    head.push_str("\r\n");
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await?;
    sock.flush().await?;
    Ok(())
}

/// TCP echo server (for CONNECT / SOCKS5 tunnel tests).
pub async fn spawn_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            if sock.write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}

/// Fresh engine backed by a unique temp dir.
pub async fn spawn_engine(exclusions: ExclusionSet) -> (SharedEngine, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "rc-it-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let disk = DiskCache::open(&dir).unwrap();
    let mem = MemCache::new(8 * 1024 * 1024);
    let engine = Arc::new(CacheEngine::new(
        disk,
        mem,
        exclusions,
        16 * 1024 * 1024,
        64 * 1024 * 1024,
    ));
    (engine, dir)
}

/// Start HTTP proxy listener; returns bound address.
pub async fn spawn_http_proxy(engine: SharedEngine) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let engine = engine.clone();
            tokio::spawn(async move {
                // reuse library serve path via handle on connection
                let _ = rustcache::listeners::http_proxy::serve_connection(stream, engine).await;
            });
        }
    });
    addr
}

/// Start MITM proxy listener; returns bound address.
pub async fn spawn_mitm(
    engine: SharedEngine,
    leaves: Arc<rustcache_core::certs::leaf::LeafIssuer>,
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let engine = engine.clone();
            let leaves = leaves.clone();
            tokio::spawn(async move {
                let _ = rustcache::listeners::mitm_proxy::serve_connection(stream, engine, leaves)
                    .await;
            });
        }
    });
    addr
}

/// Start SOCKS5 listener; returns bound address.
pub async fn spawn_socks5(engine: SharedEngine) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let engine = engine.clone();
            tokio::spawn(async move {
                let _ = rustcache::listeners::socks5::serve_connection(stream, engine).await;
            });
        }
    });
    addr
}

/// Send one absolute-form HTTP request through the proxy; return status + body.
pub async fn proxy_get(proxy: SocketAddr, url: &str) -> std::io::Result<(u16, Vec<u8>)> {
    let mut sock = TcpStream::connect(proxy).await?;
    let req = format!("GET {url} HTTP/1.1\r\nHost: origin\r\nConnection: close\r\n\r\n");
    sock.write_all(req.as_bytes()).await?;
    sock.flush().await?;
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await?;
    parse_response(&buf)
}

pub fn parse_response(buf: &[u8]) -> std::io::Result<(u16, Vec<u8>)> {
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no header end"))?;
    let head = String::from_utf8_lossy(&buf[..pos]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad status"))?;
    Ok((status, buf[pos + 4..].to_vec()))
}
