//! Local resilient HTTP proxy: accepts client traffic, routes to RustCache or DIRECT.

mod http_parse;
mod relay;
mod route;

pub use http_parse::{HttpHead, parse_head};
pub use route::{Route, RouteDecider};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::{TcpListener, TcpStream};

use crate::api::SharedUpstream;
use crate::health::HealthMonitor;
use relay::Hop;

/// Deadline for reading the first request head.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ProxyServer {
    listen: String,
    upstream: SharedUpstream,
    health: Arc<HealthMonitor>,
    bypass: Vec<String>,
}

impl ProxyServer {
    pub fn new(
        listen: &str,
        upstream: SharedUpstream,
        health: Arc<HealthMonitor>,
        bypass: Vec<String>,
    ) -> Self {
        Self {
            listen: listen.to_string(),
            upstream,
            health,
            bypass,
        }
    }

    fn target(&self) -> (String, u16, u16) {
        let u = self.upstream.read();
        (u.host.clone(), u.http_port, u.https_port)
    }

    /// Bind and serve until the process exits.
    pub async fn run(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(&self.listen)
            .await
            .with_context(|| format!("bind {}", self.listen))?;
        let local = listener.local_addr()?;
        tracing::info!(%local, "client proxy listening");
        self.serve_listener(listener).await
    }

    /// Serve on an already-bound listener (tests).
    pub async fn serve_listener(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        loop {
            let (stream, peer) = listener.accept().await?;
            let me = self.clone();
            tokio::spawn(async move {
                if let Err(e) = me.handle_conn(stream).await {
                    tracing::debug!(%peer, error = %e, "proxy conn error");
                }
            });
        }
    }

    async fn handle_conn(&self, stream: TcpStream) -> Result<()> {
        let _ = stream.set_nodelay(true);
        let (head, stream) = tokio::time::timeout(HEAD_TIMEOUT, parse_head(stream))
            .await
            .context("head timeout")??;

        if head.method.eq_ignore_ascii_case("CONNECT") {
            self.handle_connect(stream, &head).await
        } else {
            self.handle_http(stream, &head).await
        }
    }

    async fn handle_connect(&self, client: TcpStream, head: &HttpHead) -> Result<()> {
        let target = head.target.clone();
        let route = RouteDecider::new(&self.health, &self.bypass, self.health.use_upstream())
            .decide_connect(&target);

        tracing::debug!(%target, ?route, "CONNECT");

        // Per-connection fail-open: if the chosen RustCache hop is unreachable,
        // fall back to DIRECT before anything is written to the client.
        let (host, http_port, https_port) = self.target();
        match route {
            Route::Mitm => match relay::try_upstream(&host, https_port).await {
                Hop::Upstream(up) => {
                    self.health.record_success();
                    relay::connect_via_connected(client, up, &target).await
                }
                Hop::Down => {
                    self.health.record_failure("mitm hop down");
                    relay::connect_direct(client, &target).await
                }
            },
            Route::RawTunnel => match relay::try_upstream(&host, http_port).await {
                Hop::Upstream(up) => {
                    self.health.record_success();
                    relay::connect_via_connected(client, up, &target).await
                }
                Hop::Down => {
                    self.health.record_failure("raw hop down");
                    relay::connect_direct(client, &target).await
                }
            },
            Route::Direct => relay::connect_direct(client, &target).await,
        }
    }

    async fn handle_http(&self, client: TcpStream, head: &HttpHead) -> Result<()> {
        let route = RouteDecider::new(&self.health, &self.bypass, self.health.use_http_upstream())
            .decide_http(&head.target, head.host.as_deref());
        tracing::debug!(target = %head.target, ?route, "HTTP");

        match route {
            Route::Mitm | Route::RawTunnel => {
                let (host, http_port, _https) = self.target();
                match relay::try_upstream(&host, http_port).await {
                    Hop::Upstream(up) => {
                        self.health.record_success();
                        relay::http_via_connected(client, up, head).await
                    }
                    Hop::Down => {
                        self.health.record_failure("http hop down");
                        relay::http_direct(client, head).await
                    }
                }
            }
            Route::Direct => relay::http_direct(client, head).await,
        }
    }
}

/// Bound address helper for tests.
pub async fn bind_local() -> Result<(TcpListener, SocketAddr)> {
    let l = TcpListener::bind("127.0.0.1:0").await?;
    let a = l.local_addr()?;
    Ok((l, a))
}

/// Fixed upstream for tests / CLI when discovery is not used.
pub fn static_upstream(host: &str, http_port: u16, https_port: u16) -> SharedUpstream {
    crate::api::shared_upstream(crate::api::UpstreamCfg {
        api_base: format!("http://{host}:8080"),
        host: host.to_string(),
        http_port,
        https_port,
        socks_port: 1080,
    })
}
