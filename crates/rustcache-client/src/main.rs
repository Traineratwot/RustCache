//! rustcache-client CLI entry.
//!
//! GUI (`gui` / `run --tray`) must run the native event loop on the **main**
//! thread (winit/efume requirement). Async work lives on a tokio runtime.

use clap::Parser;
use tracing_subscriber::EnvFilter;

use rustcache_client::cli::{Cli, Commands, GuiPrepared, execute, prepare_gui};

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn wants_gui(cmd: &Commands) -> bool {
    match cmd {
        Commands::Gui { .. } => true,
        Commands::Run { tray, .. } => *tray,
        _ => false,
    }
}

fn main() {
    let cli = Cli::parse();
    let level = cli.log_level.clone().unwrap_or_else(|| "info".to_string());
    init_tracing(&level);

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: tokio runtime: {e}");
            std::process::exit(1);
        }
    };

    if wants_gui(&cli.command) {
        let cfg_mode = match &cli.command {
            Commands::Gui { mode } => *mode,
            Commands::Run { mode, .. } => *mode,
            _ => None,
        };
        let cfg_path = cli
            .config
            .clone()
            .unwrap_or_else(rustcache_client::ClientConfig::default_path);
        let log_level = cli.log_level.clone();

        let prepared = rt.block_on(async move {
            let mut cfg = rustcache_client::ClientConfig::load(&cfg_path)?;
            if let Some(lvl) = log_level {
                cfg.log.level = lvl;
            }
            prepare_gui(cfg, cfg_mode).await
        });

        match prepared {
            #[cfg(feature = "gui")]
            Ok(GuiPrepared::Ready(launch)) => {
                // Main thread — native event loop.
                if let Err(e) = launch.run() {
                    eprintln!("error: {e:#}");
                    std::process::exit(1);
                }
            }
            Ok(GuiPrepared::Disabled) => {
                eprintln!(
                    "GUI support is not compiled in. Build with: cargo build -p rustcache-client --features gui"
                );
                std::process::exit(5);
            }
            Err(e) => {
                eprintln!("error: {e:#}");
                std::process::exit(1);
            }
        }
        #[allow(unreachable_code)]
        return;
    }

    // Non-GUI commands: run on the runtime.
    match rt.block_on(execute(cli)) {
        Ok(code) => {
            if code != 0 {
                std::process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}
