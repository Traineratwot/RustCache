//! Integration: local proxy routes to a mock upstream or DIRECT, with fail-open.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use rustcache_client::config::ClientConfig;
use rustcache_client::health::HealthMonitor;
use rustcache_client::proxy::ProxyServer;

/// Tiny HTTP origin that answers `200 OK` with a marker body.
async fn spawn_origin(marker: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let h = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 2048];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = format!("{marker} method-line={}", req.lines().next().unwrap_or(""));
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes()).await;
            let _ = s.shutdown().await;
        }
    });
    (addr.to_string(), h)
}

/// Mock "RustCache" HTTP proxy: absolute-form GET → 200 with marker; CONNECT → 200 + echo.
async fn spawn_mock_proxy(marker: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let h = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            let marker = marker;
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                if req.starts_with("CONNECT") {
                    let _ = s
                        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                        .await;
                    // echo a few bytes then close
                    let mut tmp = [0u8; 64];
                    if let Ok(Ok(n)) =
                        tokio::time::timeout(Duration::from_millis(200), s.read(&mut tmp)).await
                    {
                        let _ = s.write_all(&tmp[..n]).await;
                    }
                    let _ = s.shutdown().await;
                } else {
                    let body = format!("{marker} via-proxy");
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.shutdown().await;
                }
            });
        }
    });
    // addr is host:port
    let host_port = addr.to_string();
    (host_port, h)
}

