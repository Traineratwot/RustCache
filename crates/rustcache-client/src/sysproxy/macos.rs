//! macOS system proxy via `networksetup` (per network service).

use anyhow::Result;
use serde_json::{Value, json};

use crate::ca::run_cmd;

fn services() -> Vec<String> {
    let Ok(out) = run_cmd("networksetup", &["-listallnetworkservices"]) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    for (i, line) in String::from_utf8_lossy(&out.stdout).lines().enumerate() {
        if i == 0 {
            continue;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('*') {
            continue;
        }
        v.push(line.to_string());
    }
    v
}

fn get_proxy(svc: &str, flag: &str) -> Result<Value> {
    let out = run_cmd("networksetup", &[flag, svc])?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut enabled = false;
    let mut server = String::new();
    let mut port = 0u16;
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Enabled: ") {
            enabled = v.trim() == "Yes";
        } else if let Some(v) = l.strip_prefix("Server: ") {
            server = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("Port: ") {
            port = v.trim().parse().unwrap_or(0);
        }
    }
    Ok(json!({"enabled": enabled, "server": server, "port": port}))
}

pub fn snapshot() -> Result<Value> {
    let mut services_json = json!({});
    for svc in services() {
        let web = get_proxy(&svc, "-getwebproxy").unwrap_or(json!({}));
        let sec = get_proxy(&svc, "-getsecurewebproxy").unwrap_or(json!({}));
        services_json[&svc] = json!({"web": web, "secure": sec});
    }
    Ok(json!({"services": services_json}))
}

pub fn apply(host: &str, port: u16, bypass: &[String]) -> Result<String> {
    let port_s = port.to_string();
    let mut applied = 0;
    for svc in services() {
        for set_flag in ["-setwebproxy", "-setsecurewebproxy"] {
            let out = run_cmd("networksetup", &[set_flag, &svc, host, &port_s])?;
            if !out.status.success() {
                anyhow::bail!(
                    "networksetup {set_flag} {svc}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
        }
        for on_flag in ["-setwebproxystate", "-setsecurewebproxystate"] {
            let _ = run_cmd("networksetup", &[on_flag, &svc, "on"]);
        }
        let mut args = vec!["-setproxybypassdomains".to_string(), svc.clone()];
        for b in bypass {
            if !b.trim().is_empty() {
                args.push(b.trim().to_string());
            }
        }
        args.push("localhost".into());
        args.push("127.0.0.1".into());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let _ = run_cmd("networksetup", &refs);
        applied += 1;
    }
    Ok(format!("applied to {applied} network services"))
}

pub fn restore(prev: &Value) -> Result<String> {
    let Some(services_json) = prev.get("services").and_then(|s| s.as_object()) else {
        for svc in services() {
            let _ = run_cmd("networksetup", &["-setwebproxystate", &svc, "off"]);
            let _ = run_cmd("networksetup", &["-setsecurewebproxystate", &svc, "off"]);
        }
        return Ok("proxies disabled on all services".into());
    };
    for (svc, snap) in services_json {
        for (flag, key, state_flag) in [
            ("-setwebproxy", "web", "-setwebproxystate"),
            ("-setsecurewebproxy", "secure", "-setsecurewebproxystate"),
        ] {
            let Some(cfg) = snap.get(key) else {
                continue;
            };
            let enabled = cfg
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if enabled {
                let server = cfg
                    .get("server")
                    .and_then(|v| v.as_str())
                    .unwrap_or("127.0.0.1");
                let port = cfg.get("port").and_then(|v| v.as_u64()).unwrap_or(0);
                let _ = run_cmd("networksetup", &[flag, svc, server, &port.to_string()]);
                let _ = run_cmd("networksetup", &[state_flag, svc, "on"]);
            } else {
                let _ = run_cmd("networksetup", &[state_flag, svc, "off"]);
            }
        }
    }
    Ok("networksetup proxy restored".into())
}

pub fn status() -> Result<String> {
    let mut bits = Vec::new();
    for svc in services() {
        if let Ok(web) = get_proxy(&svc, "-getwebproxy") {
            let en = web
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let server = web.get("server").and_then(|v| v.as_str()).unwrap_or("");
            let port = web.get("port").and_then(|v| v.as_u64()).unwrap_or(0);
            if en {
                bits.push(format!("{svc}={server}:{port}"));
            }
        }
    }
    if bits.is_empty() {
        Ok("no service with web proxy enabled".into())
    } else {
        Ok(bits.join(", "))
    }
}

pub fn dry_run(host: &str, port: u16, bypass: &[String]) -> Result<String> {
    let mut lines = Vec::new();
    for svc in services() {
        lines.push(format!("networksetup -setwebproxy \"{svc}\" {host} {port}"));
        lines.push(format!(
            "networksetup -setsecurewebproxy \"{svc}\" {host} {port}"
        ));
        lines.push(format!("networksetup -setwebproxystate \"{svc}\" on"));
        lines.push(format!("networksetup -setsecurewebproxystate \"{svc}\" on"));
        let mut b = vec![host.to_string()];
        b.extend(bypass.iter().cloned());
        b.push("localhost".into());
        b.push("127.0.0.1".into());
        lines.push(format!(
            "networksetup -setproxybypassdomains \"{svc}\" {}",
            b.join(" ")
        ));
    }
    if lines.is_empty() {
        lines.push("# no network services found".into());
    }
    Ok(lines.join("\n"))
}
