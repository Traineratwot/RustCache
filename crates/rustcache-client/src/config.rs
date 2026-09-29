//! Client configuration (`client.toml`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaptureMode {
    #[default]
    Off,
    System,
    /// Full capture via TUN — Phase 3.
    Tun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CaScope {
    #[default]
    User,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaConfig {
    /// `api` = GET /api/ca.crt, `file` = local PEM path.
    #[serde(default = "default_ca_source")]
    pub source: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub scope: CaScope,
}

fn default_ca_source() -> String {
    "api".into()
}

impl Default for CaConfig {
    fn default() -> Self {
        Self {
            source: default_ca_source(),
            file: String::new(),
            scope: CaScope::User,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file: String,
}

fn default_log_level() -> String {
    "info".into()
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    /// Base URL of the RustCache REST API.
    pub rustcache_api: String,
    /// Local resilient proxy bind address.
    pub listen: String,
    pub mode: CaptureMode,
    /// When true (default), fall back to DIRECT if RustCache is down.
    pub fail_open: bool,
    pub health_interval_ms: u64,
    pub breaker_failures: u32,
    pub breaker_open_ms: u64,
    pub proxy_bypass: Vec<String>,
    pub ca: CaConfig,
    pub log: LogConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            rustcache_api: "http://127.0.0.1:8080".into(),
            listen: "127.0.0.1:31280".into(),
            mode: CaptureMode::System,
            fail_open: true,
            health_interval_ms: 2000,
            breaker_failures: 3,
            breaker_open_ms: 5000,
            proxy_bypass: vec!["localhost".into(), "127.0.0.1".into(), "*.local".into()],
            ca: CaConfig::default(),
            log: LogConfig::default(),
        }
    }
}

impl ClientConfig {
    /// Default config path: `~/.config/rustcache/client.toml` (Windows: `%APPDATA%\rustcache\client.toml`).
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustcache")
            .join("client.toml")
    }

    /// Directory for state snapshots (proxy-restore.json).
    pub fn state_dir() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rustcache")
            .join("client")
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let cfg: Self =
            toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(path, raw).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if !self.rustcache_api.starts_with("http://") && !self.rustcache_api.starts_with("https://")
        {
            bail!(
                "rustcache_api must be http(s) URL, got {:?}",
                self.rustcache_api
            );
        }
        if self.listen.is_empty() {
            bail!("listen must be host:port");
        }
        if self.health_interval_ms < 100 {
            bail!("health_interval_ms must be >= 100");
        }
        if self.breaker_failures == 0 {
            bail!("breaker_failures must be >= 1");
        }
        Ok(())
    }

    /// Listen host:port split for binding.
    pub fn listen_addr(&self) -> Result<(String, u16)> {
        parse_host_port(&self.listen)
    }

    /// Upstream RustCache ports derived from the API base URL host (same machine by default).
    pub fn upstream_http_port(&self) -> u16 {
        3128
    }

    pub fn upstream_https_port(&self) -> u16 {
        3129
    }

    /// Host portion of `rustcache_api` (usually 127.0.0.1).
    pub fn api_host(&self) -> String {
        self.rustcache_api
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("127.0.0.1")
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string()
    }
}

/// Parse `host:port` (IPv6 in brackets supported).
pub fn parse_host_port(s: &str) -> Result<(String, u16)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, port) = rest
            .split_once("]:")
            .with_context(|| format!("bad [ipv6]:port {s}"))?;
        let port: u16 = port.parse().with_context(|| format!("bad port in {s}"))?;
        Ok((host.to_string(), port))
    } else {
        let (host, port) = s
            .rsplit_once(':')
            .with_context(|| format!("bad host:port {s}"))?;
        let port: u16 = port.parse().with_context(|| format!("bad port in {s}"))?;
        Ok((host.to_string(), port))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        ClientConfig::default().validate().unwrap();
    }

    #[test]
    fn parse_listen_v4_and_v6() {
        assert_eq!(
            parse_host_port("127.0.0.1:31280").unwrap(),
            ("127.0.0.1".into(), 31280)
        );
        assert_eq!(
            parse_host_port("[::1]:31280").unwrap(),
            ("::1".into(), 31280)
        );
    }

    #[test]
    fn load_missing_is_default() {
        let cfg = ClientConfig::load(Path::new("/nonexistent/client.toml")).unwrap();
        assert_eq!(cfg.listen, "127.0.0.1:31280");
    }
}
