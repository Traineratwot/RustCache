//! HTTPS MITM listener (port 3129).
//!
//! CONNECT → rustls server with on-the-fly leaf → shared HTTP cache path.
//! Excluded hosts are spliced (raw tunnel). Upgrade/WebSocket bypass cache.

use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use rustcache_core::certs::leaf::LeafIssuer;
use rustcache_core::http::fetch::parse_url;

use rustcache_core::cache::meta::now_ms;

use crate::engine::{SharedEngine, upstream_tls_connector};
use crate::listeners::serve::{RequestContext, resolve_cached};
use crate::listeners::wire::{HttpRequest, host_of, write_http_response};
use crate::listeners::{
    CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, connect_with_timeout, read_request_with_timeout,
};

/// How long a host stays on the MITM denylist after a failure.
///
/// The entry used to be permanent: one transient TLS error (origin hiccup,
/// client that aborts the handshake, a pinned app) disabled MITM — and so all
/// caching — for that host until the process restarted.
const DENYLIST_TTL_MS: u64 = 5 * 60 * 1000;

pub struct MitmState {
    pub engine: SharedEngine,
    pub leaves: Arc<LeafIssuer>,
    /// host → unix millis when it was denylisted.
    pub denylist: Arc<dashmap::DashMap<String, u64>>,
    /// host → ready-to-use rustls config. Building one means parsing the leaf
    /// PEM and validating the key, which is far too expensive to redo on every
    /// CONNECT (the MITM path serves one request per TLS connection).
    pub tls_configs: Arc<dashmap::DashMap<String, Arc<rustls::ServerConfig>>>,
}

/// Bounded like the leaf cache: drop everything when full rather than track LRU.
const MAX_CACHED_TLS_CONFIGS: usize = 512;

impl MitmState {
    /// True when `host` is currently denylisted; expired entries are dropped.
    fn is_denylisted(&self, host: &str) -> bool {
        let Some(since) = self.denylist.get(host).map(|e| *e.value()) else {
            return false;
        };
        if now_ms().saturating_sub(since) < DENYLIST_TTL_MS {
            return true;
        }
        self.denylist.remove(host);
        false
    }

    fn denylist(&self, host: &str) {
        self.denylist.insert(host.to_string(), now_ms());
    }

    /// Cached rustls config for `host`, built from a freshly issued leaf on miss.
    fn tls_config(&self, host: &str) -> anyhow::Result<Arc<rustls::ServerConfig>> {
        if let Some(hit) = self.tls_configs.get(host) {
            return Ok(hit.value().clone());
        }
        let leaf = self.leaves.issue(host)?;
        let cert_chain = vec![pem_to_der_cert(&leaf.cert_pem)?];
        let key = pem_to_der_key(&leaf.key_pem)?;
        let mut cfg = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, key)
            .map_err(|e| anyhow::anyhow!("tls server config: {e}"))?;
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let cfg = Arc::new(cfg);
        if self.tls_configs.len() >= MAX_CACHED_TLS_CONFIGS {
            self.tls_configs.clear();
        }
        self.tls_configs.insert(host.to_string(), cfg.clone());
        Ok(cfg)
    }
}

/// Accept loop on a pre-bound listener (bind happens in `main` so failures are visible).
pub async fn serve(
    listener: TcpListener,
    engine: SharedEngine,
    leaves: Arc<LeafIssuer>,
) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "https mitm listening");
    let denylist = Arc::new(dashmap::DashMap::new());
    let tls_configs = Arc::new(dashmap::DashMap::new());
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = MitmState {
            engine: engine.clone(),
            leaves: leaves.clone(),
            denylist: denylist.clone(),
            tls_configs: tls_configs.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, state).await {
                tracing::debug!(%peer, error = %e, "mitm connection error");
            }
        });
    }
}

