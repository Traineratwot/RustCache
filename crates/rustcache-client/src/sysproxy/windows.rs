//! Windows system proxy: HKCU Internet Settings.

use anyhow::Result;
use serde_json::{Value, json};

use crate::ca::{ensure_success, run_cmd};

const REG_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

fn reg_query(name: &str) -> Option<String> {
    let out = run_cmd("reg", &["query", &format!("HKCU\\{REG_PATH}"), "/v", name]).ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let _ = parts.next();
        let key = parts.next().unwrap_or("");
        let _ty = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("");
        if key.eq_ignore_ascii_case(name) {
            return Some(val.to_string());
        }
    }
    None
}

fn reg_add(name: &str, kind: &str, value: &str) -> Result<()> {
    let out = run_cmd(
        "reg",
        &[
            "add",
            &format!("HKCU\\{REG_PATH}"),
            "/v",
            name,
            "/t",
            kind,
            "/d",
            value,
            "/f",
        ],
    )?;
    ensure_success(&out, &format!("reg add {name}"))?;
    Ok(())
}

fn wininet_refresh() {
    let _ = run_cmd(
        "rundll32",
        &["shell32.dll,Control_RunDLL", "inetcpl.cpl,,0"],
    );
}

pub fn snapshot() -> Result<Value> {
    Ok(json!({
        "ProxyEnable": reg_query("ProxyEnable"),
        "ProxyServer": reg_query("ProxyServer"),
        "ProxyOverride": reg_query("ProxyOverride"),
    }))
}

pub fn apply(host: &str, port: u16, bypass: &[String]) -> Result<String> {
    let server = format!("http={host}:{port};https={host}:{port}");
    let mut override_list = vec![
        "<local>".to_string(),
        "localhost".into(),
        "127.0.0.1".into(),
    ];
    for b in bypass {
        if !b.trim().is_empty() {
            override_list.push(b.trim().to_string());
        }
    }
    let over = override_list.join(";");
    reg_add("ProxyEnable", "REG_DWORD", "1")?;
    reg_add("ProxyServer", "REG_SZ", &server)?;
    reg_add("ProxyOverride", "REG_SZ", &over)?;
    wininet_refresh();
    Ok(format!("HKCU ProxyServer={server}"))
}

pub fn restore(prev: &Value) -> Result<String> {
    if let Some(v) = prev.get("ProxyEnable").and_then(|x| x.as_str()) {
        let _ = reg_add("ProxyEnable", "REG_DWORD", v.trim_start_matches("0x"));
    } else {
        let _ = reg_add("ProxyEnable", "REG_DWORD", "0");
    }
    if let Some(v) = prev.get("ProxyServer").and_then(|x| x.as_str()) {
        let _ = reg_add("ProxyServer", "REG_SZ", v);
    }
    if let Some(v) = prev.get("ProxyOverride").and_then(|x| x.as_str()) {
        let _ = reg_add("ProxyOverride", "REG_SZ", v);
    }
    wininet_refresh();
    Ok("HKCU proxy restored".into())
}

pub fn status() -> Result<String> {
    let en = reg_query("ProxyEnable").unwrap_or_else(|| "?".into());
    let server = reg_query("ProxyServer").unwrap_or_else(|| "?".into());
    Ok(format!("ProxyEnable={en} ProxyServer={server}"))
}

pub fn dry_run(host: &str, port: u16, bypass: &[String]) -> Result<String> {
    let server = format!("http={host}:{port};https={host}:{port}");
    let mut override_list = vec![
        "<local>".to_string(),
        "localhost".into(),
        "127.0.0.1".into(),
    ];
    for b in bypass {
        if !b.trim().is_empty() {
            override_list.push(b.trim().to_string());
        }
    }
    Ok(format!(
        "reg add HKCU\\{REG_PATH} /v ProxyEnable /t REG_DWORD /d 1 /f\n\
         reg add HKCU\\{REG_PATH} /v ProxyServer /t REG_SZ /d \"{server}\" /f\n\
         reg add HKCU\\{REG_PATH} /v ProxyOverride /t REG_SZ /d \"{}\" /f",
        override_list.join(";")
    ))
}
