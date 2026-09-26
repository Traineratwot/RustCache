//! Config validation: ranges, bind formats, port conflicts/availability, paths.
//!
//! Lives next to the schema (not in the HTTP layer) so every writer of config —
//! API, tests, future CLI — shares one rule set.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Config;

/// One validation problem, keyed to a Settings form field.
#[derive(Debug, Clone, Serialize)]
pub struct FieldIssue {
    pub field: &'static str,
    pub message: String,
}

fn issue(field: &'static str, message: impl Into<String>) -> FieldIssue {
    FieldIssue {
        field,
        message: message.into(),
    }
}

/// Fields that only take effect after a process restart.
pub fn restart_fields_diff(old: &Config, new: &Config) -> Vec<&'static str> {
    let mut v = Vec::new();
    if old.data_dir != new.data_dir {
        v.push("data_dir");
    }
    if old.http.port != new.http.port {
        v.push("http.port");
    }
    if old.https.port != new.https.port {
        v.push("https.port");
    }
    if old.socks5.port != new.socks5.port {
        v.push("socks5.port");
    }
    if old.api.bind != new.api.bind {
        v.push("api.bind");
    }
    if old.cache.dir != new.cache.dir {
        v.push("cache.dir");
    }
    if old.ca.dir != new.ca.dir {
        v.push("ca.dir");
    }
    if old.logs.db_path != new.logs.db_path {
        v.push("logs.db_path");
    }
    if old.pac.enabled != new.pac.enabled {
        v.push("pac.enabled");
    }
    if old.pac.bind != new.pac.bind {
        v.push("pac.bind");
    }
    v
}

/// Ports this process currently listens on (from the live/old config).
fn bound_ports(old: &Config) -> Vec<u16> {
    let mut v = vec![old.http.port, old.https.port, old.socks5.port];
    if let Ok(sa) = old.api.bind.parse::<std::net::SocketAddr>() {
        v.push(sa.port());
    }
    if old.pac.enabled {
        if let Ok(sa) = old.pac.bind.parse::<std::net::SocketAddr>() {
            v.push(sa.port());
        }
    }
    v
}

/// True when nothing else holds the port. Ports we currently hold are OK —
/// they are released on restart, which is when the new value takes effect.
fn port_available(port: u16, ours: &[u16]) -> bool {
    if ours.contains(&port) {
        return true;
    }
    std::net::TcpListener::bind(("0.0.0.0", port)).is_ok()
}

fn bind_available(bind: &str, ours: &[u16]) -> bool {
    match bind.parse::<std::net::SocketAddr>() {
        Ok(sa) => {
            if ours.contains(&sa.port()) {
                return true;
            }
            std::net::TcpListener::bind(sa).is_ok()
        }
        Err(_) => false,
    }
}

fn probe_dir_writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".rustcache-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn nearest_existing(path: &Path) -> Option<&Path> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.as_os_str().is_empty() {
            cur = p.parent();
            continue;
        }
        if p.exists() {
            return Some(p);
        }
        cur = p.parent();
    }
    None
}

fn check_dir_field(issues: &mut Vec<FieldIssue>, field: &'static str, path: &Path) {
    if path.as_os_str().is_empty() {
        issues.push(issue(field, "path must not be empty"));
        return;
    }
    if path.exists() {
        if !path.is_dir() {
            issues.push(issue(
                field,
                format!("{} exists but is not a directory", path.display()),
            ));
            return;
        }
        if let Err(e) = probe_dir_writable(path) {
            issues.push(issue(
                field,
                format!("{} is not writable: {e}", path.display()),
            ));
        }
        return;
    }
    match nearest_existing(path) {
        None => issues.push(issue(
            field,
            format!("{}: no existing parent directory", path.display()),
        )),
        Some(anc) => {
            if !anc.is_dir() {
                issues.push(issue(
                    field,
                    format!("{} exists but is not a directory", anc.display()),
                ));
                return;
            }
            if let Err(e) = probe_dir_writable(anc) {
                issues.push(issue(
                    field,
                    format!(
                        "cannot create {}: parent {} is not writable: {e}",
                        path.display(),
                        anc.display()
                    ),
                ));
            }
        }
    }
}

