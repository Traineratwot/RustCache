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

use rustcache::api::{pac_router, router, ApiState};
use rustcache::config::watch::LiveConfig;
use rustcache::config::Config;
use rustcache::engine::CacheEngine;
use rustcache::listeners;

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

/// After a UI-triggered self-restart the previous process may still hold the
/// listening sockets. Wait until it exits (Linux `/proc`) plus a short grace.
fn wait_for_restart_parent() {
    let Ok(v) = std::env::var("RUSTCACHE_RESTARTED_FROM") else {
        return;
    };
    let Ok(pid) = v.parse::<u32>() else {
        return;
    };
    tracing::info!(parent = pid, "waiting for previous process to exit");
    for _ in 0..100 {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            std::thread::sleep(std::time::Duration::from_millis(100));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    tracing::warn!(
        parent = pid,
        "previous process still running after wait; continuing"
    );
}

async fn run(config_path: PathBuf, data_dir: Option<PathBuf>) -> anyhow::Result<()> {
    wait_for_restart_parent();
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
                    *engine.exclusions.write().await = set;
                    engine.set_cache_limits(cfg.cache.max_object_bytes, cfg.cache.max_bytes);
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

    let http_addr: std::net::SocketAddr = format!("0.0.0.0:{}", cfg.http.port).parse()?;
    let https_addr: std::net::SocketAddr = format!("0.0.0.0:{}", cfg.https.port).parse()?;
    let socks_addr: std::net::SocketAddr = format!("0.0.0.0:{}", cfg.socks5.port).parse()?;
    let api_addr = cfg.api.bind.clone();

    let engine_http = engine.clone();
    let engine_https = engine.clone();
    let engine_socks = engine.clone();
    let leaves_mitm = leaves.clone();

    let api_router = router(state.clone());

    #[cfg(feature = "embed-ui")]
    let api_router = api_router.fallback(rustcache::ui_embed::spa_handler);

    let listener = tokio::net::TcpListener::bind(&api_addr).await?;
    tracing::info!(addr = %api_addr, "api listening");

    // Bind proxy ports up front so a conflict (e.g. http.port == https.port)
    // is visible in health instead of one task dying silently.
    let http_listener = match tokio::net::TcpListener::bind(http_addr).await {
        Ok(l) => {
            state.listeners.set_http(true);
            Some(l)
        }
        Err(e) => {
            tracing::error!(error = %e, addr = %http_addr, "http listener bind failed");
            state.listeners.set_http(false);
            None
        }
    };
    let https_listener = match tokio::net::TcpListener::bind(https_addr).await {
        Ok(l) => {
            state.listeners.set_https(true);
            Some(l)
        }
        Err(e) => {
            tracing::error!(error = %e, addr = %https_addr, "mitm listener bind failed");
            state.listeners.set_https(false);
            None
        }
    };
    let socks_listener = match tokio::net::TcpListener::bind(socks_addr).await {
        Ok(l) => {
            state.listeners.set_socks5(true);
            Some(l)
        }
        Err(e) => {
            tracing::error!(error = %e, addr = %socks_addr, "socks5 listener bind failed");
            state.listeners.set_socks5(false);
            None
        }
    };

    // Optional dedicated LAN listener that serves only the two PAC paths.
    let pac_srv = if cfg.pac.enabled {
        let pac_addr = cfg.pac.bind.clone();
        let pac_state = state.clone();
        match tokio::net::TcpListener::bind(&pac_addr).await {
            Ok(pac_listener) => {
                state.listeners.set_pac(true);
                tracing::info!(addr = %pac_addr, "pac listener listening");
                Some(tokio::spawn(async move {
                    if let Err(e) = axum::serve(pac_listener, pac_router(pac_state)).await {
                        tracing::error!(error = %e, "pac listener failed");
                    }
                }))
            }
            Err(e) => {
                tracing::error!(error = %e, addr = %pac_addr, "pac listener bind failed");
                state.listeners.set_pac(false);
                None
            }
        }
    } else {
        state.listeners.set_pac(false);
        None
    };

    let http_srv = tokio::spawn(async move {
        if let Some(l) = http_listener {
            if let Err(e) = listeners::http_proxy::serve(l, engine_http).await {
                tracing::error!(error = %e, "http listener failed");
            }
        }
    });
    let https_srv = tokio::spawn(async move {
        if let Some(l) = https_listener {
            if let Err(e) = listeners::mitm_proxy::serve(l, engine_https, leaves_mitm).await {
                tracing::error!(error = %e, "mitm listener failed");
            }
        }
    });
    let socks_srv = tokio::spawn(async move {
        if let Some(l) = socks_listener {
            if let Err(e) = listeners::socks5::serve(l, engine_socks).await {
                tracing::error!(error = %e, "socks5 listener failed");
            }
        }
    });

    tracing::info!("RustCache started");
    axum::serve(listener, api_router).await?;

    http_srv.abort();
    https_srv.abort();
    socks_srv.abort();
    if let Some(s) = pac_srv {
        s.abort();
    }
    Ok(())
}
