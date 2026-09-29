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

/// Write `cfg` to `path` as pretty TOML, atomically.
///
/// The file is written to a sibling temp file and renamed into place, so a
/// reader never observes a half-written config. That matters here: the notify
/// watcher re-parses `config.toml` the moment it changes, and a plain
/// truncate-then-write made it race a `PUT /api/config` and log a parse error
/// (or, on a crash mid-write, leave the config truncated on disk).
///
/// On serialization failure the file is left untouched.
pub fn write_config_toml(path: impl AsRef<Path>, cfg: &Config) -> anyhow::Result<()> {
    let path = path.as_ref();
    let text = to_toml_string(cfg)?;

    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);

    let write = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::anyhow!("write config: {e}"));
    }
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
    fn write_config_toml_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("rc-cfg-atomic-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        write_config_toml(&path, &Config::default()).expect("write");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
        // Whatever is on disk always parses — never a truncated prefix.
        let back: Config = toml::from_str(&std::fs::read_to_string(&path).expect("read"))
            .expect("written config must parse");
        assert_eq!(back.http.port, Config::default().http.port);
        let _ = std::fs::remove_dir_all(&dir);
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
