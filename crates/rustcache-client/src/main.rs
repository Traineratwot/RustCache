//! rustcache-client CLI entry.

use clap::Parser;
use tracing_subscriber::EnvFilter;

use rustcache_client::cli::{Cli, execute};

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let level = cli.log_level.clone().unwrap_or_else(|| "info".to_string());
    init_tracing(&level);

    match execute(cli).await {
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
