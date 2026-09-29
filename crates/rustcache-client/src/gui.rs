//! Simple GUI: status window + system tray (feature `gui`).
//!
//! Talks to the same lib API as the CLI. `run --tray` / `gui` start this.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use parking_lot::RwLock;

use crate::api::{DiscoveredTarget, RustCacheApi, SharedUpstream};
use crate::capture::Capture;
use crate::config::{CaptureMode, ClientConfig};
use crate::health::{BreakerState, HealthMonitor};

/// Shared mutable UI state.
#[derive(Debug, Clone)]
pub struct GuiState {
    pub capture_on: bool,
    pub mode: CaptureMode,
    pub last_error: Option<String>,
    pub status_line: String,
    pub ca_installed: Option<bool>,
    pub ca_fp: String,
    pub stats_json: String,
    /// User-editable RustCache host / IP / URL.
    pub host_input: String,
    /// Human-readable discovered ports line.
    pub ports_line: String,
    pub discovered: Option<DiscoveredTarget>,
    pub connecting: bool,
}

impl Default for GuiState {
    fn default() -> Self {
        Self {
            capture_on: false,
            mode: CaptureMode::System,
            last_error: None,
            status_line: "starting…".into(),
            ca_installed: None,
            ca_fp: String::new(),
            stats_json: String::new(),
            host_input: "127.0.0.1".into(),
            ports_line: String::new(),
            discovered: None,
            connecting: false,
        }
    }
}

pub type SharedGui = Arc<RwLock<GuiState>>;

/// Everything needed to start the UI on the main thread.
pub struct GuiLaunch {
    pub cfg: ClientConfig,
    pub health: Arc<HealthMonitor>,
    pub gui: SharedGui,
    pub on_quit: Arc<dyn Fn() + Send + Sync>,
    /// Tokio handle so the UI thread can spawn async work.
    pub rt: tokio::runtime::Handle,
    /// Live upstream — updated when the user connects to a host.
    pub upstream: SharedUpstream,
}

impl GuiLaunch {
    /// Block on the native event loop (must be called from the main thread).
    pub fn run(self) -> Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([460.0, 580.0])
                .with_title("RustCache Client"),
            ..Default::default()
        };
        let app = ClientApp {
            cfg: self.cfg,
            health: self.health,
            gui: self.gui,
            on_quit: self.on_quit,
            rt: self.rt,
            upstream: self.upstream,
            last_refresh: Instant::now() - Duration::from_secs(60),
        };
        eframe::run_native(
            "RustCache Client",
            options,
            Box::new(|_cc| Ok(Box::new(app))),
        )
        .map_err(|e| anyhow::anyhow!("ui: {e}"))
    }
}

/// Launch the status window (blocking on the UI thread). Proxy/health run on the tokio runtime.
pub fn launch(
    cfg: ClientConfig,
    health: Arc<HealthMonitor>,
    gui: SharedGui,
    on_quit: impl Fn() + Send + Sync + 'static,
) -> Result<()> {
    let rt = tokio::runtime::Handle::try_current()
        .map_err(|_| anyhow::anyhow!("gui::launch requires a live tokio runtime"))?;
    let upstream =
        crate::api::shared_upstream(crate::api::UpstreamCfg::from_api_base(&cfg.rustcache_api));
    GuiLaunch {
        cfg,
        health,
        gui,
        on_quit: Arc::new(on_quit),
        rt,
        upstream,
    }
    .run()
}

struct ClientApp {
    cfg: ClientConfig,
    health: Arc<HealthMonitor>,
    gui: SharedGui,
    on_quit: Arc<dyn Fn() + Send + Sync>,
    rt: tokio::runtime::Handle,
    upstream: SharedUpstream,
    last_refresh: Instant,
}

