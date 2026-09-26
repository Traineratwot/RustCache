//! Config persistence: TOML load/save with atomic-ish write semantics.
//!
//! Saving never silently truncates the file: serialization failure returns an
//! error and leaves the previous contents intact.

use std::path::Path;

use crate::config::Config;

/// Serialize `cfg` to pretty TOML.
///
/// Returns an error instead of an empty string when serialization fails —
/// callers must not write a blank config over a good one.
pub fn to_toml_string(cfg: &Config) -> anyhow::Result<String> {
    toml::to_string_pretty(cfg).map_err(|e| anyhow::anyhow!("serialize config: {e}"))
}

/// Write `cfg` to `path` as pretty TOML.
///
/// On serialization failure the file is left untouched. On I/O failure the
/// error is returned; a partial write is still possible at the OS level, so
/// callers should treat a failed save as "config may be stale".
pub fn write_config_toml(path: impl AsRef<Path>, cfg: &Config) -> anyhow::Result<()> {
    let text = to_toml_string(cfg)?;
    std::fs::write(path, text).map_err(|e| anyhow::anyhow!("write config: {e}"))?;
    Ok(())
}

/// Load a config file. Missing file is not an error for callers that fall back
/// to defaults — see [`Config::load`].
pub fn load_config(path: impl AsRef<Path>) -> anyhow::Result<Config> {
    Config::load(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_toml_string_round_trips() {
        let cfg = Config::default();
        let s = to_toml_string(&cfg).expect("serialize");
        assert!(s.contains("[http]") || s.contains("data_dir"));
        let back: Config = toml::from_str(&s).expect("parse");
        assert_eq!(back.http.port, cfg.http.port);
    }

    #[test]
    fn write_config_toml_writes_file() {
        let dir = std::env::temp_dir().join(format!("rc-cfg-persist-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        let cfg = Config::default();
        write_config_toml(&path, &cfg).expect("write");
        assert!(path.exists());
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(!text.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
