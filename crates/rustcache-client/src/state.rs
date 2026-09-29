//! Crash-safe snapshot of system-proxy settings (`dirty` flag protocol).
//!
//! Flow: write snapshot + `dirty=true` → apply → on clean restore clear dirty.
//! On startup, if dirty → restore previous settings before applying anything new.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ClientConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProxySnapshot {
    /// Platform-specific saved settings (registry values, gsettings, networksetup).
    #[serde(default)]
    pub platform: serde_json::Value,
    /// True while system proxy is forced by us.
    #[serde(default)]
    pub dirty: bool,
    /// When the snapshot was taken (unix ms).
    #[serde(default)]
    pub ts_ms: u64,
}

pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn default_store() -> Result<Self> {
        let dir = ClientConfig::state_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
        Ok(Self {
            path: dir.join("proxy-restore.json"),
        })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<ProxySnapshot> {
        if !self.path.exists() {
            return Ok(ProxySnapshot::default());
        }
        let raw = std::fs::read_to_string(&self.path)
            .with_context(|| format!("read {}", self.path.display()))?;
        let snap: ProxySnapshot =
            serde_json::from_str(&raw).with_context(|| format!("parse {}", self.path.display()))?;
        Ok(snap)
    }

    pub fn save(&self, snap: &ProxySnapshot) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(snap)?;
        // tmp + rename so a crash mid-write cannot leave a corrupt snapshot
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Mark that we are about to force system proxy (persist previous platform state).
    pub fn mark_dirty(&self, platform: serde_json::Value) -> Result<()> {
        let snap = ProxySnapshot {
            platform,
            dirty: true,
            ts_ms: now_ms(),
        };
        self.save(&snap)
    }

    pub fn clear_dirty(&self) -> Result<()> {
        let mut snap = self.load().unwrap_or_default();
        snap.dirty = false;
        self.save(&snap)
    }

    pub fn is_dirty(&self) -> Result<bool> {
        Ok(self.load()?.dirty)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rcc-state-{}", std::process::id()));
        let store = StateStore::at(dir.join("proxy-restore.json"));
        store
            .mark_dirty(serde_json::json!({"prev": "none"}))
            .unwrap();
        assert!(store.is_dirty().unwrap());
        store.clear_dirty().unwrap();
        assert!(!store.is_dirty().unwrap());
        let snap = store.load().unwrap();
        assert_eq!(snap.platform["prev"], "none");
    }
}
