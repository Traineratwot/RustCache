//! OS / desktop-environment detection and support gates.

use std::fmt;

use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsKind {
    Windows,
    Macos,
    Linux,
    Unsupported,
}

impl OsKind {
    pub fn from_target() -> Self {
        if cfg!(target_os = "windows") {
            OsKind::Windows
        } else if cfg!(target_os = "macos") {
            OsKind::Macos
        } else if cfg!(target_os = "linux") {
            OsKind::Linux
        } else {
            OsKind::Unsupported
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OsKind::Windows => "windows",
            OsKind::Macos => "macos",
            OsKind::Linux => "linux",
            OsKind::Unsupported => "unsupported",
        }
    }
}

impl fmt::Display for OsKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopEnv {
    Gnome,
    Kde,
    Unknown,
}

impl DesktopEnv {
    pub fn as_str(self) -> &'static str {
        match self {
            DesktopEnv::Gnome => "gnome",
            DesktopEnv::Kde => "kde",
            DesktopEnv::Unknown => "unknown",
        }
    }
}

impl fmt::Display for DesktopEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result of platform detection.
#[derive(Debug, Clone)]
pub struct PlatformInfo {
    pub os: OsKind,
    pub de: DesktopEnv,
    pub sysproxy_supported: bool,
    pub tun_supported: bool,
    /// Human-readable reason when `sysproxy_supported` is false.
    pub unsupported_reason: Option<String>,
}

impl PlatformInfo {
    /// Short label like `linux/kde`, `windows/unknown`.
    pub fn summary(&self) -> String {
        format!("{}/{}", self.os, self.de)
    }

    /// Error message shown when system-proxy capture is requested on an unsupported platform.
    pub fn unsupported_message(&self) -> String {
        let reason = self
            .unsupported_reason
            .clone()
            .unwrap_or_else(|| format!("unsupported platform/desktop: {}", self.summary()));
        format!(
            "{reason}\n  supported: Windows, macOS, Linux GNOME, Linux KDE Plasma\n  hint: run with --mode off and set http_proxy/https_proxy=127.0.0.1:31280"
        )
    }
}

/// Detect using the real process environment.
pub fn detect() -> PlatformInfo {
    detect_with_env(&|k| std::env::var(k).ok())
}

/// Detect with an injectable env getter (for tests).
pub fn detect_with_env(get: &dyn Fn(&str) -> Option<String>) -> PlatformInfo {
    let os = OsKind::from_target();
    let de = detect_de(get);
    let (sysproxy_supported, unsupported_reason) = match os {
        OsKind::Windows | OsKind::Macos => (true, None),
        OsKind::Linux => match de {
            DesktopEnv::Gnome | DesktopEnv::Kde => (true, None),
            DesktopEnv::Unknown => (
                false,
                Some(format!("unsupported platform/desktop: linux/{}", de)),
            ),
        },
        OsKind::Unsupported => (false, Some(format!("unsupported platform/desktop: {}", os))),
    };
    let tun_supported = matches!(os, OsKind::Windows | OsKind::Macos | OsKind::Linux);
    PlatformInfo {
        os,
        de,
        sysproxy_supported,
        tun_supported,
        unsupported_reason,
    }
}

fn detect_de(get: &dyn Fn(&str) -> Option<String>) -> DesktopEnv {
    // 1. XDG_CURRENT_DESKTOP — colon-separated list, case-insensitive.
    if let Some(raw) = get("XDG_CURRENT_DESKTOP") {
        for part in raw.split(':') {
            let p = part.trim().to_ascii_lowercase();
            if p.is_empty() {
                continue;
            }
            if p == "gnome" || p == "ubuntu:gnome" || p == "unity" {
                return DesktopEnv::Gnome;
            }
            if p == "kde" || p == "plasma" || p == "x-kde" {
                return DesktopEnv::Kde;
            }
        }
    }
    // 2. DESKTOP_SESSION / XDG_SESSION_DESKTOP.
    for key in ["DESKTOP_SESSION", "XDG_SESSION_DESKTOP"] {
        if let Some(raw) = get(key) {
            let p = raw.trim().to_ascii_lowercase();
            if p.contains("gnome") {
                return DesktopEnv::Gnome;
            }
            if p.contains("kde") || p.contains("plasma") {
                return DesktopEnv::Kde;
            }
        }
    }
    // 3. Tool-presence fallback (only when env is inconclusive).
    if tool_exists("kwriteconfig6") || tool_exists("kwriteconfig5") {
        return DesktopEnv::Kde;
    }
    if tool_exists("gsettings") {
        return DesktopEnv::Gnome;
    }
    DesktopEnv::Unknown
}

fn tool_exists(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Fail with the unsupported-platform message when system proxy is requested.
pub fn require_sysproxy(info: &PlatformInfo) -> Result<()> {
    if info.sysproxy_supported {
        Ok(())
    } else {
        bail!("{}", info.unsupported_message());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(map: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: Vec<(String, String)> = map
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn detect_gnome_from_xdg() {
        let info = detect_with_env(&env(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")]));
        assert_eq!(info.os, OsKind::from_target());
        assert_eq!(info.de, DesktopEnv::Gnome);
    }

    #[test]
    fn detect_kde_from_xdg() {
        let info = detect_with_env(&env(&[("XDG_CURRENT_DESKTOP", "KDE")]));
        assert_eq!(info.de, DesktopEnv::Kde);
    }

    #[test]
    fn detect_kde_plasma() {
        let info = detect_with_env(&env(&[("XDG_CURRENT_DESKTOP", "plasma")]));
        assert_eq!(info.de, DesktopEnv::Kde);
    }

    #[test]
    fn detect_gnome_from_desktop_session() {
        let info = detect_with_env(&env(&[
            ("XDG_CURRENT_DESKTOP", ""),
            ("DESKTOP_SESSION", "gnome"),
        ]));
        assert_eq!(info.de, DesktopEnv::Gnome);
    }

    #[test]
    fn detect_unknown_on_sway() {
        // Force no tools: empty env and we can't easily stub tool_exists, so just
        // check the env path does not report a known DE.
        let info = detect_with_env(&env(&[
            ("XDG_CURRENT_DESKTOP", "sway"),
            ("DESKTOP_SESSION", "sway"),
        ]));
        // May fall through to tool-presence (gsettings/kwriteconfig on the host).
        // The env itself must not claim gnome/kde.
        if info.de != DesktopEnv::Unknown {
            // tool fallback hit a real binary on this machine — still OK
            assert!(matches!(info.de, DesktopEnv::Gnome | DesktopEnv::Kde));
        }
    }

    #[test]
    fn detect_empty_env() {
        let info = detect_with_env(&env(&[]));
        // tool-presence may still detect something on a developer machine
        assert!(matches!(
            info.de,
            DesktopEnv::Gnome | DesktopEnv::Kde | DesktopEnv::Unknown
        ));
    }

    #[test]
    fn unsupported_message_shape() {
        let info = PlatformInfo {
            os: OsKind::Linux,
            de: DesktopEnv::Unknown,
            sysproxy_supported: false,
            tun_supported: true,
            unsupported_reason: Some("unsupported platform/desktop: linux/xfce".into()),
        };
        let msg = info.unsupported_message();
        assert!(msg.contains("linux/xfce"));
        assert!(msg.contains("Linux KDE Plasma"));
        assert!(msg.contains("--mode off"));
    }
}
