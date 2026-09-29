//! Proxy listeners plus the I/O deadlines they share.
//!
//! Every listener accepts from the network, so each blocking read is a slot an
//! idle or hostile peer can hold. Without these deadlines a client that opens a
//! socket and never speaks pins a task (and its buffers) for the process
//! lifetime, and an unreachable origin does the same on the upstream side.

use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

use wire::{HttpRequest, read_http_request};

pub mod http_proxy;
pub mod mitm_proxy;
pub mod serve;
pub mod socks5;
pub mod wire;

/// Deadline for reading a complete request head (or a SOCKS5 handshake) from a
/// client that has already connected.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for opening a TCP connection to an origin or CONNECT target.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// [`read_http_request`] under [`HANDSHAKE_TIMEOUT`].
pub async fn read_request_with_timeout<S: AsyncReadExt + Unpin>(
    stream: &mut S,
) -> anyhow::Result<Option<HttpRequest>> {
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, read_http_request(stream)).await {
        Ok(r) => r,
        Err(_) => Err(anyhow::anyhow!("timed out reading request head")),
    }
}

/// [`TcpStream::connect`] under an explicit deadline.
pub async fn connect_with_timeout(addr: &str, timeout: Duration) -> anyhow::Result<TcpStream> {
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(r) => Ok(r?),
        Err(_) => Err(anyhow::anyhow!("connect timeout to {addr}")),
    }
}
