//! Linux system proxy: GNOME gsettings (MVP). KDE is Phase 4.

use anyhow::Result;
use serde_json::{Value, json};

use crate::ca::run_cmd;

const PROXY_SCHEMA: &str = "org.gnome.system.proxy";
const HTTP_SCHEMA: &str = "org.gnome.system.proxy.http";
const HTTPS_SCHEMA: &str = "org.gnome.system.proxy.https";

fn gsettings_get(schema: &str, key: &str) -> Result<String> {
    let out = run_cmd("gsettings", &["get", schema, key])?;
    if !out.status.success() {
        anyhow::bail!(
            "gsettings get {schema} {key}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn gsettings_set(schema: &str, key: &str, value: &str) -> Result<()> {
    let out = run_cmd("gsettings", &["set", schema, key, value])?;
    if !out.status.success() {
        anyhow::bail!(
            "gsettings set {schema} {key} {value}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn have_gsettings() -> bool {
    run_cmd("gsettings", &["--version"]).is_ok()
}

pub fn snapshot() -> Result<Value> {
    if !have_gsettings() {
        return Ok(json!({ "available": false }));
    }
    Ok(json!({
        "available": true,
        "mode": gsettings_get(PROXY_SCHEMA, "mode").ok(),
        "http_host": gsettings_get(HTTP_SCHEMA, "host").ok(),
        "http_port": gsettings_get(HTTP_SCHEMA, "port").ok(),
        "https_host": gsettings_get(HTTPS_SCHEMA, "host").ok(),
        "https_port": gsettings_get(HTTPS_SCHEMA, "port").ok(),
        "ignore_hosts": gsettings_get(PROXY_SCHEMA, "ignore-hosts").ok(),
    }))
}

pub fn apply(host: &str, port: u16, bypass: &[String]) -> Result<String> {
    if !have_gsettings() {
        anyhow::bail!(
            "gsettings not found — no GNOME session? set http_proxy/https_proxy manually"
        );
    }
    let host_q = format!("'{host}'");
    let port_s = port.to_string();
    gsettings_set(HTTP_SCHEMA, "host", &host_q)?;
    gsettings_set(HTTP_SCHEMA, "port", &port_s)?;
    gsettings_set(HTTPS_SCHEMA, "host", &host_q)?;
    gsettings_set(HTTPS_SCHEMA, "port", &port_s)?;

    // ignore-hosts: always keep loopback + user bypass list
    let mut hosts: Vec<String> = vec!["'localhost'".into(), "'127.0.0.0/8'".into(), "'::1'".into()];
    for b in bypass {
        let b = b.trim();
        if b.is_empty() {
            continue;
        }
        // gsettings wants GVariant string array
        hosts.push(format!("'{b}'"));
    }
    let arr = format!("[{}]", hosts.join(", "));
    gsettings_set(PROXY_SCHEMA, "ignore-hosts", &arr)?;
    gsettings_set(PROXY_SCHEMA, "mode", "'manual'")?;
    Ok(format!("gnome manual proxy {host}:{port}"))
}

pub fn restore(prev: &Value) -> Result<String> {
    if !have_gsettings() {
        return Ok("gsettings not available — skip restore".into());
    }
    if prev.get("available").and_then(|v| v.as_bool()) == Some(false) {
        return Ok("no previous gsettings snapshot".into());
    }
    if let Some(mode) = prev.get("mode").and_then(|v| v.as_str()) {
        gsettings_set(PROXY_SCHEMA, "mode", mode)?;
    } else {
        gsettings_set(PROXY_SCHEMA, "mode", "'none'")?;
    }
    for (schema, key, field) in [
        (HTTP_SCHEMA, "host", "http_host"),
        (HTTP_SCHEMA, "port", "http_port"),
        (HTTPS_SCHEMA, "host", "https_host"),
        (HTTPS_SCHEMA, "port", "https_port"),
    ] {
        if let Some(val) = prev.get(field).and_then(|v| v.as_str()) {
            let _ = gsettings_set(schema, key, val);
        }
    }
    if let Some(ig) = prev.get("ignore_hosts").and_then(|v| v.as_str()) {
        let _ = gsettings_set(PROXY_SCHEMA, "ignore-hosts", ig);
    }
    Ok("gnome proxy restored".into())
}

pub fn status() -> Result<String> {
    if !have_gsettings() {
        return Ok("gsettings not available".into());
    }
    let mode = gsettings_get(PROXY_SCHEMA, "mode").unwrap_or_else(|_| "?".into());
    let host = gsettings_get(HTTP_SCHEMA, "host").unwrap_or_else(|_| "?".into());
    let port = gsettings_get(HTTP_SCHEMA, "port").unwrap_or_else(|_| "?".into());
    Ok(format!("mode={mode} http={host}:{port}"))
}

pub fn dry_run(host: &str, port: u16, bypass: &[String]) -> Result<String> {
    let mut hosts = vec![
        "'localhost'".to_string(),
        "'127.0.0.0/8'".to_string(),
        "'::1'".to_string(),
    ];
    for b in bypass {
        if !b.trim().is_empty() {
            hosts.push(format!("'{}'", b.trim()));
        }
    }
    let lines = [
        format!("gsettings set {HTTP_SCHEMA} host '{host}'"),
        format!("gsettings set {HTTP_SCHEMA} port {port}"),
        format!("gsettings set {HTTPS_SCHEMA} host '{host}'"),
        format!("gsettings set {HTTPS_SCHEMA} port {port}"),
        format!(
            "gsettings set {PROXY_SCHEMA} ignore-hosts \"[{}]\"",
            hosts.join(",")
        ),
        format!("gsettings set {PROXY_SCHEMA} mode 'manual'"),
    ];
    Ok(lines.join("\n"))
}