impl eframe::App for ClientApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Periodic refresh of remote status.
        if self.last_refresh.elapsed() > Duration::from_secs(2) {
            self.last_refresh = Instant::now();
            self.refresh_async(ctx.clone());
        }

        let snap = self.health.snapshot();
        let state = self.gui.read().clone();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("RustCache Client");
            ui.separator();

            // --- RustCache host (IP / hostname / URL) ---
            ui.horizontal(|ui| {
                ui.label("RustCache host:");
                let mut host = state.host_input.clone();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut host)
                        .desired_width(180.0)
                        .hint_text("192.168.1.10 or host:8080"),
                );
                if resp.changed() {
                    self.gui.write().host_input = host.clone();
                }
                let busy = state.connecting;
                if ui
                    .add_enabled(!busy, egui::Button::new(if busy { "…" } else { "Connect" }))
                    .clicked()
                {
                    self.connect_host(host);
                }
            });
            if !state.ports_line.is_empty() {
                ui.label(egui::RichText::new(&state.ports_line).small());
            }
            ui.label(
                egui::RichText::new(format!("API: {}", self.upstream.read().api_base)).small(),
            );

            // Circuit / health
            let (color, label) = match snap.state {
                BreakerState::Closed => (
                    egui::Color32::from_rgb(40, 160, 80),
                    "connected (circuit closed)".to_string(),
                ),
                BreakerState::HalfOpen => (
                    egui::Color32::from_rgb(200, 160, 40),
                    "probing (half-open)".to_string(),
                ),
                BreakerState::Open => (
                    egui::Color32::from_rgb(200, 60, 60),
                    "DIRECT fail-open (circuit open)".to_string(),
                ),
            };
            ui.horizontal(|ui| {
                ui.colored_label(color, "●");
                ui.label(label);
            });
            ui.label(format!(
                "listeners: HTTP={} HTTPS={} SOCKS={}",
                on_off(snap.http_running),
                on_off(snap.https_running),
                on_off(snap.socks_running)
            ));
            if let Some(err) = &snap.last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("last error: {err}"));
            }

            ui.separator();
            ui.heading("Capture");
            ui.horizontal(|ui| {
                let mut on = state.capture_on;
                if ui
                    .checkbox(&mut on, "System proxy points at this client")
                    .changed()
                {
                    let mode = if on { state.mode } else { CaptureMode::Off };
                    apply_mode(&self.rt, &self.cfg, &self.gui, mode);
                }
                ui.label(format!("{:?}", state.mode));
            });
            ui.horizontal(|ui| {
                for (label, mode) in [
                    ("Off", CaptureMode::Off),
                    ("System", CaptureMode::System),
                    ("TUN (soon)", CaptureMode::Tun),
                ] {
                    let selected = state.mode == mode;
                    if ui.selectable_label(selected, label).clicked() {
                        apply_mode(&self.rt, &self.cfg, &self.gui, mode);
                    }
                }
            });
            if let Some(e) = &state.last_error {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("capture: {e}"));
            } else {
                ui.label(&state.status_line);
            }

            ui.separator();
            ui.heading("Certificate");
            match state.ca_installed {
                Some(true) => {
                    ui.colored_label(egui::Color32::from_rgb(40, 160, 80), "CA installed");
                }
                Some(false) => {
                    ui.colored_label(egui::Color32::from_rgb(200, 120, 40), "CA not installed");
                }
                None => {
                    ui.label("CA status unknown");
                }
            }
            if !state.ca_fp.is_empty() {
                ui.label(egui::RichText::new(format!("sha256: {}", state.ca_fp)).small());
            }
            ui.horizontal(|ui| {
                if ui.button("Install CA").clicked() {
                    spawn_ca_op(self.rt.clone(), self.cfg.clone(), CaOp::Install);
                }
                if ui.button("Uninstall CA").clicked() {
                    spawn_ca_op(self.rt.clone(), self.cfg.clone(), CaOp::Uninstall);
                }
                if ui.button("Refresh").clicked() {
                    self.refresh_async(ctx.clone());
                }
            });

            ui.separator();
            ui.heading("RustCache stats");
            if state.stats_json.is_empty() {
                ui.label("(no data yet)");
            } else {
                ui.add(
                    egui::TextEdit::multiline(&mut state.stats_json.as_str())
                        .desired_width(f32::INFINITY)
                        .desired_rows(6)
                        .font(egui::TextStyle::Monospace),
                );
            }

            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                if ui.button("Quit (restores system proxy)").clicked() {
                    (self.on_quit)();
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });

        ctx.request_repaint_after(Duration::from_secs(2));
    }
}

fn on_off(v: bool) -> &'static str {
    if v { "up" } else { "down" }
}

fn apply_mode(rt: &tokio::runtime::Handle, cfg: &ClientConfig, gui: &SharedGui, mode: CaptureMode) {
    let cfg = cfg.clone();
    let gui = gui.clone();
    rt.spawn(async move {
        let capture = match Capture::default_store() {
            Ok(c) => c,
            Err(e) => {
                gui.write().last_error = Some(format!("state: {e}"));
                return;
            }
        };
        let result = if mode == CaptureMode::Off {
            capture.disable()
        } else {
            capture.enable(&cfg, mode)
        };
        let mut g = gui.write();
        match result {
            Ok(msg) => {
                g.mode = mode;
                g.capture_on = mode != CaptureMode::Off;
                g.status_line = msg;
                g.last_error = None;
            }
            Err(e) => {
                g.last_error = Some(format!("{e:#}"));
            }
        }
    });
}

