//! clap CLI surface.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::api::RustCacheApi;
use crate::capture::Capture;
use crate::config::{CaptureMode, ClientConfig};
use crate::health::HealthMonitor;
use crate::proxy::ProxyServer;
use crate::state::StateStore;

#[derive(Parser, Debug)]
#[command(
    name = "rustcache-client",
    version,
    about = "Desktop client for RustCache: CA install, system proxy, resilient upstream"
)]
pub struct Cli {
    /// Path to client.toml
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    /// Override log level (default from config / RUST_LOG)
    #[arg(long, global = true)]
    pub log_level: Option<String>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Run local proxy and (optionally) force system proxy
    Run {
        /// Capture mode override
        #[arg(long, value_enum)]
        mode: Option<ModeArg>,
        /// Do not require system proxy (same as --mode off for capture, proxy still runs)
        #[arg(long)]
        no_capture: bool,
        /// Show tray + status window (requires --features gui)
        #[arg(long)]
        tray: bool,
    },
    /// Start GUI (status window + tray) with the local proxy (requires --features gui)
    Gui {
        /// Capture mode override
        #[arg(long, value_enum)]
        mode: Option<ModeArg>,
    },
    /// Install / uninstall / inspect root CA in the OS trust store
    Ca {
        #[command(subcommand)]
        action: CaAction,
    },
    /// System proxy on / off / status
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },
    /// Show client + RustCache status
    Status,
    /// Diagnostics
    Doctor,
    /// Config helpers
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum CaAction {
    /// Download CA and install into trust store
    Install {
        /// Machine-wide store (may need elevation)
        #[arg(long)]
        system: bool,
        /// Explicit PEM file instead of API / config
        #[arg(long)]
        ca_file: Option<PathBuf>,
    },
    /// Remove CA from trust store
    Uninstall {
        #[arg(long)]
        system: bool,
    },
    /// Show install status + fingerprint
    Status,
}

#[derive(Subcommand, Debug)]
pub enum ProxyAction {
    /// Point system proxy at the local client proxy
    On,
    /// Restore previous system proxy settings
    Off,
    /// Print current system proxy state
    Status,
    /// Print the commands that would run
    DryRun,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Write default config to the default path
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Print the default config path
    Path,
    /// Print effective config
    Show,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum ModeArg {
    Off,
    System,
    Tun,
}

impl From<ModeArg> for CaptureMode {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Off => CaptureMode::Off,
            ModeArg::System => CaptureMode::System,
            ModeArg::Tun => CaptureMode::Tun,
        }
    }
}

/// Exit codes (documented in the plan).
pub mod exit {
    pub const OK: i32 = 0;
    pub const GENERIC: i32 = 1;
    pub const CONFIG: i32 = 2;
    pub const ELEVATION: i32 = 3;
    pub const UNREACHABLE: i32 = 4;
    pub const PLATFORM: i32 = 5;
}

pub async fn execute(cli: Cli) -> Result<i32> {
    let cfg_path = cli
        .config
        .clone()
        .unwrap_or_else(ClientConfig::default_path);
    let mut cfg = ClientConfig::load(&cfg_path)?;
    if let Some(lvl) = &cli.log_level {
        cfg.log.level = lvl.clone();
    }

    match cli.command {
        Commands::Run {
            mode,
            no_capture,
            tray,
        } => {
            if tray {
                return cmd_gui(cfg, mode).await;
            }
            cmd_run(cfg, mode, no_capture).await?;
            Ok(exit::OK)
        }
        Commands::Gui { mode } => cmd_gui(cfg, mode).await,
        Commands::Ca { action } => cmd_ca(cfg, action).await,
        Commands::Proxy { action } => cmd_proxy(cfg, action),
        Commands::Status => cmd_status(cfg).await,
        Commands::Doctor => {
            let rep = crate::doctor::run(&cfg).await;
            for l in &rep.lines {
                println!("{l}");
            }
            if rep.rustcache_ok {
                Ok(exit::OK)
            } else {
                Ok(exit::UNREACHABLE)
            }
        }
        Commands::Config { action } => cmd_config(cfg, cfg_path, action),
    }
}

