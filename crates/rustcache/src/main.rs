//! RustCache CLI: run | gen-ca | export-ca | purge

mod api;
mod config;
mod engine;
mod listeners;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use rustcache_core::cache::disk::DiskCache;
use rustcache_core::cache::mem::MemCache;
use rustcache_core::certs::ca::{export_pem, generate_ca, load_ca};
use rustcache_core::certs::leaf::LeafIssuer;
use rustcache_core::excl::ExclusionSet;

use crate::api::{router, ApiState};
use crate::config::watch::LiveConfig;
use crate::config::Config;
use crate::engine::CacheEngine;

#[derive(Parser, Debug)]
#[command(name = "rustcache", version, about = "Caching proxy server")]
struct Cli {
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

    match cli.command {
        Commands::Run { config } => run(config).await,
        Commands::GenCa { dir, config } => {
            let cfg = Config::load_or_default(&config)?;
            let dir = dir.unwrap_or_else(|| cfg.ca_dir());
            let material = generate_ca(&dir)?;
            tracing::info!(cert = %material.cert_path.display(), "CA generated");
            println!("{}", material.cert_path.display());
            Ok(())
        }
        Commands::ExportCa { dir, config, pem } => {
            let cfg = Config::load_or_default(&config)?;
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
            let cfg = Config::load_or_default(&config)?;
            let disk = DiskCache::open(cfg.cache_dir())?;
            let n = disk.purge_all().await?;
            println!("purged {n} files");
            Ok(())
        }
    }
}

async fn run(config_path: PathBuf) -> anyhow::Result<()> {
    let cfg = Config::load_or_default(&config_path)?;
    let cache_dir = cfg.cache_dir();
    let ca_dir = cfg.ca_dir();
    let disk = DiskCache::open(&cache_dir)?;
    let mem = MemCache::new(cfg.cache.max_bytes.min(256 * 1024 * 1024));
    let exclusions = ExclusionSet::from_specs(&cfg.exclude.domains, &cfg.exclude.cidrs);
    let engine = Arc::new(CacheEngine::new(
        disk,
        mem,
        exclusions,
        cfg.cache.max_object_bytes,
        cfg.cache.max_bytes,
    ));

    let ca = load_ca(&ca_dir)?;
    let leaves = Arc::new(LeafIssuer::from_ca(&ca)?);
    let ca = Arc::new(ca);

    let live = LiveConfig::new(cfg.clone());
    let (cfg_tx, mut cfg_rx) = tokio::sync::watch::channel::<Option<Config>>(None);
    let _watcher = if config_path.exists() {
        match config::watch::spawn_watcher(&config_path, cfg_tx) {
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
                    *engine.exclusions.write().await = set;
                    live.set(cfg).await;
                }
            }
        });
    }

    let http_addr = format!("0.0.0.0:{}", cfg.http.port).parse()?;
    let https_addr = format!("0.0.0.0:{}", cfg.https.port).parse()?;
    let socks_addr = format!("0.0.0.0:{}", cfg.socks5.port).parse()?;
    let api_addr = cfg.api.bind.clone();

    let engine_http = engine.clone();
    let engine_https = engine.clone();
    let engine_socks = engine.clone();
    let leaves_mitm = leaves.clone();

    let api_router = router(state);
    let listener = tokio::net::TcpListener::bind(&api_addr).await?;
    tracing::info!(addr = %api_addr, "api listening");

    let http_srv = tokio::spawn(async move {
        if let Err(e) = listeners::http_proxy::serve(http_addr, engine_http).await {
            tracing::error!(error = %e, "http listener failed");
        }
    });
    let https_srv = tokio::spawn(async move {
        if let Err(e) = listeners::mitm_proxy::serve(https_addr, engine_https, leaves_mitm).await {
            tracing::error!(error = %e, "mitm listener failed");
        }
    });
    let socks_srv = tokio::spawn(async move {
        if let Err(e) = listeners::socks5::serve(socks_addr, engine_socks).await {
            tracing::error!(error = %e, "socks5 listener failed");
        }
    });

    tracing::info!("RustCache started");
    axum::serve(listener, api_router).await?;

    http_srv.abort();
    https_srv.abort();
    socks_srv.abort();
    Ok(())
}
