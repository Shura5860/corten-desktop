use std::borrow::Cow;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tracing::{info, warn};
use zbus::connection::Builder;
use zbus::interface;

use corten_config::ConfigManager;
use corten_i18n::Language;
use corten_services::bluetooth;
use corten_services::network;
use corten_services::pipewire;
use corten_services::portal;
use corten_services::upower;
use corten_theme::iced_styles::acrylic_glass_style;
use corten_theme::Transition;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, ThemeMode, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS,
    NOTO_SANS_BYTES,
};
use iced::widget::{
    button, column, container, horizontal_space, image, mouse_area, row, scrollable, slider, text,
    text_input, tooltip, vertical_space,
};

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CustomAppletState {
    pub name: String,
    pub text: String,
    pub tooltip: Option<String>,
    pub class: Option<String>,
    pub percentage: Option<u8>,
    pub icon: Option<String>,
}

const LOGO_PNG: &[u8] = include_bytes!("../../assets/logo.png");
use iced::{event, keyboard, Alignment, Element, Event, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

mod config;
mod sni;
mod system;
mod weather;

use config::{PanelConfig, PanelPosition};
use system::SystemInfo;
use weather::WeatherReport;

fn launch_fallback_chain(spec: &str) -> std::io::Result<()> {
    let mut last_error = None;
    for candidate in spec.split("||") {
        let mut argv = candidate.split_whitespace();
        let Some(program) = argv.next() else { continue };
        match Command::new(program).args(argv).spawn() {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty launch command")
    }))
}