async fn cmd_run(cfg: ClientConfig, mode: Option<ModeArg>, no_capture: bool) -> Result<()> {
    let mode = if no_capture {
        CaptureMode::Off
    } else {
        mode.map(Into::into).unwrap_or(cfg.mode)
    };

    // Crash recovery before touching system proxy.
    let capture = Capture::default_store()?;
    if let Some(msg) = capture.recover()? {
        tracing::warn!(%msg, "recovered dirty system-proxy state");
    }

    let health = Arc::new(HealthMonitor::new(&cfg));
    {
        let h = health.clone();
        tokio::spawn(async move {
            h.run().await;
        });
    }

    if mode != CaptureMode::Off {
        match capture.enable(&cfg, mode) {
            Ok(msg) => tracing::info!(%msg, "capture enabled"),
            Err(e) => {
                tracing::error!(error = %e, "failed to enable system proxy");
                return Err(e);
            }
        }
    }

    let (host, port) = cfg.listen_addr()?;
    let server = Arc::new(ProxyServer::new(
        &cfg.listen,
        &cfg.api_host(),
        cfg.upstream_http_port(),
        cfg.upstream_https_port(),
        health.clone(),
        cfg.proxy_bypass.clone(),
    ));
    tracing::info!(
        %host,
        port,
        api = %cfg.rustcache_api,
        mode = ?mode,
        "rustcache-client running"
    );

    // Graceful shutdown on Ctrl-C: restore system proxy.
    let server_fut = server.clone().run();
    tokio::select! {
        res = server_fut => {
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("signal — shutting down");
        }
    }

    if mode != CaptureMode::Off {
        match capture.disable() {
            Ok(msg) => tracing::info!(%msg, "capture disabled"),
            Err(e) => tracing::error!(error = %e, "failed to restore system proxy"),
        }
    }
    Ok(())
}

/// GUI entry: proxy + health + tray/window. Blocking on the UI thread.
async fn cmd_gui(cfg: ClientConfig, mode: Option<ModeArg>) -> Result<i32> {
    #[cfg(not(feature = "gui"))]
    {
        let _ = (cfg, mode);
        eprintln!(
            "GUI support is not compiled in. Build with: cargo build -p rustcache-client --features gui"
        );
        Ok(exit::PLATFORM)
    }

    #[cfg(feature = "gui")]
    {
        use std::sync::Arc as StdArc;

        use crate::gui;
        use parking_lot::RwLock;

        let mode = mode.map(Into::into).unwrap_or(cfg.mode);

        let capture = Capture::default_store()?;
        if let Some(msg) = capture.recover()? {
            tracing::warn!(%msg, "recovered dirty system-proxy state");
        }

        let health = StdArc::new(HealthMonitor::new(&cfg));
        {
            let h = health.clone();
            tokio::spawn(async move {
                h.run().await;
            });
        }

        let gui_state: gui::SharedGui = StdArc::new(RwLock::new(gui::GuiState {
            mode,
            capture_on: mode != CaptureMode::Off,
            ..Default::default()
        }));

        if mode != CaptureMode::Off {
            match capture.enable(&cfg, mode) {
                Ok(msg) => {
                    gui_state.write().status_line = msg;
                    gui_state.write().capture_on = true;
                }
                Err(e) => {
                    gui_state.write().last_error = Some(format!("{e:#}"));
                }
            }
        }

        let (host, port) = cfg.listen_addr()?;
        let server = StdArc::new(ProxyServer::new(
            &cfg.listen,
            &cfg.api_host(),
            cfg.upstream_http_port(),
            cfg.upstream_https_port(),
            health.clone(),
            cfg.proxy_bypass.clone(),
        ));
        {
            let s = server.clone();
            tokio::spawn(async move {
                if let Err(e) = s.run().await {
                    tracing::error!(error = %e, "proxy server stopped");
                }
            });
        }
        tracing::info!(%host, port, mode = ?mode, "rustcache-client GUI running");

        // Restore on any exit path from the UI.
        let capture_for_quit = Capture::default_store()?;
        let mode_for_quit = mode;
        let on_quit = move || {
            if mode_for_quit != CaptureMode::Off {
                match capture_for_quit.disable() {
                    Ok(msg) => tracing::info!(%msg, "capture disabled on quit"),
                    Err(e) => tracing::error!(error = %e, "restore failed"),
                }
            }
        };

        // eframe blocks; run it on the current thread (must be main for some platforms).
        let cfg_ui = cfg.clone();
        let health_ui = health.clone();
        let gui_ui = gui_state.clone();
        let launch =
            tokio::task::spawn_blocking(move || gui::launch(cfg_ui, health_ui, gui_ui, on_quit));
        match launch.await {
            Ok(Ok(())) => Ok(exit::OK),
            Ok(Err(e)) => {
                eprintln!("gui failed: {e:#}");
                Ok(exit::PLATFORM)
            }
            Err(e) => {
                eprintln!("gui task: {e}");
                Ok(exit::PLATFORM)
            }
        }
    }
}