/// Handle a single accepted connection (exported for integration tests).
pub async fn serve_connection(
    stream: TcpStream,
    engine: SharedEngine,
    leaves: Arc<LeafIssuer>,
) -> anyhow::Result<()> {
    let state = MitmState {
        engine,
        leaves,
        denylist: Arc::new(dashmap::DashMap::new()),
        tls_configs: Arc::new(dashmap::DashMap::new()),
    };
    handle_conn(stream, state).await
}

async fn handle_conn(mut client: TcpStream, state: MitmState) -> anyhow::Result<()> {
    let _ = client.set_nodelay(true);
    let req = match read_request_with_timeout(&mut client).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    if !req.method.eq_ignore_ascii_case("CONNECT") {
        // Some clients send absolute-URI https requests to the https proxy port.
        return handle_plain_http(&mut client, req, state).await;
    }
    let authority = req.target.clone();
    let host = host_of(&authority).to_string();

    if state.is_denylisted(&host) {
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

    let cfg = match state.tls_config(&host) {
        Ok(c) => c,
        Err(e) => {
            state.engine.metrics().add_error();
            tracing::warn!(%host, error = %e, "leaf issue failed");
            state.denylist(&host);
            return Ok(());
        }
    };

    let acceptor = tokio_rustls::TlsAcceptor::from(cfg);
    let accept = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(client));
    let mut tls_stream = match accept.await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            state.engine.metrics().add_error();
            state.denylist(&host);
            tracing::debug!(%host, error = %e, "mitm tls accept failed");
            return Ok(());
        }
        Err(_) => {
            state.engine.metrics().add_error();
            tracing::debug!(%host, "mitm tls handshake timed out");
            return Ok(());
        }
    };

    // Serve one HTTP/1.1 request over the MITM stream (Connection: close).
    let req = match read_request_with_timeout(&mut tls_stream).await? {
        Some(r) => r,
        None => return Ok(()),
    };
    if req.headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("upgrade")
            || (k.eq_ignore_ascii_case("connection") && v.to_ascii_lowercase().contains("upgrade"))
    }) {
        // WebSocket / upgrade → bypass cache, pipe to origin using CONNECT authority
        pipe_upgrade(&mut tls_stream, &req, &state, &authority).await?;
        return Ok(());
    }
    let url = format!("https://{authority}{}", path_of(&req.target));
    handle_mitm_request(&mut tls_stream, req, url, host, &state).await
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
    req: HttpRequest,
    state: MitmState,
) -> anyhow::Result<()> {
    // Treat as absolute-URI style if possible.
    let url = if req.target.contains("://") {
        req.target.clone()
    } else {
        format!("https://unknown{}", path_of(&req.target))
    };
    let host = parse_url(&url).map(|u| u.host).unwrap_or_default();
    handle_mitm_request(client, req, url, host, &state).await
}

async fn handle_mitm_request<W: AsyncWriteExt + Unpin>(
    stream: &mut W,
    req: HttpRequest,
    url: String,
    host: String,
    state: &MitmState,
) -> anyhow::Result<()> {
    let ctx = RequestContext {
        method: req.method.to_uppercase(),
        url,
        host,
        headers: req.headers,
        body: req.body,
        started: std::time::Instant::now(),
    };
    let out = resolve_cached(&state.engine, &ctx, Some(upstream_tls_connector())).await?;
    write_http_response(stream, out.status, &out.headers, &out.body, true).await
}

async fn splice_raw(
    client: &mut TcpStream,
    authority: &str,
    engine: &SharedEngine,
) -> anyhow::Result<()> {
    let mut upstream = connect_with_timeout(authority, CONNECT_TIMEOUT).await?;
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
    authority: &str,
) -> anyhow::Result<()> {
    // Connect to the CONNECT authority over TLS and pipe the upgraded connection.
    let host = host_of(authority).to_string();
    let tcp = connect_with_timeout(authority, CONNECT_TIMEOUT).await?;
    let connector = upstream_tls_connector();
    let domain =
        rustls::pki_types::ServerName::try_from(host).map_err(|_| anyhow::anyhow!("bad sni"))?;
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
