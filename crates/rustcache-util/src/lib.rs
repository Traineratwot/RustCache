//! Small, dependency-free helpers shared by the RustCache crates.
//!
//! Two concerns live here because they are copy-pasted across `rustcache-core`,
//! `rustcache` and `rustcache-client` with no need for the heavy deps of those
//! crates:
//!
//! * [`host`] — authority / URL host parsing. Every listener, the PAC handler,
//!   the exclusion matchers and the desktop client each used to carry their own
//!   slightly different `split_once(':')` and IPv6-bracket logic.
//! * [`fs`] — tilde expansion and `0600` hardening for secret files (CA key,
//!   request-log DB).

pub mod fs;
pub mod host;

pub use fs::{expand_tilde, set_private_mode};
pub use host::{
    HostParseError, authority_from_target, host_from_url, host_of, split_authority,
    split_host_port, split_host_port_or,
};