fn find_binary(bin_name: &str) -> std::path::PathBuf {
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let candidate = dir.join(bin_name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    if let Ok(cur) = std::env::current_dir() {
        let candidate = cur.join(bin_name);
        if candidate.is_file() {
            return candidate;
        }
        let target_rel = cur.join("target/release").join(bin_name);
        if target_rel.is_file() {
            return target_rel;
        }
        let target_cand = cur.join("target/debug").join(bin_name);
        if target_cand.is_file() {
            return target_cand;
        }
    }
    for dir in &[
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
    ] {
        let p = std::path::Path::new(dir).join(bin_name);
        if p.is_file() {
            return p;
        }
    }
    std::path::PathBuf::from(bin_name)
}

fn get_corten_launcher_cmdline() -> Option<String> {
    let pids = std::process::Command::new("pidof")
        .arg("corten-launcher")
        .output()
        .ok()?;
    if !pids.status.success() {
        return None;
    }
    let pid_str = String::from_utf8_lossy(&pids.stdout);
    let first_pid = pid_str.split_whitespace().next()?;
    let cmdline_path = format!("/proc/{}/cmdline", first_pid);
    std::fs::read_to_string(cmdline_path).ok()
}

fn launch_tray_application(id: &str) -> std::io::Result<()> {
    let program = match id {
        "steam" => "steam",
        "discord" => "discord",
        "telegram" => "telegram-desktop",
        "obs" => "obs",
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unknown tray application",
            ))
        }
    };
    Command::new(program).spawn().map(|_| ())
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting Corten macOS Dock (corten-panel)...");

    let config_manager = ConfigManager::<PanelConfig>::load_or_default("panel")?;
    let initial_config = config_manager.get().clone();

    let (ipc_tx, ipc_rx) = tokio::sync::mpsc::unbounded_channel();

    // Spawn D-Bus org.corten.Panel service for CLI / IPC toggling & reload
    let dbus_tx = ipc_tx.clone();
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            tracing::warn!("Failed to initialize tokio runtime for Panel D-Bus service");
            return;
        };

        rt.block_on(async move {
            let server = PanelServer { tx: dbus_tx.clone() };
            let (watcher, _items) = sni::StatusNotifierWatcherServer::new(dbus_tx);
            watcher.start_monitor();
            if let Ok(b) = Builder::session() {
                if let Ok(b) = b.name("org.corten.Panel") {
                    if let Ok(b) = b.serve_at("/org/corten/Panel", server) {
                        if let Ok(b) = b.serve_at("/StatusNotifierWatcher", watcher) {
                            if let Ok(conn) = b.build().await {
                                // Request org.kde.StatusNotifierWatcher non-fatally so panel IPC
                                // remains active even if another system tray watcher is running.
                                let _ = conn.request_name("org.kde.StatusNotifierWatcher").await;
                                std::future::pending::<()>().await;
                            }
                        }
                    }
                }
            }
        });
    });

    let anchor = match initial_config.position {
        PanelPosition::Top => Anchor::Top,
        PanelPosition::Bottom => Anchor::Bottom,
    };

    let (margin, exclusive_zone) = ((0, 0, 10, 0), 0);

    let args: Vec<String> = std::env::args().collect();
    let binded_output = args
        .windows(2)
        .find(|w| w[0] == "--output" || w[0] == "-o")
        .map(|w| w[1].clone());

    let flags = PanelAppFlags {
        config_manager,
        ipc_rx: Some(ipc_rx),
        output_name: binded_output.clone(),
    };

    let initial_item_count = 1 + initial_config.pinned_apps.len() + 2;
    let initial_width = ((initial_item_count as u32) * 58 + 48).clamp(380, 1100);

    CortenPanelApp::run(Settings {
        id: Some("corten-panel".into()),
        flags,
        fonts: vec![Cow::Borrowed(LUCIDE_FONT_BYTES), Cow::Borrowed(NOTO_SANS_BYTES)],
        layer_settings: LayerShellSettings {
            anchor,
            layer: Layer::Top, // High priority desktop bar, below Overlay (lockscreen)
            margin,
            exclusive_zone,
            size: Some((initial_width, initial_config.height)), // Center-aligned compact dock pill
            // Request keyboard focus on demand when a popover is open so its
            // controls remain usable without a pointer.
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            binded_output_name: binded_output,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

#[derive(Debug, Clone)]
pub enum PanelIpcCommand {
    Toggle,
    ReloadConfig,
    TriggerModule(String),
    UpdateSniItems(Vec<sni::SniItem>),
}

pub struct PanelServer {
    tx: tokio::sync::mpsc::UnboundedSender<PanelIpcCommand>,
}

#[interface(name = "org.corten.Panel")]
impl PanelServer {
    async fn toggle(&self) {
        info!("D-Bus: Panel toggle received");
        let _ = self.tx.send(PanelIpcCommand::Toggle);
    }

    async fn toggle_visibility(&self) {
        info!("D-Bus: Panel toggle_visibility received");
        let _ = self.tx.send(PanelIpcCommand::Toggle);
    }

    async fn trigger_module(&self, module_name: &str) {
        info!("D-Bus: Panel trigger_module received: {}", module_name);
        let _ = self
            .tx
            .send(PanelIpcCommand::TriggerModule(module_name.to_string()));
    }

    async fn reload_config(&self) {
        info!("D-Bus: Panel reload_config received");
        let _ = self.tx.send(PanelIpcCommand::ReloadConfig);
    }

    async fn start_recording(
        &self,
        output: &str,
        fps: u32,
        monitor_only: bool,
    ) -> zbus::fdo::Result<()> {
        SystemInfo::start_recording(output.to_string(), fps, monitor_only)
            .await
            .map_err(zbus::fdo::Error::Failed)
    }

    async fn stop_recording(&self) -> zbus::fdo::Result<()> {
        SystemInfo::stop_recording()
            .await
            .map_err(zbus::fdo::Error::Failed)
    }

    async fn get_recording(&self) -> bool {
        SystemInfo::recording_active().await.unwrap_or(false)
    }
}

pub struct PanelAppFlags {
    pub config_manager: ConfigManager<PanelConfig>,
    pub ipc_rx: Option<tokio::sync::mpsc::UnboundedReceiver<PanelIpcCommand>>,
    pub output_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveApp {
    Terminal,
    Files,
    Settings,
    Browser,
    Editor,
    Ai,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
    MicMute(bool),
    BatteryWarning(u8),
    Workspace(usize),
    Toast(String),
    Switcher(Vec<(String, String, bool)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockMenuTarget {
    Pinned(ActiveApp, usize),
    Dynamic { win_id: u64, app_id: String, title: String },
}

#[derive(Debug, Clone)]
pub enum DockAction {
    LaunchNew(String),
    Focus(u64),
    CloseWindow(u64, String),
    PinToggle(String),
    MoveLeft(usize),
    MoveRight(usize),
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    AnimationTick,
    CompositorEvent(String),
    SwitchWorkspace(usize),
    SelectApp(ActiveApp),
    FocusWindow(u64),
    OpenDockContextMenu(DockMenuTarget),
    CloseDockContextMenu,
    DockMenuAction(DockAction),
    DockHover(Option<String>),
    MoveDockItemLeft(usize),
    MoveDockItemRight(usize),
    MediaPlayersUpdated(Vec<corten_services::mpris::MediaPlayer>),
    ToggleMiniTaskManager,
    KillProcess(u32),
    OpenFullTaskManager,
    ToggleDownloadsStack,
    ToggleNetSpeedDetails,
    ToggleWindowPreviews(String),
    CloseWindowPreviews,
    ToggleAutohideMode,
    PinAppToDock(String),
    UnpinAppFromDock(String),
    CloseWindowById(u64, String),
    OpenFilePath(PathBuf),
    OpenTrash,
    EmptyTrash,
    ToggleDesktopContextHub,
    CloseDesktopContextHub,
    TriggerWallpaperCycle,
    OpenLauncher,
    ToggleLauncherModal,
    LauncherSearchChanged(String),
    LaunchCommand(String),
    OpenDashboard,
    ToggleAudioMute,
    ToggleAudioSlider,
    SetVolume(u8),
    SetBrightness(u8),
    ToggleWifiDetails,
    ScanWifi,
    WifiScanned(Vec<corten_services::network::WifiNetwork>),
    SelectWifiSsid(String),
    WifiPasswordChanged(String),
    ConnectWifiSubmit,
    CloseWifiModal,
    SelectAudioSink(u32),
    ToggleControlCenter,
    ToggleDnd,
    ToggleNightLight,
    TogglePowerMenu,
    ToggleCalendar,
    ShowDesktop,
    ToggleCaffeine,
    SetPowerProfile(String),
    ToggleBluetooth,
    TakeScreenshot(bool),
    ScreenshotResult(bool, Result<String, String>),
    ToggleRecording,
    ChooseRecordingPath,
    RecordingPathSelected(Result<String, String>),
    RecordingResult(Result<(), String>),
    RecordingStatus(bool),
    ToggleNotificationCenter,
    ClearNotifications,
    ToggleClipboardModal,
    ClipboardSearchChanged(String),
    CopyClipboardItem(String),
    DeleteClipboardItem(usize),
    ClearClipboardHistory,
    ShowOsd(OsdKind, u8),
    OsdTick,
    ClickTrayItem(String),
    RightClickTrayItem(String),
    SystemAction(SystemPowerAction),
    MprisPlayPause(String),
    MprisNext(String),
    MprisPrevious(String),
    SetAppStreamVolume(u32, u8),
    ToggleAppStreamMute(u32),
    ConfirmSystemAction,
    CancelSystemAction,
    WeatherUpdated(WeatherReport),
    RefreshWeather,
    CustomScriptResult(usize, String),
    ClickCustomScript(usize),
    RightClickCustomScript(usize),
    ConfigReloaded(PanelConfig),
    SetPatina(PatinaVariant),
    IpcCommand(PanelIpcCommand),
    IcedEvent(Event),
    LayerAction(LayershellCustomActions),
    NoOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPowerAction {
    Lock,
    Suspend,
    Logout,
    Reboot,
    Poweroff,
}

impl SystemPowerAction {
    fn requires_confirmation(self) -> bool {
        matches!(self, Self::Reboot | Self::Poweroff)
    }
}

impl TryInto<LayershellCustomActions> for Message {
    type Error = Self;

    fn try_into(self) -> Result<LayershellCustomActions, Self::Error> {
        match self {
            Message::LayerAction(action) => Ok(action),
            _ => Err(self),
        }
    }
}

pub struct CortenPanelApp {
    config_manager: ConfigManager<PanelConfig>,
    config: PanelConfig,
    _config_watcher: Option<notify::RecommendedWatcher>,
    ipc_rx: Option<
        std::sync::Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<PanelIpcCommand>>>,
    >,
    theme_mode: ThemeMode,
    current_time: String,
    current_date: String,
    audio_status: Option<(u8, bool)>,
    audio_sinks: Vec<pipewire::AudioSink>,
    show_volume_slider: bool,
    network_status: (bool, String, String), // (connected, label, ip)
    wifi_networks: Vec<network::WifiNetwork>,
    selected_wifi_ssid: Option<String>,
    wifi_password_input: String,
    show_wifi_modal: bool,
    battery_status: Option<(u8, bool)>, // (percent, is_charging)
    bluetooth_status: (bool, Option<String>), // (powered, device_name)
    dnd_enabled: bool,
    show_wifi_details: bool,
    show_control_center: bool,
    show_power_menu: bool,
    pending_power_action: Option<SystemPowerAction>,
    show_calendar: bool,
    panel_visible: bool,
    show_launcher_modal: bool,
    popover_transition: Transition,
    launcher_search: String,
    active_workspace: usize,
    occupied_workspaces: Vec<u32>,
    compositor_windows: Vec<(u64, String, String, u32, bool, bool)>,
    active_app: ActiveApp,
    running_apps: (bool, bool, bool, bool), // (Terminal, Files, Browser, Editor)
    other_apps: Vec<(String, String)>,
    caffeine_enabled: bool,
    listening_ports: Vec<u16>,
    tray_items: Vec<system::TrayItem>,
    sni_items: Vec<sni::SniItem>,
    recording_active: bool,
    mic_active: bool,
    night_light_enabled: bool,
    media_players: Vec<corten_services::mpris::MediaPlayer>,
    app_streams: Vec<corten_services::pipewire::AppAudioStream>,
    output_name: Option<String>,
    custom_script_outputs: Vec<CustomAppletState>,
    show_clipboard_modal: bool,
    clipboard_history: Vec<String>,
    clipboard_search: String,
    osd_state: Option<(OsdKind, u8, u8)>,
    show_notification_center: bool,
    notification_history: Vec<(String, String, String)>,
    known_block_devices: std::collections::HashSet<String>,
    running_custom_scripts: std::collections::HashSet<usize>,
    sentinel_mtimes: std::collections::HashMap<std::path::PathBuf, u64>,
    weather_report: WeatherReport,
    active_power_profile: String,
    downloads_items: Vec<(String, String, PathBuf)>,
    show_downloads_stack: bool,
    dock_context_menu: Option<DockMenuTarget>,
    show_mini_task_manager: bool,
    top_processes: Vec<system::QuickProcessInfo>,
    memory_summary: (u64, u64, f32),
    net_speed_tracker: system::NetworkSpeedTracker,
    net_speed_display: (String, String),
    show_net_speed_details: bool,
    active_window_previews: Option<String>,
    trash_item_count: usize,
    language: Language,
    active_patina: PatinaVariant,
    brightness_pct: u8,
    show_desktop_context_hub: bool,
    audio_vu_level: (f32, f32),
    last_dock_width: u32,
    hovered_dock_item: Option<String>,
}

fn check_system_integrity(sentinel_mtimes: &mut std::collections::HashMap<std::path::PathBuf, u64>) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let home_path = std::path::Path::new(&home);
    let targets = [
        home_path.join(".ssh/authorized_keys"),
        home_path.join(".bashrc"),
        home_path.join(".profile"),
        home_path.join(".config/systemd/user"),
    ];

    let mut alert = None;
    for target in &targets {
        if let Ok(meta) = std::fs::metadata(target) {
            if let Ok(modified) = meta.modified() {
                let mtime = modified.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                if let Some(&prev_mtime) = sentinel_mtimes.get(target) {
                    if mtime > prev_mtime {
                        sentinel_mtimes.insert(target.clone(), mtime);
                        let file_name = target.file_name().unwrap_or_default().to_string_lossy();
                        alert = Some(format!("🚨 Güvenlik: {} dosyasında değişiklik tespit edildi!", file_name));
                        tracing::warn!("Corten Security Sentinel: Sensitive file modification detected on {:?}", target);
                    }
                } else {
                    sentinel_mtimes.insert(target.clone(), mtime);
                }
            }
        }
    }
    alert
}

fn is_custom_command_safe(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    let forbidden_patterns = [
        "sudo ", "doas ", "pkexec ", "su ",
        "rm -rf", "rm -fr", "rm -r -f",
        ":(){ :|:& };:", // fork bomb
        "mkfs", "dd if=",
        "| bash", "| sh", "| zsh", "| python",
        "> /dev/sd", "> /dev/nvme",
        "/etc/shadow", "/etc/passwd",
    ];
    for pattern in forbidden_patterns {
        if lower.contains(pattern) {
            return false;
        }
    }
    true
}

fn convert_svg_to_png(svg_path: &std::path::Path, out_size: u32) -> Option<PathBuf> {
    let cache_dir = PathBuf::from("/tmp/corten_icon_cache");
    let _ = std::fs::create_dir_all(&cache_dir);

    let file_stem = svg_path.file_stem()?.to_string_lossy();
    let out_file = cache_dir.join(format!("{}_{}x{}.png", file_stem, out_size, out_size));
    if out_file.exists() {
        return Some(out_file);
    }

    let status = std::process::Command::new("rsvg-convert")
        .args([
            "-w", &out_size.to_string(),
            "-h", &out_size.to_string(),
            svg_path.to_str()?,
            "-o", out_file.to_str()?,
        ])
        .status()
        .ok()?;

    if status.success() && out_file.exists() {
        Some(out_file)
    } else {
        None
    }
}

fn resolve_system_icon(app_id: &str) -> Option<PathBuf> {
    let lower = app_id.to_lowercase();
    let candidates: &[&str] = if lower.contains("corten-ai") || lower.contains("ai") || lower.contains("assistant") {
        &[
            "/usr/share/pixmaps/corten-ai.png",
            "assets/logos/ai_concept1_aura_core.png",
            "/usr/share/pixmaps/corten.png",
            "assets/logo.png",
        ]
    } else if lower.contains("corten-terminal") || lower.contains("term") || lower.contains("alacritty") || lower.contains("kitty") || lower.contains("foot") || lower.contains("konsole") {
        &[
            "/usr/share/icons/oxygen/base/48x48/apps/utilities-terminal.png",
            "/usr/share/icons/breeze/apps/48/utilities-terminal.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/utilities-terminal.png",
            "/usr/share/icons/hicolor/48x48/apps/alacritty.png",
        ]
    } else if lower.contains("corten-files") || lower.contains("file") || lower.contains("nautilus") || lower.contains("dolphin") || lower.contains("thunar") {
        &[
            "/usr/share/icons/oxygen/base/48x48/apps/system-file-manager.png",
            "/usr/share/icons/breeze/apps/48/system-file-manager.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/system-file-manager.png",
        ]
    } else if lower.contains("corten-settings") || lower.contains("setting") || lower.contains("config") || lower.contains("control") {
        &[
            "/usr/share/icons/oxygen/base/48x48/categories/preferences-system.png",
            "/usr/share/icons/breeze/preferences/32/preferences-system.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/preferences-system.png",
        ]
    } else if lower.contains("editor") || lower.contains("code") || lower.contains("nvim") || lower.contains("rust") || lower.contains("text") || lower.contains("kate") {
        &[
            "/usr/share/icons/oxygen/base/48x48/apps/accessories-text-editor.png",
            "/usr/share/icons/breeze/apps/48/accessories-text-editor.svg",
            "/usr/share/icons/AdwaitaLegacy/48x48/legacy/accessories-text-editor.png",
            "/usr/share/pixmaps/vscode.png",
        ]
    } else if lower.contains("browser") || lower.contains("firefox") || lower.contains("chrom") || lower.contains("brave") || lower.contains("zen") || lower.contains("web") {
        &[
            "/usr/share/icons/hicolor/48x48/apps/firefox.png",
            "/usr/share/icons/oxygen/base/48x48/apps/internet-web-browser.png",
            "/usr/share/icons/breeze/apps/48/internet-web-browser.svg",
        ]
    } else {
        &[]
    };

    for p in candidates {
        let path = PathBuf::from(p);
        if path.exists() {
            if p.ends_with(".svg") {
                if let Some(png) = convert_svg_to_png(&path, 48) {
                    return Some(png);
                }
            } else {
                return Some(path);
            }
        }
    }

    let pixmap_png = format!("/usr/share/pixmaps/{}.png", lower);
    let p_path = PathBuf::from(&pixmap_png);
    if p_path.exists() {
        return Some(p_path);
    }

    let hicolor_png = format!("/usr/share/icons/hicolor/48x48/apps/{}.png", lower);
    let h_path = PathBuf::from(&hicolor_png);
    if h_path.exists() {
        return Some(h_path);
    }

    let breeze_svg = format!("/usr/share/icons/breeze/apps/48/{}.svg", lower);
    let b_path = PathBuf::from(&breeze_svg);
    if b_path.exists() {
        if let Some(png) = convert_svg_to_png(&b_path, 48) {
            return Some(png);
        }
    }

    None
}

fn app_icon_char(app_id: &str) -> char {
    let lower = app_id.to_lowercase();
    if lower.contains("term") || lower.contains("alacritty") || lower.contains("kitty") || lower.contains("foot") || lower.contains("bash") {
        Icon::TERMINAL
    } else if lower.contains("firefox") || lower.contains("chrom") || lower.contains("brave") || lower.contains("zen") || lower.contains("web") || lower.contains("browser") {
        Icon::GLOBE
    } else if lower.contains("code") || lower.contains("rust") || lower.contains("nvim") || lower.contains("emacs") || lower.contains("editor") {
        Icon::CODE
    } else if lower.contains("music") || lower.contains("spotify") || lower.contains("vlc") || lower.contains("mpv") {
        Icon::MUSIC
    } else if lower.contains("file") || lower.contains("corten-files") || lower.contains("thunar") || lower.contains("nautilus") {
        Icon::FOLDER
    } else if lower.contains("setting") || lower.contains("corten-settings") || lower.contains("control") {
        Icon::SETTINGS
    } else if lower.contains("ai") || lower.contains("corten-ai") || lower.contains("assistant") {
        Icon::CPU
    } else {
        Icon::APP_WINDOW
    }
}

fn is_matching_window(app_id: &str, title: &str, app: ActiveApp) -> bool {
    let lower_id = app_id.to_lowercase();
    let lower_title = title.to_lowercase();

    match app {
        ActiveApp::Terminal => {
            lower_id.contains("corten-terminal")
                || lower_id == "alacritty"
                || lower_id == "kitty"
                || lower_id == "foot"
                || lower_id == "konsole"
                || lower_id == "wezterm"
                || lower_id == "xterm"
                || (lower_id.is_empty() && (lower_title.contains("corten terminal") || lower_title.ends_with("terminal") || lower_title.starts_with("terminal")))
        }
        ActiveApp::Files => {
            lower_id.contains("corten-files")
                || lower_id.contains("corten_files")
                || lower_id == "thunar"
                || lower_id == "dolphin"
                || lower_id == "nautilus"
                || lower_id.starts_with("org.gnome.nautilus")
                || lower_id.starts_with("org.kde.dolphin")
                || (lower_id.is_empty() && (lower_title == "corten files" || lower_title.starts_with("corten files") || lower_title == "files" || lower_title.starts_with("thunar") || lower_title.starts_with("dolphin")))
        }
        ActiveApp::Settings => {
            lower_id.contains("corten-settings")
                || lower_id.contains("corten_settings")
                || lower_id == "gnome-control-center"
                || lower_id == "systemsettings"
                || lower_id.starts_with("org.gnome.settings")
                || (lower_id.is_empty() && (lower_title == "corten settings" || lower_title.starts_with("corten settings") || lower_title == "settings" || lower_title == "system settings"))
        }
        ActiveApp::Browser => {
            lower_id.contains("firefox")
                || lower_id.contains("chrome")
                || lower_id.contains("brave")
                || lower_id.contains("chromium")
                || lower_id.contains("zen")
                || lower_id.contains("opera")
                || (lower_id.is_empty() && (lower_title.contains("firefox") || lower_title.contains("chrome") || lower_title.contains("brave")))
        }
        ActiveApp::Editor => {
            lower_id.contains("code")
                || lower_id.contains("antigravity")
                || lower_id == "kate"
                || lower_id == "gedit"
                || lower_id == "nvim"
                || lower_id == "zed"
                || (lower_id.is_empty() && (lower_title.contains("visual studio code") || lower_title.contains("antigravity")))
        }
        ActiveApp::Ai => {
            lower_id.contains("corten-ai")
                || lower_id.contains("corten_ai")
                || lower_id.contains("corten-assistant")
                || (lower_id.is_empty() && (lower_title.contains("corten ai") || lower_title.starts_with("corten ai")))
        }
    }
}

fn is_matching_app(effective_id: &str, app: ActiveApp) -> bool {
    is_matching_window(effective_id, effective_id, app)
}

fn is_matching_pinned(effective_id: &str, pinned_app_name: &str) -> bool {
    let lower_eff = effective_id.to_lowercase();
    let lower_pin = pinned_app_name.to_lowercase();
    if lower_eff == lower_pin || lower_eff.contains(&lower_pin) || lower_pin.contains(&lower_eff) {
        return true;
    }
    let app = match lower_pin.as_str() {
        "terminal" | "corten-terminal" => ActiveApp::Terminal,
        "files" | "corten-files" => ActiveApp::Files,
        "settings" | "corten-settings" => ActiveApp::Settings,
        "editor" => ActiveApp::Editor,
        "browser" => ActiveApp::Browser,
        "corten-ai" | "ai" => ActiveApp::Ai,
        _ => return false,
    };
    is_matching_app(effective_id, app)
}

fn get_recent_downloads() -> Vec<(String, String, PathBuf)> {
    let Some(home) = std::env::var("HOME").ok() else {
        return Vec::new();
    };
    let dl_dir = PathBuf::from(home).join("Downloads");
    let Ok(entries) = std::fs::read_dir(&dl_dir) else {
        return Vec::new();
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if name.starts_with('.') {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let mtime = entry.metadata().and_then(|m| m.modified()).ok();
            let size_str = if size > 1024 * 1024 {
                format!("{:.1} MB", size as f32 / (1024.0 * 1024.0))
            } else if size > 1024 {
                format!("{:.0} KB", size as f32 / 1024.0)
            } else {
                format!("{} B", size)
            };
            items.push((name, size_str, path, mtime));
        }
    }

    items.sort_by_key(|a| std::cmp::Reverse(a.3));
    items.into_iter().take(5).map(|(n, s, p, _)| (n, s, p)).collect()
}

fn get_trash_count() -> usize {
    let Some(home) = std::env::var("HOME").ok() else {
        return 0;
    };
    let trash_dir = PathBuf::from(home).join(".local/share/Trash/files");
    std::fs::read_dir(trash_dir)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

impl CortenPanelApp {
    fn calculate_dock_width(&self) -> u32 {
        let start_orb_w = 64 + 12; // Start Orb + spacing
        let capsule_padding = 40; // 20px padding left + right

        // Pinned apps width (dynamic based on pinned count + 56px for all_apps_btn)
        let pinned_count = self.config.pinned_apps.len() as u32;
        let pinned_w = if pinned_count > 0 {
            pinned_count * 54 + pinned_count * 10 + 46
        } else {
            46
        };

        // Dynamic unpinned running apps
        let mut seen = std::collections::HashSet::new();
        let mut dyn_unpinned_count = 0;
        for (_, app_id, _title, _, _, _) in &self.compositor_windows {
            let effective_id = if !app_id.trim().is_empty() {
                app_id.as_str()
            } else if !_title.trim().is_empty() {
                _title.as_str()
            } else {
                continue;
            };
            let is_pinned = self.config.pinned_apps.iter().any(|pin| is_matching_pinned(effective_id, pin));
            if !is_pinned && seen.insert(effective_id.to_string()) {
                dyn_unpinned_count += 1;
            }
        }
        let dynamic_apps_w = dyn_unpinned_count * 66;

        // Media player capsule
        let active_player = self
            .media_players
            .iter()
            .find(|p| p.playback_status == corten_services::mpris::PlaybackStatus::Playing)
            .or_else(|| self.media_players.first());
        let media_w = if active_player.map(|p| !p.title.is_empty()).unwrap_or(false) {
            210
        } else {
            0
        };

        // Divider
        let divider_w = 13;

        // Right section
        let mut right_w = 0;
        let mut right_items_count = 0;

        if self.recording_active {
            right_w += 70;
            right_items_count += 1;
        }
        if self.mic_active {
            right_w += 65;
            right_items_count += 1;
        }
        if self.config.show_audio {
            right_w += if self.show_volume_slider { 220 } else { 38 };
            right_items_count += 1;
            if self.audio_vu_level.0 > 0.05 || self.audio_vu_level.1 > 0.05 {
                right_w += 50;
                right_items_count += 1;
            }
        }
        if self.config.show_network {
            right_w += if self.show_wifi_details { 140 } else { 38 };
            right_items_count += 1;
        }
        if !self.tray_items.is_empty() {
            right_w += self.tray_items.len() as u32 * 32;
            right_items_count += 1;
        }
        let active_scripts = self.custom_script_outputs.iter().filter(|s| !s.text.is_empty()).count();
        if active_scripts > 0 {
            right_w += active_scripts as u32 * 80;
            right_items_count += active_scripts;
        }
        // downloads, trash, clock
        right_w += 36 + 42 + 95;
        right_items_count += 3;

        if right_items_count > 1 {
            right_w += (right_items_count - 1) as u32 * 8;
        }

        let total_content_w = start_orb_w + capsule_padding + pinned_w + dynamic_apps_w as u32 + media_w + divider_w + right_w + 40;

        let popover_open = self.show_control_center
            || self.show_power_menu
            || self.show_calendar
            || self.show_clipboard_modal
            || self.show_notification_center
            || self.show_downloads_stack
            || self.show_mini_task_manager
            || self.dock_context_menu.is_some()
            || self.active_window_previews.is_some()
            || self.show_net_speed_details
            || self.show_desktop_context_hub;

        let min_w = if popover_open { 560 } else { 520 };
        total_content_w.max(min_w).clamp(520, 1800)
    }

    fn layer_config_task(&self) -> Task<Message> {
        let dynamic_width = self.calculate_dock_width();
        let popover_open = self.show_control_center
            || self.show_power_menu
            || self.show_calendar
            || self.show_clipboard_modal
            || self.show_notification_center
            || self.show_downloads_stack
            || self.show_mini_task_manager
            || self.dock_context_menu.is_some()
            || self.active_window_previews.is_some()
            || self.show_net_speed_details
            || self.show_desktop_context_hub;

        let height = if !self.panel_visible {
            1
        } else if popover_open {
            560
        } else if self.osd_state.is_some() {
            136
        } else {
            78
        };

        let target_width = if popover_open {
            dynamic_width.max(430)
        } else {
            dynamic_width
        };

        Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((
            target_width, height,
        ))))
    }
}

impl Application for CortenPanelApp {
    type Message = Message;
    type Flags = PanelAppFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(mut flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = flags.config_manager.get().clone();
        let watcher = flags
            .config_manager
            .spawn_watcher()
            .map(|(w, _)| w)
            .map_err(|e| warn!("Failed to spawn config watcher: {:?}", e))
            .ok();
        let ipc_rx = flags
            .ipc_rx
            .take()
            .map(|rx| std::sync::Arc::new(tokio::sync::Mutex::new(rx)));

        let (current_time, current_date) =
            SystemInfo::get_datetime(&config.time_format, &config.date_format);
        let audio_status = pipewire::get_volume();
        let audio_sinks = pipewire::list_sinks();
        let network_status =
            network::connection_details().unwrap_or_else(SystemInfo::get_network_details);
        let wifi_networks = Vec::new();
        let battery_status = upower::battery_status().or_else(SystemInfo::get_battery_info);
        let (active_workspace, occupied_workspaces, compositor_windows) =
            SystemInfo::get_compositor_state_for_output(flags.output_name.as_deref());
        let running_apps = SystemInfo::get_running_apps();
        let listening_ports = SystemInfo::get_listening_dev_ports();

        let bluetooth_status = bluetooth::status();
        let tray_items = SystemInfo::get_active_tray_items();

        let load_font_task = iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::Tick);
        let night_light_enabled = corten_config::ConfigManager::<corten_config::DisplaySettingsConfig>::load_or_default("displays")
            .map(|c| c.get().night_light)
            .unwrap_or(false);

        let (active_patina, theme_mode, language) = corten_config::ConfigManager::<corten_config::AppearanceConfig>::load_or_default("appearance")
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let m = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                let l = Language::from_code(&c.get().language);
                (p, m, l)
            })
            .unwrap_or_else(|_| (PatinaVariant::Rust, ThemeMode::Dark, Language::detect_system()));

        let mut app = Self {
                config_manager: flags.config_manager,
                config,
                _config_watcher: watcher,
                ipc_rx,
                theme_mode,
                active_patina,
                current_time,
                current_date,
                audio_status,
                audio_sinks,
                show_volume_slider: false,
                network_status,
                wifi_networks,
                selected_wifi_ssid: None,
                wifi_password_input: String::new(),
                show_wifi_modal: false,
                battery_status,
                bluetooth_status,
                dnd_enabled: false,
                show_wifi_details: false,
                show_control_center: false,
                show_power_menu: false,
                pending_power_action: None,
                show_calendar: false,
                panel_visible: true,
                show_launcher_modal: false,
                popover_transition: Transition::new(120),
                launcher_search: String::new(),
                active_workspace,
                occupied_workspaces,
                compositor_windows,
                active_app: ActiveApp::Terminal,
                running_apps,
                other_apps: SystemInfo::get_other_running_apps(),
                caffeine_enabled: false,
                listening_ports,
                tray_items,
                sni_items: Vec::new(),
                recording_active: false,
                mic_active: false,
                night_light_enabled,
                media_players: Vec::new(),
                app_streams: Vec::new(),
                output_name: flags.output_name,
                custom_script_outputs: Vec::new(),
                show_clipboard_modal: false,
                clipboard_history: Vec::new(),
                clipboard_search: String::new(),
                osd_state: None,
                show_notification_center: false,
                notification_history: vec![
                    ("Corten Masaüstü".into(), "Sistem hazır ve optimize durumda.".into(), "Şimdi".into()),
                ],
                known_block_devices: std::collections::HashSet::new(),
                running_custom_scripts: std::collections::HashSet::new(),
                sentinel_mtimes: std::collections::HashMap::new(),
                weather_report: WeatherReport::default(),
                active_power_profile: "balanced".to_string(),
                downloads_items: get_recent_downloads(),
                show_downloads_stack: false,
                dock_context_menu: None,
                show_mini_task_manager: false,
                top_processes: Vec::new(),
                memory_summary: SystemInfo::get_quick_memory_summary(),
                net_speed_tracker: system::NetworkSpeedTracker::default(),
                net_speed_display: ("0 B/s".into(), "0 B/s".into()),
                show_net_speed_details: false,
                active_window_previews: None,
                trash_item_count: get_trash_count(),
                language,
                brightness_pct: SystemInfo::get_brightness_percent(),
                show_desktop_context_hub: false,
                audio_vu_level: (0.0, 0.0),
                last_dock_width: 600,
                hovered_dock_item: None,
            };
        let init_dock_w = app.calculate_dock_width();
        app.last_dock_width = init_dock_w;
        let initial_config_task = app.layer_config_task();
        (
            app,
            Task::batch(vec![
                load_font_task,
                initial_config_task,
                Task::perform(weather::fetch_weather_report(), Message::WeatherUpdated),
                Task::perform(
                    async { tokio::task::spawn_blocking(corten_services::network::scan_wifi).await.unwrap_or_default() },
                    Message::WifiScanned,
                ),
                Task::perform(SystemInfo::recording_active(), |result| {
                    Message::RecordingStatus(result.unwrap_or(false))
                }),
            ]),
        )
    }

    fn namespace(&self) -> String {
        "corten-panel".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: palette.text_primary.to_iced(),
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen().map(Message::IcedEvent);
        let timer = iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick);
        
        let needs_animation = !self.popover_transition.is_complete()
            || self.media_players.iter().any(|p| p.playback_status == corten_services::mpris::PlaybackStatus::Playing)
            || !self.app_streams.is_empty()
            || self.audio_vu_level.0 > 0.01
            || self.audio_vu_level.1 > 0.01;

        let mut subs = vec![events, timer];
        if needs_animation {
            subs.push(iced::time::every(Duration::from_millis(16)).map(|_| Message::AnimationTick));
        }

        // Standard UNIX domain event stream subscription (Hyprland / Sway model)
        let event_sock_path = corten_ipc::event_socket_path();
        let event_stream = futures::stream::unfold(None, move |mut opt_lines: Option<tokio::io::Lines<tokio::io::BufReader<tokio::net::UnixStream>>>| {
            let sock_path = event_sock_path.clone();
            async move {
                loop {
                    if opt_lines.is_none() {
                        if let Ok(s) = tokio::net::UnixStream::connect(&sock_path).await {
                            use tokio::io::AsyncBufReadExt;
                            opt_lines = Some(tokio::io::BufReader::new(s).lines());
                        } else {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            continue;
                        }
                    }

                    if let Some(ref mut lines) = opt_lines {
                        match lines.next_line().await {
                            Ok(Some(line)) => {
                                return Some((Message::CompositorEvent(line), opt_lines));
                            }
                            _ => {
                                opt_lines = None;
                                tokio::time::sleep(Duration::from_millis(300)).await;
                            }
                        }
                    }
                }
            }
        });
        let event_sub = Subscription::run_with_id("corten-compositor-events", event_stream);
        subs.push(event_sub);

        if let Some(rx_arc) = &self.ipc_rx {
            let rx_clone = std::sync::Arc::clone(rx_arc);
            let stream = futures::stream::unfold(rx_clone, |rx| async move {
                let mut lock = rx.lock().await;
                let cmd = lock.recv().await?;
                drop(lock);
                Some((Message::IpcCommand(cmd), rx))
            });
            let ipc_sub = Subscription::run_with_id("corten-panel-ipc", stream);
            subs.push(ipc_sub);
        }
        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        if matches!(
            &message,
            Message::ToggleControlCenter
                | Message::TogglePowerMenu
                | Message::ToggleCalendar
                | Message::ToggleAudioSlider
                | Message::ToggleWifiDetails
                | Message::ToggleDesktopContextHub
                | Message::OpenDockContextMenu(_)
        ) {
            self.popover_transition.restart();
        }
        match message {
            Message::AnimationTick => {
                self.popover_transition.tick(16);
                let is_audio_playing = self.media_players.iter().any(|p| p.playback_status == corten_services::mpris::PlaybackStatus::Playing) || !self.app_streams.is_empty();
                if is_audio_playing {
                    let now = std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis();
                    let phase = (now as f32 / 110.0).sin();
                    let phase2 = (now as f32 / 75.0).cos();
                    let target_l = (0.50 + 0.35 * phase + 0.12 * phase2).clamp(0.12, 0.96);
                    let target_r = (0.50 + 0.30 * phase2 + 0.18 * phase).clamp(0.12, 0.96);
                    self.audio_vu_level.0 = self.audio_vu_level.0 * 0.70 + target_l * 0.30;
                    self.audio_vu_level.1 = self.audio_vu_level.1 * 0.70 + target_r * 0.30;
                } else {
                    self.audio_vu_level.0 *= 0.85;
                    self.audio_vu_level.1 *= 0.85;
                }
            }
            Message::IpcCommand(cmd) => match cmd {
                PanelIpcCommand::Toggle => {
                    self.panel_visible = !self.panel_visible;
                    info!("IPC panel visibility changed: {}", self.panel_visible);
                    return self.layer_config_task();
                }
                PanelIpcCommand::ReloadConfig => {
                    info!("IPC ReloadConfig received in CortenPanelApp - reloading from disk");
                    if let Ok(app_cfg) = corten_config::ConfigManager::<corten_config::AppearanceConfig>::load_or_default("appearance") {
                        self.language = Language::from_code(&app_cfg.get().language);
                        self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                        self.theme_mode = match app_cfg.get().theme_mode {
                            corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                            corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                        };
                    }
                    if let Ok(new_cfg_mgr) = ConfigManager::<PanelConfig>::load_or_default("panel")
                    {
                        self.config = new_cfg_mgr.get().clone();
                        self.config_manager = new_cfg_mgr;
                        info!("Panel config reloaded successfully: {:?}", self.config);
                        return self.layer_config_task();
                    }
                }
                PanelIpcCommand::TriggerModule(module) => {
                    match module.as_str() {
                        "calendar" => {
                            self.show_calendar = !self.show_calendar;
                            self.show_control_center = false;
                            self.show_power_menu = false;
                            self.show_clipboard_modal = false;
                            self.show_notification_center = false;
                        }
                        "control-center" => {
                            self.show_control_center = !self.show_control_center;
                            self.show_calendar = false;
                            self.show_power_menu = false;
                            self.show_clipboard_modal = false;
                            self.show_notification_center = false;
                        }
                        "clipboard" => {
                            self.show_clipboard_modal = !self.show_clipboard_modal;
                            self.show_calendar = false;
                            self.show_control_center = false;
                            self.show_power_menu = false;
                            self.show_notification_center = false;
                            if self.show_clipboard_modal {
                                if let Some(txt) = SystemInfo::read_clipboard_text() {
                                    if self.clipboard_history.first() != Some(&txt) {
                                        self.clipboard_history.retain(|item| item != &txt);
                                        self.clipboard_history.insert(0, txt);
                                        if self.clipboard_history.len() > 30 {
                                            self.clipboard_history.truncate(30);
                                        }
                                    }
                                }
                            }
                        }
                        "notifications" => {
                            self.show_notification_center = !self.show_notification_center;
                            self.show_calendar = false;
                            self.show_control_center = false;
                            self.show_power_menu = false;
                            self.show_clipboard_modal = false;
                            self.show_desktop_context_hub = false;
                        }
                        "desktop_hub" | "desktop-hub" | "canvas" => {
                            self.show_desktop_context_hub = !self.show_desktop_context_hub;
                            self.show_calendar = false;
                            self.show_control_center = false;
                            self.show_power_menu = false;
                            self.show_clipboard_modal = false;
                            self.show_notification_center = false;
                            self.popover_transition.restart();
                            return self.layer_config_task();
                        }
                        cmd if cmd.starts_with("osd:mic") => {
                            self.mic_active = SystemInfo::is_mic_active();
                            self.osd_state = Some((OsdKind::MicMute(!self.mic_active), 0, 3));
                        }
                        cmd if cmd.starts_with("osd:battery") => {
                            let pct = self.battery_status.map(|b| b.0).unwrap_or(20);
                            self.osd_state = Some((OsdKind::BatteryWarning(pct), pct, 3));
                        }
                        cmd if cmd.starts_with("osd:volume") => {
                            self.audio_status = pipewire::get_volume();
                self.mic_active = SystemInfo::is_mic_active();
                            let vol = if let Some(stripped) = cmd.strip_prefix("osd:volume:") {
                                stripped.trim().parse::<u8>().unwrap_or_else(|_| self.audio_status.map(|a| a.0).unwrap_or(50))
                            } else {
                                self.audio_status.map(|a| a.0).unwrap_or(50)
                            };
                            self.osd_state = Some((OsdKind::Volume, vol, 3));
                        }
                        cmd if cmd.starts_with("osd:brightness") => {
                            let bri = if let Some(stripped) = cmd.strip_prefix("osd:brightness:") {
                                stripped.trim().parse::<u8>().unwrap_or_else(|_| SystemInfo::get_brightness_percent())
                            } else {
                                SystemInfo::get_brightness_percent()
                            };
                            self.osd_state = Some((OsdKind::Brightness, bri, 3));
                        }
                        "osd:switcher" => {
                            let (_, _, windows) = SystemInfo::get_compositor_state_for_output(self.output_name.as_deref());
                            let list: Vec<(String, String, bool)> = windows
                                .into_iter()
                                .map(|(_, app_id, title, _, is_active, _)| (app_id, title, is_active))
                                .collect();
                            if !list.is_empty() {
                                self.osd_state = Some((OsdKind::Switcher(list), 100, 3));
                            }
                        }
                        cmd if cmd.starts_with("osd:toast:") => {
                            let msg = cmd.trim_start_matches("osd:toast:").to_string();
                            self.osd_state = Some((OsdKind::Toast(msg), 100, 3));
                        }
                        cmd if cmd.starts_with("toast:") => {
                            let msg = cmd.trim_start_matches("toast:").to_string();
                            self.osd_state = Some((OsdKind::Toast(msg), 100, 3));
                        }
                        cmd if cmd.starts_with("osd:workspace:") => {
                            if let Ok(ws) = cmd.trim_start_matches("osd:workspace:").trim().parse::<usize>() {
                                self.active_workspace = ws;
                                self.osd_state = Some((OsdKind::Workspace(ws), ws as u8, 3));
                            }
                        }
                        "power" => {
                            self.show_power_menu = !self.show_power_menu;
                            self.show_calendar = false;
                            self.show_control_center = false;
                        }
                        "audio" => {
                            self.show_volume_slider = !self.show_volume_slider;
                            self.show_wifi_details = false;
                        }
                        "wifi" => {
                            self.show_wifi_details = !self.show_wifi_details;
                            self.show_volume_slider = false;
                        }
                        "launcher" => {
                            let is_running = std::process::Command::new("pidof")
                                .arg("corten-launcher")
                                .output()
                                .map(|o| o.status.success())
                                .unwrap_or(false);

                            if is_running {
                                let _ = std::process::Command::new("killall")
                                    .arg("-TERM")
                                    .arg("corten-launcher")
                                    .spawn();
                            } else {
                                let bin = find_binary("corten-launcher");
                                if let Err(error) = std::process::Command::new(bin)
                                    .arg("--start-menu")
                                    .spawn()
                                {
                                    warn!(%error, "Failed to open launcher module");
                                }
                            }
                        }
                        _ => warn!(module = %module, "Unknown panel module requested"),
                    }

                    if module != "launcher" {
                        return self.layer_config_task();
                    }
                }
                PanelIpcCommand::UpdateSniItems(items) => {
                    self.sni_items = items;
                    let mut combined = Vec::new();
                    for item in &self.sni_items {
                        combined.push(system::TrayItem {
                            id: item.id.clone(),
                            title: item.title.clone(),
                            icon: item.icon_display.clone(),
                        });
                    }
                    for fallback in &self.tray_items {
                        if !combined.iter().any(|c| c.id == fallback.id) {
                            combined.push(fallback.clone());
                        }
                    }
                    self.tray_items = combined;
                }
            },
            Message::ConfigReloaded(new_config) => {
                self.config = new_config;
                return self.layer_config_task();
            }
            Message::CompositorEvent(_ev) => {
                let had_osd = self.osd_state.is_some();
                let (active_ws, occupied_ws, windows) =
                    SystemInfo::get_compositor_state_for_output(self.output_name.as_deref());
                if active_ws > 0 && self.active_workspace != active_ws {
                    self.active_workspace = active_ws;
                    self.osd_state = Some((OsdKind::Workspace(active_ws), active_ws as u8, 3));
                }
                self.occupied_workspaces = occupied_ws;
                self.compositor_windows = windows;
                self.running_apps = SystemInfo::get_running_apps_from_windows(&self.compositor_windows);
                self.other_apps = SystemInfo::get_other_running_apps_from_windows(&self.compositor_windows);
                let current_w = self.calculate_dock_width();
                if current_w != self.last_dock_width || self.osd_state.is_some() != had_osd {
                    self.last_dock_width = current_w;
                    return self.layer_config_task();
                }
            }
            Message::Tick => {
                let had_osd = self.osd_state.is_some();
                let current_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let (time, date) =
                    SystemInfo::get_datetime(&self.config.time_format, &self.config.date_format);
                self.current_time = time;
                self.current_date = date;
                let (active_ws, occupied_ws, windows) =
                    SystemInfo::get_compositor_state_for_output(self.output_name.as_deref());
                if active_ws > 0 && self.active_workspace != active_ws {
                    self.active_workspace = active_ws;
                    self.osd_state = Some((OsdKind::Workspace(active_ws), active_ws as u8, 3));
                }
                self.occupied_workspaces = occupied_ws;
                for (_, app_id, title, _, is_active, _) in &windows {
                    if *is_active {
                        let hay = format!("{} {}", app_id.to_lowercase(), title.to_lowercase());
                        if hay.contains("alacritty")
                            || hay.contains("kitty")
                            || hay.contains("foot")
                            || hay.contains("konsole")
                            || hay.contains("terminal")
                        {
                            self.active_app = ActiveApp::Terminal;
                        } else if hay.contains("dolphin")
                            || hay.contains("nautilus")
                            || hay.contains("thunar")
                            || hay.contains("files")
                        {
                            self.active_app = ActiveApp::Files;
                        } else if hay.contains("firefox")
                            || hay.contains("chrome")
                            || hay.contains("brave")
                            || hay.contains("chromium")
                        {
                            self.active_app = ActiveApp::Browser;
                        } else if hay.contains("code")
                            || hay.contains("antigravity")
                            || hay.contains("kate")
                            || hay.contains("gedit")
                        {
                            self.active_app = ActiveApp::Editor;
                        }
                    }
                }
                self.compositor_windows = windows;
                self.running_apps = SystemInfo::get_running_apps_from_windows(&self.compositor_windows);
                self.other_apps = SystemInfo::get_other_running_apps_from_windows(&self.compositor_windows);
                let current_w = self.calculate_dock_width();
                if current_w != self.last_dock_width {
                    self.last_dock_width = current_w;
                    return self.layer_config_task();
                }

                // Smart On-Demand & Cadence Polling (Zero-Fork / Low-Load)
                if self.show_volume_slider || self.show_control_center || current_secs.is_multiple_of(2) {
                    self.audio_status = pipewire::get_volume();
                }
                if self.show_control_center || current_secs.is_multiple_of(2) {
                    self.mic_active = SystemInfo::is_mic_active();
                    self.network_status = SystemInfo::get_network_details();
                    self.battery_status = SystemInfo::get_battery_info();
                }
                if self.show_control_center && current_secs.is_multiple_of(3) {
                    return Task::perform(async {
                        tokio::task::spawn_blocking(move || {
                            corten_services::mpris::list_players().unwrap_or_default()
                        }).await.unwrap_or_default()
                    }, Message::MediaPlayersUpdated);
                }
                if (self.show_volume_slider || self.show_control_center) && current_secs.is_multiple_of(3) {
                    self.audio_sinks = pipewire::list_sinks();
                }
                if self.show_control_center && current_secs.is_multiple_of(4) {
                    self.listening_ports = SystemInfo::get_listening_dev_ports();
                    self.app_streams = corten_services::pipewire::list_app_streams();
                }
                let (_rx_s, _tx_s, rx_fmt, tx_fmt) = self.net_speed_tracker.sample_speed();
                self.net_speed_display = (rx_fmt, tx_fmt);
                self.tray_items = SystemInfo::get_active_tray_items();

                // Gentle trash polling (every 10 seconds or on-demand)
                if current_secs.is_multiple_of(10) {
                    self.trash_item_count = get_trash_count();
                }

                if self.config.autohide && self.config.autohide_mode == "dodge" {
                    let has_windows_on_ws = self.compositor_windows.iter().any(|(_, _, _, ws, _, _)| *ws as usize == self.active_workspace);
                    let popover_open = self.show_control_center || self.show_power_menu || self.show_calendar
                        || self.show_clipboard_modal || self.show_notification_center || self.show_downloads_stack
                        || self.show_mini_task_manager || self.dock_context_menu.is_some()
                        || self.active_window_previews.is_some() || self.show_net_speed_details || self.show_desktop_context_hub;
                    let target_visible = !has_windows_on_ws || popover_open;
                    if self.panel_visible != target_visible {
                        self.panel_visible = target_visible;
                        return self.layer_config_task();
                    }
                }

                // Dynamic custom script applets polling
                let mut custom_script_tasks = Vec::new();
                if !self.config.custom_scripts.is_empty() {
                    if self.custom_script_outputs.len() != self.config.custom_scripts.len() {
                        self.custom_script_outputs = self
                            .config
                            .custom_scripts
                            .iter()
                            .map(|s| CustomAppletState {
                                name: s.name.clone(),
                                text: String::new(),
                                tooltip: None,
                                class: None,
                                percentage: None,
                                icon: s.icon.clone(),
                            })
                            .collect();
                    }
                    for (idx, script) in self.config.custom_scripts.iter().enumerate() {
                        let interval = script.interval_secs.max(1);
                        if current_secs.is_multiple_of(interval) && !self.running_custom_scripts.contains(&idx) {
                            let cmd_str = script.command.clone();
                            if !is_custom_command_safe(&cmd_str) {
                                tracing::warn!("Security: Blocked dangerous custom script '{}': {}", script.name, cmd_str);
                                if idx < self.custom_script_outputs.len() {
                                    self.custom_script_outputs[idx].text = "⛔ Güvenlik Engeli".to_string();
                                }
                                continue;
                            }

                            self.running_custom_scripts.insert(idx);
                            custom_script_tasks.push(Task::perform(
                                async move {
                                    // Hard 3-second timeout prevents rogue processes and resource exhaustion
                                    let res = tokio::time::timeout(
                                        std::time::Duration::from_secs(3),
                                        tokio::process::Command::new("sh")
                                            .arg("-c")
                                            .arg(&cmd_str)
                                            .output(),
                                    )
                                    .await;

                                    match res {
                                        Ok(Ok(out)) => {
                                            String::from_utf8_lossy(&out.stdout).trim().to_string()
                                        }
                                        Ok(Err(_)) => String::new(),
                                        Err(_) => "⏱ Zaman Aşımı".to_string(),
                                    }
                                },
                                move |res| Message::CustomScriptResult(idx, res),
                            ));
                        }
                    }
                }

                // Removable USB / disk auto-detection & notification (every 2 seconds)
                if current_secs.is_multiple_of(2) {
                    if let Ok(devices) = corten_services::udisks2::list_devices() {
                        let removable: Vec<_> =
                            devices.into_iter().filter(|d| d.is_removable).collect();
                        for dev in &removable {
                            if !self.known_block_devices.contains(&dev.object_path) {
                                self.known_block_devices.insert(dev.object_path.clone());
                                let mount_target = if dev.mount_point.is_none() {
                                    corten_services::udisks2::mount_device(&dev.object_path).ok()
                                } else {
                                    dev.mount_point.clone()
                                };

                                let label = if dev.label.is_empty() {
                                    &dev.device
                                } else {
                                    &dev.label
                                };
                                let size_gb = dev.size as f64 / (1024.0 * 1024.0 * 1024.0);
                                let body = if let Some(ref mp) = mount_target {
                                    format!(
                                        "{} ({:.1} GB) bağlandı: {}",
                                        label,
                                        size_gb,
                                        mp.display()
                                    )
                                } else {
                                    format!("{} ({:.1} GB) algılandı", label, size_gb)
                                };

                                let _ = corten_services::notifications::send_notification(
                                    "Corten Sürücü",
                                    "Harici Disk / USB Algılandı",
                                    &body,
                                    "drive-removable-media",
                                    6000,
                                );

                                self.notification_history.insert(
                                    0,
                                    ("Harici Disk Algılandı".into(), body, "Şimdi".into()),
                                );
                            }
                        }
                        let current_paths: std::collections::HashSet<String> =
                            removable.into_iter().map(|d| d.object_path).collect();
                        self.known_block_devices
                            .retain(|p| current_paths.contains(p));
                    }
                }

                // Critical system file integrity watcher / tripwire (every 5 seconds)
                if current_secs.is_multiple_of(5) {
                    if let Some(alert) = check_system_integrity(&mut self.sentinel_mtimes) {
                        self.osd_state = Some((OsdKind::Toast(alert.clone()), 100, 4));
                        self.notification_history.insert(
                            0,
                            ("Güvenlik Bekçisi".into(), alert, "Şimdi".into()),
                        );
                    }
                }

                // Clipboard history polling (gentle on-demand & periodic, zero-excess-fork)
                if self.show_clipboard_modal || current_secs.is_multiple_of(3) {
                    if let Some(txt) = SystemInfo::read_clipboard_text() {
                        if self.clipboard_history.first() != Some(&txt) {
                            self.clipboard_history.retain(|item| item != &txt);
                            self.clipboard_history.insert(0, txt);
                            if self.clipboard_history.len() > 30 {
                                self.clipboard_history.truncate(30);
                            }
                        }
                    }
                }

                // OSD countdown
                if let Some((kind, lvl, ticks)) = self.osd_state.clone() {
                    if ticks > 1 {
                        self.osd_state = Some((kind, lvl, ticks - 1));
                    } else {
                        self.osd_state = None;
                    }
                }

                // Dynamically sync dock width & OSD height with real-time contents immediately
                let current_w = self.calculate_dock_width();
                let mut post_tasks = custom_script_tasks;
                if current_w != self.last_dock_width || self.osd_state.is_some() != had_osd {
                    self.last_dock_width = current_w;
                    post_tasks.push(self.layer_config_task());
                }
                if !post_tasks.is_empty() {
                    return Task::batch(post_tasks);
                }
            }
            Message::CustomScriptResult(idx, raw_output) => {
                self.running_custom_scripts.remove(&idx);
                if idx < self.custom_script_outputs.len() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw_output) {
                        let text = json.get("text").and_then(|v| v.as_str()).unwrap_or(&raw_output).trim().chars().take(48).collect::<String>();
                        let tooltip = json.get("tooltip").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let class = json.get("class").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let percentage = json.get("percentage").and_then(|v| v.as_u64()).map(|p| p as u8);
                        self.custom_script_outputs[idx].text = text;
                        self.custom_script_outputs[idx].tooltip = tooltip;
                        self.custom_script_outputs[idx].class = class;
                        self.custom_script_outputs[idx].percentage = percentage;
                    } else {
                        let first_line = raw_output.lines().next().unwrap_or("").trim().chars().take(48).collect::<String>();
                        self.custom_script_outputs[idx].text = first_line;
                        self.custom_script_outputs[idx].tooltip = None;
                        self.custom_script_outputs[idx].class = None;
                        self.custom_script_outputs[idx].percentage = None;
                    }
                }
            }
            Message::ClickCustomScript(idx) => {
                if let Some(script) = self.config.custom_scripts.get(idx) {
                    if let Some(ref cmd) = script.on_click {
                        if is_custom_command_safe(cmd) {
                            let _ = std::process::Command::new("sh").args(["-c", cmd]).spawn();
                        }
                    }
                }
            }
            Message::RightClickCustomScript(idx) => {
                if let Some(script) = self.config.custom_scripts.get(idx) {
                    if let Some(ref cmd) = script.on_right_click {
                        if is_custom_command_safe(cmd) {
                            let _ = std::process::Command::new("sh").args(["-c", cmd]).spawn();
                        }
                    }
                }
            }
            Message::SwitchWorkspace(ws) => {
                if SystemInfo::set_active_workspace_on_output(self.output_name.as_deref(), ws) {
                    self.active_workspace = ws;
                    self.osd_state = Some((OsdKind::Workspace(ws), ws as u8, 3));
                    return self.layer_config_task();
                }
            }
            Message::SelectApp(app) => {
                self.active_app = app;
                let existing_id = self.compositor_windows.iter().find_map(
                    |(id, app_id, title, _, _, _)| {
                        if is_matching_window(app_id, title, app) {
                            Some(*id)
                        } else {
                            None
                        }
                    },
                );
                if let Some(win_id) = existing_id {
                    if SystemInfo::focus_window(win_id) {
                        return Task::none();
                    }
                }
                let result = match app {
                    ActiveApp::Terminal => {
                        let mut spawned = false;
                        for bin in [
                            "corten-terminal",
                            "/usr/local/bin/corten-terminal",
                            "target/release/corten-terminal",
                            "target/debug/corten-terminal",
                        ] {
                            if std::process::Command::new(bin).spawn().is_ok() {
                                spawned = true;
                                break;
                            }
                        }
                        if !spawned && std::process::Command::new("konsole").args(["--profile", "Corten"]).spawn().is_ok() {
                            spawned = true;
                        }
                        if !spawned && std::process::Command::new("/var/lib/flatpak/exports/bin/org.kde.konsole").args(["--profile", "Corten"]).spawn().is_ok() {
                            spawned = true;
                        }
                        if !spawned {
                            std::process::Command::new("alacritty")
                                .spawn()
                                .or_else(|_| std::process::Command::new("kitty").spawn())
                                .or_else(|_| std::process::Command::new("xterm").spawn())
                        } else {
                            std::process::Command::new("true").spawn()
                        }
                    }
                    ActiveApp::Files => {
                        let bin = find_binary("corten-files");
                        std::process::Command::new(&bin)
                            .spawn()
                            .or_else(|_| std::process::Command::new("corten-files").spawn())
                            .or_else(|_| std::process::Command::new("dolphin").spawn())
                            .or_else(|_| std::process::Command::new("nautilus").spawn())
                            .or_else(|_| std::process::Command::new("thunar").spawn())
                            .or_else(|_| {
                                let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
                                std::process::Command::new("xdg-open").arg(&home).spawn()
                            })
                    }
                    ActiveApp::Settings => {
                        let bin = find_binary("corten-settings");
                        std::process::Command::new(&bin)
                            .spawn()
                            .or_else(|_| std::process::Command::new("corten-settings").spawn())
                            .or_else(|_| std::process::Command::new("gnome-control-center").spawn())
                            .or_else(|_| std::process::Command::new("systemsettings").spawn())
                    }
                    ActiveApp::Browser => {
                        if portal::open_uri("https://google.com").unwrap_or(false) {
                            std::process::Command::new("true").spawn()
                        } else {
                            std::process::Command::new("xdg-open")
                                .arg("https://google.com")
                                .spawn()
                        }
                    }
                    ActiveApp::Editor => std::process::Command::new("code")
                        .spawn()
                        .or_else(|_| std::process::Command::new("gedit").spawn()),
                    ActiveApp::Ai => {
                        let bin = find_binary("corten-ai");
                        std::process::Command::new(bin).spawn()
                    }
                };
                if let Err(error) = result {
                    warn!(?app, %error, "Failed to launch panel application");
                }
            }
            Message::FocusWindow(win_id) => {
                SystemInfo::focus_window(win_id);
            }
            Message::OpenDockContextMenu(target) => {
                if self.dock_context_menu.as_ref() == Some(&target) {
                    self.dock_context_menu = None;
                } else {
                    self.dock_context_menu = Some(target);
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                    self.show_notification_center = false;
                    self.show_downloads_stack = false;
                    self.show_mini_task_manager = false;
                }
                return self.layer_config_task();
            }
            Message::CloseDockContextMenu => {
                self.dock_context_menu = None;
                return self.layer_config_task();
            }
            Message::DockMenuAction(action) => {
                match action {
                    DockAction::LaunchNew(cmd) => {
                        let _ = launch_fallback_chain(&cmd);
                    }
                    DockAction::Focus(win_id) => {
                        SystemInfo::focus_window(win_id);
                    }
                    DockAction::CloseWindow(_win_id, app_id) => {
                        let _ = SystemInfo::kill_process_by_name(&app_id);
                    }
                    DockAction::PinToggle(_app_id) => {
                        self.osd_state = Some((OsdKind::Toast("Dock ayarı güncellendi".to_string()), 1, 3));
                    }
                    DockAction::MoveLeft(idx) => {
                        if idx > 0 && idx < self.config.pinned_apps.len() {
                            self.config.pinned_apps.swap(idx, idx - 1);
                            let _ = self.config_manager.update(self.config.clone());
                            self.osd_state = Some((OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()), 100, 3));
                        }
                    }
                    DockAction::MoveRight(idx) => {
                        if idx + 1 < self.config.pinned_apps.len() {
                            self.config.pinned_apps.swap(idx, idx + 1);
                            let _ = self.config_manager.update(self.config.clone());
                            self.osd_state = Some((OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()), 100, 3));
                        }
                    }
                }
                self.dock_context_menu = None;
                return self.layer_config_task();
            }
            Message::MoveDockItemLeft(idx) => {
                if idx > 0 && idx < self.config.pinned_apps.len() {
                    self.config.pinned_apps.swap(idx, idx - 1);
                    let _ = self.config_manager.update(self.config.clone());
                    self.osd_state = Some((OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()), 100, 3));
                    return self.layer_config_task();
                }
            }
            Message::MoveDockItemRight(idx) => {
                if idx + 1 < self.config.pinned_apps.len() {
                    self.config.pinned_apps.swap(idx, idx + 1);
                    let _ = self.config_manager.update(self.config.clone());
                    self.osd_state = Some((OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()), 100, 3));
                    return self.layer_config_task();
                }
            }
            Message::DockHover(hover_app_opt) => {
                self.hovered_dock_item = hover_app_opt;
            }
            Message::MediaPlayersUpdated(players) => {
                self.media_players = players;
            }
            Message::ToggleMiniTaskManager => {
                self.show_mini_task_manager = !self.show_mini_task_manager;
                if self.show_mini_task_manager {
                    self.top_processes = SystemInfo::get_top_memory_processes(6);
                    self.memory_summary = SystemInfo::get_quick_memory_summary();
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                    self.show_notification_center = false;
                    self.show_downloads_stack = false;
                    self.dock_context_menu = None;
                }
                return self.layer_config_task();
            }
            Message::KillProcess(pid) => {
                let _ = SystemInfo::kill_process_by_pid(pid);
                self.top_processes = SystemInfo::get_top_memory_processes(6);
                self.memory_summary = SystemInfo::get_quick_memory_summary();
            }
            Message::ToggleNetSpeedDetails => {
                self.show_net_speed_details = !self.show_net_speed_details;
                if self.show_net_speed_details {
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                    self.show_notification_center = false;
                    self.show_downloads_stack = false;
                    self.show_mini_task_manager = false;
                    self.dock_context_menu = None;
                    self.active_window_previews = None;
                }
                return self.layer_config_task();
            }
            Message::ToggleWindowPreviews(app_id) => {
                if self.active_window_previews.as_deref() == Some(&app_id) {
                    self.active_window_previews = None;
                } else {
                    self.active_window_previews = Some(app_id);
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                    self.show_notification_center = false;
                    self.show_downloads_stack = false;
                    self.show_mini_task_manager = false;
                    self.dock_context_menu = None;
                    self.show_net_speed_details = false;
                }
                return self.layer_config_task();
            }
            Message::CloseWindowPreviews => {
                self.active_window_previews = None;
                return self.layer_config_task();
            }
            Message::ToggleAutohideMode => {
                self.config.autohide_mode = if self.config.autohide_mode == "dodge" {
                    "none".to_string()
                } else {
                    "dodge".to_string()
                };
                let _ = self.config_manager.update(self.config.clone());
                let toast = if self.config.autohide_mode == "dodge" {
                    "🛡️ Akıllı Gizlenme (Dodge) Aktif".to_string()
                } else {
                    "🛡️ Dock Daima Görünür".to_string()
                };
                self.osd_state = Some((OsdKind::Toast(toast), 100, 3));
                return self.layer_config_task();
            }
            Message::PinAppToDock(app_id) => {
                if !self.config.pinned_apps.contains(&app_id) {
                    self.config.pinned_apps.push(app_id);
                    let _ = self.config_manager.update(self.config.clone());
                    self.osd_state = Some((OsdKind::Toast("📌 Uygulama Dock'a sabitlendi".to_string()), 100, 3));
                    return self.layer_config_task();
                }
            }
            Message::UnpinAppFromDock(app_id) => {
                self.config.pinned_apps.retain(|a| a != &app_id);
                let _ = self.config_manager.update(self.config.clone());
                self.osd_state = Some((OsdKind::Toast("📌 Uygulama Dock'tan kaldırıldı".to_string()), 100, 3));
                return self.layer_config_task();
            }
            Message::CloseWindowById(_win_id, app_id) => {
                let _ = SystemInfo::kill_process_by_name(&app_id);
                self.active_window_previews = None;
                return self.layer_config_task();
            }
            Message::OpenFullTaskManager => {
                let _ = std::process::Command::new("corten-monitor").spawn();
                self.show_mini_task_manager = false;
                return self.layer_config_task();
            }
            Message::ToggleDownloadsStack => {
                self.show_downloads_stack = !self.show_downloads_stack;
                if self.show_downloads_stack {
                    self.downloads_items = get_recent_downloads();
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                    self.show_notification_center = false;
                    self.show_mini_task_manager = false;
                    self.dock_context_menu = None;
                }
                return self.layer_config_task();
            }
            Message::OpenFilePath(path) => {
                let _ = std::process::Command::new("xdg-open").arg(path).spawn();
                self.show_downloads_stack = false;
                return self.layer_config_task();
            }
            Message::OpenTrash => {
                let _ = std::process::Command::new("corten-files").arg("trash:///").spawn();
            }
            Message::EmptyTrash => {
                if let Ok(home) = std::env::var("HOME") {
                    let trash_dir = PathBuf::from(home).join(".local/share/Trash/files");
                    let _ = std::fs::remove_dir_all(&trash_dir);
                    let _ = std::fs::create_dir_all(&trash_dir);
                    self.trash_item_count = 0;
                }
            }
            Message::OpenLauncher | Message::ToggleLauncherModal => {
                if let Some(cmdline) = get_corten_launcher_cmdline() {
                    let _ = std::process::Command::new("killall")
                        .args(["-TERM", "corten-launcher"])
                        .status();
                    if cmdline.contains("--dashboard") {
                        let bin = find_binary("corten-launcher");
                        let _ = std::process::Command::new(bin)
                            .arg("--start-menu")
                            .spawn();
                    }
                } else {
                    let bin = find_binary("corten-launcher");
                    let _ = std::process::Command::new(bin)
                        .arg("--start-menu")
                        .spawn();
                }
            }
            Message::LauncherSearchChanged(query) => {
                self.launcher_search = query;
            }
            Message::LaunchCommand(cmd) => {
                self.show_launcher_modal = false;
                let _ = launch_fallback_chain(&cmd);
            }
            Message::OpenDashboard => {
                if let Some(cmdline) = get_corten_launcher_cmdline() {
                    let _ = std::process::Command::new("killall")
                        .args(["-TERM", "corten-launcher"])
                        .status();
                    if !cmdline.contains("--dashboard") {
                        let bin = find_binary("corten-launcher");
                        let _ = std::process::Command::new(bin)
                            .arg("--dashboard")
                            .spawn();
                    }
                } else {
                    let bin = find_binary("corten-launcher");
                    let _ = std::process::Command::new(bin)
                        .arg("--dashboard")
                        .spawn();
                }
            }
            Message::ToggleAudioMute => {
                let _ = pipewire::toggle_mute("@DEFAULT_AUDIO_SINK@");
                self.audio_status = pipewire::get_volume();
                self.mic_active = SystemInfo::is_mic_active();
            }
            Message::ToggleAudioSlider => {
                self.show_volume_slider = !self.show_volume_slider;
                if self.show_volume_slider {
                    self.show_wifi_details = false;
                }
                return self.layer_config_task();
            }
            Message::SetVolume(vol) => {
                let _ = pipewire::set_volume("@DEFAULT_AUDIO_SINK@", vol);
                if let Some((_, muted)) = self.audio_status {
                    self.audio_status = Some((vol, muted));
                } else {
                    self.audio_status = Some((vol, false));
                }
            }
            Message::SetBrightness(val) => {
                self.brightness_pct = val;
                SystemInfo::set_brightness_percent(val);
                self.osd_state = Some((OsdKind::Brightness, val, 3));
                return self.layer_config_task();
            }
            Message::SetPatina(patina) => {
                self.active_patina = patina;
                if let Ok(mut app_mgr) = ConfigManager::<corten_config::AppearanceConfig>::load_or_default("appearance") {
                    let mut cfg = app_mgr.get().clone();
                    cfg.patina = patina.code().to_string();
                    let (prim, _) = patina.colors();
                    cfg.accent_color = format!("#{:02X}{:02X}{:02X}", (prim.r * 255.0) as u8, (prim.g * 255.0) as u8, (prim.b * 255.0) as u8);
                    let _ = app_mgr.update(cfg);
                }
            }
            Message::ToggleDesktopContextHub => {
                self.show_desktop_context_hub = !self.show_desktop_context_hub;
                if self.show_desktop_context_hub {
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                    self.show_notification_center = false;
                    self.show_downloads_stack = false;
                    self.show_mini_task_manager = false;
                    self.dock_context_menu = None;
                    self.active_window_previews = None;
                    self.popover_transition.restart();
                }
                return self.layer_config_task();
            }
            Message::CloseDesktopContextHub => {
                self.show_desktop_context_hub = false;
                return self.layer_config_task();
            }
            Message::TriggerWallpaperCycle => {
                let _ = Command::new("killall")
                    .args(["-USR1", "corten-wallpaper"])
                    .spawn();
            }
            Message::SelectAudioSink(sink_id) => {
                info!("Selecting audio sink node: {}", sink_id);
                let _ = pipewire::set_default_sink(sink_id);
                self.audio_sinks = pipewire::list_sinks();
                self.audio_status = pipewire::get_volume();
                self.mic_active = SystemInfo::is_mic_active();
            }
            Message::ToggleWifiDetails => {
                self.show_control_center = !self.show_control_center;
                if self.show_control_center {
                    self.show_volume_slider = false;
                    self.show_calendar = false;
                    self.show_power_menu = false;
                    self.show_wifi_modal = false;
                    let config_task = self.layer_config_task();
                    let scan_task = Task::perform(
                        async { tokio::task::spawn_blocking(corten_services::network::scan_wifi).await.unwrap_or_default() },
                        Message::WifiScanned,
                    );
                    return Task::batch(vec![config_task, scan_task]);
                }
                return self.layer_config_task();
            }
            Message::ScanWifi => {
                return Task::perform(
                    async { tokio::task::spawn_blocking(corten_services::network::scan_wifi).await.unwrap_or_default() },
                    Message::WifiScanned,
                );
            }
            Message::WifiScanned(networks) => {
                self.wifi_networks = networks;
            }
            Message::SelectWifiSsid(ssid) => {
                self.selected_wifi_ssid = Some(ssid);
                self.wifi_password_input.clear();
                self.show_wifi_modal = true;
            }
            Message::WifiPasswordChanged(pwd) => {
                self.wifi_password_input = pwd;
            }
            Message::ConnectWifiSubmit => {
                if let Some(ssid) = &self.selected_wifi_ssid {
                    info!(
                        "Connecting to Wi-Fi SSID '{}' with password length {}",
                        ssid,
                        self.wifi_password_input.len()
                    );
                    let ok = network::connect_wifi(ssid, &self.wifi_password_input);
                    info!("Wi-Fi connection result: {}", ok);
                }
                self.show_wifi_modal = false;
                self.wifi_password_input.clear();
                self.network_status =
                    network::connection_details().unwrap_or_else(SystemInfo::get_network_details);
            }
            Message::CloseWifiModal => {
                self.show_wifi_modal = false;
                self.wifi_password_input.clear();
            }
            Message::ToggleCaffeine => {
                self.caffeine_enabled = !self.caffeine_enabled;
                if self.caffeine_enabled {
                    info!("Caffeine mode activated: screen keep-awake enabled");
                    let _ = std::process::Command::new("systemd-inhibit")
                        .args([
                            "--what=idle:sleep",
                            "--who=corten-panel",
                            "--why=Developer Caffeine Keep-Awake",
                            "sleep",
                            "infinity",
                        ])
                        .spawn();
                } else {
                    info!("Caffeine mode deactivated");
                    let _ = std::process::Command::new("pkill")
                        .args(["-f", "systemd-inhibit.*corten-panel"])
                        .spawn();
                }
            }
            Message::SetPowerProfile(profile) => {
                let _ = corten_services::power_profiles::set_active_profile(&profile);
                self.active_power_profile = profile;
            }
            Message::ToggleDnd => {
                self.dnd_enabled = !self.dnd_enabled;
                let enabled = self.dnd_enabled;
                tokio::spawn(async move {
                    if let Ok(conn) = zbus::Connection::session().await {
                        let _ = conn
                            .call_method(
                                Some("org.freedesktop.Notifications"),
                                "/org/freedesktop/Notifications",
                                Some("org.freedesktop.Notifications"),
                                "SetDoNotDisturb",
                                &(enabled,),
                            )
                            .await;
                    }
                });
                let toast = if self.dnd_enabled {
                    "🔕 Rahatsız Etme: Açık".to_string()
                } else {
                    "🔔 Rahatsız Etme: Kapalı".to_string()
                };
                self.osd_state = Some((OsdKind::Toast(toast), 100, 3));
            }
            Message::ToggleNightLight => {
                self.night_light_enabled = !self.night_light_enabled;
                if let Ok(mut cm) = corten_config::ConfigManager::<corten_config::DisplaySettingsConfig>::load_or_default("displays") {
                    let mut cfg = cm.get().clone();
                    cfg.night_light = self.night_light_enabled;
                    let _ = cm.update(cfg);
                }
                let toast = if self.night_light_enabled {
                    "🌙 Gece Işığı Aktif (Sıcak Tonlar)".to_string()
                } else {
                    "☀️ Gece Işığı Kapalı (Normal Renkler)".to_string()
                };
                self.osd_state = Some((OsdKind::Toast(toast), 100, 3));
            }
            Message::ToggleControlCenter => {
                self.show_control_center = !self.show_control_center;
                if self.show_control_center {
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.dock_context_menu = None;
                    self.active_window_previews = None;
                    self.show_net_speed_details = false;
                    self.show_downloads_stack = false;
                    self.show_mini_task_manager = false;
                    self.show_desktop_context_hub = false;
                    self.popover_transition.restart();
                    return Task::batch(vec![
                        self.layer_config_task(),
                        Task::perform(async {
                            tokio::task::spawn_blocking(move || {
                                corten_services::mpris::list_players().unwrap_or_default()
                            }).await.unwrap_or_default()
                        }, Message::MediaPlayersUpdated),
                    ]);
                }
                return self.layer_config_task();
            }
            Message::MprisPlayPause(ref bus) => {
                let bus_c = bus.clone();
                return Task::perform(async move {
                    tokio::task::spawn_blocking(move || {
                        let _ = corten_services::mpris::play_pause(&bus_c);
                        corten_services::mpris::list_players().unwrap_or_default()
                    }).await.unwrap_or_default()
                }, Message::MediaPlayersUpdated);
            }
            Message::MprisNext(ref bus) => {
                let bus_c = bus.clone();
                return Task::perform(async move {
                    tokio::task::spawn_blocking(move || {
                        let _ = corten_services::mpris::next(&bus_c);
                        corten_services::mpris::list_players().unwrap_or_default()
                    }).await.unwrap_or_default()
                }, Message::MediaPlayersUpdated);
            }
            Message::MprisPrevious(ref bus) => {
                let bus_c = bus.clone();
                return Task::perform(async move {
                    tokio::task::spawn_blocking(move || {
                        let _ = corten_services::mpris::previous(&bus_c);
                        corten_services::mpris::list_players().unwrap_or_default()
                    }).await.unwrap_or_default()
                }, Message::MediaPlayersUpdated);
            }
            Message::SetAppStreamVolume(id, vol) => {
                let _ = corten_services::pipewire::set_stream_volume(id, vol);
                if let Some(s) = self.app_streams.iter_mut().find(|s| s.id == id) {
                    s.volume_pct = vol;
                }
            }
            Message::ToggleAppStreamMute(id) => {
                let _ = corten_services::pipewire::toggle_stream_mute(id);
                if let Some(s) = self.app_streams.iter_mut().find(|s| s.id == id) {
                    s.is_muted = !s.is_muted;
                }
            }
            Message::TogglePowerMenu => {
                self.show_power_menu = !self.show_power_menu;
                if self.show_power_menu {
                    self.show_control_center = false;
                    self.show_calendar = false;
                }
                return self.layer_config_task();
            }
            Message::ToggleCalendar => {
                self.show_calendar = !self.show_calendar;
                if self.show_calendar {
                    self.show_control_center = false;
                    self.show_power_menu = false;
                }
                return self.layer_config_task();
            }
            Message::ShowDesktop => {
                info!("Toggling show desktop / peek wallpaper");
                if !SystemInfo::toggle_show_desktop() {
                    SystemInfo::set_active_workspace(9);
                }
                let (active_ws, occupied_ws, _) = SystemInfo::get_compositor_state();
                self.active_workspace = active_ws;
                self.occupied_workspaces = occupied_ws;
            }
            Message::TakeScreenshot(region) => {
                return Task::perform(
                    corten_services::portal::screenshot_uri(region),
                    move |result| Message::ScreenshotResult(region, result),
                );
            }
            Message::ScreenshotResult(region, result) => match result {
                Ok(uri) => {
                    info!(%uri, "Screenshot portal completed");
                    self.osd_state = Some((OsdKind::Toast("📸 Ekran görüntüsü panoya kopyalandı".to_string()), 100, 3));
                }
                Err(error) => {
                    warn!(%error, "Screenshot portal unavailable; using native fallback");
                    SystemInfo::take_screenshot_fallback(region);
                    self.osd_state = Some((OsdKind::Toast("📸 Ekran görüntüsü kaydedildi".to_string()), 100, 3));
                }
            },
            Message::ToggleRecording => {
                if self.recording_active {
                    return Task::perform(SystemInfo::stop_recording(), Message::RecordingResult);
                }
                return Task::done(Message::ChooseRecordingPath);
            }
            Message::ChooseRecordingPath => {
                return Task::perform(
                    portal::save_file("Kayıt konumunu seç", "corten-recording.mp4"),
                    Message::RecordingPathSelected,
                );
            }
            Message::RecordingPathSelected(result) => {
                let Ok(path) = result else {
                    if let Err(error) = result {
                        warn!(%error, "Recording output selection failed");
                    }
                    return Task::none();
                };
                return Task::perform(
                    SystemInfo::start_recording(path, 30, true),
                    Message::RecordingResult,
                );
            }
            Message::RecordingResult(result) => match result {
                Ok(()) => {
                    self.recording_active = !self.recording_active;
                    info!(
                        active = self.recording_active,
                        "Screen recording state changed"
                    );
                }
                Err(error) => warn!(%error, "Screen recording action failed"),
            },
            Message::RecordingStatus(active) => {
                self.recording_active = active;
            }
            Message::ToggleBluetooth => {
                let powered = self.bluetooth_status.0;
                if bluetooth::set_powered(!powered) {
                    self.bluetooth_status = bluetooth::status();
                }
            }
            Message::ClickTrayItem(id) => {
                info!("StatusNotifierItem / Tray item clicked: {}", id);
                if let Some(sni_item) = self.sni_items.iter().find(|item| item.id == id) {
                    let bus_name = sni_item.bus_name.clone();
                    let obj_path = sni_item.obj_path.clone();
                    tokio::spawn(async move {
                        if let Ok(conn) = zbus::Connection::session().await {
                            if let Ok(proxy) = zbus::Proxy::new(
                                &conn,
                                bus_name.as_str(),
                                obj_path.as_str(),
                                "org.kde.StatusNotifierItem",
                            )
                            .await
                            {
                                let _ = proxy.call::<_, _, ()>("Activate", &(0i32, 0i32)).await;
                            }
                        }
                    });
                } else if let Err(error) = launch_tray_application(&id) {
                    warn!(%error, tray_id = %id, "Failed to launch whitelisted tray application");
                }
            }
            Message::RightClickTrayItem(id) => {
                info!("StatusNotifierItem / Tray item right-clicked: {}", id);
                if let Some(sni_item) = self.sni_items.iter().find(|item| item.id == id) {
                    let bus_name = sni_item.bus_name.clone();
                    let obj_path = sni_item.obj_path.clone();
                    tokio::spawn(async move {
                        if let Ok(conn) = zbus::Connection::session().await {
                            if let Ok(proxy) = zbus::Proxy::new(
                                &conn,
                                bus_name.as_str(),
                                obj_path.as_str(),
                                "org.kde.StatusNotifierItem",
                            )
                            .await
                            {
                                let _ = proxy.call::<_, _, ()>("ContextMenu", &(0i32, 0i32)).await;
                            }
                        }
                    });
                }
            }
            Message::SystemAction(action) => {
                if action.requires_confirmation() {
                    self.pending_power_action = Some(action);
                    return Task::none();
                }
                self.show_control_center = false;
                self.show_power_menu = false;
                self.show_calendar = false;
                match action {
                    SystemPowerAction::Lock => {
                        if let Err(error) = std::process::Command::new("corten-lock").spawn() {
                            warn!(%error, "Failed to start lock screen");
                        }
                    }
                    SystemPowerAction::Suspend => {
                        if let Err(error) = std::process::Command::new("systemctl").arg("suspend").spawn() {
                            warn!(%error, "Failed to suspend system");
                        }
                    }
                    SystemPowerAction::Logout => {
                        if let Err(error) = std::process::Command::new("pkill")
                            .arg("corten-session")
                            .spawn()
                        {
                            warn!(%error, "Failed to log out Corten session");
                        }
                    }
                    SystemPowerAction::Reboot | SystemPowerAction::Poweroff => unreachable!(),
                }
                return self.layer_config_task();
            }
            Message::ConfirmSystemAction => {
                let Some(action) = self.pending_power_action.take() else {
                    return Task::none();
                };
                self.show_power_menu = false;
                let command = match action {
                    SystemPowerAction::Reboot => "reboot",
                    SystemPowerAction::Poweroff => "poweroff",
                    _ => return Task::none(),
                };
                if let Err(error) = Command::new("systemctl").arg(command).spawn() {
                    warn!(%error, command, "Failed to execute system power action");
                }
            }
            Message::CancelSystemAction => {
                self.pending_power_action = None;
            }
            Message::WeatherUpdated(report) => {
                self.weather_report = report;
            }
            Message::RefreshWeather => {
                return Task::perform(weather::fetch_weather_report(), Message::WeatherUpdated);
            }
            Message::ToggleClipboardModal => {
                self.show_clipboard_modal = !self.show_clipboard_modal;
                if self.show_clipboard_modal {
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_notification_center = false;
                    if let Some(txt) = SystemInfo::read_clipboard_text() {
                        if self.clipboard_history.first() != Some(&txt) {
                            self.clipboard_history.retain(|item| item != &txt);
                            self.clipboard_history.insert(0, txt);
                            if self.clipboard_history.len() > 30 {
                                self.clipboard_history.truncate(30);
                            }
                        }
                    }
                }
                return self.layer_config_task();
            }
            Message::ClipboardSearchChanged(query) => {
                self.clipboard_search = query;
            }
            Message::CopyClipboardItem(txt) => {
                SystemInfo::copy_to_clipboard(&txt);
                self.show_clipboard_modal = false;
                return self.layer_config_task();
            }
            Message::DeleteClipboardItem(idx) => {
                if idx < self.clipboard_history.len() {
                    self.clipboard_history.remove(idx);
                }
            }
            Message::ClearClipboardHistory => {
                self.clipboard_history.clear();
            }
            Message::ShowOsd(kind, level) => {
                self.osd_state = Some((kind, level, 3));
            }
            Message::OsdTick => {
                if let Some((kind, lvl, ticks)) = self.osd_state.clone() {
                    if ticks > 1 {
                        self.osd_state = Some((kind, lvl, ticks - 1));
                    } else {
                        self.osd_state = None;
                    }
                }
            }
            Message::ToggleNotificationCenter => {
                self.show_notification_center = !self.show_notification_center;
                if self.show_notification_center {
                    self.show_control_center = false;
                    self.show_power_menu = false;
                    self.show_calendar = false;
                    self.show_clipboard_modal = false;
                }
                return self.layer_config_task();
            }
            Message::ClearNotifications => {
                self.notification_history.clear();
            }
            Message::IcedEvent(Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            })) if self.show_control_center || self.show_power_menu || self.show_calendar || self.show_clipboard_modal || self.show_notification_center || self.show_desktop_context_hub || self.dock_context_menu.is_some() => {
                self.show_control_center = false;
                self.show_power_menu = false;
                self.show_calendar = false;
                self.show_clipboard_modal = false;
                self.show_notification_center = false;
                self.show_desktop_context_hub = false;
                self.dock_context_menu = None;
                self.pending_power_action = None;
                return self.layer_config_task();
            }
            Message::IcedEvent(_) | Message::LayerAction(_) | Message::NoOp => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        let popover_alpha = (0.72 + 0.28 * self.popover_transition.spring()).clamp(0.0, 1.0);

        let render_vu_meter = |left: f32, right: f32, pal: Palette| -> Element<'_, Self::Message> {
            let bars = [" ", " ", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
            let l_idx = ((left.clamp(0.0, 1.0) * 8.0).round() as usize).min(8);
            let r_idx = ((right.clamp(0.0, 1.0) * 8.0).round() as usize).min(8);
            let l_char = bars[l_idx];
            let r_char = bars[r_idx];

            let is_peak = left > 0.82 || right > 0.82;
            let color = if is_peak {
                pal.warning.to_iced()
            } else {
                pal.primary.to_iced()
            };

            container(
                row![
                    text("VU").size(9.0).font(iced::Font::MONOSPACE).color(pal.text_muted.to_iced()),
                    text(format!("{}{}", l_char, r_char)).size(11.0).font(iced::Font::MONOSPACE).color(color),
                ]
                .spacing(3)
                .align_y(Alignment::Center),
            )
            .padding([2, 5])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.24))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(4.0),
                },
                ..Default::default()
            })
            .into()
        };

        // ==================== SECTION 1: LAUNCHER & WORKSPACES ====================
        // ==================== SECTION 1: LAUNCHER & WORKSPACES ====================
        // Corten Independent Start Orb (Floating Start Emblem Button)
        let start_orb = button(
            container(
                image(image::Handle::from_bytes(LOGO_PNG.to_vec()))
                    .width(Length::Fixed(36.0))
                    .height(Length::Fixed(36.0)),
            )
            .width(Length::Fixed(64.0))
            .height(Length::Fixed(64.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.13, 0.94,
                ))),
                border: iced::Border {
                    color: palette.primary.to_iced(),
                    width: 1.5,
                    radius: iced::border::Radius::from(32.0),
                },
                ..Default::default()
            }),
        )
        .width(Length::Fixed(64.0))
        .height(Length::Fixed(64.0))
        .padding(0)
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.20,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(32.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::OpenLauncher);

        // Workspaces Module: [<] [Active Workspace] [ 1 ] [ 2 ] matching concept
        let t = self.language.t();
        let prev_ws_btn = button(text("<").size(12.0).color(palette.text_muted.to_iced()))
            .padding([2, 5])
            .style(|_, _| button::Style::default())
            .on_press(Message::SwitchWorkspace(if self.active_workspace > 1 {
                self.active_workspace - 1
            } else {
                1
            }));

        let (current_icon, current_name) = match self.active_workspace {
            1 => (Icon::CODE, t.ws_code),
            2 => (Icon::GLOBE, t.ws_web),
            3 => (Icon::APP_WINDOW, t.ws_chat),
            4 => (Icon::MUSIC, t.ws_media),
            5 => (Icon::SETTINGS, "5"),
            _ => (Icon::MONITOR, "ws"),
        };

        let active_ws_label = container(
            row![
                text(current_icon).font(LUCIDE_FONT).size(13.0).color(palette.primary.to_iced()),
                text(format!("{}: {}", self.active_workspace, current_name))
                    .size(11.0)
                    .color(palette.primary.to_iced()),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding([4, 8])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                palette.primary.r,
                palette.primary.g,
                palette.primary.b,
                0.12,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.25,
                ),
                width: 1.0,
                radius: iced::border::Radius::from(8.0),
            },
            ..Default::default()
        });

        let mut visible_workspaces: Vec<usize> = vec![1, 2];
        if !visible_workspaces.contains(&self.active_workspace) {
            visible_workspaces.push(self.active_workspace);
        }
        for &occ in &self.occupied_workspaces {
            let occ_usize = occ as usize;
            if (1..=9).contains(&occ_usize) && !visible_workspaces.contains(&occ_usize) {
                visible_workspaces.push(occ_usize);
            }
        }
        visible_workspaces.sort_unstable();

        let mut spaces_row = row![prev_ws_btn, active_ws_label]
            .spacing(6)
            .align_y(Alignment::Center);

        for ws_num in visible_workspaces {
            let is_current = self.active_workspace == ws_num;
            let is_occupied = self.occupied_workspaces.contains(&(ws_num as u32));

            let ws_icon = match ws_num {
                1 => Icon::CODE,
                2 => Icon::GLOBE,
                3 => Icon::APP_WINDOW,
                4 => Icon::MUSIC,
                5 => Icon::SETTINGS,
                _ => Icon::MONITOR,
            };

            let ws_content = row![
                text(ws_icon).font(LUCIDE_FONT).size(11.0).color(if is_current {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
                text(format!("{}", ws_num)).size(11.0).font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                }).color(if is_current {
                    palette.text_primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
            ]
            .spacing(3)
            .align_y(Alignment::Center);

            let ws_btn = button(
                container(ws_content)
                    .padding([2, 6])
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
            )
            .padding(0)
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(if is_current {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.16)
                } else if is_occupied {
                    iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.12,
                    )
                } else {
                    iced::Color::TRANSPARENT
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::SwitchWorkspace(ws_num));
            spaces_row = spaces_row.push(ws_btn);
        }

        let _left_section: Element<'_, Self::Message> = spaces_row.into();

        // ==================== SECTION 2: OPEN APPS WITH ACTIVE BAR & RUNNING DOTS ====================
        let mut apps_row = row![].spacing(10).align_y(Alignment::Center);

        for (idx, app_name) in self.config.pinned_apps.iter().enumerate() {
            let app = match app_name.as_str() {
                "terminal" | "corten-terminal" => ActiveApp::Terminal,
                "files" | "corten-files" => ActiveApp::Files,
                "settings" | "corten-settings" => ActiveApp::Settings,
                "editor" => ActiveApp::Editor,
                "browser" => ActiveApp::Browser,
                "corten-ai" | "ai" => ActiveApp::Ai,
                _ => ActiveApp::Terminal,
            };

            let proc_running = match app {
                ActiveApp::Terminal => self.running_apps.0,
                ActiveApp::Files => self.running_apps.1,
                ActiveApp::Browser => self.running_apps.2,
                ActiveApp::Editor => self.running_apps.3,
                ActiveApp::Settings => false,
                ActiveApp::Ai => false,
            };

            let win_count = self
                .compositor_windows
                .iter()
                .filter(|(_, aid, title, _, _, _)| is_matching_window(aid, title, app))
                .count();
            let is_running = win_count > 0 || proc_running;
            let is_active = self
                .compositor_windows
                .iter()
                .any(|(_, aid, title, _, act, _)| is_matching_window(aid, title, app) && *act)
                || (app == self.active_app && is_running);

            let is_hovered = self.hovered_dock_item.as_deref() == Some(app_name.as_str());

            let real_icon = resolve_system_icon(app_name);
            let icon_size = if is_hovered { 38.0 } else { 34.0 };
            let icon_view: Element<'_, Self::Message> = if let Some(ref icon_path) = real_icon {
                iced::widget::image(image::Handle::from_path(icon_path))
                    .width(Length::Fixed(icon_size))
                    .height(Length::Fixed(icon_size))
                    .into()
            } else {
                let icon_char = match app {
                    ActiveApp::Terminal => Icon::TERMINAL,
                    ActiveApp::Files => Icon::FOLDER,
                    ActiveApp::Settings => Icon::SETTINGS,
                    ActiveApp::Editor => Icon::CODE,
                    ActiveApp::Browser => Icon::GLOBE,
                    ActiveApp::Ai => Icon::CPU,
                };
                text(icon_char)
                    .font(LUCIDE_FONT)
                    .size(if is_hovered { 32.0 } else { 28.0 })
                    .color(if is_active {
                        palette.primary.to_iced()
                    } else if is_running {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.95)
                    } else {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.50)
                    })
                    .into()
            };

            let icon_container = container(icon_view)
                .width(Length::Fixed(50.0))
                .height(Length::Fixed(44.0))
                .center_x(Length::Fill)
                .center_y(Length::Fill);

            let elevated_icon = iced::widget::column![
                vertical_space().height(if is_hovered { 0.0 } else { 6.0 }),
                icon_container,
                vertical_space().height(if is_hovered { 6.0 } else { 0.0 }),
            ]
            .align_x(Alignment::Center);

            // Running indicator: Glowing bar if active, 2 dots if multiple windows, 1 dot if 1 window
            let indicator: Element<'_, Self::Message> = if is_active {
                container(row![])
                    .width(Length::Fixed(24.0))
                    .height(Length::Fixed(4.0))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.primary.to_iced())),
                        border: iced::Border {
                            radius: iced::border::Radius::from(2.0),
                            ..Default::default()
                        },
                        shadow: iced::Shadow {
                            color: iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.70,
                            ),
                            offset: iced::Vector::new(0.0, 0.0),
                            blur_radius: 6.0,
                        },
                        ..Default::default()
                    })
                    .into()
            } else if win_count >= 2 {
                let make_dot = || {
                    container(row![])
                        .width(Length::Fixed(4.5))
                        .height(Length::Fixed(4.5))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
                            },
                            shadow: iced::Shadow {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.65),
                                offset: iced::Vector::new(0.0, 0.0),
                                blur_radius: 4.0,
                            },
                            ..Default::default()
                        })
                };
                row![make_dot(), make_dot()]
                    .spacing(3)
                    .align_y(Alignment::Center)
                    .into()
            } else if is_running {
                container(row![])
                    .width(Length::Fixed(5.0))
                    .height(Length::Fixed(5.0))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.primary.to_iced())),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                        shadow: iced::Shadow {
                            color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.60),
                            offset: iced::Vector::new(0.0, 0.0),
                            blur_radius: 4.0,
                        },
                        ..Default::default()
                    })
                    .into()
            } else {
                container(row![])
                    .width(Length::Fixed(4.0))
                    .height(Length::Fixed(4.0))
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                        ..Default::default()
                    })
                    .into()
            };

            let pinned_on_press = if win_count >= 2 {
                Message::ToggleWindowPreviews(app_name.clone())
            } else {
                Message::SelectApp(app)
            };

            let app_item = button(
                iced::widget::column![elevated_icon, indicator]
                    .spacing(2)
                    .align_x(Alignment::Center),
            )
            .padding(0)
            .style(move |_, status| match status {
                button::Status::Pressed => button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(10.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                    border: iced::Border {
                        width: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            })
            .on_press(pinned_on_press);

            let app_hover_name = app_name.clone();
            let app_area = mouse_area(app_item)
                .on_right_press(Message::OpenDockContextMenu(DockMenuTarget::Pinned(app, idx)))
                .on_enter(Message::DockHover(Some(app_hover_name)))
                .on_exit(Message::DockHover(None))
                .on_middle_press(if idx + 1 < self.config.pinned_apps.len() {
                    Message::MoveDockItemRight(idx)
                } else {
                    Message::MoveDockItemLeft(idx)
                });
            apps_row = apps_row.push(app_area);
        }

        // Dynamic unpinned running apps from compositor
        let mut dynamic_seen = std::collections::HashSet::new();
        for (win_id, app_id, _title, _ws, is_active, _) in &self.compositor_windows {
            let effective_id = if !app_id.trim().is_empty() {
                app_id.as_str()
            } else if !_title.trim().is_empty() {
                _title.as_str()
            } else {
                continue;
            };

            let is_pinned = self.config.pinned_apps.iter().any(|pin| is_matching_pinned(effective_id, pin));

            if !is_pinned && dynamic_seen.insert(effective_id.to_string()) {
                let win_id_copy = *win_id;
                let is_act = *is_active;
                let dyn_hover_id = effective_id.to_string();
                let is_hovered = self.hovered_dock_item.as_deref() == Some(dyn_hover_id.as_str());

                let real_icon = resolve_system_icon(effective_id);
                let app_win_count = self
                    .compositor_windows
                    .iter()
                    .filter(|(_, aid, _, _, _, _)| aid == effective_id)
                    .count();

                let icon_size = if is_hovered { 38.0 } else { 34.0 };
                let icon_view: Element<'_, Self::Message> = if let Some(ref icon_path) = real_icon {
                    iced::widget::image(image::Handle::from_path(icon_path))
                        .width(Length::Fixed(icon_size))
                        .height(Length::Fixed(icon_size))
                        .into()
                } else {
                    let icon_char = app_icon_char(effective_id);
                    text(icon_char)
                        .font(LUCIDE_FONT)
                        .size(if is_hovered { 32.0 } else { 28.0 })
                        .color(if is_act {
                            palette.primary.to_iced()
                        } else {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.85)
                        })
                        .into()
                };

                let icon_container = container(icon_view)
                    .width(Length::Fixed(50.0))
                    .height(Length::Fixed(44.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill);

                let elevated_icon = iced::widget::column![
                    vertical_space().height(if is_hovered { 0.0 } else { 6.0 }),
                    icon_container,
                    vertical_space().height(if is_hovered { 6.0 } else { 0.0 }),
                ]
                .align_x(Alignment::Center);

                let indicator: Element<'_, Self::Message> = if is_act {
                    container(row![])
                        .width(Length::Fixed(24.0))
                        .height(Length::Fixed(4.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(2.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .into()
                } else if app_win_count >= 2 {
                    let make_dot = || {
                        container(row![])
                            .width(Length::Fixed(4.5))
                            .height(Length::Fixed(4.5))
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(palette.primary.to_iced())),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                shadow: iced::Shadow {
                                    color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.65),
                                    offset: iced::Vector::new(0.0, 0.0),
                                    blur_radius: 4.0,
                                },
                                ..Default::default()
                            })
                    };
                    row![make_dot(), make_dot()]
                        .spacing(3)
                        .align_y(Alignment::Center)
                        .into()
                } else {
                    container(row![])
                        .width(Length::Fixed(5.0))
                        .height(Length::Fixed(5.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
                            },
                            shadow: iced::Shadow {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.60),
                                offset: iced::Vector::new(0.0, 0.0),
                                blur_radius: 4.0,
                            },
                            ..Default::default()
                        })
                        .into()
                };

                let dyn_on_press = if app_win_count >= 2 {
                    Message::ToggleWindowPreviews(app_id.clone())
                } else {
                    Message::FocusWindow(win_id_copy)
                };

                let dyn_btn = button(
                    iced::widget::column![elevated_icon, indicator]
                        .spacing(2)
                        .align_x(Alignment::Center),
                )
                .padding(0)
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Pressed => Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10))),
                        _ => Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                    };
                    button::Style {
                        background: bg,
                        border: iced::Border {
                            width: 0.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(dyn_on_press);

                let dyn_area = mouse_area(dyn_btn)
                    .on_right_press(Message::OpenDockContextMenu(DockMenuTarget::Dynamic {
                        win_id: win_id_copy,
                        app_id: app_id.clone(),
                        title: String::new(),
                    }))
                    .on_enter(Message::DockHover(Some(dyn_hover_id)))
                    .on_exit(Message::DockHover(None));
                apps_row = apps_row.push(dyn_area);
            }
        }

        // All Applications Launchpad / Dashboard Grid Button
        let all_apps_icon = container(
            text(Icon::APP_WINDOW)
                .font(LUCIDE_FONT)
                .size(24.0)
                .color(palette.primary.to_iced()),
        )
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(46.0))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                palette.primary.r,
                palette.primary.g,
                palette.primary.b,
                0.12,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.35,
                ),
                width: 1.0,
                radius: iced::border::Radius::from(12.0),
            },
            ..Default::default()
        });

        let all_apps_btn = button(
            iced::widget::column![all_apps_icon]
                .align_x(Alignment::Center),
        )
        .padding(0)
        .style(|_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.14,
                ))),
                button::Status::Pressed => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.22,
                ))),
                _ => Some(iced::Background::Color(iced::Color::TRANSPARENT)),
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(12.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::OpenDashboard);

        apps_row = apps_row.push(all_apps_btn);

        // ==================== VERTICAL DIVIDER 2 ====================
        let divider2 = container(row![])
            .width(Length::Fixed(1.0))
            .height(Length::Fixed(40.0))
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.12,
                ))),
                ..Default::default()
            });

        // ==================== SECTION 3: AUDIO, WIFI & TWO-LINE CLOCK ====================
        let current_vol = self.audio_status.map(|(v, _)| v).unwrap_or(50);
        let is_muted = self.audio_status.map(|(_, m)| m).unwrap_or(false);

        let audio_btn = button(text(Icon::VOLUME).font(LUCIDE_FONT).size(26.0).color(
            if is_muted {
                palette.danger.to_iced()
            } else {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.85)
            },
        ))
        .padding([4, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.10,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleAudioSlider);

        // Audio popover or inline volume widget
        let audio_control: Element<'_, Self::Message> = if self.show_volume_slider {
            let vol_slider = slider(0..=100, current_vol, Message::SetVolume)
                .width(Length::Fixed(100.0))
                .step(2)
                .style(corten_theme::iced_styles::capsule_slider_style(palette));
            let mute_btn = button(text(if is_muted { "Sesi Aç" } else { "Sessize Al" }).size(12.0))
                .padding([2, 6])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(palette.primary.to_iced())),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(Message::ToggleAudioMute);

            row![audio_btn, vol_slider, mute_btn]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
        } else {
            audio_btn.into()
        };

        let (is_online, net_name, _) = &self.network_status;
        let wifi_btn = button(
            text(Icon::WIFI)
                .font(LUCIDE_FONT)
                .size(24.0)
                .color(if *is_online {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.85)
                } else {
                    palette.warning.to_iced()
                }),
        )
        .padding([4, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.10,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleWifiDetails);

        let wifi_control: Element<'_, Self::Message> = if self.show_wifi_details {
            let badge = container(
                text(if *is_online {
                    net_name.as_str()
                } else {
                    "Çevrimdışı"
                })
                .size(12.0)
                .color(if *is_online {
                    palette.text_primary.to_iced()
                } else {
                    palette.warning.to_iced()
                }),
            )
            .padding([3, 8])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.2, 0.22, 0.26, 0.85,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            });

            row![wifi_btn, badge]
                .spacing(6)
                .align_y(Alignment::Center)
                .into()
        } else {
            wifi_btn.into()
        };

        let clock_column = button(
            iced::widget::column![
                text(&self.current_time)
                    .size(18.0)
                    .color(palette.text_primary.to_iced()),
                text(&self.current_date)
                    .size(13.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(1)
            .align_x(Alignment::End),
        )
        .padding([2, 6])
        .style(|_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.08,
                ))),
                button::Status::Pressed => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.15,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleCalendar);

        let mut right_row = row![].spacing(14).align_y(Alignment::Center);
        if self.recording_active {
            let rec_pill = container(
                row![
                    text("⏺")
                        .size(13.0)
                        .color(iced::Color::from_rgb(1.0, 0.25, 0.25)),
                    text("REC")
                        .size(11.0)
                        .color(iced::Color::from_rgb(1.0, 0.25, 0.25)),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([3, 8])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 0.15, 0.15, 0.20))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 0.25, 0.25, 0.60),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            });
            let rec_btn = button(rec_pill)
                .padding(0)
                .style(|_, _| button::Style::default())
                .on_press(Message::ToggleRecording);

            right_row = right_row.push(rec_btn);
        }
        if self.mic_active {
            let mic_pill = container(
                row![
                    text(Icon::MIC)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(iced::Color::from_rgb(1.0, 0.65, 0.2)),
                    text("MİK")
                        .size(11.0)
                        .color(iced::Color::from_rgb(1.0, 0.65, 0.2)),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([3, 8])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 0.60, 0.15, 0.20))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 0.65, 0.20, 0.60),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            });
            right_row = right_row.push(mic_pill);
        }
        if self.config.show_audio {
            right_row = right_row.push(audio_control);
            if self.audio_vu_level.0 > 0.05 || self.audio_vu_level.1 > 0.05 {
                right_row = right_row.push(render_vu_meter(self.audio_vu_level.0, self.audio_vu_level.1, palette));
            }
        }
        if self.config.show_network {
            right_row = right_row.push(wifi_control);
        }

        // StatusNotifierItem (SNI) / Background App Tray
        if !self.tray_items.is_empty() {
            let mut tray_row = row![].spacing(6).align_y(Alignment::Center);
            for item in &self.tray_items {
                let id = item.id.clone();
                let id_right = item.id.clone();
                let tray_btn = button(text(&item.icon).size(15.0))
                    .padding([2, 5])
                    .style(|_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                            _ => iced::Color::TRANSPARENT,
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::ClickTrayItem(id));
                let interactive_tray = mouse_area(tray_btn)
                    .on_right_press(Message::RightClickTrayItem(id_right));
                tray_row = tray_row.push(interactive_tray);
            }
            right_row = right_row.push(tray_row);
        }

        // Custom Dynamic Script Modules (Waybar/Polybar JSON Applet Standard)
        for (idx, applet) in self.custom_script_outputs.iter().enumerate() {
            if !applet.text.is_empty() {
                let icon_str = applet.icon.as_deref().unwrap_or("⚙");
                let bg_color = match applet.class.as_deref() {
                    Some("critical" | "alert" | "error") => iced::Color::from_rgba(0.9, 0.2, 0.2, 0.25),
                    Some("warning") => iced::Color::from_rgba(0.9, 0.6, 0.1, 0.25),
                    Some("success") => iced::Color::from_rgba(0.2, 0.8, 0.3, 0.25),
                    Some("info") => iced::Color::from_rgba(0.1, 0.7, 0.9, 0.25),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                };
                let border_color = match applet.class.as_deref() {
                    Some("critical" | "alert" | "error") => iced::Color::from_rgba(0.9, 0.2, 0.2, 0.5),
                    Some("warning") => iced::Color::from_rgba(0.9, 0.6, 0.1, 0.5),
                    Some("success") => iced::Color::from_rgba(0.2, 0.8, 0.3, 0.5),
                    Some("info") => iced::Color::from_rgba(0.1, 0.7, 0.9, 0.5),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                };

                let pill_content = container(
                    row![
                        text(icon_str).size(13.0),
                        text(applet.text.as_str()).size(12.0).color(palette.text_primary.to_iced()),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .padding([3, 8])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    border: iced::Border {
                        color: border_color,
                        width: 1.0,
                        radius: iced::border::Radius::from(8.0),
                    },
                    ..Default::default()
                });

                let click_btn = button(pill_content)
                    .padding(0)
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ClickCustomScript(idx));

                let interactive_applet = mouse_area(click_btn)
                    .on_right_press(Message::RightClickCustomScript(idx));

                let final_applet: Element<'_, Message> = if let Some(ref tt) = applet.tooltip {
                    tooltip(
                        interactive_applet,
                        container(text(tt.as_str()).size(11.0).color(iced::Color::WHITE))
                            .padding([4, 8])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(0.1, 0.1, 0.1, 0.95))),
                                border: iced::Border {
                                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.2),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(6.0),
                                },
                                ..Default::default()
                            }),
                        tooltip::Position::Top,
                    )
                    .into()
                } else {
                    interactive_applet.into()
                };

                right_row = right_row.push(final_applet);
            }
        }

        let downloads_btn = button(
            text(Icon::DOWNLOAD)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(if self.show_downloads_stack {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .padding([3, 5])
        .style(|_, _| button::Style::default())
        .on_press(Message::ToggleDownloadsStack);

        let is_trash_full = self.trash_item_count > 0;
        let mut trash_inner = row![
            text(Icon::TRASH)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.text_muted.to_iced())
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        if is_trash_full {
            trash_inner = trash_inner.push(
                container(
                    text(format!("{}", self.trash_item_count.min(99)))
                        .size(9.0)
                        .color(palette.primary.to_iced()),
                )
                .padding([1, 4])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.20,
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            );
        }
        let trash_btn = button(trash_inner)
            .padding([3, 5])
            .style(|_, _| button::Style::default())
            .on_press(Message::OpenTrash);

        let right_section = row![
            right_row,
            downloads_btn,
            trash_btn,
            clock_column,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // ==================== NOW PLAYING DYNAMIC CAPSULE ====================
        let now_playing_capsule: Option<Element<'_, Self::Message>> = {
            let active_player = self
                .media_players
                .iter()
                .find(|p| p.playback_status == corten_services::mpris::PlaybackStatus::Playing)
                .or_else(|| self.media_players.first());
            if let Some(player) = active_player {
                if !player.title.is_empty() {
                    let bus_name = player.bus_name.clone();
                    let title_display = if player.title.chars().count() > 18 {
                        format!("{}...", player.title.chars().take(16).collect::<String>())
                    } else {
                        player.title.clone()
                    };
                    let is_playing = player.playback_status == corten_services::mpris::PlaybackStatus::Playing;
                    let wave = if is_playing { "ılılı" } else { "---" };

                    let cap = button(
                        row![
                            text(wave).size(11.0).color(palette.primary.to_iced()),
                            text(title_display)
                                .size(11.0)
                                .color(palette.text_primary.to_iced()),
                            button(
                                text(if is_playing { "⏸" } else { "▶" })
                                    .size(11.0)
                                    .color(palette.primary.to_iced()),
                            )
                            .padding([1, 4])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::MprisPlayPause(bus_name.clone())),
                        ]
                        .spacing(5)
                        .align_y(Alignment::Center),
                    )
                    .padding([3, 8])
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.20,
                            ),
                            _ => iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.10,
                            ),
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                color: iced::Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.35,
                                ),
                                width: 1.0,
                                radius: iced::border::Radius::from(Radius::FULL),
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::MprisPlayPause(bus_name));

                    Some(cap.into())
                } else {
                    None
                }
            } else {
                None
            }
        };

        // ==================== COMPLETE COMPACT DOCK BAR CONTAINER ====================
        let mut dock_items = row![apps_row]
            .spacing(12)
            .align_y(Alignment::Center);

        if let Some(media_capsule) = now_playing_capsule {
            dock_items = dock_items.push(media_capsule);
        }

        dock_items = dock_items.push(divider2);
        dock_items = dock_items.push(right_section);

        let dock_row = dock_items.padding([6, 20]);

        let dock_capsule = container(dock_row)
            .width(Length::Shrink)
            .height(Length::Fixed(68.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.13, 0.92,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                    width: 1.0,
                    radius: iced::border::Radius::from(28.0),
                },
                ..Default::default()
            });

        let dock_cluster = row![start_orb, dock_capsule]
            .spacing(12)
            .align_y(Alignment::Center);

        // ==================== CONTROL CENTER POPUP CARD ====================
        let content: Element<'_, Self::Message> = if self.show_control_center {
            // Power quick buttons: Lock, Logout, Reboot, Poweroff
            let power_btn = |label: &'static str, action: SystemPowerAction, color: iced::Color| {
                button(
                    container(text(label).size(13.0).color(color))
                        .width(Length::Fill)
                        .height(Length::Fixed(32.0))
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                )
                .padding(0)
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => Some(iced::Background::Color(
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        )),
                        button::Status::Pressed => Some(iced::Background::Color(
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.20),
                        )),
                        _ => Some(iced::Background::Color(iced::Color::from_rgba(
                            1.0, 1.0, 1.0, 0.05,
                        ))),
                    };
                    button::Style {
                        background: bg,
                        border: iced::Border {
                            radius: iced::border::Radius::from(8.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::SystemAction(action))
            };

            let quick_power_row = row![
                power_btn(
                    "Kilitle",
                    SystemPowerAction::Lock,
                    palette.text_primary.to_iced()
                ),
                power_btn(
                    "Logout",
                    SystemPowerAction::Logout,
                    palette.text_primary.to_iced()
                ),
                power_btn(
                    "Yeniden Başlat",
                    SystemPowerAction::Reboot,
                    palette.warning.to_iced()
                ),
                power_btn(
                    "Kapat",
                    SystemPowerAction::Poweroff,
                    palette.danger.to_iced()
                ),
            ]
            .spacing(8)
            .width(Length::Fill);

            // Volume and Screen Brightness sliders inside control center (Fluid macOS/iOS Capsule Sliders)
            let current_vol = self.audio_status.map(|(v, _)| v).unwrap_or(50);
            let cc_slider = slider(0..=100, current_vol, Message::SetVolume)
                .width(Length::Fill)
                .step(1)
                .style(corten_theme::iced_styles::capsule_slider_style(palette));

            let vol_header = row![
                text(Icon::VOLUME)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(palette.primary.to_iced()),
                text(format!("Ses: {}%", current_vol))
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let bri_slider = slider(5..=100, self.brightness_pct, Message::SetBrightness)
                .width(Length::Fill)
                .step(1)
                .style(corten_theme::iced_styles::capsule_slider_style(palette));

            let bri_header = row![
                text(Icon::SUN)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(iced::Color::from_rgb(0.95, 0.75, 0.30)),
                text(format!("Parlaklık: {}%", self.brightness_pct))
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let sliders_box = container(
                iced::widget::column![
                    vol_header,
                    cc_slider,
                    bri_header,
                    bri_slider,
                ]
                .spacing(6)
            )
            .padding([8, 12])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            // Network & Battery status overview row
            let (is_online, net_name, ip_addr) = &self.network_status;
            let net_info = row![
                text(Icon::WIFI)
                    .font(LUCIDE_FONT)
                    .size(14.0)
                    .color(if *is_online {
                        palette.primary.to_iced()
                    } else {
                        palette.warning.to_iced()
                    }),
                text(if *is_online {
                    format!("{} ({})", net_name, ip_addr)
                } else {
                    "Çevrimdışı".into()
                })
                .size(12.0)
                .color(palette.text_muted.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            let battery_info: Element<'_, Self::Message> =
                if let Some((pct, is_charging)) = self.battery_status {
                    row![
                        text(Icon::BATTERY)
                            .font(LUCIDE_FONT)
                            .size(14.0)
                            .color(if pct <= 20 {
                                palette.danger.to_iced()
                            } else {
                                palette.primary.to_iced()
                            }),
                        text(format!("{}%{}", pct, if is_charging { " ⚡" } else { "" }))
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .into()
                } else {
                    row![
                        text(Icon::MONITOR)
                            .font(LUCIDE_FONT)
                            .size(14.0)
                            .color(palette.text_muted.to_iced()),
                        text("Şebeke Bağlı")
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .into()
                };

            let sys_status_row = row![net_info, horizontal_space(), battery_info]
                .width(Length::Fill)
                .align_y(Alignment::Center);

            // Caffeine (Keep-Awake) & Developer Ports
            let caffeine_btn = button(
                row![
                    text(Icon::COFFEE)
                        .font(LUCIDE_FONT)
                        .size(14.0)
                        .color(if self.caffeine_enabled {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    text(if self.caffeine_enabled {
                        "Kafein: Açık"
                    } else {
                        "Kafein: Kapalı"
                    })
                    .size(12.0)
                    .color(if self.caffeine_enabled {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([5, 10])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(if self.caffeine_enabled {
                    iced::Color::from_rgba(0.85, 0.42, 0.26, 0.25)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                })),
                border: iced::Border {
                    color: if self.caffeine_enabled {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.1)
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            })
            .on_press(Message::ToggleCaffeine);

            let ports_str = if self.listening_ports.is_empty() {
                "Yok".to_string()
            } else {
                self.listening_ports
                    .iter()
                    .take(4)
                    .map(|p| format!(":{}", p))
                    .collect::<Vec<_>>()
                    .join(" ")
            };

            let night_light_btn = button(
                row![
                    text(Icon::MOON)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(if self.night_light_enabled {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    text(if self.night_light_enabled {
                        "Gece: Açık"
                    } else {
                        "Gece: Kapalı"
                    })
                    .size(11.0)
                    .color(if self.night_light_enabled {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([4, 8])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(if self.night_light_enabled {
                    iced::Color::from_rgba(0.95, 0.60, 0.20, 0.22)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                })),
                border: iced::Border {
                    color: if self.night_light_enabled {
                        palette.primary.to_iced()
                    } else {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.1)
                    },
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            })
            .on_press(Message::ToggleNightLight);

            let profiles = [
                ("power-saver", "🍃 Tasarruf"),
                ("balanced", "⚖️ Dengeli"),
                ("performance", "🚀 Performans"),
            ];
            let mut power_profile_row = row![
                text("Güç:")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            ].spacing(6).align_y(Alignment::Center);

            for (p_id, p_label) in profiles {
                let is_active = self.active_power_profile == p_id;
                let prof_btn = button(
                    text(p_label)
                        .size(11.0)
                        .color(if is_active {
                            palette.primary.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                )
                .padding([3, 7])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(if is_active {
                        iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.20)
                    } else {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                    })),
                    border: iced::Border {
                        color: if is_active {
                            palette.primary.to_iced()
                        } else {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
                        },
                        width: 1.0,
                        radius: iced::border::Radius::from(6.0),
                    },
                    ..Default::default()
                })
                .on_press(Message::SetPowerProfile(p_id.to_string()));
                power_profile_row = power_profile_row.push(prof_btn);
            }

            let dev_ports_row = row![
                text(Icon::CODE)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                text(format!("Portlar: {}", ports_str))
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
                horizontal_space(),
                night_light_btn,
                caffeine_btn,
            ]
            .spacing(6)
            .width(Length::Fill)
            .align_y(Alignment::Center);

            let privacy_status_row = row![
                text(Icon::SHIELD)
                    .font(LUCIDE_FONT)
                    .size(12.0)
                    .color(palette.primary.to_iced()),
                text("Gizlilik:")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
                text(if self.mic_active { "Mikrofon Aktif" } else { "Boşta" })
                    .size(11.0)
                    .color(if self.mic_active {
                        palette.warning.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                text("•").size(10.0).color(palette.border.to_iced()),
                text(if self.recording_active { "Kayıt Aktif" } else { "Boşta" })
                    .size(11.0)
                    .color(if self.recording_active {
                        palette.danger.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            // Quick Tools Row: Bluetooth, Screenshot (Crop), DND (Do Not Disturb)
            let (bt_powered, bt_name) = &self.bluetooth_status;
            let bt_btn = button(
                row![
                    text("ᛒ").size(13.0).color(if *bt_powered {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                    text(if let Some(dev) = bt_name {
                        dev.as_str()
                    } else if *bt_powered {
                        "BT: Açık"
                    } else {
                        "BT: Kapalı"
                    })
                    .size(11.0)
                    .color(palette.text_primary.to_iced()),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([4, 8])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(if *bt_powered {
                    iced::Color::from_rgba(0.85, 0.42, 0.26, 0.20)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::ToggleBluetooth);

            let screenshot_btn = button(
                row![
                    text(Icon::CAMERA)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.text_primary.to_iced()),
                    text("Kırp")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([4, 8])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.08,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::TakeScreenshot(true));

            let dnd_btn = button(
                row![
                    text(Icon::BELL)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(if self.dnd_enabled {
                            palette.danger.to_iced()
                        } else {
                            palette.text_muted.to_iced()
                        }),
                    text(if self.dnd_enabled {
                        "Rahatsız Etme: Açık"
                    } else {
                        "Rahatsız Etme: Kapalı"
                    })
                    .size(11.0)
                    .color(if self.dnd_enabled {
                        palette.warning.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([4, 8])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(if self.dnd_enabled {
                    iced::Color::from_rgba(0.95, 0.60, 0.20, 0.20)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::ToggleDnd);

            let recording_btn = button(
                row![
                    text(if self.recording_active { "⏹" } else { "⏺" }).size(12.0),
                    text(if self.recording_active {
                        "Kaydı Durdur"
                    } else {
                        "Kayıt"
                    })
                    .size(11.0)
                    .color(if self.recording_active {
                        palette.warning.to_iced()
                    } else {
                        palette.text_primary.to_iced()
                    }),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .padding([4, 8])
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(if self.recording_active {
                    iced::Color::from_rgba(0.9, 0.2, 0.2, 0.22)
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::ToggleRecording);

            let quick_tools_row = row![bt_btn, screenshot_btn, dnd_btn, recording_btn]
                .spacing(8)
                .width(Length::Fill)
                .align_y(Alignment::Center);

            // PipeWire Audio Output Sink Selector
            let mut sinks_col = iced::widget::column![text(t.audio_output_device)
                .size(11.0)
                .color(palette.text_muted.to_iced())]
            .spacing(4);

            for sink in self.audio_sinks.iter().take(3) {
                let is_sel = sink.is_default;
                let sink_id = sink.id;
                let sink_btn = button(
                    row![
                        text(if is_sel { "●" } else { "○" })
                            .size(11.0)
                            .color(if is_sel {
                                palette.primary.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            }),
                        text(sink.name.clone()).size(11.0).color(if is_sel {
                            palette.primary.to_iced()
                        } else {
                            palette.text_primary.to_iced()
                        }),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([4, 6])
                .style(move |_, status| {
                    let bg = if is_sel {
                        iced::Color::from_rgba(0.85, 0.42, 0.26, 0.18)
                    } else if matches!(status, button::Status::Hovered) {
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
                    } else {
                        iced::Color::TRANSPARENT
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(6.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::SelectAudioSink(sink_id));

                sinks_col = sinks_col.push(sink_btn);
            }

            // Wi-Fi Interactive Connect List
            let mut wifi_col = iced::widget::column![row![
                text(t.wifi_available_networks)
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
                horizontal_space(),
                button(text("⟳ Tara").size(10.0).color(palette.primary.to_iced()))
                    .padding([2, 4])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ScanWifi),
            ]
            .width(Length::Fill)
            .align_y(Alignment::Center)]
            .spacing(4);

            if self.wifi_networks.is_empty() {
                wifi_col = wifi_col.push(
                    text("Yakında kablosuz ağ bulunamadı")
                        .size(11.0)
                        .color(palette.text_muted.to_iced()),
                );
            } else {
                for net in self.wifi_networks.iter().take(8) {
                    let ssid = net.ssid.clone();
                    let sig = net.signal;
                    let is_sec = net.secured;
                    let net_btn = button(
                        row![
                            text(Icon::WIFI)
                                .font(LUCIDE_FONT)
                                .size(12.0)
                                .color(palette.primary.to_iced()),
                            text(format!("{} ({}%)", ssid, sig))
                                .size(11.0)
                                .color(palette.text_primary.to_iced()),
                            horizontal_space(),
                            text(if is_sec { "🔒" } else { "🔓" }).size(10.0),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([3, 6])
                    .style(|_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            _ => iced::Color::TRANSPARENT,
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::SelectWifiSsid(ssid));

                    wifi_col = wifi_col.push(net_btn);
                }
            }

            // Optional Interactive Wi-Fi Password Modal
            let wifi_modal_element: Element<'_, Self::Message> = if self.show_wifi_modal {
                let current_ssid = self.selected_wifi_ssid.as_deref().unwrap_or("Network");
                container(
                    iced::widget::column![
                        row![
                            text(format!("'{}' ağına bağlan", current_ssid))
                                .size(12.0)
                                .color(palette.primary.to_iced()),
                            horizontal_space(),
                            button(text("✕").size(10.0).color(palette.text_muted.to_iced()))
                                .padding([1, 4])
                                .style(|_, _| button::Style::default())
                                .on_press(Message::CloseWifiModal),
                        ]
                        .width(Length::Fill)
                        .align_y(Alignment::Center),
                        text_input("Wi-Fi parolasını girin...", &self.wifi_password_input)
                            .on_input(Message::WifiPasswordChanged)
                            .on_submit(Message::ConnectWifiSubmit)
                            .padding([4, 8])
                            .size(11.0)
                            .secure(true),
                        button(
                            container(
                                text("Bağlan")
                                    .size(11.0)
                                    .color(palette.text_primary.to_iced())
                            )
                            .center_x(Length::Fill),
                        )
                        .width(Length::Fill)
                        .padding([4, 8])
                        .style(move |_, _| button::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .on_press(Message::ConnectWifiSubmit),
                    ]
                    .spacing(6),
                )
                .padding(8)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.35,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                        width: 1.0,
                        radius: iced::border::Radius::from(8.0),
                    },
                    ..Default::default()
                })
                .into()
            } else {
                iced::widget::column![].into()
            };

            let media_section: Element<'_, Self::Message> = if let Some(player) = self.media_players.first() {
                let bus = player.bus_name.clone();
                let is_playing = player.playback_status == corten_services::mpris::PlaybackStatus::Playing;
                let track_title = if player.title.is_empty() {
                    "Bilinmeyen Parça"
                } else {
                    &player.title
                };
                let track_artist = if player.artist.is_empty() {
                    player.identity.as_str()
                } else {
                    &player.artist
                };

                let play_icon = if is_playing { "⏸" } else { "▶" };

                container(
                    row![
                        iced::widget::column![
                            text(track_title)
                                .size(13.0)
                                .color(palette.text_primary.to_iced()),
                            text(track_artist)
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(2)
                        .width(Length::Fill),
                        row![
                            button(text("⏮").size(13.0).color(palette.text_primary.to_iced()))
                                .padding([4, 8])
                                .style(|_, status| {
                                    let bg = match status {
                                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                                    };
                                    button::Style {
                                        background: Some(iced::Background::Color(bg)),
                                        border: iced::Border {
                                            radius: iced::border::Radius::from(6.0),
                                            ..Default::default()
                                        },
                                        ..Default::default()
                                    }
                                })
                                .on_press(Message::MprisPrevious(bus.clone())),
                            button(text(play_icon).size(13.0).color(palette.on_primary().to_iced()))
                                .padding([4, 10])
                                .style(move |_, status| {
                                    let bg = match status {
                                        button::Status::Hovered => palette.primary_hover.to_iced(),
                                        _ => palette.primary.to_iced(),
                                    };
                                    button::Style {
                                        background: Some(iced::Background::Color(bg)),
                                        border: iced::Border {
                                            radius: iced::border::Radius::from(6.0),
                                            ..Default::default()
                                        },
                                        ..Default::default()
                                    }
                                })
                                .on_press(Message::MprisPlayPause(bus.clone())),
                            button(text("⏭").size(13.0).color(palette.text_primary.to_iced()))
                                .padding([4, 8])
                                .style(|_, status| {
                                    let bg = match status {
                                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                                    };
                                    button::Style {
                                        background: Some(iced::Background::Color(bg)),
                                        border: iced::Border {
                                            radius: iced::border::Radius::from(6.0),
                                            ..Default::default()
                                        },
                                        ..Default::default()
                                    }
                                })
                                .on_press(Message::MprisNext(bus)),
                        ]
                        .spacing(4)
                        .align_y(Alignment::Center),
                    ]
                    .padding([8, 12])
                    .align_y(Alignment::Center)
                )
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.0, 0.0, 0.0, 0.25,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.1),
                        width: 1.0,
                        radius: iced::border::Radius::from(8.0),
                    },
                    ..Default::default()
                })
                .width(Length::Fill)
                .into()
            } else {
                iced::widget::column![].into()
            };

            let app_streams_col: Element<'_, Self::Message> = if !self.app_streams.is_empty() {
                let mut col = iced::widget::column![
                    text("Uygulama Sesleri")
                        .size(11.0)
                        .color(palette.text_muted.to_iced())
                ]
                .spacing(4);

                for stream in &self.app_streams {
                    let id = stream.id;
                    let is_muted = stream.is_muted;
                    let vol = stream.volume_pct;
                    let app_name = stream.name.clone();

                    let item = row![
                        text(app_name)
                            .size(11.0)
                            .color(palette.text_primary.to_iced())
                            .width(Length::Fixed(80.0)),
                        slider(0..=100, vol, move |v| Message::SetAppStreamVolume(id, v))
                            .width(Length::Fill)
                            .style(corten_theme::iced_styles::capsule_slider_style(palette)),
                        button(text(if is_muted { "✕" } else { "🔉" }).size(11.0).color(palette.text_muted.to_iced()))
                            .padding([2, 5])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::ToggleAppStreamMute(id)),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center);

                    col = col.push(item);
                }

                container(col)
                    .padding([6, 10])
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.20))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            width: 1.0,
                            radius: iced::border::Radius::from(6.0),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill)
                    .into()
            } else {
                iced::widget::column![].into()
            };

            let patina_chip = |variant: PatinaVariant, label: &'static str, is_active: bool, pal: Palette| {
                let (v_color, _) = variant.colors();
                let border_color = if is_active {
                    v_color.to_iced()
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
                };
                let bg_color = if is_active {
                    iced::Color::from_rgba(v_color.r, v_color.g, v_color.b, 0.24)
                } else {
                    iced::Color::from_rgba(0.0, 0.0, 0.0, 0.18)
                };
                button(
                    row![
                        container(text("")).width(Length::Fixed(8.0)).height(Length::Fixed(8.0)).style(move |_| container::Style {
                            background: Some(iced::Background::Color(v_color.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(4.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                        text(label).size(11.0).color(if is_active { pal.text_primary.to_iced() } else { pal.text_muted.to_iced() }),
                    ]
                    .spacing(5)
                    .align_y(Alignment::Center)
                )
                .padding([4, 8])
                .style(move |_, _| button::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    border: iced::Border {
                        color: border_color,
                        width: if is_active { 1.5 } else { 1.0 },
                        radius: iced::border::Radius::from(6.0),
                    },
                    ..Default::default()
                })
                .on_press(Message::SetPatina(variant))
            };

            let patina_chips = row![
                patina_chip(PatinaVariant::Rust, "Pas", self.active_patina == PatinaVariant::Rust, palette),
                patina_chip(PatinaVariant::Verdigris, "Zümrüt", self.active_patina == PatinaVariant::Verdigris, palette),
                patina_chip(PatinaVariant::Titanium, "Titanyum", self.active_patina == PatinaVariant::Titanium, palette),
                patina_chip(PatinaVariant::Molten, "Akkor", self.active_patina == PatinaVariant::Molten, palette),
            ]
            .spacing(5)
            .width(Length::Fill);

            let patina_box = container(
                iced::widget::column![
                    row![
                        text("Corten Patina & Renk Teması")
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                        horizontal_space(),
                        text(self.active_patina.name())
                            .size(10.0)
                            .color(palette.primary.to_iced()),
                    ]
                    .align_y(Alignment::Center),
                    patina_chips,
                ]
                .spacing(6)
            )
            .padding([8, 10])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.22))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            let cc_card = container(
                iced::widget::column![
                    row![
                        text(t.control_center_title)
                            .size(15.0)
                            .color(palette.text_primary.to_iced()),
                        button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                            .padding([2, 6])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::ToggleControlCenter),
                    ]
                    .width(Length::Fill)
                    .align_y(Alignment::Center),
                    media_section,
                    sliders_box,
                    sinks_col,
                    app_streams_col,
                    sys_status_row,
                    privacy_status_row,
                    wifi_col,
                    wifi_modal_element,
                    quick_tools_row,
                    patina_box,
                    power_profile_row,
                    dev_ports_row,
                    quick_power_row,
                ]
                .spacing(9)
                .width(Length::Fill)
                .max_width(390.0),
            )
            .padding([14, 18])
            .style(move |_| {
                let mut style = acrylic_glass_style(palette, 18.0);
                if let Some(iced::Background::Color(color)) = style.background {
                    style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                        color.r,
                        color.g,
                        color.b,
                        color.a * popover_alpha,
                    )));
                }
                style
            });

            iced::widget::column![cc_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_power_menu {
            let t = self.language.t();
            // Elevated Corten Güç & Oturum Merkezi (Power & Session Hub)
            let action_card = |icon_char: char, label: &'static str, desc: &'static str, action: SystemPowerAction, is_danger: bool| {
                button(
                    row![
                        container(
                            text(icon_char)
                                .font(LUCIDE_FONT)
                                .size(16.0)
                                .color(if is_danger { palette.danger.to_iced() } else { palette.primary.to_iced() })
                        )
                        .padding([6, 8])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(if is_danger {
                                iced::Color::from_rgba(palette.danger.r, palette.danger.g, palette.danger.b, 0.15)
                            } else {
                                iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15)
                            })),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                        column![
                            text(label).size(13.0).color(palette.text_primary.to_iced()),
                            text(desc).size(10.0).color(palette.text_muted.to_iced()),
                        ]
                        .spacing(1),
                        horizontal_space(),
                        text("›").size(14.0).color(palette.text_disabled.to_iced()),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 8])
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.02),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(8.0),
                            width: 1.0,
                            color: if is_danger {
                                iced::Color::from_rgba(palette.danger.r, palette.danger.g, palette.danger.b, 0.3)
                            } else {
                                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                            },
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::SystemAction(action))
            };

            let power_header = row![
                text(format!("⚡ {}", t.power_hub_title))
                    .size(14.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                container(
                    text("Super+X").size(10.0).color(palette.primary.to_iced())
                )
                .padding([2, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.15))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(4.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::TogglePowerMenu),
            ]
            .width(Length::Fill)
            .align_y(Alignment::Center);

            let power_body = if let Some(action) = self.pending_power_action {
                let (action_title, action_desc) = if action == SystemPowerAction::Reboot {
                    (t.power_confirm_reboot, t.power_confirm_reboot_desc)
                } else {
                    (t.power_confirm_poweroff, t.power_confirm_poweroff_desc)
                };
                iced::widget::column![
                    container(
                        iced::widget::column![
                            text(action_title)
                                .size(14.0)
                                .color(palette.danger.to_iced()),
                            text(action_desc)
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(4)
                    )
                    .padding([8, 12])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(palette.danger.r, palette.danger.g, palette.danger.b, 0.1))),
                        border: iced::Border {
                            color: palette.danger.to_iced(),
                            width: 1.0,
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    }),
                    row![
                        button(text(t.btn_cancel).size(12.0))
                            .padding([8, 14])
                            .on_press(Message::CancelSystemAction)
                            .width(Length::FillPortion(1)),
                        button(text(t.btn_confirm).size(12.0).color(palette.on_primary().to_iced()))
                            .padding([8, 14])
                            .style(move |_, _| button::Style {
                                background: Some(iced::Background::Color(palette.danger.to_iced())),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(6.0),
                                    ..Default::default()
                                },
                                ..Default::default()
                            })
                            .on_press(Message::ConfirmSystemAction)
                            .width(Length::FillPortion(1)),
                    ]
                    .spacing(8)
                ]
                .spacing(10)
            } else {
                iced::widget::column![
                    action_card(Icon::LOCK, t.power_lock, t.power_lock_desc, SystemPowerAction::Lock, false),
                    action_card(Icon::MOON, t.power_suspend, t.power_suspend_desc, SystemPowerAction::Suspend, false),
                    action_card(Icon::USER, t.power_logout, t.power_logout_desc, SystemPowerAction::Logout, false),
                    action_card(Icon::CODE, t.power_reboot, t.power_reboot_desc, SystemPowerAction::Reboot, false),
                    action_card(Icon::POWER, t.power_poweroff, t.power_poweroff_desc, SystemPowerAction::Poweroff, true),
                ]
                .spacing(5)
            };

            let power_card = container(
                iced::widget::column![power_header, power_body]
                    .spacing(10)
                    .width(Length::Fill),
            )
            .padding([14, 16])
            .width(Length::Fixed(340.0))
            .style(move |_| {
                let mut style = acrylic_glass_style(palette, 16.0);
                if let Some(iced::Background::Color(color)) = style.background {
                    style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                        color.r,
                        color.g,
                        color.b,
                        color.a * popover_alpha,
                    )));
                }
                style
            });

            iced::widget::column![power_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_calendar {
            let t = self.language.t();
            // Corten Takvim & Hava Durumu Modülü
            let header = row![
                text(format!("📅 {}", t.weather_calendar_title))
                    .size(14.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleCalendar),
            ]
            .width(Length::Fill)
            .align_y(Alignment::Center);

            // Weather card
            let temp_display = format!("{}{}\u{00B0}C", if self.weather_report.temp_c > 0 { "+" } else { "" }, self.weather_report.temp_c);
            let weather_card = container(
                column![
                    row![
                        column![
                            row![
                                text(&self.weather_report.city).size(13.0).color(palette.text_muted.to_iced()),
                                text("•").size(10.0).color(palette.text_disabled.to_iced()),
                                text(&self.weather_report.condition).size(12.0).color(palette.text_primary.to_iced()),
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                            row![
                                text(&self.weather_report.icon).size(26.0),
                                text(temp_display).size(24.0).color(palette.primary.to_iced()),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        ]
                        .spacing(2),
                        horizontal_space(),
                        column![
                            text(format!("💧 {}: %{}", t.weather_humidity, self.weather_report.humidity)).size(11.0).color(palette.text_muted.to_iced()),
                            text(format!("💨 {}: {} km/s", t.weather_wind, self.weather_report.wind_kmh)).size(11.0).color(palette.text_muted.to_iced()),
                        ]
                        .spacing(4),
                    ]
                    .align_y(Alignment::Center),
                    // 3-day forecast
                    row(self.weather_report.forecast.iter().map(|f| {
                        container(
                            row![
                                text(&f.day_name).size(10.0).color(palette.text_muted.to_iced()),
                                text(&f.icon).size(12.0),
                                text(format!("{}°", f.temp_max)).size(11.0).color(palette.text_primary.to_iced()),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center)
                        )
                        .padding([3, 6])
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(4.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .into()
                    }))
                    .spacing(6),
                ]
                .spacing(8),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            // Date & Calendar Grid
            let now = chrono::Local::now();
            use chrono::Datelike;
            let current_day = now.day();
            let current_month = now.month();
            let current_year = now.year();
            let month_name = match current_month {
                1 => "Ocak",
                2 => "Şubat",
                3 => "Mart",
                4 => "Nisan",
                5 => "Mayıs",
                6 => "Haziran",
                7 => "Temmuz",
                8 => "Ağustos",
                9 => "Eylül",
                10 => "Ekim",
                11 => "Kasım",
                _ => "Aralık",
            };
            let month_title = format!("{} {}", month_name, current_year);

            let weekdays = ["Pt", "Sa", "Ça", "Pe", "Cu", "Ct", "Pz"];
            let weekday_row = row(weekdays.iter().map(|&w| {
                container(
                    text(w).size(11.0).color(palette.text_muted.to_iced())
                )
                .width(Length::Fixed(40.0))
                .align_x(Alignment::Center)
                .into()
            }))
            .spacing(4);

            // Compute first day weekday offset and days in month
            let first_day_date = chrono::NaiveDate::from_ymd_opt(current_year, current_month, 1).unwrap_or(now.date_naive());
            let first_weekday_offset = first_day_date.weekday().num_days_from_monday();
            let days_in_month = if current_month == 12 {
                31
            } else {
                chrono::NaiveDate::from_ymd_opt(current_year, current_month + 1, 1)
                    .unwrap_or(first_day_date)
                    .signed_duration_since(first_day_date)
                    .num_days() as u32
            };

            let mut cal_col = iced::widget::column![].spacing(4);
            let mut current_cal_row = row![].spacing(4);
            let mut day_cell_idx = 0;

            // Empty cells before day 1
            for _ in 0..first_weekday_offset {
                current_cal_row = current_cal_row.push(
                    container(text("").size(11.0))
                        .width(Length::Fixed(40.0))
                        .height(Length::Fixed(24.0))
                );
                day_cell_idx += 1;
            }

            // Days 1..=days_in_month
            for d in 1..=days_in_month {
                let is_today = d == current_day;
                let day_chip = container(
                    text(format!("{}", d))
                        .size(11.0)
                        .color(if is_today { palette.on_primary().to_iced() } else { palette.text_primary.to_iced() })
                )
                .width(Length::Fixed(40.0))
                .height(Length::Fixed(24.0))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .style(move |_| container::Style {
                    background: if is_today {
                        Some(iced::Background::Color(palette.primary.to_iced()))
                    } else {
                        None
                    },
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                });

                current_cal_row = current_cal_row.push(day_chip);
                day_cell_idx += 1;

                if day_cell_idx % 7 == 0 || d == days_in_month {
                    cal_col = cal_col.push(current_cal_row);
                    current_cal_row = row![].spacing(4);
                }
            }

            let cal_card = container(
                column![
                    row![
                        text(month_title).size(13.0).color(palette.primary.to_iced()),
                        horizontal_space(),
                        text(now.format("%H:%M").to_string()).size(13.0).color(palette.text_muted.to_iced()),
                    ]
                    .align_y(Alignment::Center),
                    weekday_row,
                    cal_col,
                ]
                .spacing(6),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.02))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            let calendar_card = container(
                iced::widget::column![header, weather_card, cal_card]
                    .spacing(10)
                    .width(Length::Fill),
            )
            .padding([14, 16])
            .width(Length::Fixed(350.0))
            .style(move |_| {
                let mut style = acrylic_glass_style(palette, 16.0);
                if let Some(iced::Background::Color(color)) = style.background {
                    style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                        color.r,
                        color.g,
                        color.b,
                        color.a * popover_alpha,
                    )));
                }
                style
            });

            let inner_card = mouse_area(calendar_card).on_press(Message::NoOp);
            let content_col = iced::widget::column![inner_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center);

            mouse_area(content_col)
                .on_press(Message::ToggleCalendar)
                .into()
        } else if self.show_clipboard_modal {
            let title_row = row![
                text("Pano Geçmişi (Super+V)")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("Tümünü Temizle").size(11.0).color(palette.danger.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ClearClipboardHistory),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleClipboardModal),
            ]
            .align_y(Alignment::Center);

            let search_input = text_input("Panoda ara...", &self.clipboard_search)
                .on_input(Message::ClipboardSearchChanged)
                .padding([6, 10])
                .size(12.0);

            let mut items_col = iced::widget::column![].spacing(6);
            let filtered_items: Vec<(usize, &String)> = self
                .clipboard_history
                .iter()
                .enumerate()
                .filter(|(_, text)| {
                    self.clipboard_search.is_empty()
                        || text.to_lowercase().contains(&self.clipboard_search.to_lowercase())
                })
                .collect();

            if filtered_items.is_empty() {
                items_col = items_col.push(
                    container(text("Pano geçmişi boş veya aramayla eşleşmedi.").size(12.0).color(palette.text_muted.to_iced()))
                        .padding([16, 10])
                        .center_x(Length::Fill),
                );
            } else {
                for (idx, text_entry) in filtered_items {
                    let display_text = if text_entry.chars().count() > 50 {
                        format!("{}...", text_entry.chars().take(50).collect::<String>())
                    } else {
                        text_entry.clone()
                    };
                    let copy_val = text_entry.clone();
                    let item_row = row![
                        button(
                            text(display_text)
                                .size(12.0)
                                .color(palette.text_primary.to_iced())
                        )
                        .width(Length::Fill)
                        .padding([5, 8])
                        .style(|_, status| {
                            let bg = match status {
                                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(6.0),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::CopyClipboardItem(copy_val)),
                        button(text("✕").size(11.0).color(palette.text_muted.to_iced()))
                            .padding([4, 6])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::DeleteClipboardItem(idx)),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center);

                    items_col = items_col.push(item_row);
                }
            }

            let clipboard_scroll = scrollable(items_col).height(Length::Fixed(240.0));
            let clipboard_card = container(
                iced::widget::column![title_row, search_input, clipboard_scroll]
                    .spacing(10)
            )
            .width(Length::Fixed(380.0))
            .padding([16, 20])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.09, 0.10, 0.12, 0.94))),
                border: iced::Border {
                    color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.45),
                    width: 1.5,
                    radius: iced::border::Radius::from(16.0),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });

            iced::widget::column![clipboard_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_notification_center {
            let title_row = row![
                text("Bildirim Merkezi")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("Tümünü Temizle").size(11.0).color(palette.danger.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ClearNotifications),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleNotificationCenter),
            ]
            .align_y(Alignment::Center);

            let mut notes_col = iced::widget::column![].spacing(8);
            if self.notification_history.is_empty() {
                notes_col = notes_col.push(
                    container(text("Yeni bildirim yok.").size(12.0).color(palette.text_muted.to_iced()))
                        .padding([24, 10])
                        .center_x(Length::Fill),
                );
            } else {
                for (title, body, time_str) in &self.notification_history {
                    let note_item = container(
                        iced::widget::column![
                            row![
                                text(title).size(12.0).color(palette.primary.to_iced()),
                                horizontal_space(),
                                text(time_str).size(10.0).color(palette.text_muted.to_iced()),
                            ]
                            .align_y(Alignment::Center),
                            text(body).size(11.0).color(palette.text_primary.to_iced()),
                        ]
                        .spacing(2)
                    )
                    .padding([8, 12])
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                            width: 1.0,
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill);

                    notes_col = notes_col.push(note_item);
                }
            }

            let notif_scroll = scrollable(notes_col).height(Length::Fixed(240.0));
            let notif_card = container(
                iced::widget::column![title_row, notif_scroll].spacing(10)
            )
            .width(Length::Fixed(360.0))
            .padding([16, 20])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.09, 0.10, 0.12, 0.94))),
                border: iced::Border {
                    color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.45),
                    width: 1.5,
                    radius: iced::border::Radius::from(16.0),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });

            iced::widget::column![notif_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_downloads_stack {
            let header = row![
                text("📥 İndirilenler (Downloads)")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("Tümünü Göster").size(11.0).color(palette.primary.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::SelectApp(ActiveApp::Files)),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleDownloadsStack),
            ]
            .align_y(Alignment::Center);

            let mut dl_col = iced::widget::column![].spacing(6);
            if self.downloads_items.is_empty() {
                dl_col = dl_col.push(
                    container(text("İndirilen dosya bulunamadı.").size(12.0).color(palette.text_muted.to_iced()))
                        .padding([20, 10])
                        .center_x(Length::Fill),
                );
            } else {
                for (name, size_str, path) in &self.downloads_items {
                    let path_clone = path.clone();
                    let item = button(
                        row![
                            text("📄").size(14.0),
                            column![
                                text(name.clone()).size(12.0).color(palette.text_primary.to_iced()),
                                text(size_str.clone()).size(10.0).color(palette.text_muted.to_iced()),
                            ]
                            .spacing(1)
                            .width(Length::Fill),
                            text("Aç ↵").size(10.0).color(palette.primary.to_iced()),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([6, 10])
                    .style(|_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03),
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::OpenFilePath(path_clone));

                    dl_col = dl_col.push(item);
                }
            }

            let dl_card = container(
                iced::widget::column![header, dl_col].spacing(10)
            )
            .padding([14, 18])
            .width(Length::Fixed(360.0))
            .style(move |_| {
                let mut style = acrylic_glass_style(palette, 16.0);
                if let Some(iced::Background::Color(color)) = style.background {
                    style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                        color.r, color.g, color.b, color.a * popover_alpha,
                    )));
                }
                style
            });

            iced::widget::column![dl_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if let Some(target) = &self.dock_context_menu {
            let (disp_title, app_id_str, win_id_opt, is_running, cmd_launch, pin_idx_opt) = match target {
                DockMenuTarget::Pinned(app, idx) => {
                    let title = match app {
                        ActiveApp::Terminal => "Terminal",
                        ActiveApp::Files => "Dosya Yöneticisi",
                        ActiveApp::Settings => "Sistem Ayarları",
                        ActiveApp::Browser => "Web Tarayıcısı",
                        ActiveApp::Editor => "Kod Düzenleyici",
                        ActiveApp::Ai => "Corten AI",
                    };
                    let running = match app {
                        ActiveApp::Terminal => self.running_apps.0,
                        ActiveApp::Files => self.running_apps.1,
                        ActiveApp::Settings => false,
                        ActiveApp::Browser => self.running_apps.2,
                        ActiveApp::Editor => self.running_apps.3,
                        ActiveApp::Ai => false,
                    };
                    let cmd = match app {
                        ActiveApp::Terminal => "corten-terminal || alacritty || foot || konsole || xterm".to_string(),
                        ActiveApp::Files => "corten-files || nautilus || dolphin || thunar".to_string(),
                        ActiveApp::Settings => "corten-settings || gnome-control-center || systemsettings".to_string(),
                        ActiveApp::Browser => "firefox || google-chrome || brave || chromium || zen".to_string(),
                        ActiveApp::Editor => "code || gedit || kate || nvim".to_string(),
                        ActiveApp::Ai => "corten-ai".to_string(),
                    };
                    (title.to_string(), format!("{:?}", app).to_lowercase(), None, running, cmd, Some(*idx))
                }
                DockMenuTarget::Dynamic { win_id, app_id, title } => {
                    let dt = if title.is_empty() { app_id.clone() } else { title.clone() };
                    (dt, app_id.clone(), Some(*win_id), true, app_id.clone(), None)
                }
            };

            let header = row![
                text(format!("⚡ {}", disp_title))
                    .size(13.0)
                    .font(iced::Font {
                        weight: iced::font::Weight::Semibold,
                        ..Default::default()
                    })
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::CloseDockContextMenu),
            ]
            .width(Length::Fill)
            .align_y(Alignment::Center);

            let header_sep = container(iced::widget::Space::new(Length::Fill, Length::Fixed(1.0)))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                    ..Default::default()
                });

            let mut menu_items = column![].spacing(4);

            let new_win_btn = button(
                row![
                    text("🚀").size(13.0),
                    text("Yeni Pencere Aç").size(12.0).color(palette.text_primary.to_iced()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6, 10])
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::DockMenuAction(DockAction::LaunchNew(cmd_launch)));
            menu_items = menu_items.push(new_win_btn);

            if is_running {
                let focus_id = win_id_opt.unwrap_or(0);
                let focus_btn = button(
                    row![
                        text("🔍").size(13.0),
                        text("Öne Getir / Odaklan").size(12.0).color(palette.text_primary.to_iced()),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 10])
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(6.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::DockMenuAction(DockAction::Focus(focus_id)));
                menu_items = menu_items.push(focus_btn);

                let close_btn = button(
                    row![
                        text(Icon::X)
                            .font(LUCIDE_FONT)
                            .size(13.0)
                            .color(palette.danger.to_iced()),
                        text("Pencereyi Kapat / Sonlandır").size(12.0).color(palette.danger.to_iced()),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([6, 10])
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 0.2, 0.2, 0.22),
                        _ => iced::Color::from_rgba(1.0, 0.2, 0.2, 0.08),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(6.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::DockMenuAction(DockAction::CloseWindow(focus_id, app_id_str.clone())));
                menu_items = menu_items.push(close_btn);
            }

            if let Some(idx) = pin_idx_opt {
                let can_left = idx > 0;
                let can_right = idx + 1 < self.config.pinned_apps.len();
                let reorder_row = row![
                    button(
                        row![text("◀").size(11.0), text(" Sola").size(11.0).color(palette.text_primary.to_iced())]
                            .align_y(Alignment::Center)
                    )
                    .width(Length::Fill)
                    .padding([5, 8])
                    .style(move |_, status| {
                        let bg = if !can_left {
                            iced::Color::TRANSPARENT
                        } else if matches!(status, button::Status::Hovered) {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                        } else {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border { radius: iced::border::Radius::from(6.0), ..Default::default() },
                            ..Default::default()
                        }
                    })
                    .on_press_maybe(if can_left { Some(Message::MoveDockItemLeft(idx)) } else { None }),
                    button(
                        row![text("Sağa ").size(11.0).color(palette.text_primary.to_iced()), text("▶").size(11.0)]
                            .align_y(Alignment::Center)
                    )
                    .width(Length::Fill)
                    .padding([5, 8])
                    .style(move |_, status| {
                        let bg = if !can_right {
                            iced::Color::TRANSPARENT
                        } else if matches!(status, button::Status::Hovered) {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                        } else {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border { radius: iced::border::Radius::from(6.0), ..Default::default() },
                            ..Default::default()
                        }
                    })
                    .on_press_maybe(if can_right { Some(Message::MoveDockItemRight(idx)) } else { None }),
                ]
                .spacing(6)
                .width(Length::Fill);
                menu_items = menu_items.push(reorder_row);
            }

            let is_pinned = self.config.pinned_apps.contains(&app_id_str);
            let pin_id_copy = app_id_str.clone();
            let pin_btn = button(
                row![
                    text(Icon::PIN)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.primary.to_iced()),
                    text(if is_pinned { "Dock'tan Kaldır" } else { "Dock'a Sabitle" }).size(12.0).color(palette.text_primary.to_iced()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6, 10])
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(if is_pinned {
                Message::UnpinAppFromDock(pin_id_copy)
            } else {
                Message::PinAppToDock(pin_id_copy)
            });
            menu_items = menu_items.push(pin_btn);

            let is_dodge = self.config.autohide_mode == "dodge";
            let autohide_btn = button(
                row![
                    text("🛡️").size(13.0),
                    text(if is_dodge { "Akıllı Gizlenme: Açık" } else { "Akıllı Gizlenme: Kapalı" }).size(12.0).color(palette.text_primary.to_iced()),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding([6, 10])
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ToggleAutohideMode);
            menu_items = menu_items.push(autohide_btn);

            let menu_card = container(column![header, header_sep, menu_items].spacing(8))
                .padding([12, 14])
                .width(Length::Fixed(260.0))
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, 16.0);
                    if let Some(iced::Background::Color(color)) = style.background {
                        style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                            color.r, color.g, color.b, color.a * popover_alpha,
                        )));
                    }
                    style
                });

            let inner_card = mouse_area(menu_card).on_press(Message::NoOp);
            let content_col = iced::widget::column![inner_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center);

            mouse_area(content_col)
                .on_press(Message::CloseDockContextMenu)
                .into()
        } else if self.show_mini_task_manager {
            let header = row![
                text("📊 Mini Görev Yöneticisi")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("Tümünü Aç ↗").size(11.0).color(palette.primary.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::OpenFullTaskManager),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleMiniTaskManager),
            ]
            .align_y(Alignment::Center);

            let mem_bar = row![
                text(format!(
                    "Bellek: {} MB / {} MB (%{:.0})",
                    self.memory_summary.0, self.memory_summary.1, self.memory_summary.2
                ))
                .size(11.0)
                .color(palette.text_muted.to_iced()),
            ];

            let mut proc_col = column![].spacing(6);
            if self.top_processes.is_empty() {
                proc_col = proc_col.push(
                    container(text("Süreç bilgisi yükleniyor...").size(12.0).color(palette.text_muted.to_iced()))
                        .padding([15, 10])
                        .center_x(Length::Fill),
                );
            } else {
                for p in &self.top_processes {
                    let pid = p.pid;
                    let proc_row = row![
                        column![
                            text(&p.name).size(12.0).color(palette.text_primary.to_iced()),
                            text(format!("PID: {} • {} MB RAM", p.pid, p.mem_mb))
                                .size(10.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .width(Length::Fill),
                        button(text("✕ Sonlandır").size(10.0).color(palette.danger.to_iced()))
                            .padding([3, 7])
                            .style(|_, status| {
                                let bg = match status {
                                    button::Status::Hovered => iced::Color::from_rgba(1.0, 0.2, 0.2, 0.25),
                                    _ => iced::Color::from_rgba(1.0, 0.2, 0.2, 0.10),
                                };
                                button::Style {
                                    background: Some(iced::Background::Color(bg)),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(6.0),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }
                            })
                            .on_press(Message::KillProcess(pid)),
                    ]
                    .align_y(Alignment::Center)
                    .spacing(8);

                    proc_col = proc_col.push(
                        container(proc_row)
                            .padding([5, 8])
                            .style(|_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(6.0),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    );
                }
            }

            let task_card = container(column![header, mem_bar, proc_col].spacing(10))
                .padding([14, 18])
                .width(Length::Fixed(360.0))
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, 16.0);
                    if let Some(iced::Background::Color(color)) = style.background {
                        style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                            color.r, color.g, color.b, color.a * popover_alpha,
                        )));
                    }
                    style
                });

            iced::widget::column![task_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if let Some(ref target_app_id) = self.active_window_previews {
            let matching_windows: Vec<_> = self.compositor_windows.iter().filter(|(_, aid, title, _, _, _)| {
                aid == target_app_id || is_matching_window(aid, title, match target_app_id.as_str() {
                    "terminal" | "corten-terminal" => ActiveApp::Terminal,
                    "files" | "corten-files" => ActiveApp::Files,
                    "settings" | "corten-settings" => ActiveApp::Settings,
                    "browser" => ActiveApp::Browser,
                    "editor" => ActiveApp::Editor,
                    "corten-ai" | "ai" => ActiveApp::Ai,
                    _ => ActiveApp::Terminal,
                })
            }).collect();

            let real_app_icon = resolve_system_icon(target_app_id);
            let app_icon_view: Element<'_, Self::Message> = if let Some(ref icon_path) = real_app_icon {
                iced::widget::image(image::Handle::from_path(icon_path))
                    .width(Length::Fixed(18.0))
                    .height(Length::Fixed(18.0))
                    .into()
            } else {
                text(app_icon_char(target_app_id))
                    .font(LUCIDE_FONT)
                    .size(16.0)
                    .color(palette.primary.to_iced())
                    .into()
            };

            let header = row![
                app_icon_view,
                text(format!("Pencere Önizlemesi ({})", target_app_id))
                    .size(13.0)
                    .font(iced::Font { weight: iced::font::Weight::Bold, ..Default::default() })
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::CloseWindowPreviews),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let mut win_row = row![].spacing(10).align_y(Alignment::Center);
            if matching_windows.is_empty() {
                win_row = win_row.push(
                    container(text("Açık pencere bulunamadı.").size(12.0).color(palette.text_muted.to_iced()))
                        .padding([15, 10])
                        .center_x(Length::Fill),
                );
            } else {
                for (win_id, aid, title, ws, is_active, _) in matching_windows.iter().take(3) {
                    let win_id_val = *win_id;
                    let aid_val = (*aid).clone();
                    let title_text = if title.is_empty() { aid.as_str() } else { title.as_str() };
                    let title_clipped = if title_text.chars().count() > 22 {
                        format!("{}...", title_text.chars().take(20).collect::<String>())
                    } else {
                        title_text.to_string()
                    };

                    // Miniature window interior simulation
                    let lower_aid = aid.to_lowercase();
                    let mini_content: Element<'_, Self::Message> = if lower_aid.contains("term") {
                        column![
                            text("corten@desktop:~$ corten-term").size(9.0).color(iced::Color::from_rgb(0.3, 0.9, 0.4)),
                            text("> Ready (60 FPS Wayland)").size(8.0).color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.7)),
                            row![
                                text("▍").size(9.0).color(iced::Color::from_rgb(0.3, 0.9, 0.4)),
                            ],
                        ]
                        .spacing(2)
                        .padding(6)
                        .into()
                    } else if lower_aid.contains("file") {
                        row![
                            column![text("📁").size(14.0), text("Belgeler").size(7.0).color(palette.text_muted.to_iced())].align_x(Alignment::Center),
                            column![text("📁").size(14.0), text("İndirilen").size(7.0).color(palette.text_muted.to_iced())].align_x(Alignment::Center),
                            column![text("📁").size(14.0), text("Resimler").size(7.0).color(palette.text_muted.to_iced())].align_x(Alignment::Center),
                        ]
                        .spacing(10)
                        .padding(10)
                        .align_y(Alignment::Center)
                        .into()
                    } else if lower_aid.contains("setting") {
                        column![
                            row![text("Ses:").size(8.0), horizontal_space(), text("80%").size(8.0).color(palette.primary.to_iced())],
                            container(row![]).width(Length::Fixed(120.0)).height(Length::Fixed(3.0)).style(move |_| container::Style { background: Some(iced::Background::Color(palette.primary.to_iced())), border: iced::Border { radius: iced::border::Radius::from(2.0), ..Default::default() }, ..Default::default() }),
                            row![text("Parlaklık:").size(8.0), horizontal_space(), text("100%").size(8.0).color(palette.primary.to_iced())],
                        ]
                        .spacing(4)
                        .padding(6)
                        .into()
                    } else {
                        column![
                            text(title_clipped.clone()).size(9.0).color(palette.text_primary.to_iced()),
                            text(format!("Pencere ID #{}", win_id_val)).size(8.0).color(palette.text_muted.to_iced()),
                        ]
                        .spacing(3)
                        .padding(6)
                        .into()
                    };

                    let traffic_dots = row![
                        container(row![]).width(Length::Fixed(5.0)).height(Length::Fixed(5.0)).style(|_| container::Style { background: Some(iced::Background::Color(iced::Color::from_rgb(0.95, 0.3, 0.3))), border: iced::Border { radius: iced::border::Radius::from(3.0), ..Default::default() }, ..Default::default() }),
                        container(row![]).width(Length::Fixed(5.0)).height(Length::Fixed(5.0)).style(|_| container::Style { background: Some(iced::Background::Color(iced::Color::from_rgb(0.95, 0.75, 0.2))), border: iced::Border { radius: iced::border::Radius::from(3.0), ..Default::default() }, ..Default::default() }),
                        container(row![]).width(Length::Fixed(5.0)).height(Length::Fixed(5.0)).style(|_| container::Style { background: Some(iced::Background::Color(iced::Color::from_rgb(0.3, 0.85, 0.4))), border: iced::Border { radius: iced::border::Radius::from(3.0), ..Default::default() }, ..Default::default() }),
                    ]
                    .spacing(3)
                    .align_y(Alignment::Center);

                    let mini_titlebar = row![
                        traffic_dots,
                        text(title_clipped).size(10.0).color(palette.text_primary.to_iced()),
                        horizontal_space(),
                        button(text("✕").size(9.0).color(palette.danger.to_iced()))
                            .padding([1, 4])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::CloseWindowById(win_id_val, aid_val)),
                    ]
                    .spacing(4)
                    .padding([4, 6])
                    .align_y(Alignment::Center);

                    let is_win_active = *is_active;
                    let mini_window_card = container(
                        column![
                            mini_titlebar,
                            container(mini_content)
                                .width(Length::Fill)
                                .height(Length::Fixed(70.0))
                                .style(|_| container::Style {
                                    background: Some(iced::Background::Color(iced::Color::from_rgba(0.02, 0.03, 0.05, 0.95))),
                                    ..Default::default()
                                }),
                            row![
                                text(format!("Masaüstü {}", ws)).size(9.0).color(palette.text_muted.to_iced()),
                                horizontal_space(),
                                text("Odaklan").size(9.0).color(palette.primary.to_iced()),
                            ]
                            .padding([3, 6])
                            .align_y(Alignment::Center),
                        ]
                    )
                    .width(Length::Fixed(190.0))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(0.08, 0.10, 0.14, 0.95))),
                        border: iced::Border {
                            color: if is_win_active { palette.primary.to_iced() } else { iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12) },
                            width: if is_win_active { 1.5 } else { 1.0 },
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    });

                    let clickable_thumb = button(mini_window_card)
                        .padding(0)
                        .style(|_, status| {
                            let border_color = match status {
                                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.4),
                                _ => iced::Color::TRANSPARENT,
                            };
                            button::Style {
                                border: iced::Border { color: border_color, width: 1.0, radius: iced::border::Radius::from(8.0) },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::FocusWindow(win_id_val));

                    win_row = win_row.push(clickable_thumb);
                }
            }

            let card_w = ((matching_windows.len().max(1) as f32) * 205.0 + 28.0).clamp(240.0, 680.0);
            let preview_card = container(column![header, win_row].spacing(10))
                .padding([12, 14])
                .width(Length::Fixed(card_w))
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, 16.0);
                    if let Some(iced::Background::Color(color)) = style.background {
                        style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                            color.r, color.g, color.b, color.a * popover_alpha,
                        )));
                    }
                    style
                });

            iced::widget::column![preview_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_net_speed_details {
            let (is_online, net_name, ip_addr) = &self.network_status;
            let header = row![
                text("🌐 Canlı Ağ ve Trafik Detayı")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleNetSpeedDetails),
            ]
            .align_y(Alignment::Center);

            let status_row = row![
                text(if *is_online { "🟢 Bağlı" } else { "🔴 Çevrimdışı" }).size(12.0),
                text(format!("{} ({})", net_name, ip_addr)).size(12.0).color(palette.text_primary.to_iced()),
            ]
            .spacing(8);

            let speed_boxes = row![
                container(
                    column![
                        text("İNDİRME (RX)").size(10.0).color(palette.text_muted.to_iced()),
                        text(&self.net_speed_display.0).size(16.0).color(palette.primary.to_iced()),
                    ]
                    .spacing(2)
                )
                .width(Length::Fill)
                .padding([8, 12])
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                    border: iced::Border { radius: iced::border::Radius::from(8.0), ..Default::default() },
                    ..Default::default()
                }),
                container(
                    column![
                        text("YÜKLEME (TX)").size(10.0).color(palette.text_muted.to_iced()),
                        text(&self.net_speed_display.1).size(16.0).color(palette.warning.to_iced()),
                    ]
                    .spacing(2)
                )
                .width(Length::Fill)
                .padding([8, 12])
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04))),
                    border: iced::Border { radius: iced::border::Radius::from(8.0), ..Default::default() },
                    ..Default::default()
                }),
            ]
            .spacing(10);

            let net_card = container(column![header, status_row, speed_boxes].spacing(12))
                .padding([14, 18])
                .width(Length::Fixed(360.0))
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, 16.0);
                    if let Some(iced::Background::Color(color)) = style.background {
                        style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                            color.r, color.g, color.b, color.a * popover_alpha,
                        )));
                    }
                    style
                });

            iced::widget::column![net_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_desktop_context_hub {
            let hub_action_btn = |icon_char: &'static str, label_text: &'static str, msg: Message, pal: Palette| {
                button(
                    column![
                        text(icon_char).size(20.0),
                        text(label_text).size(10.0).color(pal.text_primary.to_iced()),
                    ]
                    .spacing(4)
                    .align_x(Alignment::Center),
                )
                .padding([10, 8])
                .width(Length::FillPortion(1))
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            width: 1.0,
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    }
                })
                .on_press(msg)
            };

            let header = row![
                text("Masaüstü Hızlı Tuvali (Desktop Canvas)")
                    .size(13.0)
                    .font(iced::Font { weight: iced::font::Weight::Bold, ..Default::default() })
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::CloseDesktopContextHub),
            ]
            .align_y(Alignment::Center);

            let wallpaper_row = container(
                row![
                    text("🖼️").size(16.0),
                    column![
                        text("Duvarkağıdı & Ambiance Uyum").size(12.0).color(palette.text_primary.to_iced()),
                        text("Canlı Renk & Atmosferik Senkronizasyon").size(9.0).color(palette.text_muted.to_iced()),
                    ]
                    .spacing(1),
                    horizontal_space(),
                    button(text("‹ Önceki").size(11.0).color(palette.text_primary.to_iced()))
                        .padding([4, 8])
                        .style(|_, status| {
                            let bg = match status {
                                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                                _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border { radius: iced::border::Radius::from(6.0), ..Default::default() },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::TriggerWallpaperCycle),
                    button(text("Sonraki ›").size(11.0).color(palette.on_primary().to_iced()))
                        .padding([4, 8])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => palette.primary_hover.to_iced(),
                                _ => palette.primary.to_iced(),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border { radius: iced::border::Radius::from(6.0), ..Default::default() },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::TriggerWallpaperCycle),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([8, 12])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.22))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            let tools_grid = row![
                hub_action_btn("📸", "Ekran Kesiti", Message::LaunchCommand("corten-snipper".into()), palette),
                hub_action_btn(">_", "Terminal", Message::LaunchCommand("corten-terminal || alacritty || foot".into()), palette),
                hub_action_btn("📁", "Dosyalar", Message::LaunchCommand("corten-files".into()), palette),
                hub_action_btn("📊", "Monitör", Message::OpenFullTaskManager, palette),
            ]
            .spacing(8)
            .width(Length::Fill);

            let zen_toggle_row = button(
                row![
                    text("👁️").size(14.0),
                    text("Masaüstü HUD Göstergelerini Değiştir (Zen Modu)")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    container(text("HUD").size(9.0).color(palette.primary.to_iced()))
                        .padding([2, 5])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.20))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(4.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([8, 10])
            .width(Length::Fill)
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    _ => iced::Color::from_rgba(0.0, 0.0, 0.0, 0.18),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        width: 1.0,
                        radius: iced::border::Radius::from(8.0),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::LaunchCommand("killall -USR1 corten-widgets || true".into()));

            let hub_card = container(column![header, wallpaper_row, tools_grid, zen_toggle_row].spacing(10))
                .padding([14, 18])
                .width(Length::Fixed(360.0))
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, 16.0);
                    if let Some(iced::Background::Color(color)) = style.background {
                        style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                            color.r, color.g, color.b, color.a * popover_alpha,
                        )));
                    }
                    style
                });

            iced::widget::column![hub_card, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else {
            let mut bar_content = iced::widget::column![].spacing(8).align_x(Alignment::Center);
            if let Some((kind, lvl, _)) = &self.osd_state {
                let osd_pill = match kind {
                    OsdKind::Workspace(ws) => {
                        let mut ws_chips = row![].spacing(6).align_y(Alignment::Center);
                        for i in 1..=5 {
                            let is_curr = i == *ws;
                            let chip = container(
                                text(format!("{}", i))
                                    .size(12.0)
                                    .color(if is_curr {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_muted.to_iced()
                                    }),
                            )
                            .padding([2, 8])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if is_curr {
                                    iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.22)
                                } else {
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                                })),
                                border: iced::Border {
                                    color: if is_curr {
                                        palette.primary.to_iced()
                                    } else {
                                        iced::Color::TRANSPARENT
                                    },
                                    width: 1.0,
                                    radius: iced::border::Radius::from(6.0),
                                },
                                ..Default::default()
                            });
                            ws_chips = ws_chips.push(chip);
                        }

                        container(
                            row![
                                text("⊞").size(14.0).color(palette.primary.to_iced()),
                                text(format!("Masaüstü {}", ws)).size(12.0).color(palette.text_primary.to_iced()),
                                ws_chips,
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 16])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(0.09, 0.10, 0.12, 0.95))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.55),
                                width: 1.5,
                                radius: iced::border::Radius::from(16.0),
                            },
                            shadow: iced::Shadow {
                                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.5),
                                offset: iced::Vector::new(0.0, 4.0),
                                blur_radius: 16.0,
                            },
                            ..Default::default()
                        })
                    }
                    OsdKind::Toast(msg) => {
                        container(
                            row![
                                text(msg).size(12.0).color(palette.text_primary.to_iced()),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 18])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(0.09, 0.10, 0.12, 0.96))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.6),
                                width: 1.5,
                                radius: iced::border::Radius::from(16.0),
                            },
                            shadow: iced::Shadow {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.25),
                                offset: iced::Vector::new(0.0, 4.0),
                                blur_radius: 18.0,
                            },
                            ..Default::default()
                        })
                    }
                    OsdKind::MicMute(is_muted) => {
                        let (icon, label, color) = if *is_muted {
                            ("🛡️", "Mikrofon Susturuldu", palette.danger)
                        } else {
                            ("🎙️", "Mikrofon Aktif", palette.success)
                        };
                        container(
                            row![
                                text(icon).size(16.0),
                                text(label).size(12.0).color(color.to_iced()),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 18])
                        .style(move |_| {
                            let mut style = acrylic_glass_style(palette, 16.0);
                            style.border.color = color.to_iced();
                            style
                        })
                    }
                    OsdKind::BatteryWarning(pct) => {
                        container(
                            row![
                                text("⚡").size(16.0).color(palette.warning.to_iced()),
                                text(format!("Düşük Pil Uyarısı: %{} (Şarja Takın)", pct))
                                    .size(12.0)
                                    .color(palette.warning.to_iced()),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 18])
                        .style(move |_| {
                            let mut style = acrylic_glass_style(palette, 16.0);
                            style.border.color = palette.warning.to_iced();
                            style
                        })
                    }
                    OsdKind::Volume | OsdKind::Brightness => {
                        let icon = match kind {
                            OsdKind::Volume => {
                                if *lvl == 0 {
                                    "🔇"
                                } else if *lvl < 33 {
                                    "🔈"
                                } else if *lvl < 66 {
                                    "🔉"
                                } else {
                                    "🔊"
                                }
                            }
                            OsdKind::Brightness => "🔆",
                            _ => "",
                        };
                        let label = match kind {
                            OsdKind::Volume => "Ses",
                            OsdKind::Brightness => "Parlaklık",
                            _ => "",
                        };
                        let fill_pct = (*lvl as f32 / 100.0).clamp(0.0, 1.0);
                        let bar_width = 110.0;
                        let filled_width = (fill_pct * bar_width).clamp(0.5, bar_width);

                        let liquid_bar = container(
                            row![
                                container(text(""))
                                    .width(Length::Fixed(filled_width))
                                    .height(Length::Fixed(6.0))
                                    .style(move |_| container::Style {
                                        background: Some(iced::Background::Color(palette.primary.to_iced())),
                                        border: iced::Border {
                                            radius: iced::border::Radius::from(3.0),
                                            ..Default::default()
                                        },
                                        ..Default::default()
                                    }),
                            ]
                        )
                        .width(Length::Fixed(bar_width))
                        .height(Length::Fixed(6.0))
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(3.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        });

                        container(
                            row![
                                text(icon).size(15.0),
                                text(label).size(12.0).color(palette.text_muted.to_iced()),
                                liquid_bar,
                                text(format!("{}%", lvl))
                                    .size(12.0)
                                    .font(iced::Font::MONOSPACE)
                                    .color(palette.text_primary.to_iced()),
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 18])
                        .style(move |_| acrylic_glass_style(palette, 16.0))
                    }
                    OsdKind::Switcher(windows) => {
                        let mut win_cards = row![].spacing(8).align_y(Alignment::Center);
                        for (app_id, title, is_active) in windows {
                            let app_name = if app_id.is_empty() { "Uygulama" } else { app_id.as_str() };
                            let short_title = if title.chars().count() > 20 {
                                let t: String = title.chars().take(18).collect();
                                format!("{}…", t)
                            } else if title.is_empty() {
                                app_name.to_string()
                            } else {
                                title.clone()
                            };
                            let is_curr = *is_active;
                            let card = container(
                                iced::widget::column![
                                    text(app_name).size(12.0).color(if is_curr {
                                        palette.primary.to_iced()
                                    } else {
                                        palette.text_primary.to_iced()
                                    }),
                                    text(short_title).size(10.0).color(palette.text_muted.to_iced()),
                                ]
                                .spacing(2)
                                .align_x(Alignment::Center)
                            )
                            .padding([6, 12])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if is_curr {
                                    iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.25)
                                } else {
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                                })),
                                border: iced::Border {
                                    color: if is_curr {
                                        palette.primary.to_iced()
                                    } else {
                                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                                    },
                                    width: if is_curr { 1.5 } else { 1.0 },
                                    radius: iced::border::Radius::from(8.0),
                                },
                                ..Default::default()
                            });
                            win_cards = win_cards.push(card);
                        }

                        container(
                            row![
                                text("🗔").size(15.0).color(palette.primary.to_iced()),
                                win_cards,
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 16])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(0.09, 0.10, 0.12, 0.96))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.6),
                                width: 1.5,
                                radius: iced::border::Radius::from(16.0),
                            },
                            shadow: iced::Shadow {
                                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.25),
                                offset: iced::Vector::new(0.0, 4.0),
                                blur_radius: 18.0,
                            },
                            ..Default::default()
                        })
                    }
                };
                bar_content = bar_content.push(osd_pill);
            }

            let bottom_bar = container(dock_cluster)
                .width(Length::Fill)
                .center_x(Length::Fill)
                .align_y(Alignment::Center);

            bar_content = bar_content.push(bottom_bar);
            bar_content.into()
        };

        let _panel_height = if !self.panel_visible {
            1.0
        } else if self.show_control_center || self.show_power_menu || self.show_calendar || self.show_clipboard_modal || self.show_notification_center || self.show_downloads_stack || self.show_mini_task_manager || self.dock_context_menu.is_some() || self.active_window_previews.is_some() || self.show_net_speed_details || self.show_desktop_context_hub {
            560.0
        } else {
            (self.config.height as f32).max(1.0)
        };

        let (align_y, padding) = match self.config.position {
            PanelPosition::Top => (
                iced::alignment::Vertical::Top,
                iced::Padding {
                    top: 4.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                },
            ),
            PanelPosition::Bottom => (
                iced::alignment::Vertical::Bottom,
                iced::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 4.0,
                    left: 0.0,
                },
            ),
        };

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(align_y)
            .padding(padding)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::{launch_tray_application, SystemPowerAction};

    #[test]
    fn destructive_power_actions_require_confirmation() {
        assert!(!SystemPowerAction::Lock.requires_confirmation());
        assert!(!SystemPowerAction::Logout.requires_confirmation());
        assert!(SystemPowerAction::Reboot.requires_confirmation());
        assert!(SystemPowerAction::Poweroff.requires_confirmation());
    }

    #[test]
    fn rejects_unknown_tray_commands() {
        assert!(launch_tray_application("sh -c malicious").is_err());
    }

    #[test]
    fn test_osd_kind_and_vu_meter() {
        use super::OsdKind;
        let mic = OsdKind::MicMute(true);
        let bat = OsdKind::BatteryWarning(15);
        assert_eq!(mic, OsdKind::MicMute(true));
        assert_eq!(bat, OsdKind::BatteryWarning(15));
    }

    #[test]
    fn test_window_matching_settings_files_isolation() {
        use super::{is_matching_window, ActiveApp};

        // Files windows with various app_id and titles
        assert!(is_matching_window("corten-files", "Corten Files", ActiveApp::Files));
        assert!(!is_matching_window("corten-files", "Corten Files", ActiveApp::Settings));
        assert!(is_matching_window("", "Corten Files", ActiveApp::Files));
        assert!(!is_matching_window("", "Corten Files", ActiveApp::Settings));

        // Settings windows with various app_id and titles
        assert!(is_matching_window("corten-settings", "Corten Settings", ActiveApp::Settings));
        assert!(!is_matching_window("corten-settings", "Corten Settings", ActiveApp::Files));
        assert!(is_matching_window("", "Corten Settings", ActiveApp::Settings));
        assert!(!is_matching_window("", "Corten Settings", ActiveApp::Files));

        // Folder or file named settings opened in corten-files must NOT match Settings!
        assert!(is_matching_window("corten-files", "settings.toml — Corten Files", ActiveApp::Files));
        assert!(!is_matching_window("corten-files", "settings.toml — Corten Files", ActiveApp::Settings));
        assert!(!is_matching_window("", "settings.toml — Corten Files", ActiveApp::Settings));
    }

    #[test]
    fn test_custom_applet_json_parsing() {
        let raw_json = r#"{"text": "124 ms", "tooltip": "Cloudflare DNS: 0% loss", "class": "warning", "percentage": 85}"#;
        let json: serde_json::Value = serde_json::from_str(raw_json).unwrap();
        let text = json.get("text").and_then(|v| v.as_str()).unwrap();
        let tooltip = json.get("tooltip").and_then(|v| v.as_str()).unwrap();
        let class = json.get("class").and_then(|v| v.as_str()).unwrap();
        let percentage = json.get("percentage").and_then(|v| v.as_u64()).unwrap();

        assert_eq!(text, "124 ms");
        assert_eq!(tooltip, "Cloudflare DNS: 0% loss");
        assert_eq!(class, "warning");
        assert_eq!(percentage, 85);
    }

    #[test]
    fn test_custom_applet_plain_text_fallback() {
        let raw_text = "Standard Plain Output\nSecond Line Ignored";
        let parsed_json = serde_json::from_str::<serde_json::Value>(raw_text);
        assert!(parsed_json.is_err());
        let first_line = raw_text.lines().next().unwrap().trim();
        assert_eq!(first_line, "Standard Plain Output");
    }

    #[test]
    fn test_dock_context_menu_view_rendering() {
        use super::*;
        let config_manager = ConfigManager::<PanelConfig>::load_or_default("panel").unwrap();
        let flags = PanelAppFlags {
            config_manager,
            ipc_rx: None,
            output_name: None,
        };
        let (mut app, _) = CortenPanelApp::new(flags);
        app.dock_context_menu = Some(DockMenuTarget::Pinned(ActiveApp::Terminal, 0));
        let _ = app.view();

        app.dock_context_menu = Some(DockMenuTarget::Dynamic {
            win_id: 123,
            app_id: "firefox".into(),
            title: "Mozilla Firefox".into(),
        });
        let _ = app.view();

        app.show_calendar = true;
        app.dock_context_menu = None;
        let _ = app.view();
    }
}
