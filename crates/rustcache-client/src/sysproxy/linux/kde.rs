//! KDE Plasma system proxy via `kioslaverc` (`kwriteconfig6`/`kwriteconfig5`).

use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use crate::ca::run_cmd;
use crate::sysproxy::handler::SysProxyHandler;

const GROUP: &str = "Proxy Settings";

pub struct KdeHandler;

fn config_file() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        });
    base.join("kioslaverc")
}

/// Prefer `kwriteconfig6`, fall back to `kwriteconfig5`.
fn kwrite_tool() -> Option<&'static str> {
    ["kwriteconfig6", "kwriteconfig5"]
        .into_iter()
        .find(|&tool| run_cmd(tool, &["--help"]).is_ok() || which(tool))
}

fn kread_tool() -> Option<&'static str> {
    ["kreadconfig6", "kreadconfig5"]
        .into_iter()
        .find(|&tool| which(tool))
}

fn which(tool: &str) -> bool {
    std::process::Command::new("sh")
        .args(["-c", &format!("command -v {tool} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn kread(key: &str) -> Option<String> {
    let tool = kread_tool()?;
    let file = config_file();
    let out = run_cmd(
        tool,
        &["--file", file.to_str()?, "--group", GROUP, "--key", key],
    )
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn kwrite(key: &str, value: &str) -> Result<()> {
    let Some(tool) = kwrite_tool() else {
        return write_kioslaverc_direct(key, Some(value));
    };
    let file = config_file();
    let out = run_cmd(
        tool,
        &[
            "--file",
            file.to_str().unwrap_or("kioslaverc"),
            "--group",
            GROUP,
            "--key",
            key,
            value,
        ],
    )?;
    if !out.status.success() {
        anyhow::bail!(
            "{tool} {key}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn kdelete(key: &str) -> Result<()> {
    let Some(tool) = kwrite_tool() else {
        return write_kioslaverc_direct(key, None);
    };
    let file = config_file();
    // kwriteconfig --delete removes the key
    let _ = run_cmd(
        tool,
        &[
            "--file",
            file.to_str().unwrap_or("kioslaverc"),
            "--group",
            GROUP,
            "--key",
            key,
            "--delete",
        ],
    );
    Ok(())
}

/// Direct INI write when kwriteconfig is not available.
fn write_kioslaverc_direct(key: &str, value: Option<&str>) -> Result<()> {
    use std::io::Write;

    let path = config_file();
    let mut content = std::fs::read_to_string(&path).unwrap_or_default();

    // Ensure the group header exists.
    if !content.lines().any(|l| l.trim() == format!("[{GROUP}]")) {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&format!("[{GROUP}]\n"));
    }

    // Find the group block and replace / remove the key.
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut in_group = false;
    let mut key_written = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in lines.drain(..) {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if in_group && !key_written {
                if let Some(v) = value {
                    out_lines.push(format!("{key}={v}"));
                    key_written = true;
                }
            }
            in_group = t == format!("[{GROUP}]");
            out_lines.push(line);
            continue;
        }
        if in_group && t.starts_with(&format!("{key}=")) {
            if let Some(v) = value {
                out_lines.push(format!("{key}={v}"));
                key_written = true;
            }
            // value==None → drop the line (delete)
            continue;
        }
        out_lines.push(line);
    }
    if in_group && !key_written {
        if let Some(v) = value {
            out_lines.push(format!("{key}={v}"));
        }
    }

    let mut f = std::fs::File::create(&path)?;
    writeln!(f, "{}", out_lines.join("\n"))?;
    Ok(())
}

/// Best-effort notify KDE to re-read proxy config. Errors are swallowed.
fn notify_kde() {
    let _ = run_cmd(
        "dbus-send",
        &[
            "--type=signal",
            "--session",
            "/KIO/Scheduler",
            "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
            "string:\"\"",
        ],
    );
    let _ = run_cmd(
        "qdbus",
        &[
            "org.kde.KIO.Scheduler",
            "/KIO/Scheduler",
            "reparseSlaveConfiguration",
            "",
        ],
    );
}

/// Format `host port` (space-separated) as KDE expects.
fn host_port_value(host: &str, port: u16) -> String {
    format!("{host} {port}")
}

fn parse_host_port(raw: &str) -> Option<(String, u16)> {
    let mut parts = raw.split_whitespace();
    let host = parts.next()?.to_string();
    let port = parts.next()?.parse().ok()?;
    Some((host, port))
}

fn build_bypass_list(bypass: &[String]) -> String {
    let mut items = vec![
        "localhost".to_string(),
        "127.0.0.1".into(),
        "::1".into(),
        "*.local".into(),
    ];
    for b in bypass {
        let b = b.trim();
        if !b.is_empty() && !items.contains(&b.to_string()) {
            items.push(b.to_string());
        }
    }
    items.join(",")
}

impl SysProxyHandler for KdeHandler {
    fn id(&self) -> &'static str {
        "kde"
    }

    fn snapshot(&self) -> Result<Value> {
        Ok(json!({
            "ProxyType": kread("ProxyType"),
            "httpProxy": kread("httpProxy"),
            "httpsProxy": kread("httpsProxy"),
            "ftpProxy": kread("ftpProxy"),
            "socksProxy": kread("socksProxy"),
            "NoProxyFor": kread("NoProxyFor"),
        }))
    }

    fn apply(&self, host: &str, port: u16, bypass: &[String]) -> Result<String> {
        let hp = host_port_value(host, port);
        kwrite("httpProxy", &hp)?;
        kwrite("httpsProxy", &hp)?;
        kwrite("NoProxyFor", &build_bypass_list(bypass))?;
        kwrite("ProxyType", "1")?;
        notify_kde();
        Ok(format!("kde manual proxy {host}:{port}"))
    }

    fn restore(&self, prev: &Value) -> Result<String> {
        for key in [
            "ProxyType",
            "httpProxy",
            "httpsProxy",
            "ftpProxy",
            "socksProxy",
            "NoProxyFor",
        ] {
            match prev.get(key).and_then(|v| v.as_str()) {
                Some(val) => kwrite(key, val)?,
                None => kdelete(key)?,
            }
        }
        notify_kde();
        Ok("kde proxy restored".into())
    }

    fn status(&self) -> Result<String> {
        let ty = kread("ProxyType").unwrap_or_else(|| "?".into());
        let http = kread("httpProxy").unwrap_or_else(|| "-".into());
        Ok(format!("ProxyType={ty} http={http}"))
    }

    fn dry_run(&self, host: &str, port: u16, bypass: &[String]) -> Result<String> {
        let tool = kwrite_tool().unwrap_or("kwriteconfig6");
        let file = config_file();
        let file_s = file.to_str().unwrap_or("kioslaverc");
        let hp = host_port_value(host, port);
        let nobypass = build_bypass_list(bypass);
        let lines = [
            format!("{tool} --file {file_s} --group '{GROUP}' --key httpProxy '{hp}'"),
            format!("{tool} --file {file_s} --group '{GROUP}' --key httpsProxy '{hp}'"),
            format!("{tool} --file {file_s} --group '{GROUP}' --key NoProxyFor '{nobypass}'"),
            format!("{tool} --file {file_s} --group '{GROUP}' --key ProxyType 1"),
            "dbus-send --type=signal --session /KIO/Scheduler org.kde.KIO.Scheduler.reparseSlaveConfiguration string:\"\"".into(),
        ];
        Ok(lines.join("\n"))
    }

    fn current_proxy(&self) -> Result<Option<(String, u16)>> {
        let ty = kread("ProxyType").unwrap_or_default();
        if ty != "1" {
            return Ok(None);
        }
        let raw = kread("httpProxy").unwrap_or_default();
        Ok(parse_host_port(&raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_roundtrip() {
        let v = host_port_value("127.0.0.1", 31280);
        assert_eq!(v, "127.0.0.1 31280");
        let (h, p) = parse_host_port(&v).unwrap();
        assert_eq!(h, "127.0.0.1");
        assert_eq!(p, 31280);
    }

    #[test]
    fn bypass_list_includes_defaults() {
        let l = build_bypass_list(&["example.com".into()]);
        assert!(l.contains("localhost"));
        assert!(l.contains("127.0.0.1"));
        assert!(l.contains("*.local"));
        assert!(l.contains("example.com"));
    }

    #[test]
    fn dry_run_contains_kwrite_and_dbus() {
        let h = KdeHandler;
        let s = h.dry_run("127.0.0.1", 31280, &[]).unwrap();
        assert!(s.contains("httpProxy '127.0.0.1 31280'"));
        assert!(s.contains("ProxyType 1"));
        assert!(s.contains("dbus-send"));
        assert!(s.contains("kde") || s.contains("kwriteconfig"));
    }
}
