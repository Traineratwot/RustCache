//! SOCKS5 listener: no-auth CONNECT only (UDP/Bind rejected).

use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::engine::SharedEngine;
use crate::listeners::{CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT};
use rustcache_core::stats::{Outcome, ReqRecord};

const VER: u8 = 0x05;
const METHOD_NO_AUTH: u8 = 0x00;
const METHOD_NO_ACCEPTABLE: u8 = 0xFF;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

/// Accept loop on a pre-bound listener (bind happens in `main` so failures are visible).
pub async fn serve(listener: TcpListener, engine: SharedEngine) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "socks5 listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        let engine = engine.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, engine).await {
                tracing::debug!(%peer, error = %e, "socks5 error");
            }
        });
    }
}

/// Handle a single accepted connection (exported for integration tests).
pub async fn serve_connection(stream: TcpStream, engine: SharedEngine) -> anyhow::Result<()> {
    handle(stream, engine).await
}

/// A negotiated CONNECT request: where to dial and how to label it in the log.
struct Socks5Target {
    addr: std::net::SocketAddr,
    label: String,
}

async fn handle(mut client: TcpStream, engine: SharedEngine) -> anyhow::Result<()> {
    let started = Instant::now();

    // Only the negotiation is time-boxed. The tunnel that follows is
    // deliberately unbounded — long-lived connections are the whole point.
    let negotiated =
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, negotiate(&mut client, &engine, started))
            .await
        {
            Ok(r) => r?,
            Err(_) => return Err(anyhow::anyhow!("socks5 handshake timed out")),
        };
    // Negotiation already answered the client (rejection / resolve failure).
    let Some(target) = negotiated else {
        return Ok(());
    };

    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target.addr)).await {
        Ok(Ok(mut upstream)) => {
            // success reply
            let mut reply = vec![VER, 0x00, 0x00, ATYP_IPV4];
            reply.extend_from_slice(&[0, 0, 0, 0]);
            reply.extend_from_slice(&0u16.to_be_bytes());
            client.write_all(&reply).await?;
            engine.metrics().add_tunnel();
            let (a, b) = tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
            engine.metrics().add_served(a + b);
            engine.record(ReqRecord {
                ts: rustcache_core::cache::meta::now_ms(),
                method: "SOCKS5".into(),
                url: target.addr.to_string(),
                host: target.label,
                status: 0,
                outcome: Outcome::Tunnel,
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: a + b,
            });
            Ok(())
        }
        failed => {
            // 0x04 host unreachable on timeout, 0x05 connection refused otherwise.
            let reply_code = if failed.is_err() { 0x04 } else { 0x05 };
            engine.metrics().add_error();
            client
                .write_all(&[VER, reply_code, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
                .await?;
            engine.record(ReqRecord {
                ts: rustcache_core::cache::meta::now_ms(),
                method: "SOCKS5".into(),
                url: target.addr.to_string(),
                host: target.label,
                status: 0,
                outcome: Outcome::Error,
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: 0,
            });
            Ok(())
        }
    }
}

/// Read greeting + CONNECT request.
///
/// `Ok(None)` means the client was already answered with a SOCKS5 error reply
/// (unsupported command, unknown address type, DNS failure) and the caller
/// should just close.
async fn negotiate(
    client: &mut TcpStream,
    engine: &SharedEngine,
    started: Instant,
) -> anyhow::Result<Option<Socks5Target>> {
    // greeting
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    if head[0] != VER {
        return Err(anyhow::anyhow!("bad socks version {}", head[0]));
    }
    let nmethods = head[1] as usize;
    let mut methods = vec![0u8; nmethods];
    if nmethods > 0 {
        client.read_exact(&mut methods).await?;
    }
    if !methods.contains(&METHOD_NO_AUTH) {
        client.write_all(&[VER, METHOD_NO_ACCEPTABLE]).await?;
        return Ok(None);
    }
    client.write_all(&[VER, METHOD_NO_AUTH]).await?;

    // request
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    let (ver, cmd, _rsv, atyp) = (req[0], req[1], req[2], req[3]);
    if ver != VER {
        return Err(anyhow::anyhow!("bad request ver"));
    }
    if cmd != CMD_CONNECT {
        // reject UDP/Bind
        client
            .write_all(&[VER, 0x07, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
            .await?;
        engine.metrics().add_error();
        engine.record(ReqRecord {
            ts: rustcache_core::cache::meta::now_ms(),
            method: "SOCKS5".into(),
            url: String::new(),
            host: String::new(),
            status: 0,
            outcome: Outcome::RejectCmd,
            duration_ms: started.elapsed().as_millis() as u64,
            resp_bytes: 0,
        });
        return Ok(None);
    }

    let (host, host_label) = match atyp {
        ATYP_IPV4 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip).await?;
            (
                std::net::IpAddr::V4(ip.into()),
                ip.map(|b| b.to_string()).join("."),
            )
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            client.read_exact(&mut name).await?;
            let name = String::from_utf8_lossy(&name).to_string();
            let ip = tokio::net::lookup_host((name.as_str(), 0))
                .await
                .ok()
                .and_then(|mut it| it.next())
                .map(|a| a.ip());
            match ip {
                Some(ip) => (ip, name),
                None => {
                    client
                        .write_all(&[VER, 0x04, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
                        .await?;
                    return Ok(None);
                }
            }
        }
        ATYP_IPV6 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            (
                std::net::IpAddr::V6(ip.into()),
                std::net::Ipv6Addr::from(ip).to_string(),
            )
        }
        _ => {
            client
                .write_all(&[VER, 0x08, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(None);
        }
    };
    let mut portb = [0u8; 2];
    client.read_exact(&mut portb).await?;
    let port = u16::from_be_bytes(portb);
    Ok(Some(Socks5Target {
        addr: std::net::SocketAddr::new(host, port),
        label: host_label,
    }))
}
