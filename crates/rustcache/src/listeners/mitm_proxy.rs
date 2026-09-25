//! HTTPS MITM listener (port 3129).
//!
//! CONNECT → rustls server with on-the-fly leaf → HTTP cache path.
//! Excluded hosts are spliced (raw tunnel). Upgrade/WebSocket bypass cache.

use std::sync::Arc;
use std::time::Instant;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use rustcache_core::certs::leaf::LeafIssuer;
use rustcache_core::http::fetch::parse_url;
use rustcache_core::stats::ring::ReqRecord;

use crate::engine::{upstream_tls_connector, Lookup, SharedEngine};
use crate::listeners::http_proxy::{read_http_request, write_response_to, HttpRequest};

pub struct MitmState {
    pub engine: SharedEngine,
    pub leaves: Arc<LeafIssuer>,
    pub denylist: Arc<dashmap::DashMap<String, u64>>,
}

pub async fn serve(
    addr: std::net::SocketAddr,
    engine: SharedEngine,
    leaves: Arc<LeafIssuer>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "https mitm listening");
    let denylist = Arc::new(dashmap::DashMap::new());
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = MitmState {
            engine: engine.clone(),
            leaves: leaves.clone(),
            denylist: denylist.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, state).await {
                tracing::debug!(%peer, error = %e, "mitm connection error");
            }
        });
    }
}

async fn handle_conn(mut client: TcpStream, state: MitmState) -> anyhow::Result<()> {
    let _ = client.set_nodelay(true);
    let req = match read_http_request(&mut client).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    if !req.method.eq_ignore_ascii_case("CONNECT") {
        // Some clients send absolute-URI https requests to the https proxy port.
        return handle_plain_http(&mut client, &req, state).await;
    }
    let authority = req.target.clone();
    let host = authority
        .split(':')
        .next()
        .unwrap_or(&authority)
        .to_string();

    if state.denylist.contains_key(&host) {
        tracing::debug!(%host, "mitm denylist → splice");
        return splice_raw(&mut client, &authority, &state.engine).await;
    }

    // Excluded host → raw splice (no MITM)
    if state
        .engine
        .is_excluded_url(&format!("https://{authority}/"))
        .await
    {
        state.engine.metrics().add_bypass();
        return splice_raw(&mut client, &authority, &state.engine).await;
    }

    // Accept CONNECT and perform TLS MITM
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    let leaf = match state.leaves.issue(&host) {
        Ok(l) => l,
        Err(e) => {
            state.engine.metrics().add_error();
            tracing::warn!(%host, error = %e, "leaf issue failed");
            state.denylist.insert(host.clone(), 0);
            return Ok(());
        }
    };

    let cert_chain = vec![pem_to_der_cert(&leaf.cert_pem)?];
    let key = pem_to_der_key(&leaf.key_pem)?;
    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| anyhow::anyhow!("tls server config: {e}"))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];

    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let mut tls_stream = match acceptor.accept(client).await {
        Ok(s) => s,
        Err(e) => {
            state.engine.metrics().add_error();
            state.denylist.insert(host.clone(), 0);
            tracing::debug!(%host, error = %e, "mitm tls accept failed");
            return Ok(());
        }
    };

    // Serve one HTTP/1.1 request over the MITM stream (Connection: close).
    let req = match read_http_request_tls(&mut tls_stream).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    if req.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("upgrade")
            || (k.eq_ignore_ascii_case("connection") && v.to_ascii_lowercase().contains("upgrade"))
    }) {
        // WebSocket / upgrade → bypass cache, pipe to origin
        pipe_upgrade(&mut tls_stream, &req, &state).await?;
        return Ok(());
    }
    let url = format!("https://{authority}{}", path_of(&req.target));
    handle_mitm_request(&mut tls_stream, &req, &url, &host, &state).await?;
    Ok(())
}

fn path_of(target: &str) -> String {
    if target.starts_with('/') {
        target.to_string()
    } else {
        format!("/{target}")
    }
}

async fn handle_plain_http(
    client: &mut TcpStream,
    req: &HttpRequest,
    state: MitmState,
) -> anyhow::Result<()> {
    // Treat as absolute-URI style if possible.
    let url = if req.target.contains("://") {
        req.target.clone()
    } else {
        format!("https://unknown{}", path_of(&req.target))
    };
    let host = parse_url(&url).map(|u| u.host).unwrap_or_default();
    handle_mitm_request(client, req, &url, &host, &state).await
}

