//! Linux system-proxy handlers: GNOME (gsettings) and KDE (kioslaverc).

pub mod gnome;
pub mod kde;

use crate::platform::{DesktopEnv, PlatformInfo};
use crate::sysproxy::handler::SysProxyHandler;

use gnome::GnomeHandler;
use kde::KdeHandler;

/// Pick the right handler for the detected DE.
pub fn handler_for(info: &PlatformInfo) -> Box<dyn SysProxyHandler> {
    match info.de {
        DesktopEnv::Kde => Box::new(KdeHandler),
        _ => Box::new(GnomeHandler),
    }
}
