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
                let (host, port) = cfg.listen_addr()?;
                crate::sysproxy::enable(&host, port, &cfg.proxy_bypass, &self.store)
            }
            CaptureMode::Tun => {
                anyhow::bail!("TUN mode is Phase 3 and not implemented yet")
            }
        }
    }

    pub fn disable(&self) -> Result<String> {
        crate::sysproxy::disable(&self.store)
    }

    pub fn status(&self) -> Result<String> {
        crate::sysproxy::status()
    }
}
