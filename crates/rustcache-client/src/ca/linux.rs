//! Linux trust store: system CA bundle + NSS (Firefox/Chrome).

use std::path::PathBuf;

use anyhow::{Result, bail};

use super::{CA_FILENAME, CA_NICKNAME, ensure_success, run_cmd};
use crate::config::CaScope;

const SYSTEM_DIR: &str = "/usr/local/share/ca-certificates";

fn system_path() -> PathBuf {
    PathBuf::from(SYSTEM_DIR).join(CA_FILENAME)
}

fn nss_dbs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".pki/nssdb"));
        let ff = home.join(".mozilla/firefox");
        if let Ok(rd) = std::fs::read_dir(&ff) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    v.push(p);
                }
            }
        }
    }
    v
}

fn have_certutil() -> bool {
    run_cmd("certutil", &["-H"]).is_ok()
}

pub fn install(pem: &[u8], scope: CaScope) -> Result<String> {
    let mut notes = Vec::new();
    match scope {
        CaScope::System => {
            check_system_writable()?;
            let path = system_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, pem)?;
            let out = run_cmd("update-ca-certificates", &[])?;
            ensure_success(&out, "update-ca-certificates")?;
            notes.push(format!("system: {}", path.display()));
        }
        CaScope::User => {
            // User-level system store needs root; only NSS is user-writable.
            notes.push("user scope: NSS only (system store requires --system/root)".into());
        }
    }

    if have_certutil() {
        let tmp = std::env::temp_dir().join(format!("rcc-ca-{}.crt", std::process::id()));
        std::fs::write(&tmp, pem)?;
        let tmp_s = tmp.to_string_lossy().to_string();
        for db in nss_dbs() {
            if !db.exists() {
                continue;
            }
            let db_s = db.to_string_lossy().to_string();
            // -A adds or replaces by nickname
            let out = run_cmd(
                "certutil",
                &[
                    "-A",
                    "-n",
                    CA_NICKNAME,
                    "-t",
                    "C,,",
                    "-i",
                    &tmp_s,
                    "-d",
                    &format!("sql:{db_s}"),
                ],
            );
            match out {
                Ok(o) if o.status.success() => {
                    notes.push(format!("nss: {db_s}"));
                }
                Ok(o) => {
                    notes.push(format!(
                        "nss {db_s}: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ));
                }
                Err(e) => notes.push(format!("nss {db_s}: {e}")),
            }
        }
        let _ = std::fs::remove_file(&tmp);
    } else {
        notes.push("certutil not found — skipped NSS (Firefox)".into());
    }

    Ok(notes.join("; "))
}

pub fn uninstall(scope: CaScope) -> Result<String> {
    let mut notes = Vec::new();
    if matches!(scope, CaScope::System) {
        let path = system_path();
        if path.exists() {
            std::fs::remove_file(&path)?;
            let out = run_cmd("update-ca-certificates", &["--fresh"])?;
            ensure_success(&out, "update-ca-certificates --fresh")?;
            notes.push("system removed".into());
        } else {
            notes.push("system: not present".into());
        }
    }
    if have_certutil() {
        for db in nss_dbs() {
            if !db.exists() {
                continue;
            }
            let db_s = db.to_string_lossy().to_string();
            let out = run_cmd(
                "certutil",
                &["-D", "-n", CA_NICKNAME, "-d", &format!("sql:{db_s}")],
            );
            match out {
                Ok(o) if o.status.success() => notes.push(format!("nss removed: {db_s}")),
                Ok(_) => notes.push(format!("nss: not in {db_s}")),
                Err(e) => notes.push(format!("nss {db_s}: {e}")),
            }
        }
    }
    Ok(notes.join("; "))
}

pub fn status() -> Result<(bool, String)> {
    let mut bits = Vec::new();
    let mut installed = false;
    let path = system_path();
    if path.exists() {
        installed = true;
        bits.push(format!("system: {}", path.display()));
    }
    if have_certutil() {
        for db in nss_dbs() {
            if !db.exists() {
                continue;
            }
            let db_s = db.to_string_lossy().to_string();
            let out = run_cmd(
                "certutil",
                &["-L", "-n", CA_NICKNAME, "-d", &format!("sql:{db_s}")],
            );
            if let Ok(o) = out {
                if o.status.success() {
                    installed = true;
                    bits.push(format!("nss: {db_s}"));
                }
            }
        }
    }
    if bits.is_empty() {
        bits.push("not installed".into());
    }
    Ok((installed, bits.join("; ")))
}

/// Ensure we are not silently claiming success without write access.
fn check_system_writable() -> Result<()> {
    let dir = PathBuf::from(SYSTEM_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    let probe = dir.join(".rcc-write-probe");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => bail!("cannot write {SYSTEM_DIR}: {e} (need root / --system)"),
    }
}
