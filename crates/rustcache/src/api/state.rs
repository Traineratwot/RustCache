//! REST API state.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rustcache_core::certs::ca::CaMaterial;
use rustcache_core::excl::{ExclusionSet, Matcher};

use crate::config::watch::LiveConfig;
use crate::config::Config;
use crate::engine::SharedEngine;

/// Which proxy listeners actually bound. Health uses this — a TCP probe on a
/// shared port cannot tell HTTP proxy from MITM when both were configured equal.
#[derive(Clone, Default)]
pub struct ListenerStatus {
    inner: Arc<Flags>,
}

#[derive(Default)]
struct Flags {
    http: AtomicBool,
    https: AtomicBool,
    socks5: AtomicBool,
    pac: AtomicBool,
}

impl ListenerStatus {
    pub fn set_http(&self, up: bool) {
        self.inner.http.store(up, Ordering::Relaxed);
    }
    pub fn set_https(&self, up: bool) {
        self.inner.https.store(up, Ordering::Relaxed);
    }
    pub fn set_socks5(&self, up: bool) {
        self.inner.socks5.store(up, Ordering::Relaxed);
    }
    pub fn set_pac(&self, up: bool) {
        self.inner.pac.store(up, Ordering::Relaxed);
    }
    pub fn http(&self) -> bool {
        self.inner.http.load(Ordering::Relaxed)
    }
    pub fn https(&self) -> bool {
        self.inner.https.load(Ordering::Relaxed)
    }
    pub fn socks5(&self) -> bool {
        self.inner.socks5.load(Ordering::Relaxed)
    }
    pub fn pac(&self) -> bool {
        self.inner.pac.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub struct ApiState {
    pub engine: SharedEngine,
    pub config: LiveConfig,
    pub ca: Arc<CaMaterial>,
    pub config_path: PathBuf,
    pub started_at: Instant,
    pub listeners: ListenerStatus,
}

impl ApiState {
    pub async fn exclusions(&self) -> Vec<Matcher> {
        self.engine.exclusion_matchers().await
    }

    pub async fn set_exclusions(&self, domains: Vec<String>, cidrs: Vec<String>) -> ExclusionSet {
        let set = ExclusionSet::from_specs(&domains, &cidrs);
        self.engine.set_exclusions(set.clone()).await;
        set
    }

    pub async fn reload_config(&self) -> anyhow::Result<Config> {
        let cfg = Config::load(&self.config_path)?;
        self.apply_config(cfg.clone()).await?;
        self.config.set(cfg.clone()).await;
        Ok(cfg)
    }

    pub async fn apply_config(&self, cfg: Config) -> anyhow::Result<()> {
        let set = ExclusionSet::from_specs(&cfg.exclude.domains, &cfg.exclude.cidrs);
        self.engine.set_exclusions(set).await;
        self.engine
            .set_cache_limits(cfg.cache.max_object_bytes, cfg.cache.max_bytes);
        Ok(())
    }
}
