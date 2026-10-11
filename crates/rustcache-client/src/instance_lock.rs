//! Single-instance lock: at most one long-running client process.
//!
//! `flock(LOCK_EX | LOCK_NB)` on a lock file. The kernel releases the lock
//! when the process dies (including SIGKILL), so a crash cannot leave a
//! permanent "already running" state. Drop also unlocks explicitly so a
//! re-acquire in the same process never races the close path.

use std::fs::{File, OpenOptions};
use std::path::Path;

/// Held for the process lifetime; dropping unlocks and closes the fd.
pub struct InstanceLock {
    file: File,
}

impl InstanceLock {
    /// Acquire the exclusive lock at `path`. Fails if another instance holds it.
    pub fn acquire(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("mkdir {}: {e}", parent.display()))?;
        }
        let mut file = open_locked(path)?;
        write_pid(&mut file);
        Ok(Self { file })
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

fn write_pid(file: &mut File) {
    use std::io::Write as _;
    let _ = file.write_all(format!("{}\n", std::process::id()).as_bytes());
}

#[cfg(unix)]
fn open_locked(path: &Path) -> anyhow::Result<File> {
    use std::os::unix::io::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| anyhow::anyhow!("open lock {}: {e}", path.display()))?;
    // SAFETY: flock only touches the fd we own; LOCK_NB never blocks.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        anyhow::bail!(
            "another rustcache-client is already running (lock: {})",
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
                "another rustcache-client is already running (lock: {})",
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

/// Explicit unlock so the next `acquire` in this process cannot lose a race
/// with fd close. Safe no-op where the OS lock is only released by close.
#[cfg(unix)]
fn unlock(file: &File) {
    use std::os::unix::io::AsRawFd;
    // SAFETY: fd belongs to `file`; errors are irrelevant (close still releases).
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

#[cfg(not(unix))]
fn unlock(_file: &File) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Serialize lock tests: they share /tmp and the kernel flock state.
    static TEST_GATE: Mutex<()> = Mutex::new(());
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Unique per call — never reuse a path even across tests on one thread.
    fn tmp_lock(name: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join("rustcache-client-lock-test")
            .join(format!("{name}-{}-{nanos}-{n}", std::process::id()))
    }

    #[test]
    fn second_acquire_fails_while_first_holds() {
        let _g = TEST_GATE.lock().unwrap();
        let path = tmp_lock("exclusive");
        let first = InstanceLock::acquire(&path).expect("first acquire");
        assert!(
            InstanceLock::acquire(&path).is_err(),
            "second instance must be rejected"
        );
        drop(first);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn acquire_succeeds_after_release() {
        let _g = TEST_GATE.lock().unwrap();
        let path = tmp_lock("release");
        for round in 0..32 {
            let held = InstanceLock::acquire(&path)
                .unwrap_or_else(|e| panic!("acquire round {round} after prior drop: {e}"));
            drop(held);
        }
        let _ = std::fs::remove_file(&path);
    }
}