async fn handle_mitm_request<W: AsyncWriteExt + Unpin + tokio::io::AsyncRead>(
    stream: &mut W,
    req: &HttpRequest,
    url: &str,
    host: &str,
    state: &MitmState,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let engine = &state.engine;
    let method = req.method.to_uppercase();
    let is_get_head = method == "GET" || method == "HEAD";

    let outcome;
    let status: u16;
    let resp_len: u64;

    if !is_get_head || engine.is_excluded_url(url).await {
        engine.metrics().add_bypass();
        outcome = "BYPASS";
        let tls = upstream_tls_connector();
        match engine
            .fetcher
            .fetch(
                &method,
                url,
                &req.headers,
                if req.body.is_empty() {
                    None
                } else {
                    Some(req.body.as_slice())
                },
                Some(tls),
                engine.max_object_bytes,
            )
            .await
        {
            Ok(r) => {
                write_response_to(stream, r.status, &r.headers, &r.body).await?;
                status = r.status;
                resp_len = r.body.len() as u64;
                engine.metrics().add_served(resp_len);
            }
            Err(e) => {
                engine.metrics().add_error();
                write_response_to(stream, 502, &[], e.to_string().as_bytes()).await?;
                status = 502;
                resp_len = 0;
            }
        }
    } else {
        let lookup = engine.lookup(url).await;
        let tls = upstream_tls_connector();
        match lookup {
            Lookup::Hit(entry) => {
                engine.metrics().add_hit(entry.body.len() as u64);
                write_response_to(stream, entry.meta.status, &entry.meta.headers, &entry.body)
                    .await?;
                status = entry.meta.status;
                resp_len = entry.body.len() as u64;
                outcome = "HIT";
                engine.metrics().add_served(resp_len);
            }
            Lookup::Revalidate { entry } => {
                match engine
                    .fetch_and_store(&method, url, &req.headers, None, Some(tls), Some(&entry))
                    .await
                {
                    Ok(new_entry) => {
                        if new_entry.body == entry.body {
                            engine.metrics().add_hit(new_entry.body.len() as u64);
                            outcome = "HIT_REVALIDATED";
                        } else {
                            engine.metrics().add_miss();
                            outcome = "REVALIDATED";
                        }
                        write_response_to(
                            stream,
                            new_entry.meta.status,
                            &new_entry.meta.headers,
                            &new_entry.body,
                        )
                        .await?;
                        status = new_entry.meta.status;
                        resp_len = new_entry.body.len() as u64;
                        engine.metrics().add_served(resp_len);
                    }
                    Err(e) => {
                        engine.metrics().add_error();
                        write_response_to(stream, 502, &[], e.to_string().as_bytes()).await?;
                        status = 502;
                        resp_len = 0;
                        outcome = "ERROR";
                    }
                }
            }
            Lookup::Miss => {
                match engine
                    .fetch_and_store(&method, url, &req.headers, None, Some(tls), None)
                    .await
                {
                    Ok(entry) => {
                        engine.metrics().add_miss();
                        write_response_to(
                            stream,
                            entry.meta.status,
                            &entry.meta.headers,
                            &entry.body,
                        )
                        .await?;
                        status = entry.meta.status;
                        resp_len = entry.body.len() as u64;
                        outcome = "MISS";
                        engine.metrics().add_served(resp_len);
                    }
                    Err(e) => {
                        engine.metrics().add_error();
                        write_response_to(stream, 502, &[], e.to_string().as_bytes()).await?;
                        status = 502;
                        resp_len = 0;
                        outcome = "ERROR";
                    }
                }
            }
        }
    }

    engine.record(ReqRecord {
        ts: rustcache_core::cache::meta::now_ms(),
        method,
        url: url.into(),
        host: host.into(),
        status,
        outcome: outcome.into(),
        duration_ms: started.elapsed().as_millis() as u64,
        resp_bytes: resp_len,
    });
    Ok(())
}

async fn splice_raw(
    client: &mut TcpStream,
    authority: &str,
    engine: &SharedEngine,
) -> anyhow::Result<()> {
    let mut upstream = TcpStream::connect(authority).await?;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    let (a, b) = tokio::io::copy_bidirectional(client, &mut upstream).await?;
    engine.metrics().add_tunnel();
    engine.metrics().add_served(a + b);
    Ok(())
}

async fn pipe_upgrade<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    client: &mut S,
    req: &HttpRequest,
    state: &MitmState,
) -> anyhow::Result<()> {
    // Connect to origin over TLS and forward the raw request, then pipe.
    let url = if req.target.contains("://") {
        req.target.clone()
    } else {
        "https://example.com/".into()
    };
    let parsed = parse_url(&url)?;
    let authority = match parsed.port {
        Some(p) => format!("{}:{}", parsed.host, p),
        None => format!("{}:443", parsed.host),
    };
    let tcp = TcpStream::connect(&authority).await?;
    let connector = upstream_tls_connector();
    let domain = rustls::pki_types::ServerName::try_from(parsed.host.clone())
        .map_err(|_| anyhow::anyhow!("bad sni"))?;
    let mut upstream = connector.connect(domain, tcp).await?;

    // Re-serialize the request line as origin-form.
    let mut head = format!("{} {} HTTP/1.1\r\n", req.method, path_of(&req.target));
    for (k, v) in &req.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    upstream.write_all(head.as_bytes()).await?;
    if !req.body.is_empty() {
        upstream.write_all(&req.body).await?;
    }
    upstream.flush().await?;
    let _ = tokio::io::copy_bidirectional(client, &mut upstream).await?;
    state.engine.metrics().add_bypass();
    Ok(())
}

async fn read_http_request_tls<S: AsyncReadExt + Unpin>(
    stream: &mut S,
) -> anyhow::Result<Option<HttpRequest>> {
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
    }
    let body = rest.into_iter().take(content_length).collect();
    Ok(Some(HttpRequest {
        method,
        target,
        headers,
        body,
    }))
}

fn pem_to_der_cert(pem: &str) -> anyhow::Result<CertificateDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    let certs: Vec<CertificateDer> =
        rustls_pemfile::certs(&mut reader).collect::<Result<_, _>>()?;
    certs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no cert in pem"))
}

fn pem_to_der_key(pem: &str) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    let key = rustls_pemfile::private_key(&mut reader)?
        .ok_or_else(|| anyhow::anyhow!("no private key in pem"))?;
    Ok(key)
}
