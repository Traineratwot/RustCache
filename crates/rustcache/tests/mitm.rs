//! Integration: MITM HTTPS path with our CA in the client's RootCertStore.

mod common;

use std::sync::Arc;

use common::*;
use rustcache_core::cache::key::cache_key;
use rustcache_core::cache::meta::{now_ms, CacheMeta};
use rustcache_core::certs::ca::generate_ca;
use rustcache_core::certs::leaf::LeafIssuer;
use rustcache_core::excl::ExclusionSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Seed a fresh cacheable entry so MITM can HIT without contacting origin TLS.
async fn seed(engine: &rustcache::engine::CacheEngine, url: &str, body: &[u8]) {
    let key = cache_key(url);
    let meta = CacheMeta {
        key: key.as_str().to_string(),
        url: url.to_string(),
        status: 200,
        headers: vec![
            ("content-type".into(), "text/plain".into()),
            ("content-length".into(), body.len().to_string()),
        ],
        stored_at: now_ms(),
        last_access: now_ms(),
        body_len: body.len() as u64,
        etag: None,
        last_modified: None,
        expires_at: Some(now_ms() + 60_000),
        cacheable: true,
    };
    engine.disk.store(meta.clone(), body).await.unwrap();
    engine
        .mem
        .insert(
            key.as_str(),
            rustcache_core::cache::mem::CachedEntry {
                meta,
                body: Arc::new(body.to_vec()),
            },
            None,
        )
        .await;
}

#[tokio::test]
async fn mitm_https_hit_with_trusted_ca() {
    install_crypto();
    // Origin exists only as a CONNECT target; HIT path must not fetch TLS.
    let origin_state = OriginState::new("from-origin", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;

    let ca_dir = dir.join("ca");
    let ca = generate_ca(&ca_dir).unwrap();
    let leaves = Arc::new(LeafIssuer::from_ca(&ca).unwrap());

    // Pre-seed the exact URL MITM will request: https://localhost:{port}/seeded
    // CONNECT authority is localhost:port so leaf SAN matches client SNI.
    let url = format!("https://localhost:{}/seeded", origin.port());
    seed(&engine, &url, b"mitm-cached-body").await;

    let mitm = spawn_mitm(engine.clone(), leaves).await;

    // Client with our CA as trust root
    let mut roots = rustls::RootCertStore::empty();
    let ca_certs: Vec<_> = rustls_pemfile::certs(&mut ca.cert_pem.as_bytes())
        .filter_map(|c| c.ok())
        .collect();
    roots.add_parsable_certificates(ca_certs);
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));

    let mut sock = TcpStream::connect(mitm).await.unwrap();
    let auth = format!("localhost:{}", origin.port());
    let req = format!("CONNECT {auth} HTTP/1.1\r\nHost: {auth}\r\n\r\n");
    sock.write_all(req.as_bytes()).await.unwrap();
    sock.flush().await.unwrap();

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
    let head = String::from_utf8_lossy(&buf).to_string();
    assert!(head.starts_with("HTTP/1.1 200"), "CONNECT: {head}");

    // TLS handshake against MITM leaf (must verify with our CA)
    let domain = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls = connector
        .connect(domain, sock)
        .await
        .expect("TLS to MITM leaf");
    tls.write_all(b"GET /seeded HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tls.flush().await.unwrap();

    // Server may drop without TLS close_notify — read until full response is buffered.
    let mut resp = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match tls.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                resp.extend_from_slice(&tmp[..n]);
                if let Some(pos) = resp.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&resp[..pos]).to_string();
                    let cl = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            if k.eq_ignore_ascii_case("content-length") {
                                v.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if resp.len() >= pos + 4 + cl {
                        break;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                if resp.is_empty() {
                    panic!("tls read: {e}");
                }
                break;
            }
        }
    }
    let (status, body) = parse_response(&resp).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"mitm-cached-body");

    // Origin was never contacted for the HIT
    assert_eq!(origin_state.hits(), 0);

    let snap = engine.metrics().snapshot();
    assert_eq!(snap.hits, 1, "MITM HIT expected, snap={snap:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn mitm_issues_leaf_matching_host() {
    install_crypto();
    let (_engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let ca_dir = dir.join("ca");
    let ca = generate_ca(&ca_dir).unwrap();
    let issuer = LeafIssuer::from_ca(&ca).unwrap();
    let leaf = issuer.issue("shop.example.com").unwrap();
    assert!(leaf.cert_pem.contains("BEGIN CERTIFICATE"));
    let der = rustls_pemfile::certs(&mut leaf.cert_pem.as_bytes())
        .next()
        .unwrap()
        .unwrap();
    assert!(
        der.as_ref()
            .windows(b"shop.example.com".len())
            .any(|w| w == b"shop.example.com"),
        "SAN/CN must embed host"
    );
    let _ = std::fs::remove_dir_all(dir);
}
