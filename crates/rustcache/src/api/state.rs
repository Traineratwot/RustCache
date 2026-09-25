//! REST API state.

use std::path::PathBuf;
use std::sync::Arc;

use rustcache_core::certs::ca::CaMaterial;
use rustcache_core::excl::{ExclusionSet, Matcher};

use crate::config::watch::LiveConfig;
use crate::config::Config;
use crate::engine::SharedEngine;

#[derive(Clone)]
pub struct ApiState {
    pub engine: SharedEngine,
    pub config: LiveConfig,
    pub ca: Arc<CaMaterial>,
    pub config_path: PathBuf,
}

impl ApiState {
    pub async fn exclusions(&self) -> Vec<Matcher> {
        self.engine.exclusions.read().await.matchers().to_vec()
    }

    pub async fn set_exclusions(&self, domains: Vec<String>, cidrs: Vec<String>) -> ExclusionSet {
        let set = ExclusionSet::from_specs(&domains, &cidrs);
        let mut guard = self.engine.exclusions.write().await;
        *guard = set.clone();
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
        *self.engine.exclusions.write().await = set;
        Ok(())
    }
}