async fn http_get_via(proxy_addr: &str, absolute_url: &str) -> String {
    let mut s = TcpStream::connect(proxy_addr).await.unwrap();
    let req = format!("GET {absolute_url} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    String::from_utf8_lossy(&out).to_string()
}

async fn connect_via(proxy_addr: &str, target: &str) -> String {
    let mut s = TcpStream::connect(proxy_addr).await.unwrap();
    let req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 256];
    // read 200 head
    loop {
        let n = s.read(&mut buf).await.unwrap();
        out.extend_from_slice(&buf[..n]);
        if out.windows(4).any(|w| w == b"\r\n\r\n") || n == 0 {
            break;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn health_ok() -> HealthMonitor {
    // Default monitor starts Closed; we never probe — keep it closed.
    HealthMonitor::new(&ClientConfig::default())
}

fn health_down() -> HealthMonitor {
    let m = HealthMonitor::new(&ClientConfig::default());
    for _ in 0..5 {
        m.record_failure("down");
    }
    m
}

#[tokio::test]
async fn http_forwards_to_upstream_when_closed() {
    let (mock_addr, _mh) = spawn_mock_proxy("UP").await;
    let (host, port_s) = mock_addr.rsplit_once(':').unwrap();
    let port: u16 = port_s.parse().unwrap();

    let (l, la) = rustcache_client::proxy::bind_local().await.unwrap();
    let srv = Arc::new(ProxyServer::new(
        &la.to_string(),
        host,
        port,
        port + 1, // unused https
        Arc::new(health_ok()),
        vec!["localhost".into()],
    ));
    tokio::spawn(srv.serve_listener(l));

    let resp = http_get_via(&la.to_string(), "http://example.com/").await;
    assert!(resp.contains("UP via-proxy"), "got: {resp}");
}

#[tokio::test]
async fn http_goes_direct_when_breaker_open() {
    let (origin_addr, _oh) = spawn_origin("DIR").await;
    let (_l2, _la2) = rustcache_client::proxy::bind_local().await.unwrap();

    // Build server whose "upstream" ports are a black hole (closed port).
    let (l, la) = rustcache_client::proxy::bind_local().await.unwrap();
    let blackhole_port = {
        // bind then drop so the port is likely free/refused
        let tmp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        tmp.local_addr().unwrap().port()
    };
    let srv = Arc::new(ProxyServer::new(
        &la.to_string(),
        "127.0.0.1",
        blackhole_port,
        blackhole_port,
        Arc::new(health_down()),
        vec![],
    ));
    tokio::spawn(srv.serve_listener(l));

    // Absolute URL pointing at our real origin so Direct works.
    let url = format!("http://{origin_addr}/x");
    let resp = http_get_via(&la.to_string(), &url).await;
    assert!(resp.contains("DIR"), "got: {resp}");
}

#[tokio::test]
async fn connect_direct_when_open() {
    let (origin_addr, _oh) = spawn_origin("D").await;
    let (l, la) = rustcache_client::proxy::bind_local().await.unwrap();
    let blackhole_port = {
        let tmp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        tmp.local_addr().unwrap().port()
    };
    let srv = Arc::new(ProxyServer::new(
        &la.to_string(),
        "127.0.0.1",
        blackhole_port,
        blackhole_port,
        Arc::new(health_down()),
        vec![],
    ));
    tokio::spawn(srv.serve_listener(l));

    let head = connect_via(&la.to_string(), &origin_addr).await;
    assert!(head.contains("200"), "got: {head}");
}

#[tokio::test]
async fn connect_uses_mitm_upstream_when_closed() {
    // Mock proxy on "https" port answers CONNECT with 200.
    let (mock_addr, _mh) = spawn_mock_proxy("M").await;
    let (host, port_s) = mock_addr.rsplit_once(':').unwrap();
    let port: u16 = port_s.parse().unwrap();

    let (l, la) = rustcache_client::proxy::bind_local().await.unwrap();
    let srv = Arc::new(ProxyServer::new(
        &la.to_string(),
        host,
        port.saturating_sub(1).max(1), // http unused
        port,
        Arc::new(health_ok()),
        vec![],
    ));
    tokio::spawn(srv.serve_listener(l));

    let head = connect_via(&la.to_string(), "example.com:443").await;
    assert!(head.contains("200"), "got: {head}");
}

#[tokio::test]
async fn bypass_host_goes_direct() {
    let (origin_addr, _oh) = spawn_origin("BYP").await;
    // origin is 127.0.0.1:PORT — bypass localhost forces Direct even when upstream is "up"
    let (mock_addr, _mh) = spawn_mock_proxy("SHOULD_NOT_HIT").await;
    let (host, port_s) = mock_addr.rsplit_once(':').unwrap();
    let port: u16 = port_s.parse().unwrap();

    let (l, la) = rustcache_client::proxy::bind_local().await.unwrap();
    let srv = Arc::new(ProxyServer::new(
        &la.to_string(),
        host,
        port,
        port,
        Arc::new(health_ok()),
        vec!["127.0.0.1".into(), "localhost".into()],
    ));
    tokio::spawn(srv.serve_listener(l));

    let url = format!("http://{origin_addr}/");
    let resp = http_get_via(&la.to_string(), &url).await;
    assert!(resp.contains("BYP"), "got: {resp}");
    assert!(!resp.contains("SHOULD_NOT_HIT"), "got: {resp}");
}

/// Health says "closed/up" but the TCP hop is dead — must still fail-open DIRECT
/// on the same connection (this is the smoke-test case when rustcache is stopped).
#[tokio::test]
async fn hop_down_fails_open_even_when_breaker_closed() {
    let (origin_addr, _oh) = spawn_origin("FALLBACK").await;
    let blackhole_port = {
        let tmp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        tmp.local_addr().unwrap().port()
    };

    let (l, la) = rustcache_client::proxy::bind_local().await.unwrap();
    // health_ok → breaker Closed → route wants upstream, but port is dead.
    let srv = Arc::new(ProxyServer::new(
        &la.to_string(),
        "127.0.0.1",
        blackhole_port,
        blackhole_port,
        Arc::new(health_ok()),
        vec![],
    ));
    tokio::spawn(srv.serve_listener(l));

    let url = format!("http://{origin_addr}/");
    let resp = http_get_via(&la.to_string(), &url).await;
    assert!(resp.contains("FALLBACK"), "got: {resp}");
}
