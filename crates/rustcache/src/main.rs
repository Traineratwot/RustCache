//! RustCache CLI: run | gen-ca | export-ca | purge

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use rustcache_core::cache::disk::DiskCache;
use rustcache_core::cache::mem::MemCache;
use rustcache_core::certs::ca::{export_pem, generate_ca, load_ca};
use rustcache_core::certs::leaf::LeafIssuer;
use rustcache_core::excl::ExclusionSet;

use rustcache::api::{ApiState, router};
use rustcache::config::Config;
use rustcache::config::watch::LiveConfig;
use rustcache::engine::CacheEngine;

#[derive(Parser, Debug)]
#[command(name = "rustcache", version, about = "Caching proxy server")]
struct Cli {
    /// Data directory root (cache / CA / logs.db). Overrides config `data_dir`.
    #[arg(long, value_name = "DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Start all listeners + REST API
    Run {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Create root CA if missing
    GenCa {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Print path or PEM of root CA
    ExportCa {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// Print PEM instead of path
        #[arg(long)]
        pem: bool,
    },
    /// Clear the on-disk cache
    Purge {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    install_crypto_provider();
    let cli = Cli::parse();
    let data_dir = cli.data_dir;

    match cli.command {
        Commands::Run { config } => run(config, data_dir).await,
        Commands::GenCa { dir, config } => {
            let mut cfg = Config::load_or_default(&config)?;
            cfg.apply_data_dir_override(data_dir.as_deref());
            let dir = dir.unwrap_or_else(|| cfg.ca_dir());
            let material = generate_ca(&dir)?;
            tracing::info!(cert = %material.cert_path.display(), "CA generated");
            println!("{}", material.cert_path.display());
            Ok(())
        }
        Commands::ExportCa { dir, config, pem } => {
            let mut cfg = Config::load_or_default(&config)?;
            cfg.apply_data_dir_override(data_dir.as_deref());
            let dir = dir.unwrap_or_else(|| cfg.ca_dir());
            let material = load_ca(&dir)?;
            if pem {
                print!("{}", String::from_utf8_lossy(&export_pem(&material)));
            } else {
                println!("{}", material.cert_path.display());
            }
            Ok(())
        }
        Commands::Purge { config } => {
            let mut cfg = Config::load_or_default(&config)?;
            cfg.apply_data_dir_override(data_dir.as_deref());
            let disk = DiskCache::open(cfg.cache_dir())?;
            let n = disk.purge_all().await?;
            println!("purged {n} files");
            Ok(())
        }
    }
}

async fn run(config_path: PathBuf, data_dir: Option<PathBuf>) -> anyhow::Result<()> {
    rustcache::startup::wait_for_restart_parent();
    let mut cfg = Config::load_or_default(&config_path)?;
    cfg.apply_data_dir_override(data_dir.as_deref());
    let cache_dir = cfg.cache_dir();
    let ca_dir = cfg.ca_dir();
    tracing::info!(
        data_dir = %cfg.data_dir_path().display(),
        cache = %cache_dir.display(),
        ca = %ca_dir.display(),
        logs = %cfg.logs_db_path().display(),
        "data paths"
    );
    let disk = DiskCache::open(&cache_dir)?;
    let mem = MemCache::new(cfg.cache.max_bytes.min(256 * 1024 * 1024));
    let exclusions = ExclusionSet::from_specs(&cfg.exclude.domains, &cfg.exclude.cidrs);
    let logs = Arc::new(rustcache_core::stats::LogStore::open(cfg.logs_db_path())?);
    let engine = Arc::new(CacheEngine::new(
        disk,
        mem,
        exclusions,
        logs.clone(),
        cfg.cache.max_object_bytes,
        cfg.cache.max_bytes,
    ));
    engine.set_optimistic(cfg.cache.optimistic);

    let ca = load_ca(&ca_dir)?;
    let leaves = Arc::new(LeafIssuer::from_ca(&ca)?);
    let ca = Arc::new(ca);

    let live = LiveConfig::new(cfg.clone());
    let (cfg_tx, mut cfg_rx) = tokio::sync::watch::channel::<Option<Config>>(None);
    let _watcher = if config_path.exists() {
        match rustcache::config::watch::spawn_watcher(&config_path, cfg_tx) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::warn!(error = %e, "config watcher not started");
                None
            }
        }
    } else {
        None
    };

    let state = ApiState {
        engine: engine.clone(),
        config: live.clone(),
        ca: ca.clone(),
        config_path: config_path.clone(),
        started_at: std::time::Instant::now(),
        listeners: rustcache::api::ListenerStatus::default(),
    };

    // Apply live config reloads (exclusions etc.)
    {
        let engine = engine.clone();
        let live = live.clone();
        tokio::spawn(async move {
            while let Ok(()) = cfg_rx.changed().await {
                let cfg = cfg_rx.borrow_and_update().clone();
                if let Some(cfg) = cfg {
                    tracing::info!("config reloaded");
                    let set = ExclusionSet::from_specs(&cfg.exclude.domains, &cfg.exclude.cidrs);
                    engine.set_exclusions(set).await;
                    engine.set_cache_limits(cfg.cache.max_object_bytes, cfg.cache.max_bytes);
                    engine.set_optimistic(cfg.cache.optimistic);
                    live.set(cfg).await;
                }
            }
        });
    }

    // Periodic request-log retention cleanup.
    {
        let logs = logs.clone();
        let live = live.clone();
        tokio::spawn(async move {
            loop {
                let iv = {
                    let cfg = live.get().await;
                    cfg.logs.cleanup_interval_secs.clamp(10, 86_400)
                };
                tokio::time::sleep(std::time::Duration::from_secs(iv)).await;
                let cfg = live.get().await;
                match logs.cleanup(cfg.logs.max_rows, cfg.logs.max_age_days).await {
                    Ok((by_age, by_rows)) => {
                        if by_age + by_rows > 0 {
                            tracing::info!(by_age, by_rows, "request log cleanup");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "request log cleanup failed"),
                }
            }
        });
    }

    let api_addr = cfg.api.bind.clone();

    let api_router = router(state.clone());

    #[cfg(feature = "embed-ui")]
    let api_router = api_router.fallback(rustcache::ui_embed::spa_handler);

    let listener = tokio::net::TcpListener::bind(&api_addr).await?;
    tracing::info!(addr = %api_addr, "api listening");

    let tasks = rustcache::startup::bring_up_listeners(&cfg, &state, &engine, &leaves).await;

    tracing::info!("RustCache started");
    axum::serve(listener, api_router).await?;

    tasks.abort();
    Ok(())
}
