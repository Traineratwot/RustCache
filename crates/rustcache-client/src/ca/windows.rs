//! Windows trust store via `certutil`.

use std::path::PathBuf;

use anyhow::Result;

use super::{CA_NICKNAME, ensure_success, run_cmd};
use crate::config::CaScope;

fn write_temp(pem: &[u8]) -> Result<PathBuf> {
    let p = std::env::temp_dir().join(format!("rcc-ca-{}.crt", std::process::id()));
    std::fs::write(&p, pem)?;
    Ok(p)
}

fn user_flag(scope: CaScope) -> Option<&'static str> {
    match scope {
        CaScope::User => Some("-user"),
        CaScope::System => None,
    }
}

fn store_args(scope: CaScope, rest: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if let Some(u) = user_flag(scope) {
        v.push(u.to_string());
    }
    for r in rest {
        v.push((*r).to_string());
    }
    v
}

fn run_certutil(args: &[String]) -> Result<std::process::Output> {
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run_cmd("certutil", &refs)
}

pub fn install(pem: &[u8], scope: CaScope) -> Result<String> {
    let tmp = write_temp(pem)?;
    let tmp_s = tmp.to_string_lossy().to_string();
    let args = store_args(scope, &["-addstore", "Root", &tmp_s]);
    let out = run_certutil(&args)?;
    let msg = ensure_success(&out, "certutil -addstore")?;
    let _ = std::fs::remove_file(&tmp);
    Ok(format!("added to Root store ({msg})"))
}

pub fn uninstall(scope: CaScope) -> Result<String> {
    // Find SHA1 thumbprint of our cert, then -delstore.
    let args = store_args(scope, &["-store", "Root"]);
    let out = run_certutil(&args)?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // certutil -store dumps certificates; look for our CN then a nearby "Cert Hash(sha1):".
    let mut thumb: Option<String> = None;
    let mut saw_cn = false;
    for line in stdout.lines() {
        if line.contains("RustCache") || line.contains(CA_NICKNAME) {
            saw_cn = true;
        }
        if saw_cn {
            if let Some(h) = line.trim().strip_prefix("Cert Hash(sha1):") {
                thumb = Some(h.trim().replace(' ', ""));
                break;
            }
        }
    }
    let Some(thumb) = thumb else {
        return Ok("not found in Root store".into());
    };
    let args = store_args(scope, &["-delstore", "Root", &thumb]);
    let out = run_certutil(&args)?;
    ensure_success(&out, "certutil -delstore")?;
    Ok(format!("removed {thumb}"))
}

pub fn status() -> Result<(bool, String)> {
    for scope in [CaScope::User, CaScope::System] {
        let args = store_args(scope, &["-store", "Root"]);
        let Ok(out) = run_certutil(&args) else {
            continue;
        };
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        if stdout.contains("RustCache") || stdout.contains(CA_NICKNAME) {
            return Ok((true, format!("found in Root ({scope:?})")));
        }
    }
    Ok((false, "not installed".into()))
}
