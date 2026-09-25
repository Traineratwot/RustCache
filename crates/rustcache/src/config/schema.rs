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
    }
}
