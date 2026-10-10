//! Elevate just enough to create a TUN device (Linux).
//!
//! Creating `/dev/net/tun` + capture routes needs `CAP_NET_ADMIN`. When the
//! GUI runs as a normal user we **ask for root** (`pkexec` password dialog,
//! then `sudo`) to run a tiny privileged helper: `rustcache-client tun-stack`.
//! The helper owns the device; the user-facing process stays unprivileged.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

/// True when this process can create a TUN device (root or file caps).
pub fn have_tun_privileges() -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return true;
    }
    match try_probe_tun() {
        Ok(()) => true,
        Err(e) => {
            let msg = format!("{e:#}").to_ascii_lowercase();
            // Device node missing is not a privilege problem.
            if msg.contains("no such file") || msg.contains("not found") {
                return true;
            }
            false
        }
    }
}

fn try_probe_tun() -> Result<()> {
    // Cheap privilege probe: `ip tuntap add` needs CAP_NET_ADMIN just like
    // tun2proxy's device create. Avoids depending on the `tun` crate here.
    let out = std::process::Command::new("ip")
        .args([
            "tuntap",
            "add",
            "mode",
            "tun",
            "name",
            "rustcache-tun-probe",
        ])
        .output()
        .context("ip tuntap add")?;
    let _ = std::process::Command::new("ip")
        .args(["link", "del", "rustcache-tun-probe"])
        .output();
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "ip tuntap add: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Handle to a running TUN stack (in-process task or privileged helper).
pub enum StackHandle {
    /// Tokio task running the stack in this process (we already have CAP_NET_ADMIN).
    Task(tokio::task::JoinHandle<()>),
    /// Privileged helper process (pkexec/sudo) that owns the TUN device.
    Child(Child),
}

impl StackHandle {
    pub fn abort(&mut self) {
        match self {
            StackHandle::Task(h) => {
                h.abort();
            }
            StackHandle::Child(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

impl Drop for StackHandle {
    fn drop(&mut self) {
        self.abort();
    }
}

/// Start the TUN stack, elevating to root when this process cannot create the device.
///
/// Returns after the caller has been given a handle; readiness is still
/// verified separately by [`crate::tun::enable`] via the ready channel.
pub fn spawn_stack(local_proxy: &str) -> Result<StackHandle> {
    if have_tun_privileges() {
        let proxy = local_proxy.to_string();
        let handle = tokio::spawn(async move {
            let (tx, _rx) = std::sync::mpsc::channel();
            let _ = crate::tun::stack::run(&proxy, tx).await;
        });
        return Ok(StackHandle::Task(handle));
    }
    spawn_privileged_helper(local_proxy)
}

/// Ask for root and run `rustcache-client tun-stack --proxy host:port`.
///
/// Order: `pkexec` (GUI password dialog — what the user expects to see) →
/// `sudo -n` (fail-fast, works for NOPASSWD sudoers) → interactive `sudo`.
/// A helper that exits before creating the device is treated as a failed
/// elevation — we fall through to the next one instead of returning a
/// "started" handle for a dead child.
pub fn spawn_privileged_helper(local_proxy: &str) -> Result<StackHandle> {
    let exe = std::env::current_exe().context("current_exe")?;
    let exe_s = exe.to_string_lossy().to_string();
    let mut last_err = String::new();

    // (tool, args-before-exe, human label, ready grace)
    // Interactive methods get a long grace — the user is typing a password.
    let methods: [(&str, Vec<String>, &str, u64); 3] = [
        (
            "pkexec",
            vec![],
            "pkexec (enter root password in the dialog)",
            60,
        ),
        ("sudo", vec!["-n".into()], "sudo -n", 5),
        ("sudo", vec![], "sudo", 60),
    ];

    for (tool, pre_args, label, grace_secs) in methods {
        let mut cmd = Command::new(tool);
        cmd.args(&pre_args)
            .arg(&exe_s)
            .arg("tun-stack")
            .arg("--proxy")
            .arg(local_proxy)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Inherit stderr: a piped stderr dies with the parent (SIGPIPE)
            // and takes the TUN device with it.
            .stderr(Stdio::inherit());

        tracing::info!(tool = label, proxy = local_proxy, "requesting root for TUN");
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                last_err = format!("{tool}: {e}");
                tracing::debug!(tool, error = %e, "elevation helper not available");
                continue;
            }
        };

        // Give the helper a moment to create the device — or die if elevation
        // was denied / the tool exited immediately.
        match wait_child_ready(&mut child, std::time::Duration::from_secs(grace_secs)) {
            Ok(()) => {
                tracing::info!(tool = label, "privileged TUN helper is up");
                return Ok(StackHandle::Child(child));
            }
            Err(reason) => {
                last_err = format!("{label}: {reason}");
                tracing::warn!(tool = label, %reason, "elevation attempt failed, trying next");
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    bail!(
        "TUN needs root (CAP_NET_ADMIN) — no elevation method worked ({last_err}). \
         Approve the password prompt, or run with sudo, or: \
         sudo setcap cap_net_admin,cap_net_raw+eip {}",
        exe.display()
    )
}

/// Wait until the helper has created the TUN device. Returns `Err` when the
/// process exits early or the device never appears — never report "up"
/// without a live interface.
fn wait_child_ready(child: &mut Child, grace: std::time::Duration) -> Result<()> {
    let deadline = std::time::Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                bail!("helper exited ({status})");
            }
            Ok(None) => {
                if std::path::Path::new("/sys/class/net/rustcache-tun0").exists() {
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    bail!("helper running but rustcache-tun0 never appeared");
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => bail!("wait helper: {e}"),
        }
    }
}

/// Find setcap for optional persistent file-cap install.
pub fn find_setcap() -> PathBuf {
    for p in ["/usr/sbin/setcap", "/sbin/setcap", "/usr/bin/setcap"] {
        if Path::new(p).exists() {
            return PathBuf::from(p);
        }
    }
    PathBuf::from("setcap")
}

/// Optional: install file capabilities so the next start needs no prompt.
pub fn install_file_caps() -> Result<String> {
    let exe = std::env::current_exe()?;
    let setcap = find_setcap();
    let caps = "cap_net_admin,cap_net_raw+eip";
    let exe_s = exe.to_string_lossy().to_string();

    for tool in ["pkexec", "sudo"] {
        let status = Command::new(tool)
            .args([setcap.to_str().unwrap_or("setcap"), caps, &exe_s])
            .status();
        if let Ok(s) = status {
            if s.success() {
                return Ok(format!("installed {caps} on {}", exe.display()));
            }
        }
    }
    bail!(
        "setcap failed (need root). Manually: sudo setcap {caps} {}",
        exe.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_setcap_falls_back_to_path_name() {
        let p = find_setcap();
        assert!(p.to_string_lossy().contains("setcap"));
    }
}
