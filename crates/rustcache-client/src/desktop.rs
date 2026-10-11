//! Linux desktop integration: user-level `.desktop` entry + hicolor icon.
//!
//! Wayland docks resolve the window icon from a matching `.desktop` file
//! (`app_id`), not from the runtime window icon. Installing a user entry on
//! GUI start is what makes the brand mark show up in Plasma/GNOME docks.

use anyhow::Result;

/// Freedesktop app id — must match `.desktop` basename and `ViewportBuilder::app_id`.
pub const APP_ID: &str = "rustcache-client";

/// Best-effort install of `~/.local/share/applications` entry + hicolor icons.
/// Never fails the GUI: callers ignore the error.
#[cfg(target_os = "linux")]
pub fn install_user_desktop_entry() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let apps = home.join(".local/share/applications");
    let icons_root = home.join(".local/share/icons/hicolor");
    std::fs::create_dir_all(&apps)?;

    let exe = std::env::current_exe()?;
    let exe_str = exe.to_string_lossy();
    // Quote if the path has spaces; keep simple otherwise.
    let exec = if exe_str.contains(' ') {
        format!("\"{exe_str}\" gui")
    } else {
        format!("{exe_str} gui")
    };

    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=RustCache Client\n\
         GenericName=Caching proxy client\n\
         Comment=CA install, system proxy, resilient local proxy to RustCache\n\
         Exec={exec}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Network;Settings;\n\
         Keywords=proxy;cache;network;\n\
         StartupNotify=true\n\
         StartupWMClass={APP_ID}\n"
    );
    let desktop_path = apps.join(format!("{APP_ID}.desktop"));
    write_atomic(&desktop_path, desktop.as_bytes())?;

    // hicolor sized PNGs so the DE can pick a crisp tile.
    const SIZES: &[(u32, &[u8])] = &[
        (128, include_bytes!("../assets/icon-128.png")),
        (256, include_bytes!("../assets/icon-256.png")),
        (512, include_bytes!("../assets/icon-512.png")),
    ];
    for (size, png) in SIZES {
        let dir = icons_root.join(format!("{size}x{size}/apps"));
        std::fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(format!("{APP_ID}.png")), png)?;
    }

    // Refresh the desktop database when available (never required).
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&apps)
        .status();
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn install_user_desktop_entry() -> Result<()> {
    Ok(())
}

/// tmp + rename so a concurrent DE scan never sees a half-written file.
#[cfg(target_os = "linux")]
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
