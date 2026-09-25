//! Effective runtime configuration (TOML).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub http: HttpConfig,
    #[serde(default)]
    pub https: HttpsConfig,
    #[serde(default)]
    pub socks5: Socks5Config,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub exclude: ExcludeConfig,
    #[serde(default)]
    pub ca: CaConfig,
    #[serde(default)]
    pub pac: PacConfig,
    #[serde(default)]
    pub logs: LogConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    #[serde(default = "default_http_port")]
    pub port: u16,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            port: default_http_port(),
        }
    }
}

fn default_http_port() -> u16 {
    3128
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpsConfig {
    #[serde(default = "default_https_port")]
    pub port: u16,
}

impl Default for HttpsConfig {
    fn default() -> Self {
        Self {
            port: default_https_port(),
        }
    }
}

fn default_https_port() -> u16 {
    3129
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Socks5Config {
    #[serde(default = "default_socks_port")]
    pub port: u16,
}

impl Default for Socks5Config {
    fn default() -> Self {
        Self {
            port: default_socks_port(),
        }
    }
}

fn default_socks_port() -> u16 {
    1080
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_bind")]
    pub bind: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: default_api_bind(),
        }
    }
}

fn default_api_bind() -> String {
    "127.0.0.1:8080".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_cache_dir")]
    pub dir: String,
    #[serde(default = "default_max_bytes")]
    pub max_bytes: u64,
    #[serde(default = "default_max_object_bytes")]
    pub max_object_bytes: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            dir: default_cache_dir(),
            max_bytes: default_max_bytes(),
            max_object_bytes: default_max_object_bytes(),
        }
    }
}

fn default_cache_dir() -> String {
    "~/.local/share/rustcache/cache".into()
}
fn default_max_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn default_max_object_bytes() -> u64 {
    50 * 1024 * 1024
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExcludeConfig {
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub cidrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaConfig {
    #[serde(default = "default_ca_dir")]
    pub dir: String,
}

impl Default for CaConfig {
    fn default() -> Self {
        Self {
            dir: default_ca_dir(),
        }
    }
}

fn default_ca_dir() -> String {
    "~/.local/share/rustcache/ca".into()
}

/// Proxy return strategy emitted into `FindProxyForURL`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum PacMode {
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "socks")]
    Socks,
    #[default]
    #[serde(rename = "http+socks")]
    HttpSocks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PacConfig {
    #[serde(default = "default_pac_enabled")]
    pub enabled: bool,
    #[serde(default = "default_pac_bind")]
    pub bind: String,
    #[serde(default)]
    pub mode: PacMode,
}

impl Default for PacConfig {
    fn default() -> Self {
        Self {
            enabled: default_pac_enabled(),
            bind: default_pac_bind(),
            mode: PacMode::default(),
        }
    }
}

fn default_pac_enabled() -> bool {
    true
}

fn default_pac_bind() -> String {
    "0.0.0.0:8081".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    #[serde(default = "default_logs_db_path")]
    pub db_path: String,
    #[serde(default = "default_logs_max_rows")]
    pub max_rows: u64,
    #[serde(default = "default_logs_max_age_days")]
    pub max_age_days: u64,
    #[serde(default = "default_logs_cleanup_interval_secs")]
    pub cleanup_interval_secs: u64,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            db_path: default_logs_db_path(),
            max_rows: default_logs_max_rows(),
            max_age_days: default_logs_max_age_days(),
            cleanup_interval_secs: default_logs_cleanup_interval_secs(),
        }
    }
}

fn default_logs_db_path() -> String {
    "~/.local/share/rustcache/logs.db".into()
}
fn default_logs_max_rows() -> u64 {
    10_000
}
fn default_logs_max_age_days() -> u64 {
    7
}
fn default_logs_cleanup_interval_secs() -> u64 {
    300
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if p == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(p)
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())?;
        let cfg: Config = toml::from_str(&text)?;
        Ok(cfg)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        if path.as_ref().exists() {
            Self::load(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn cache_dir(&self) -> PathBuf {
        expand_tilde(&self.cache.dir)
    }

    pub fn ca_dir(&self) -> PathBuf {
        expand_tilde(&self.ca.dir)
    }

    pub fn logs_db_path(&self) -> PathBuf {
        expand_tilde(&self.logs.db_path)
    }

    /// Redacted copy safe for `/api/config`.
    pub fn redacted(&self) -> Config {
        self.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_example() {
        let cfg: Config = toml::from_str(
            r#"
[http]
port = 3128
[https]
port = 3129
[socks5]
port = 1080
[api]
bind = "127.0.0.1:8080"
[cache]
dir = "/tmp/rc"
max_bytes = 100
max_object_bytes = 10
[exclude]
domains = ["*.local"]
cidrs = ["10.0.0.0/8"]
[ca]
dir = "/tmp/ca"
"#,
        )
        .unwrap();
        assert_eq!(cfg.http.port, 3128);
        assert_eq!(cfg.cache.max_bytes, 100);
        assert_eq!(cfg.exclude.domains, vec!["*.local"]);
    }

    #[test]
    fn defaults_apply() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.api.bind, "127.0.0.1:8080");
        assert!(cfg.pac.enabled);
        assert_eq!(cfg.pac.bind, "0.0.0.0:8081");
        assert_eq!(cfg.pac.mode, PacMode::HttpSocks);
        assert_eq!(cfg.logs.db_path, "~/.local/share/rustcache/logs.db");
        assert_eq!(cfg.logs.max_rows, 10_000);
        assert_eq!(cfg.logs.max_age_days, 7);
        assert_eq!(cfg.logs.cleanup_interval_secs, 300);
    }

    #[test]
    fn logs_parse() {
        let cfg: Config = toml::from_str(
            r#"
[logs]
db_path = "/tmp/rc-logs.db"
max_rows = 50
max_age_days = 3
cleanup_interval_secs = 60
"#,
        )
        .unwrap();
        assert_eq!(cfg.logs.db_path, "/tmp/rc-logs.db");
        assert_eq!(cfg.logs.max_rows, 50);
        assert_eq!(cfg.logs.max_age_days, 3);
        assert_eq!(cfg.logs.cleanup_interval_secs, 60);
    }

    #[test]
    fn logs_db_path_expands_tilde() {
        let cfg: Config = toml::from_str("").unwrap();
        let p = cfg.logs_db_path();
        assert!(!p.to_string_lossy().starts_with('~'));
    }

    #[test]
    fn pac_parse() {
        let cfg: Config = toml::from_str(
            r#"
[pac]
enabled = false
bind = "0.0.0.0:9090"
mode = "http"
"#,
        )
        .unwrap();
        assert!(!cfg.pac.enabled);
        assert_eq!(cfg.pac.bind, "0.0.0.0:9090");
        assert_eq!(cfg.pac.mode, PacMode::Http);

        let cfg: Config = toml::from_str(
            r#"
[pac]
mode = "socks"
"#,
        )
        .unwrap();
        assert_eq!(cfg.pac.mode, PacMode::Socks);

        let cfg: Config = toml::from_str(
            r#"
[pac]
mode = "http+socks"
"#,
        )
        .unwrap();
        assert_eq!(cfg.pac.mode, PacMode::HttpSocks);
    }
}
