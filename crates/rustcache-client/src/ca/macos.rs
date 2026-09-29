//! macOS trust store via `security`.

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::{CA_NICKNAME, ensure_success, run_cmd};
use crate::config::CaScope;

fn login_keychain() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/var/empty"))
        .join("Library/Keychains/login.keychain-db")
}

const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

fn keychain(scope: CaScope) -> String {
    match scope {
        CaScope::User => login_keychain().to_string_lossy().to_string(),
        CaScope::System => SYSTEM_KEYCHAIN.to_string(),
    }
}

fn cert_cn() -> &'static str {
    // generate_ca uses CN=RustCache MITM Root — match on that; fall back to nickname.
    "RustCache MITM Root"
}

fn write_temp(pem: &[u8]) -> Result<PathBuf> {
    let p = std::env::temp_dir().join(format!("rcc-ca-{}.crt", std::process::id()));
    std::fs::write(&p, pem)?;
    Ok(p)
}

pub fn install(pem: &[u8], scope: CaScope) -> Result<String> {
    let tmp = write_temp(pem)?;
    let kc = keychain(scope);
    let tmp_s = tmp.to_string_lossy().to_string();
    let out = run_cmd(
        "security",
        &[
            "add-trusted-cert",
            "-d",
            "-r",
            "trustRoot",
            "-k",
            &kc,
            &tmp_s,
        ],
    )?;
    let msg = ensure_success(&out, "security add-trusted-cert")?;
    let _ = std::fs::remove_file(&tmp);
    Ok(format!("added to {kc} ({msg})"))
}

pub fn uninstall(scope: CaScope) -> Result<String> {
    let kc = keychain(scope);
    // Delete by SHA-1 of existing cert with our CN.
    let out = run_cmd(
        "security",
        &["find-certificate", "-c", cert_cn(), "-a", "-Z", &kc],
    )?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let mut removed = 0;
    for line in stdout.lines() {
        if let Some(sha1) = line.strip_prefix("SHA-1 hash: ") {
            let out2 = run_cmd("security", &["delete-certificate", "-Z", sha1.trim(), &kc]);
            if let Ok(o) = out2 {
                if o.status.success() {
                    removed += 1;
                }
            }
        }
    }
    Ok(format!("removed {removed} from {kc}"))
}

pub fn status() -> Result<(bool, String)> {
    let kcs = [keychain(CaScope::User), keychain(CaScope::System)];
    let mut bits = Vec::new();
    let mut installed = false;
    for kc in kcs {
        let out = run_cmd(
            "security",
            &["find-certificate", "-c", cert_cn(), "-a", &kc],
        )?;
        if out.status.success() && !out.stdout.is_empty() {
            installed = true;
            bits.push(format!("found in {kc}"));
        }
    }
    if bits.is_empty() {
        // also try CA_NICKNAME
        for kc in [keychain(CaScope::User)] {
            let out = run_cmd("security", &["find-certificate", "-c", CA_NICKNAME, &kc])?;
            if out.status.success() {
                installed = true;
                bits.push(format!("found in {kc}"));
            }
        }
    }
    if bits.is_empty() {
        bits.push("not installed".into());
    }
    Ok((installed, bits.join("; ")))
}

#[allow(dead_code)]
fn require_scope_system_elevated() -> Result<()> {
    // System keychain writes usually need admin; surface a clear error.
    let kc = SYSTEM_KEYCHAIN;
    if std::fs::metadata(kc).is_err() {
        bail!("system keychain missing: {kc}");
    }
    Ok(())
}
