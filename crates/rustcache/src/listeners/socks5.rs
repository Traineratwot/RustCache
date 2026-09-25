//! SOCKS5 listener: no-auth CONNECT only (UDP/Bind rejected).

use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::engine::SharedEngine;
use rustcache_core::stats::ReqRecord;

const VER: u8 = 0x05;
const METHOD_NO_AUTH: u8 = 0x00;
const METHOD_NO_ACCEPTABLE: u8 = 0xFF;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

pub async fn serve(addr: std::net::SocketAddr, engine: SharedEngine) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
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

async fn handle(mut client: TcpStream, engine: SharedEngine) -> anyhow::Result<()> {
    let started = Instant::now();
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
        return Ok(());
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
            outcome: "REJECT_CMD".into(),
            duration_ms: started.elapsed().as_millis() as u64,
            resp_bytes: 0,
        });
        return Ok(());
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
                    return Ok(());
                }
            }
        }
        ATYP_IPV6 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip).await?;
            (
                std::net::IpAddr::V6(ip.into()),
                format!("{:?}", std::net::Ipv6Addr::from(ip)),
            )
        }
        _ => {
            client
                .write_all(&[VER, 0x08, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
                .await?;
            return Ok(());
        }
    };
    let mut portb = [0u8; 2];
    client.read_exact(&mut portb).await?;
    let port = u16::from_be_bytes(portb);
    let target = std::net::SocketAddr::new(host, port);

    match TcpStream::connect(target).await {
        Ok(mut upstream) => {
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
                url: target.to_string(),
                host: host_label,
                status: 0,
                outcome: "TUNNEL".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: a + b,
            });
            Ok(())
        }
        Err(_e) => {
            engine.metrics().add_error();
            client
                .write_all(&[VER, 0x05, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
                .await?;
            engine.record(ReqRecord {
                ts: rustcache_core::cache::meta::now_ms(),
                method: "SOCKS5".into(),
                url: target.to_string(),
                host: host_label,
                status: 0,
                outcome: "ERROR".into(),
                duration_ms: started.elapsed().as_millis() as u64,
                resp_bytes: 0,
            });
            Ok(())
        }
    }
}
