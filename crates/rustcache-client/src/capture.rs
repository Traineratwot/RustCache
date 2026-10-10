//! Capture orchestration: system proxy on/off + dirty recovery.

#[cfg(feature = "tun")]
use anyhow::Context as _;
use anyhow::Result;
#[cfg(feature = "tun")]
use parking_lot::Mutex;

use crate::config::{CaptureMode, ClientConfig};
use crate::state::StateStore;

pub struct Capture {
    store: StateStore,
    /// Keeps the TUN stack (task or privileged helper) alive while capture is on.
    #[cfg(feature = "tun")]
    tun: Mutex<Option<crate::tun::privs::StackHandle>>,
}

impl Capture {
    pub fn new(store: StateStore) -> Self {
        Self {
            store,
            #[cfg(feature = "tun")]
            tun: Mutex::new(None),
        }
    }

    pub fn default_store() -> Result<Self> {
        Ok(Self::new(StateStore::default_store()?))
    }

    /// Restore leftover system-proxy changes from a previous crash.
    pub fn recover(&self) -> Result<Option<String>> {
        crate::sysproxy::recover_if_dirty(&self.store)
    }

    /// Turn capture on for the configured mode.
    pub fn enable(&self, cfg: &ClientConfig, mode: CaptureMode) -> Result<String> {
        match mode {
            CaptureMode::Off => self.disable(),
            CaptureMode::System => {
                let info = crate::platform::detect();
                crate::platform::require_sysproxy(&info)?;
                let (host, port) = cfg.listen_addr()?;
                crate::sysproxy::enable(&host, port, &cfg.proxy_bypass, &self.store)
            }
            CaptureMode::Tun => {
                #[cfg(feature = "tun")]
                {
                    let (host, port) = cfg.listen_addr()?;
                    let local_proxy = format!("{host}:{port}");
                    // block_in_place lets us run async TUN setup from this sync fn
                    // without deadlocking the multi-threaded runtime.
                    let result = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(crate::tun::enable(&local_proxy))
                    });
                    match result {
                        Ok(session) => {
                            let tun_json = session.snapshot.to_json();
                            self.store
                                .mark_dirty_parts(serde_json::json!({}), Some(tun_json))?;
                            *self.tun.lock() = Some(session.stack);
                            Ok(format!(
                                "tun active on {} ({})",
                                session.snapshot.iface, session.snapshot.addr
                            ))
                        }
                        Err(e) => {
                            // Surface the real error — do NOT pretend TUN works.
                            // (System-proxy fallback stays available via mode=System.)
                            Err(e).context("TUN capture failed")
                        }
                    }
                }
                #[cfg(not(feature = "tun"))]
                {
                    let _ = cfg;
                    anyhow::bail!("TUN is not compiled in (build with --features tun)")
                }
            }
        }
    }

    pub fn disable(&self) -> Result<String> {
        // Stop TUN first (if active), then system proxy.
        #[cfg(feature = "tun")]
        {
            if let Some(mut handle) = self.tun.lock().take() {
                handle.abort();
            }
            // The helper's tproxy_remove only runs on clean exit; if it was
            // killed or crashed, routes + the resolv.conf bind-mount survive
            // and the internet stays dead. Always force-restore as root.
            let steps = match crate::tun::privs::spawn_teardown() {
                Ok(steps) => steps,
                Err(e) => {
                    tracing::warn!(error = %e, "tun teardown elevation failed");
                    vec![format!("teardown skipped: {e:#}")]
                }
            };
            let _ = self.store.clear_tun();
            let joined = steps.join("; ");
            let sys = crate::sysproxy::disable(&self.store)?;
            Ok(format!("{joined}; {sys}"))
        }
        #[cfg(not(feature = "tun"))]
        crate::sysproxy::disable(&self.store)
    }

    pub fn status(&self) -> Result<String> {
        crate::sysproxy::status()
    }
}