async fn cmd_ca(cfg: ClientConfig, action: CaAction) -> Result<i32> {
    let api = RustCacheApi::new(&cfg.rustcache_api).with_timeout(Duration::from_secs(5));
    match action {
        CaAction::Install { system, ca_file } => {
            match crate::ca::install(&api, &cfg.ca, ca_file.as_deref(), system).await {
                Ok(st) => {
                    println!("installed: {}", st.detail);
                    println!("sha256: {}", st.fingerprint_sha256);
                    Ok(exit::OK)
                }
                Err(e) => {
                    eprintln!("ca install failed: {e:#}");
                    let s = e.to_string();
                    if s.contains("root") || s.contains("Permission") || s.contains("elevat") {
                        Ok(exit::ELEVATION)
                    } else {
                        Ok(exit::PLATFORM)
                    }
                }
            }
        }
        CaAction::Uninstall { system } => match crate::ca::uninstall(&api, &cfg.ca, system).await {
            Ok(st) => {
                println!("{}", st.detail);
                Ok(exit::OK)
            }
            Err(e) => {
                eprintln!("ca uninstall failed: {e:#}");
                Ok(exit::PLATFORM)
            }
        },
        CaAction::Status => {
            let st = crate::ca::status(&api, &cfg.ca).await?;
            println!("installed: {}", st.installed);
            println!("sha256: {}", st.fingerprint_sha256);
            println!("detail: {}", st.detail);
            Ok(exit::OK)
        }
    }
}

fn cmd_proxy(cfg: ClientConfig, action: ProxyAction) -> Result<i32> {
    let capture = Capture::default_store()?;
    match action {
        ProxyAction::On => {
            if let Some(msg) = capture.recover()? {
                tracing::warn!(%msg, "recovered dirty state first");
            }
            match capture.enable(&cfg, CaptureMode::System) {
                Ok(msg) => {
                    println!("{msg}");
                    Ok(exit::OK)
                }
                Err(e) => {
                    eprintln!("proxy on failed: {e:#}");
                    Ok(exit::PLATFORM)
                }
            }
        }
        ProxyAction::Off => match capture.disable() {
            Ok(msg) => {
                println!("{msg}");
                Ok(exit::OK)
            }
            Err(e) => {
                eprintln!("proxy off failed: {e:#}");
                Ok(exit::PLATFORM)
            }
        },
        ProxyAction::Status => {
            println!("{}", capture.status()?);
            Ok(exit::OK)
        }
        ProxyAction::DryRun => {
            let (host, port) = cfg.listen_addr()?;
            println!(
                "{}",
                crate::sysproxy::dry_run(&host, port, &cfg.proxy_bypass)?
            );
            Ok(exit::OK)
        }
    }
}

async fn cmd_status(cfg: ClientConfig) -> Result<i32> {
    let api = RustCacheApi::new(&cfg.rustcache_api);
    match api.health().await {
        Ok(h) => {
            println!(
                "rustcache: ok={} http={} https={} socks={}",
                h.ok, h.http_running, h.https_running, h.socks_running
            );
        }
        Err(e) => {
            println!("rustcache: unreachable ({e})");
            return Ok(exit::UNREACHABLE);
        }
    }
    if let Ok(ca) = crate::ca::status(&api, &cfg.ca).await {
        println!("ca: installed={}", ca.installed);
    }
    if let Ok(sp) = crate::sysproxy::status() {
        println!("sysproxy: {sp}");
    }
    if let Ok(store) = StateStore::default_store() {
        if let Ok(d) = store.is_dirty() {
            println!("dirty: {d}");
        }
    }
    println!("listen: {}", cfg.listen);
    println!("mode: {:?}", cfg.mode);
    Ok(exit::OK)
}

fn cmd_config(cfg: ClientConfig, path: PathBuf, action: ConfigAction) -> Result<i32> {
    match action {
        ConfigAction::Init { force } => {
            if path.exists() && !force {
                bail!("{} exists (use --force)", path.display());
            }
            ClientConfig::default().save(&path)?;
            println!("wrote {}", path.display());
            Ok(exit::OK)
        }
        ConfigAction::Path => {
            println!("{}", path.display());
            Ok(exit::OK)
        }
        ConfigAction::Show => {
            let raw = toml::to_string_pretty(&cfg)?;
            print!("{raw}");
            Ok(exit::OK)
        }
    }
}
