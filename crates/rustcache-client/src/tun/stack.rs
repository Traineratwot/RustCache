//! TUN capture stack — thin wrapper around `tun2proxy`.
//!
//! Creates the TUN device, installs capture routes via `tproxy-config`, and
//! forwards all TCP/UDP through our local resilient HTTP proxy
//! (`host:port` of the client proxy), which in turn talks to RustCache or DIRECT.
//!
//! Replaces the hand-rolled smoltcp stack: no manual listeners on :80/:443,
//! no IP rewriting — tun2proxy/ipstack speaks HTTP CONNECT and absolute-form
//! GET to the proxy for every flow. DNS is handled by tun2proxy virtual DNS
//! (fake-IP 198.18.0.0/15), so no resolv.conf / host-route juggling.

use anyhow::{Context, Result};
use tun2proxy::{ArgDns, Args, CancellationToken, LevelFilter, ProxyParameters};

/// TUN interface name (stable — doctor / dirty-recovery depend on it).
pub const TUN_IFACE: &str = "rustcache-tun0";

/// How long to wait for the TUN iface before declaring setup failure.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

type ReadyTx = std::sync::mpsc::Sender<std::result::Result<(), String>>;

/// Run the TUN stack until the process is killed or a signal arrives.
///
/// `local_proxy` is `host:port` of the resilient client proxy. When `ready`
/// is present it receives `Ok(())` once the TUN device exists, or `Err(msg)`
/// if setup failed — so [`super::enable`] never reports success for a device
/// that was never created.
///
/// Runs on a dedicated OS thread: `general_run_async` needs a tokio runtime,
/// and this fn may be called from inside another runtime (CLI `execute` is
/// driven by `main`'s `rt.block_on`) — nesting `Runtime::block_on` panics.
pub fn event_loop(local_proxy: &str, ready: Option<ReadyTx>) -> Result<()> {
    let proxy = local_proxy.to_string();
    let handle = std::thread::Builder::new()
        .name("tun2proxy".into())
        .spawn(move || event_loop_inner(&proxy, ready))
        .context("spawn tun2proxy thread")?;
    match handle.join() {
        Ok(res) => res,
        Err(_) => Err(anyhow::anyhow!("tun2proxy thread panicked")),
    }
}

fn event_loop_inner(local_proxy: &str, ready: Option<ReadyTx>) -> Result<()> {
    let args = build_args(local_proxy).map_err(|e| {
        if let Some(tx) = &ready {
            let _ = tx.send(Err(format!("{e:#}")));
        }
        e
    })?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")?;

    let shutdown = CancellationToken::new();

    // Cancel on SIGINT/SIGTERM so tproxy-config can restore routes on exit.
    {
        let shutdown = shutdown.clone();
        rt.spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut term) = signal(SignalKind::terminate()) else {
                return;
            };
            let Ok(mut int) = signal(SignalKind::interrupt()) else {
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
            shutdown.cancel();
        });
    }

    // Device creation + route setup happen inside general_run_async (needs
    // CAP_NET_ADMIN). Poll for the iface and signal readiness from a watcher
    // so the parent is not blocked on the full stack lifetime.
    if let Some(tx) = ready {
        rt.spawn(watch_iface(tx));
    }

    let res = rt.block_on(tun2proxy::general_run_async(
        args,
        tun2proxy::DEFAULT_MTU,
        false,
        shutdown,
    ));

    match res {
        Ok(sessions) => {
            tracing::info!(sessions, "tun2proxy exited cleanly");
            Ok(())
        }
        Err(e) => {
            tracing::error!(error = %e, "tun2proxy failed");
            Err(anyhow::anyhow!("tun stack: {e:#}"))
        }
    }
}

/// Async variant: same as [`event_loop`] but joins the tokio runtime of the
/// caller. Used by the in-process path (we already have CAP_NET_ADMIN).
pub async fn run(local_proxy: &str, ready: ReadyTx) -> Result<()> {
    let args = build_args(local_proxy)?;

    let shutdown = CancellationToken::new();
    let shutdown_bg = shutdown.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return;
        };
        let Ok(mut int) = signal(SignalKind::interrupt()) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        shutdown_bg.cancel();
    });

    tokio::spawn(watch_iface(ready));

    match tun2proxy::general_run_async(args, tun2proxy::DEFAULT_MTU, false, shutdown).await {
        Ok(sessions) => {
            tracing::info!(sessions, "tun2proxy exited cleanly");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("tun stack: {e:#}")),
    }
}

/// Poll until the TUN iface exists (or timeout) and report to `tx`.
async fn watch_iface(tx: ReadyTx) {
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    loop {
        if std::path::Path::new("/sys/class/net/")
            .join(TUN_IFACE)
            .exists()
        {
            let _ = tx.send(Ok(()));
            return;
        }
        if std::time::Instant::now() >= deadline {
            let _ = tx.send(Err(format!(
                "TUN device {TUN_IFACE} did not appear within {:?} (need root / CAP_NET_ADMIN)",
                READY_TIMEOUT
            )));
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

fn build_args(local_proxy: &str) -> Result<Args> {
    let proxy_url = format!("http://{local_proxy}");
    let proxy = ProxyParameters::try_from(proxy_url.as_str())
        .map_err(|e| anyhow::anyhow!("proxy url `{proxy_url}`: {e}"))?;

    Ok(Args {
        proxy,
        tun: Some(TUN_IFACE.to_string()),
        setup: true,
        dns: ArgDns::Virtual,
        // virtual_dns_pool defaults to 198.18.0.0/15 (Args::default)
        verbosity: LevelFilter::Info,
        ..Default::default()
    })
}
