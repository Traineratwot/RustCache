//! Effective runtime configuration (TOML).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Root data directory. Relative cache/ca/logs paths resolve under it.
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            http: HttpConfig::default(),
            https: HttpsConfig::default(),
            socks5: Socks5Config::default(),
            api: ApiConfig::default(),
            cache: CacheConfig::default(),
            exclude: ExcludeConfig::default(),
            ca: CaConfig::default(),
            pac: PacConfig::default(),
            logs: LogConfig::default(),
        }
    }
}

fn default_data_dir() -> String {
    "~/.local/share/rustcache".into()
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
    "cache".into()
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
    "ca".into()
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
    "logs.db".into()
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

/// Resolve `p` under `data_dir` when relative; absolute (or `~/`) paths stand alone.
fn resolve_under(data_dir: &Path, p: &str) -> PathBuf {
    let expanded = expand_tilde(p);
    if expanded.is_absolute() {
        expanded
    } else {
        data_dir.join(expanded)
    }
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

    /// Override `data_dir` (CLI `--data-dir`). Relative path overrides still follow it.
    pub fn apply_data_dir_override(&mut self, dir: Option<impl AsRef<Path>>) {
        if let Some(d) = dir {
            self.data_dir = d.as_ref().to_string_lossy().into_owned();
        }
    }

    pub fn data_dir_path(&self) -> PathBuf {
        expand_tilde(&self.data_dir)
    }

    pub fn cache_dir(&self) -> PathBuf {
        resolve_under(&self.data_dir_path(), &self.cache.dir)
    }

    pub fn ca_dir(&self) -> PathBuf {
        resolve_under(&self.data_dir_path(), &self.ca.dir)
    }

    pub fn logs_db_path(&self) -> PathBuf {
        resolve_under(&self.data_dir_path(), &self.logs.db_path)
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
        let cfg: Config = Config::default();
        assert_eq!(cfg.data_dir, "~/.local/share/rustcache");
        assert_eq!(cfg.api.bind, "127.0.0.1:8080");
        assert!(cfg.pac.enabled);
        assert_eq!(cfg.pac.bind, "0.0.0.0:8081");
        assert_eq!(cfg.pac.mode, PacMode::HttpSocks);
        assert_eq!(cfg.cache.dir, "cache");
        assert_eq!(cfg.ca.dir, "ca");
        assert_eq!(cfg.logs.db_path, "logs.db");
        assert_eq!(cfg.logs.max_rows, 10_000);
        assert_eq!(cfg.logs.max_age_days, 7);
        assert_eq!(cfg.logs.cleanup_interval_secs, 300);
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.data_dir, "~/.local/share/rustcache");
        assert_eq!(cfg.cache.dir, "cache");
    }

    #[test]
    fn relative_paths_resolve_under_data_dir() {
        let cfg: Config = toml::from_str(
            r#"
data_dir = "/var/lib/rustcache"
"#,
        )
        .unwrap();
        assert_eq!(cfg.data_dir_path(), PathBuf::from("/var/lib/rustcache"));
        assert_eq!(cfg.cache_dir(), PathBuf::from("/var/lib/rustcache/cache"));
        assert_eq!(cfg.ca_dir(), PathBuf::from("/var/lib/rustcache/ca"));
        assert_eq!(
            cfg.logs_db_path(),
            PathBuf::from("/var/lib/rustcache/logs.db")
        );
    }

    #[test]
    fn absolute_paths_override_data_dir() {
        let cfg: Config = toml::from_str(
            r#"
data_dir = "/var/lib/rustcache"
[cache]
dir = "/tmp/rc-cache"
[ca]
dir = "/tmp/rc-ca"
[logs]
db_path = "/tmp/rc-logs.db"
"#,
        )
        .unwrap();
        assert_eq!(cfg.cache_dir(), PathBuf::from("/tmp/rc-cache"));
        assert_eq!(cfg.ca_dir(), PathBuf::from("/tmp/rc-ca"));
        assert_eq!(cfg.logs_db_path(), PathBuf::from("/tmp/rc-logs.db"));
    }

    #[test]
    fn data_dir_override_moves_relative_paths() {
        let mut cfg: Config = toml::from_str("").unwrap();
        cfg.apply_data_dir_override(Some("/opt/rc"));
        assert_eq!(cfg.data_dir_path(), PathBuf::from("/opt/rc"));
        assert_eq!(cfg.cache_dir(), PathBuf::from("/opt/rc/cache"));
        assert_eq!(cfg.ca_dir(), PathBuf::from("/opt/rc/ca"));
        assert_eq!(cfg.logs_db_path(), PathBuf::from("/opt/rc/logs.db"));
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
        assert!(p.ends_with("logs.db"));
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
