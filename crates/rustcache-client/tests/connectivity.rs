//! Hermetic connectivity tests: mock origin + mock upstream + marker checks.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Spawn a mock HTTP origin that echoes request headers in the body.
/// Does NOT include the client marker (that's injected by ProxyServer).
async fn spawn_origin() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
    (format!("{addr}"), handle)
}

/// Spawn a mock "RustCache" upstream that adds X-RustCache-Status and X-RustCache-Client.
async fn spawn_mock_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
                // Simulate a RustCache response with cache status.
                let body = b"cached";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nX-RustCache-Status: HIT\r\nX-RustCache-Client: 1\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.flush().await;
            });
        }
    });
    (format!("{addr}"), handle)
}

/// Simple GET returning (status, full_response_bytes).
async fn get(addr: &str, target: &str, host: &str) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    s.flush().await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let status = String::from_utf8_lossy(&buf)
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    (status, buf)
}

#[tokio::test]
async fn direct_origin_has_no_marker() {
    let (origin_addr, _h) = spawn_origin().await;
    let (status, resp) = get(&origin_addr, "/", &origin_addr).await;
    assert_eq!(status, 200);
    assert!(
        !rustcache_client::proxy::http_parse::has_client_marker(&resp),
        "origin must not add client marker"
    );
}

#[tokio::test]
async fn mock_upstream_injects_marker_and_status() {
    let (upstream_addr, _h) = spawn_mock_upstream().await;
    let (status, resp) = get(&upstream_addr, "http://example.com/", "example.com").await;
    assert_eq!(status, 200);
    assert!(rustcache_client::proxy::http_parse::has_client_marker(
        &resp
    ));
    assert_eq!(
        rustcache_client::proxy::http_parse::rustcache_status(&resp),
        Some("HIT".into())
    );
}

#[tokio::test]
async fn rewrite_for_origin_adds_marker() {
    use rustcache_client::proxy::http_parse::{HttpHead, rewrite_for_origin};

    let raw =
        b"GET /test HTTP/1.1\r\nHost: example.com\r\nX-RustCache-Client: 99\r\nAccept: */*\r\n\r\n";
    let head = HttpHead {
        method: "GET".into(),
        target: "/test".into(),
        version: "HTTP/1.1".into(),
        host: Some("example.com".into()),
        raw: raw.to_vec(),
    };
    let out = rewrite_for_origin(&head, "example.com").unwrap();
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("X-RustCache-Client: 1"));
    assert!(!s.contains("X-RustCache-Client: 99"));
}

#[tokio::test]
async fn inject_client_marker_upstream_form() {
    use rustcache_client::proxy::http_parse::inject_client_marker;

    let raw = b"GET http://example.com/a HTTP/1.1\r\nHost: example.com\r\nAccept: */*\r\n\r\n";
    let out = inject_client_marker(raw);
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("GET http://example.com/a HTTP/1.1"));
    assert!(s.contains("X-RustCache-Client: 1"));
    assert!(s.contains("Accept: */*"));
}