fn check_file_field(issues: &mut Vec<FieldIssue>, field: &'static str, path: &Path) {
    if path.as_os_str().is_empty() {
        issues.push(issue(field, "path must not be empty"));
        return;
    }
    if path.exists() {
        if path.is_dir() {
            issues.push(issue(
                field,
                format!("{} is a directory, expected a file", path.display()),
            ));
            return;
        }
        if let Err(e) = std::fs::OpenOptions::new().append(true).open(path) {
            issues.push(issue(
                field,
                format!("{} cannot be opened for write: {e}", path.display()),
            ));
        }
        return;
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            check_dir_field(issues, field, parent);
        }
    }
}

/// Validate a full config candidate against the live one.
///
/// Collects every issue (ranges, bind format, internal port conflicts, port
/// availability, path type/permissions) so the Settings form can highlight all
/// problems at once.
pub fn validate_config(new: &Config, old: &Config) -> Result<(), Vec<FieldIssue>> {
    let mut issues = Vec::new();

    if new.data_dir.trim().is_empty() {
        issues.push(issue("data_dir", "must not be empty"));
    }
    if new.http.port == 0 {
        issues.push(issue("http.port", "must be 1..=65535"));
    }
    if new.https.port == 0 {
        issues.push(issue("https.port", "must be 1..=65535"));
    }
    if new.socks5.port == 0 {
        issues.push(issue("socks5.port", "must be 1..=65535"));
    }
    if new.cache.dir.trim().is_empty() {
        issues.push(issue("cache.dir", "must not be empty"));
    }
    if new.cache.max_bytes == 0 {
        issues.push(issue("cache.max_bytes", "must be > 0"));
    }
    if new.cache.max_object_bytes == 0 {
        issues.push(issue("cache.max_object_bytes", "must be > 0"));
    }
    if new.cache.max_object_bytes > new.cache.max_bytes {
        issues.push(issue(
            "cache.max_object_bytes",
            "must be <= cache.max_bytes",
        ));
    }
    if new.ca.dir.trim().is_empty() {
        issues.push(issue("ca.dir", "must not be empty"));
    }
    if new.logs.db_path.trim().is_empty() {
        issues.push(issue("logs.db_path", "must not be empty"));
    }
    if !(1..=10_000_000).contains(&new.logs.max_rows) {
        issues.push(issue("logs.max_rows", "must be 1..=10000000"));
    }
    if !(1..=3650).contains(&new.logs.max_age_days) {
        issues.push(issue("logs.max_age_days", "must be 1..=3650"));
    }
    if !(10..=86_400).contains(&new.logs.cleanup_interval_secs) {
        issues.push(issue("logs.cleanup_interval_secs", "must be 10..=86400"));
    }

    // Bind format first — availability checks need a parsed addr.
    let api_bind_ok = if new.api.bind.trim().is_empty() {
        issues.push(issue("api.bind", "must not be empty"));
        false
    } else if new.api.bind.parse::<std::net::SocketAddr>().is_err() {
        issues.push(issue("api.bind", "must be host:port, e.g. 127.0.0.1:8080"));
        false
    } else {
        true
    };
    let pac_bind_ok = if new.pac.bind.trim().is_empty() {
        issues.push(issue("pac.bind", "must not be empty"));
        false
    } else if new.pac.bind.parse::<std::net::SocketAddr>().is_err() {
        issues.push(issue("pac.bind", "must be host:port, e.g. 0.0.0.0:8081"));
        false
    } else {
        true
    };

    // preferred_ip is hot (regenerated per PAC request) — not in restart_fields_diff.
    {
        let v = new.pac.preferred_ip.trim();
        if !v.is_empty() && !is_valid_preferred_host(v) {
            issues.push(issue(
                "pac.preferred_ip",
                "must be an IP address or hostname, or empty for auto",
            ));
        }
    }

    // Internal conflicts: two listeners cannot share one port.
    {
        let mut used: Vec<(&'static str, u16)> = Vec::new();
        let mut claim = |issues: &mut Vec<FieldIssue>, field: &'static str, port: u16| {
            if port == 0 {
                return;
            }
            if let Some((other, _)) = used.iter().find(|(_, p)| *p == port) {
                issues.push(issue(
                    field,
                    format!("port {port} is already used by {other}"),
                ));
            } else {
                used.push((field, port));
            }
        };
        claim(&mut issues, "http.port", new.http.port);
        claim(&mut issues, "https.port", new.https.port);
        claim(&mut issues, "socks5.port", new.socks5.port);
        if api_bind_ok {
            if let Ok(sa) = new.api.bind.parse::<std::net::SocketAddr>() {
                claim(&mut issues, "api.bind", sa.port());
            }
        }
        if new.pac.enabled && pac_bind_ok {
            if let Ok(sa) = new.pac.bind.parse::<std::net::SocketAddr>() {
                claim(&mut issues, "pac.bind", sa.port());
            }
        }
    }

    // Port availability — only for values that change; current ports stay ours
    // until restart. Unchanged binds are already held by this process.
    let ours = bound_ports(old);
    if new.http.port != old.http.port && new.http.port != 0 && !port_available(new.http.port, &ours)
    {
        issues.push(issue(
            "http.port",
            format!("port {} is already in use", new.http.port),
        ));
    }
    if new.https.port != old.https.port
        && new.https.port != 0
        && !port_available(new.https.port, &ours)
    {
        issues.push(issue(
            "https.port",
            format!("port {} is already in use", new.https.port),
        ));
    }
    if new.socks5.port != old.socks5.port
        && new.socks5.port != 0
        && !port_available(new.socks5.port, &ours)
    {
        issues.push(issue(
            "socks5.port",
            format!("port {} is already in use", new.socks5.port),
        ));
    }
    if api_bind_ok && new.api.bind != old.api.bind && !bind_available(&new.api.bind, &ours) {
        issues.push(issue(
            "api.bind",
            format!("address {} is already in use", new.api.bind),
        ));
    }
    // PAC listener only binds when enabled.
    if new.pac.enabled
        && pac_bind_ok
        && new.pac.bind != old.pac.bind
        && !bind_available(&new.pac.bind, &ours)
    {
        issues.push(issue(
            "pac.bind",
            format!("address {} is already in use", new.pac.bind),
        ));
    }

    // Paths: type + create/write permissions.
    if !new.data_dir.trim().is_empty() {
        check_dir_field(&mut issues, "data_dir", &new.data_dir_path());
    }
    if !new.cache.dir.trim().is_empty() {
        check_dir_field(&mut issues, "cache.dir", &new.cache_dir());
    }
    if !new.ca.dir.trim().is_empty() {
        check_dir_field(&mut issues, "ca.dir", &new.ca_dir());
    }
    if !new.logs.db_path.trim().is_empty() {
        check_file_field(&mut issues, "logs.db_path", &new.logs_db_path());
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

/// IP (optionally `[ipv6]`) or simple hostname: no scheme/port/path/spaces.
fn is_valid_preferred_host(v: &str) -> bool {
    let bare = v.trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    if v.starts_with('[') || v.ends_with(']') {
        return false;
    }
    if v.len() > 253 || v.contains(['/', ':', ' ']) {
        return false;
    }
    // Dotted-numeric must be a full IPv4, not an incomplete address like "1.2.3".
    if v.split('.')
        .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    !v.split('.').any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// Validate only the log-retention ranges (shared by `PUT /api/logs/settings`).
pub fn validate_log_settings(
    max_rows: Option<u64>,
    max_age_days: Option<u64>,
    cleanup_interval_secs: Option<u64>,
) -> Result<(), FieldIssue> {
    if let Some(v) = max_rows {
        if !(1..=10_000_000).contains(&v) {
            return Err(issue("max_rows", "must be 1..=10000000"));
        }
    }
    if let Some(v) = max_age_days {
        if !(1..=3650).contains(&v) {
            return Err(issue("max_age_days", "must be 1..=3650"));
        }
    }
    if let Some(v) = cleanup_interval_secs {
        if !(10..=86_400).contains(&v) {
            return Err(issue("cleanup_interval_secs", "must be 10..=86400"));
        }
    }
    Ok(())
}

/// Resolve a possibly-relative path under the data dir (used by tests/helpers).
pub fn resolve_path(data_dir: &str, p: &str) -> PathBuf {
    let base = PathBuf::from(data_dir);
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Config {
        let mut c = Config::default();
        c.data_dir = std::env::temp_dir()
            .join(format!("rc-validate-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::create_dir_all(&c.data_dir);
        c
    }

    #[test]
    fn accepts_default_config() {
        let old = base();
        let new = old.clone();
        assert!(validate_config(&new, &old).is_ok());
    }

    #[test]
    fn rejects_zero_ports_and_empty_paths() {
        let old = base();
        let mut new = old.clone();
        new.http.port = 0;
        new.cache.dir = String::new();
        let err = validate_config(&new, &old).unwrap_err();
        assert!(err.iter().any(|i| i.field == "http.port"));
        assert!(err.iter().any(|i| i.field == "cache.dir"));
    }

    #[test]
    fn rejects_internal_port_conflict() {
        let old = base();
        let mut new = old.clone();
        new.https.port = new.http.port;
        let err = validate_config(&new, &old).unwrap_err();
        assert!(err.iter().any(|i| i.message.contains("already used by")));
    }

    #[test]
    fn rejects_max_object_bigger_than_max_bytes() {
        let old = base();
        let mut new = old.clone();
        new.cache.max_bytes = 100;
        new.cache.max_object_bytes = 200;
        let err = validate_config(&new, &old).unwrap_err();
        assert!(err.iter().any(|i| i.field == "cache.max_object_bytes"));
    }

    #[test]
    fn rejects_bad_bind_format() {
        let old = base();
        let mut new = old.clone();
        new.api.bind = "not-an-addr".into();
        let err = validate_config(&new, &old).unwrap_err();
        assert!(err.iter().any(|i| i.field == "api.bind"));
    }

    #[test]
    fn log_settings_ranges() {
        assert!(validate_log_settings(Some(100), Some(7), Some(60)).is_ok());
        assert!(validate_log_settings(Some(0), None, None).is_err());
        assert!(validate_log_settings(None, Some(99999), None).is_err());
        assert!(validate_log_settings(None, None, Some(5)).is_err());
    }

    #[test]
    fn preferred_ip_empty_and_valid_ok() {
        let old = base();
        for v in ["", "   ", "192.168.1.5", "::1", "[::1]", "proxy.lan"] {
            let mut new = old.clone();
            new.pac.preferred_ip = v.into();
            assert!(
                validate_config(&new, &old).is_ok(),
                "expected ok for preferred_ip={v:?}"
            );
        }
    }

    #[test]
    fn preferred_ip_rejects_invalid() {
        let old = base();
        for v in ["http://x", "1.2.3.4:8080", "a b", "1.2.3", "-bad.host"] {
            let mut new = old.clone();
            new.pac.preferred_ip = v.into();
            let err = validate_config(&new, &old).unwrap_err();
            assert!(
                err.iter().any(|i| i.field == "pac.preferred_ip"),
                "expected FieldIssue for preferred_ip={v:?}"
            );
        }
    }

    #[test]
    fn preferred_ip_is_hot_not_restart() {
        let old = base();
        let mut new = old.clone();
        new.pac.preferred_ip = "10.0.0.5".into();
        assert!(restart_fields_diff(&old, &new).is_empty());
    }
}
