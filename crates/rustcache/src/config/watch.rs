//! Config hot-reload via notify.

use std::path::Path;
use std::sync::Arc;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::watch;

use crate::config::Config;

/// Watch `path` for modifications and push reloaded configs on `tx`.
pub fn spawn_watcher(
    path: impl AsRef<Path>,
    tx: watch::Sender<Option<Config>>,
) -> anyhow::Result<RecommendedWatcher> {
    let path = path.as_ref().to_path_buf();
    let watch_path = path.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            if matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) {
                match Config::load(&watch_path) {
                    Ok(cfg) => {
                        if tx.send(Some(cfg)).is_err() {
                            tracing::debug!("config watcher: receiver dropped");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "config reload failed; keeping previous config");
                    }
                }
            }
        }
    })?;
    watcher.watch(&path, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// Shared live config holder.
///
/// Stores `Arc<Config>` so hot readers avoid cloning the whole config tree.
#[derive(Clone)]
pub struct LiveConfig {
    inner: Arc<tokio::sync::RwLock<Arc<Config>>>,
}

impl LiveConfig {
    pub fn new(cfg: Config) -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(Arc::new(cfg))),
        }
    }

    /// Snapshot of the current config (cheap `Arc` clone).
    pub async fn get(&self) -> Arc<Config> {
        self.inner.read().await.clone()
    }

    pub async fn set(&self, cfg: Config) {
        *self.inner.write().await = Arc::new(cfg);
    }
}
