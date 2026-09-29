//! Capture orchestration: system proxy on/off + dirty recovery.

use anyhow::Result;

use crate::config::{CaptureMode, ClientConfig};
use crate::state::StateStore;

pub struct Capture {
    store: StateStore,
}

impl Capture {
    pub fn new(store: StateStore) -> Self {
        Self { store }
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
                    match tokio::runtime::Handle::try_current() {
                        Ok(rt) => {
                            // We need async — block on the TUN setup.
                            let result = rt.block_on(crate::tun::enable(&local_proxy));
                            match result {
                                Ok(session) => {
                                    let tun_json = session.snapshot.to_json();
                                    self.store
                                        .mark_dirty_parts(serde_json::json!({}), Some(tun_json))?;
                                    Ok(format!(
                                        "tun active on {} ({})",
                                        session.snapshot.iface, session.snapshot.addr
                                    ))
                                }
                                Err(e) => {
                                    // Fail-open: fall back to system proxy.
                                    tracing::warn!(error = %e, "TUN failed — falling back to system proxy");
                                    let info = crate::platform::detect();
                                    if info.sysproxy_supported {
                                        let (host, port) = cfg.listen_addr()?;
                                        crate::sysproxy::enable(
                                            &host,
                                            port,
                                            &cfg.proxy_bypass,
                                            &self.store,
                                        )
                                    } else {
                                        Err(e)
                                    }
                                }
                            }
                        }
                        Err(_) => anyhow::bail!("TUN requires a tokio runtime"),
                    }
                }
                #[cfg(not(feature = "tun"))]
                {
                    anyhow::bail!("TUN is not compiled in (build with --features tun)")
                }
            }
        }
    }

    pub fn disable(&self) -> Result<String> {
        // Disable TUN first (if active), then system proxy.
        #[cfg(feature = "tun")]
        {
            let snap = self.store.load()?;
            if let Some(ref tun_json) = snap.tun {
                if let Some(tun_snap) = crate::tun::TunSnapshot::from_json(tun_json) {
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        let _ = rt.block_on(crate::tun::disable(&tun_snap));
                    }
                }
            }
        }
        crate::sysproxy::disable(&self.store)
    }

    pub fn status(&self) -> Result<String> {
        crate::sysproxy::status()
    }
}
