//! RustCache desktop client: resilient local proxy, CA install, system proxy.
//!
//! Talks to an already-running RustCache — never manages that process.

pub mod api;
pub mod ca;
pub mod capture;
pub mod cli;
pub mod config;
pub mod doctor;
#[cfg(feature = "gui")]
pub mod gui;
pub mod health;
pub mod proxy;
pub mod state;
pub mod sysproxy;

pub use config::ClientConfig;
