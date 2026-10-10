//! Filesystem helpers: `~/` expansion and owner-only modes for secrets.

use std::path::{Path, PathBuf};

/// Expand a leading `~/` (or a bare `~`) to the current user's home directory.
///
/// Absolute paths and paths without a tilde are returned unchanged. When the
/// home directory cannot be determined the input is returned as-is, so a
/// configured `~/...` path degrades to a relative path rather than failing.
///
/// `HOME` is consulted first (Unix, Git Bash on Windows); `USERPROFILE` is the
/// fallback so native Windows resolves the same way `dirs::home_dir` would.
pub fn expand_tilde(p: &str) -> PathBuf {
    let home = home_dir();
    if p == "~" {
        return home.unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = home {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Force mode `0600` on `path` so only the owner can read a secret.
///
/// Used for the CA private key, the request-log SQLite database and its WAL/SHM
/// sidecars. On non-Unix platforms this is a no-op — ACL hardening is out of
/// scope and callers still get a working (if less locked-down) file.
#[cfg(unix)]
pub fn set_private_mode(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
}

/// Non-Unix stub: permissions are not managed here.
#[cfg(not(unix))]
pub fn set_private_mode(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_leaves_absolute_paths() {
        assert_eq!(expand_tilde("/etc/passwd"), PathBuf::from("/etc/passwd"));
        assert_eq!(expand_tilde("rel/path"), PathBuf::from("rel/path"));
        assert_eq!(expand_tilde(""), PathBuf::from(""));
    }

    #[test]
    fn expand_tilde_without_home_degrades_to_input() {
        // Edition 2024 marks env mutation unsafe; safe here because this is a
        // single-threaded unit test that restores the previous value.
        let saved = std::env::var_os("HOME");
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }
        assert_eq!(expand_tilde("~/x"), PathBuf::from("~/x"));
        assert_eq!(expand_tilde("~"), PathBuf::from("~"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn expand_tilde_uses_home_when_present() {
        let saved = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", "/home/test");
        }
        assert_eq!(expand_tilde("~/x"), PathBuf::from("/home/test/x"));
        assert_eq!(expand_tilde("~"), PathBuf::from("/home/test"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn set_private_mode_makes_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rustcache-util-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("secret");
        std::fs::write(&f, b"x").unwrap();
        set_private_mode(&f).unwrap();
        let mode = std::fs::metadata(&f).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
