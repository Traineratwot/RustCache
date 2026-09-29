//! Root CA install / uninstall / status in OS trust stores.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::api::RustCacheApi;
use crate::config::{CaConfig, CaScope};

#[derive(Debug, Clone)]
pub struct CaStatus {
    pub installed: bool,
    pub fingerprint_sha256: String,
    pub matches_local: bool,
    pub detail: String,
}

/// PEM bytes from API, file, or explicit path.
pub async fn fetch_ca_pem(
    api: &RustCacheApi,
    ca: &CaConfig,
    explicit: Option<&Path>,
) -> Result<Vec<u8>> {
    if let Some(p) = explicit {
        return std::fs::read(p).with_context(|| format!("read {}", p.display()));
    }
    if ca.source == "file" {
        if ca.file.is_empty() {
            bail!("ca.source=file but ca.file is empty");
        }
        let p = expand_tilde(&ca.file);
        return std::fs::read(&p).with_context(|| format!("read {}", p.display()));
    }
    api.ca_pem().await.context("GET /api/ca.crt")
}

pub fn fingerprint_sha256(pem: &[u8]) -> String {
    // Fingerprint the PEM body (stable enough for "is this the same cert?").
    let mut h = Sha256::new();
    h.update(pem);
    hex::encode(h.finalize())
}

pub fn is_pem_cert(pem: &[u8]) -> bool {
    let s = String::from_utf8_lossy(pem);
    s.contains("BEGIN CERTIFICATE")
}

pub async fn install(
    api: &RustCacheApi,
    ca: &CaConfig,
    explicit: Option<&Path>,
    system: bool,
) -> Result<CaStatus> {
    let pem = fetch_ca_pem(api, ca, explicit).await?;
    if !is_pem_cert(&pem) {
        bail!("not a PEM certificate");
    }
    let fp = fingerprint_sha256(&pem);
    let scope = if system { CaScope::System } else { ca.scope };
    platform_install(&pem, scope).map(|detail| CaStatus {
        installed: true,
        fingerprint_sha256: fp,
        matches_local: true,
        detail,
    })
}

pub async fn uninstall(_api: &RustCacheApi, ca: &CaConfig, system: bool) -> Result<CaStatus> {
    let scope = if system { CaScope::System } else { ca.scope };
    let detail = platform_uninstall(scope)?;
    Ok(CaStatus {
        installed: false,
        fingerprint_sha256: String::new(),
        matches_local: false,
        detail,
    })
}

pub async fn status(api: &RustCacheApi, ca: &CaConfig) -> Result<CaStatus> {
    let local_fp = match fetch_ca_pem(api, ca, None).await {
        Ok(pem) => fingerprint_sha256(&pem),
        Err(_) => String::new(),
    };
    let (installed, detail) = platform_status()?;
    Ok(CaStatus {
        installed,
        fingerprint_sha256: local_fp.clone(),
        matches_local: installed && !local_fp.is_empty(),
        detail,
    })
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

#[cfg(target_os = "linux")]
fn platform_install(pem: &[u8], scope: CaScope) -> Result<String> {
    linux::install(pem, scope)
}
#[cfg(target_os = "linux")]
fn platform_uninstall(scope: CaScope) -> Result<String> {
    linux::uninstall(scope)
}
#[cfg(target_os = "linux")]
fn platform_status() -> Result<(bool, String)> {
    linux::status()
}

#[cfg(target_os = "macos")]
fn platform_install(pem: &[u8], scope: CaScope) -> Result<String> {
    macos::install(pem, scope)
}
#[cfg(target_os = "macos")]
fn platform_uninstall(scope: CaScope) -> Result<String> {
    macos::uninstall(scope)
}
#[cfg(target_os = "macos")]
fn platform_status() -> Result<(bool, String)> {
    macos::status()
}

#[cfg(target_os = "windows")]
fn platform_install(pem: &[u8], scope: CaScope) -> Result<String> {
    windows::install(pem, scope)
}
#[cfg(target_os = "windows")]
fn platform_uninstall(scope: CaScope) -> Result<String> {
    windows::uninstall(scope)
}
#[cfg(target_os = "windows")]
fn platform_status() -> Result<(bool, String)> {
    windows::status()
}

/// Shared helper: run a command, capture output, return stderr/stdout on failure.
pub(crate) fn run_cmd(prog: &str, args: &[&str]) -> Result<std::process::Output> {
    let out = std::process::Command::new(prog)
        .args(args)
        .output()
        .with_context(|| format!("spawn {prog}"))?;
    Ok(out)
}

pub(crate) fn ensure_success(out: &std::process::Output, what: &str) -> Result<String> {
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        bail!("{what} failed ({}): {err}{stdout}", out.status);
    }
}

pub const CA_NICKNAME: &str = "RustCache";
pub const CA_FILENAME: &str = "rustcache-ca.crt";
