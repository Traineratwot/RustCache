//! Integration: CONNECT tunnel and SOCKS5.

mod common;

use common::*;
use rustcache_core::excl::ExclusionSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn connect_tunnel_echo() {
    install_crypto();
    let echo = spawn_echo().await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let mut sock = TcpStream::connect(proxy).await.unwrap();
    let req = format!(
        "CONNECT 127.0.0.1:{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
        echo.port(),
        echo.port()
    );
    sock.write_all(req.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();

    // read 200 Connection Established
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        let n = sock.read(&mut tmp).await.unwrap();
        assert!(n > 0, "eof before CONNECT response");
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    assert!(head.starts_with("HTTP/1.1 200"), "got: {head}");

    sock.write_all(b"ping-tunnel").await.unwrap();
    sock.flush().await.unwrap();
    let mut out = vec![0u8; 11];
    sock.read_exact(&mut out).await.unwrap();
    assert_eq!(&out, b"ping-tunnel");

    let snap = engine.metrics().snapshot();
    assert_eq!(snap.tunnels, 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn socks5_connect_and_reject_udp() {
    install_crypto();
    let echo = spawn_echo().await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let socks = spawn_socks5(engine.clone()).await;

    // --- CONNECT success ---
    let mut sock = TcpStream::connect(socks).await.unwrap();
    // greeting: ver5, 1 method: no-auth
    sock.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut sel = [0u8; 2];
    sock.read_exact(&mut sel).await.unwrap();
    assert_eq!(sel, [0x05, 0x00]);

    // CONNECT 127.0.0.1:port (IPv4)
    let ip: [u8; 4] = [127, 0, 0, 1];
    let port = echo.port().to_be_bytes();
    let mut req = vec![0x05, 0x01, 0x00, 0x01];
    req.extend_from_slice(&ip);
    req.extend_from_slice(&port);
    sock.write_all(&req).await.unwrap();

    let mut reply = [0u8; 10];
    sock.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[0], 0x05);
    assert_eq!(reply[1], 0x00, "SOCKS5 CONNECT should succeed");

    sock.write_all(b"socks-echo").await.unwrap();
    let mut out = vec![0u8; 10];
    sock.read_exact(&mut out).await.unwrap();
    assert_eq!(&out, b"socks-echo");
    drop(sock);

    // --- UDP ASSOCIATE (cmd 0x03) rejected ---
    let mut sock = TcpStream::connect(socks).await.unwrap();
    sock.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut sel = [0u8; 2];
    sock.read_exact(&mut sel).await.unwrap();
    assert_eq!(sel, [0x05, 0x00]);
    // cmd = 0x03 UDP
    let mut req = vec![0x05, 0x03, 0x00, 0x01];
    req.extend_from_slice(&[127, 0, 0, 1]);
    req.extend_from_slice(&echo.port().to_be_bytes());
    sock.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    sock.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[0], 0x05);
    assert_eq!(
        reply[1], 0x07,
        "UDP/Bind must be rejected with cmd not supported"
    );

    // --- BIND (cmd 0x02) rejected ---
    let mut sock = TcpStream::connect(socks).await.unwrap();
    sock.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
    let mut sel = [0u8; 2];
    sock.read_exact(&mut sel).await.unwrap();
    let mut req = vec![0x05, 0x02, 0x00, 0x01];
    req.extend_from_slice(&[127, 0, 0, 1]);
    req.extend_from_slice(&echo.port().to_be_bytes());
    sock.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    sock.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0x07);

    let _ = std::fs::remove_dir_all(dir);
}
