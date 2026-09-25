//! RustCache core library: cache, certs, stats, exclusions, HTTP policy.

pub mod cache;
pub mod certs;
pub mod error;
pub mod excl;
pub mod http;
pub mod stats;

pub use error::{Error, Result};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
