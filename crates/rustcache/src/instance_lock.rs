//! Single-instance lock: at most one `rustcache run` process per data dir.
//!
//! `flock(LOCK_EX | LOCK_NB)` on a lock file. The kernel releases the lock
//! when the process dies (including SIGKILL), so a crash cannot leave a
//! permanent "already running" state. The UI self-restart waits for the old
//! process to exit first, so the lock is free by the time the new one starts.

use std::fs::{File, OpenOptions};
use std::path::Path;

/// Held for the process lifetime; dropping closes the fd and releases the lock.
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// Acquire the exclusive lock at `path`. Fails if another instance holds it.
    pub fn acquire(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("mkdir {}: {e}", parent.display()))?;
        }
        let file = open_locked(path)?;
        write_pid(&file);
        Ok(Self { _file: file })
    }
}

fn write_pid(file: &File) {
    use std::io::Write as _;
    let mut f = file;
    let _ = f.write_all(format!("{}\n", std::process::id()).as_bytes());
}

#[cfg(unix)]
fn open_locked(path: &Path) -> anyhow::Result<File> {
    use std::os::unix::io::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| anyhow::anyhow!("open lock {}: {e}", path.display()))?;
    // SAFETY: flock only touches the fd we own; LOCK_NB never blocks.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        anyhow::bail!(
            "another rustcache is already running (lock: {})",
            path.display()
        );
    }
    Ok(file)
}

#[cfg(windows)]
fn open_locked(path: &Path) -> anyhow::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    // share_mode(0) = exclusive open: second process gets a sharing violation.
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .map_err(|_| {
            anyhow::anyhow!(
                "another rustcache is already running (lock: {})",
                path.display()
            )
        })
}

#[cfg(not(any(unix, windows)))]
fn open_locked(path: &Path) -> anyhow::Result<File> {
    anyhow::bail!(
        "single-instance lock is not supported on this platform ({})",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_lock(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join("rustcache-lock-test").join(name)
    }

    #[test]
    fn second_acquire_fails_while_first_holds() {
        let path = tmp_lock("exclusive.lock");
        let _first = InstanceLock::acquire(&path).expect("first acquire");
        assert!(
            InstanceLock::acquire(&path).is_err(),
            "second instance must be rejected"
        );
    }

    #[test]
    fn acquire_succeeds_after_release() {
        let path = tmp_lock("release.lock");
        {
            let _held = InstanceLock::acquire(&path).expect("first");
        }
        let _again = InstanceLock::acquire(&path).expect("second after drop");
    }
}