enum CaOp {
    Install,
    Uninstall,
}

fn spawn_ca_op(rt: tokio::runtime::Handle, cfg: ClientConfig, op: CaOp) {
    rt.spawn(async move {
        let api = RustCacheApi::new(&cfg.rustcache_api).with_timeout(Duration::from_secs(5));
        let _ = match op {
            CaOp::Install => crate::ca::install(&api, &cfg.ca, None, false).await,
            CaOp::Uninstall => crate::ca::uninstall(&api, &cfg.ca, false).await,
        };
    });
}

impl ClientApp {
    /// User pressed Connect: discover ports and retarget the proxy.
    fn connect_host(&self, host: String) {
        let rt = self.rt.clone();
        let gui = self.gui.clone();
        let upstream = self.upstream.clone();
        let mut cfg = self.cfg.clone();
        gui.write().connecting = true;
        gui.write().last_error = None;
        rt.spawn(async move {
            match crate::api::discover_target(&host).await {
                Ok(d) => {
                    *upstream.write() = crate::api::UpstreamCfg::from_discovered(&d);
                    // Persist the API base so the next start finds the same instance.
                    cfg.rustcache_api = d.api_base.clone();
                    let _ = cfg.save(&ClientConfig::default_path());
                    let mut g = gui.write();
                    g.connecting = false;
                    g.discovered = Some(d.clone());
                    g.host_input = d.host.clone();
                    g.ports_line = format!(
                        "discovered → http:{}  https:{}  socks:{}",
                        d.http_port, d.https_port, d.socks_port
                    );
                    g.status_line = format!("connected to {}", d.api_base);
                    g.last_error = None;
                    g.ca_installed = None; // force refresh
                }
                Err(e) => {
                    let mut g = gui.write();
                    g.connecting = false;
                    g.ports_line.clear();
                    g.last_error = Some(format!("connect failed: {e}"));
                }
            }
        });
    }

    fn refresh_async(&self, ctx: egui::Context) {
        let api =
            RustCacheApi::new(&self.cfg.rustcache_api).with_timeout(Duration::from_millis(800));
        let gui = self.gui.clone();
        let cfg = self.cfg.clone();
        self.rt.spawn(async move {
            if let Ok(st) = crate::ca::status(&api, &cfg.ca).await {
                let mut g = gui.write();
                g.ca_installed = Some(st.installed);
                g.ca_fp = st.fingerprint_sha256;
            }
            if let Ok(v) = api.stats().await {
                gui.write().stats_json = serde_json::to_string_pretty(&v)
                    .unwrap_or_default()
                    .chars()
                    .take(2000)
                    .collect();
            }
            ctx.request_repaint();
        });
    }
}

/// Build a simple solid-color RGBA tray icon.
pub fn tray_icon_rgba(color: [u8; 3]) -> Vec<u8> {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4];
    let r = (S / 2) as i32;
    for y in 0..S as i32 {
        for x in 0..S as i32 {
            let dx = x - r;
            let dy = y - r;
            let inside = dx * dx + dy * dy <= r * r;
            let i = ((y as usize) * S + (x as usize)) * 4;
            if inside {
                px[i] = color[0];
                px[i + 1] = color[1];
                px[i + 2] = color[2];
                px[i + 3] = 255;
            }
        }
    }
    px
}

/// Optional system tray. Returns the tray so it stays alive.
pub struct Tray {
    #[allow(dead_code)]
    _tray: tray_icon::TrayIcon,
}

pub fn build_tray(on_toggle: impl Fn() + Send + Sync + 'static) -> Result<Tray> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIconBuilder};

    let menu = Menu::new();
    let toggle = MenuItem::new("Capture On/Off", true, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&toggle)?;
    menu.append(&quit)?;

    let icon = Icon::from_rgba(tray_icon_rgba([40, 160, 80]), 32, 32)?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("RustCache Client")
        .with_icon(icon)
        .build()?;

    let toggle_id = toggle.id().clone();
    let quit_id = quit.id().clone();
    let on_toggle = Arc::new(on_toggle);
    std::thread::spawn(move || {
        let receiver = MenuEvent::receiver();
        while let Ok(ev) = receiver.recv() {
            if ev.id == toggle_id {
                on_toggle();
            } else if ev.id == quit_id {
                std::process::exit(0);
            }
        }
    });

    Ok(Tray { _tray: tray })
}
