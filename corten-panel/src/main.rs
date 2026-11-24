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
use corten_theme::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, Motion, NOTO_SANS, NOTO_SANS_BYTES, Palette,
    PatinaVariant, Radius, Spacing, ThemeMode, Transition, Typography,
};
use iced::widget::{
    button, column, container, horizontal_space, image, mouse_area, row, scrollable, slider, text,
    text_input, vertical_space,
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
use iced::{Alignment, Element, Event, Length, Subscription, event, keyboard};
use iced_layershell::Application;
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
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
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(dir) = current_exe.parent()
    {
        let candidate = dir.join(bin_name);
        if candidate.is_file() {
            return candidate;
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
    for dir in &["/usr/local/bin", "/usr/bin", "/bin"] {
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
            ));
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
            let server = PanelServer {
                tx: dbus_tx.clone(),
            };
            let (watcher, _items) = sni::StatusNotifierWatcherServer::new(dbus_tx);
            watcher.start_monitor();
            if let Ok(b) = Builder::session()
                && let Ok(b) = b.name("org.corten.Panel")
                && let Ok(b) = b.serve_at("/org/corten/Panel", server)
                && let Ok(b) = b.serve_at("/StatusNotifierWatcher", watcher)
                && let Ok(conn) = b.build().await
            {
                // Request org.kde.StatusNotifierWatcher non-fatally so panel IPC
                // remains active even if another system tray watcher is running.
                let _ = conn.request_name("org.kde.StatusNotifierWatcher").await;
                std::future::pending::<()>().await;
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

    if args.iter().any(|a| a == "--preview" || a == "-p") {
        info!("Opening Corten Panel in windowed preview mode (corten-panel --preview)...");
        let rx_cell = std::sync::Mutex::new(Some(ipc_rx));
        let out_clone = binded_output.clone();
        iced::application(
            "Corten Panel & Control Deck — Live Preview",
            CortenPanelApp::update_preview,
            CortenPanelApp::view_preview,
        )
        .subscription(CortenPanelApp::subscription_preview)
        .window(iced::window::Settings {
            size: iced::Size::new(980.0, 560.0),
            resizable: true,
            ..Default::default()
        })
        .font(LUCIDE_FONT_BYTES)
        .font(NOTO_SANS_BYTES)
        .default_font(NOTO_SANS)
        .run_with(move || {
            let cm = ConfigManager::<PanelConfig>::load_or_default("panel")
                .expect("Failed to load panel config");
            let rx = rx_cell.lock().ok().and_then(|mut g| g.take());
            let (mut app, task) = CortenPanelApp::new(PanelAppFlags {
                config_manager: cm,
                ipc_rx: rx,
                output_name: out_clone.clone(),
            });
            app.show_control_center = true;
            app.popover_transition = Transition::new(Motion::DURATION_EMPHASIZED_DECELERATE);
            if app.compositor_windows.is_empty() {
                app.compositor_windows = vec![
                    (1, "brave-browser".into(), "Brave".into(), 1, false, false),
                    (2, "spotify".into(), "Spotify".into(), 1, false, false),
                ];
            }
            (app, task)
        })?;
        return Ok(());
    }

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
        fonts: vec![
            Cow::Borrowed(LUCIDE_FONT_BYTES),
            Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        layer_settings: LayerShellSettings {
            anchor,
            layer: Layer::Top, // High priority desktop bar, below Overlay (lockscreen)
            margin,
            exclusive_zone,
            size: Some((initial_width, initial_config.height.max(82))), // Center-aligned compact dock pill + subtle ^ drawer handle
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
    Forge,
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
    Dynamic {
        win_id: u64,
        app_id: String,
        title: String,
    },
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
    TogglePinClipboardItem(String),
    SetClipboardFilter(String),
    DeleteClipboardItem(usize),
    ClearClipboardHistory,
    PickScreenColor,
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
    RouteAppStreamToNextSink(u32),
    ConfirmSystemAction,
    CancelSystemAction,
    WeatherUpdated(WeatherReport),
    RefreshWeather,
    CustomScriptResult(usize, String),
    ClickCustomScript(usize),
    RightClickCustomScript(usize),
    ConfigReloaded(PanelConfig),
    SetPatina(PatinaVariant),
    ToggleAppDrawer,
    DrawerAppsLoaded(Vec<DrawerAppEntry>),
    DrawerSearchChanged(String),
    DrawerCategorySelected(String),
    LaunchDrawerApp(String),
    IpcCommand(PanelIpcCommand),
    IcedEvent(Event),
    LayerAction(LayershellCustomActions),
    NoOp,
}

#[derive(Debug, Clone)]
pub struct DrawerAppEntry {
    name: String,
    exec: String,
    category: &'static str,
    icon_path: Option<PathBuf>,
    icon_fallback: char,
}

fn scan_installed_drawer_apps() -> Vec<DrawerAppEntry> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        dirs.push(h.join(".local/share/applications"));
        dirs.push(h.join(".local/share/flatpak/exports/share/applications"));
    }

    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut in_main = false;
            let mut name = String::new();
            let mut exec = String::new();
            let mut icon = String::new();
            let mut categories = String::new();
            let mut no_display = false;
            let mut hidden = false;
            let mut is_app = false;

            for raw_line in content.lines() {
                let line = raw_line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with('[') {
                    in_main = line == "[Desktop Entry]";
                    continue;
                }
                if !in_main {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    match k.trim() {
                        "Type" => is_app = v.trim() == "Application",
                        "Name" if name.is_empty() => name = v.trim().to_string(),
                        "Exec" if exec.is_empty() => exec = v.trim().to_string(),
                        "Icon" if icon.is_empty() => icon = v.trim().to_string(),
                        "Categories" => categories = v.trim().to_ascii_lowercase(),
                        "NoDisplay" => no_display = v.trim().eq_ignore_ascii_case("true"),
                        "Hidden" => hidden = v.trim().eq_ignore_ascii_case("true"),
                        _ => {}
                    }
                }
            }

            if !is_app || no_display || hidden || name.is_empty() || exec.is_empty() {
                continue;
            }
            // Strip freedesktop Exec field codes (%u, %U, %f, %F, %i, %c, %k)
            let cleaned_exec: String = exec
                .split_whitespace()
                .filter(|tok| {
                    !matches!(
                        *tok,
                        "%u" | "%U" | "%f" | "%F" | "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N"
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            if cleaned_exec.is_empty() {
                continue;
            }

            let dedup_key = name.to_ascii_lowercase();
            if !seen.insert(dedup_key) {
                continue;
            }

            let category = if categories.contains("network") || categories.contains("webbrowser") {
                "internet"
            } else if categories.contains("development") || categories.contains("ide") {
                "dev"
            } else if categories.contains("audiovideo")
                || categories.contains("audio")
                || categories.contains("video")
                || categories.contains("music")
            {
                "media"
            } else if categories.contains("system")
                || categories.contains("settings")
                || categories.contains("utility")
            {
                "system"
            } else {
                "other"
            };

            let icon_key = if !icon.is_empty() {
                icon.as_str()
            } else {
                path.file_stem().and_then(|s| s.to_str()).unwrap_or(&name)
            };
            let icon_path = if PathBuf::from(icon_key).is_file() {
                Some(PathBuf::from(icon_key))
            } else {
                resolve_system_icon(icon_key)
            };
            let icon_fallback = app_icon_char(icon_key);

            apps.push(DrawerAppEntry {
                name,
                exec: cleaned_exec,
                category,
                icon_path,
                icon_fallback,
            });
        }
    }

    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

fn launch_drawer_exec(exec: &str) {
    let cmd_str = exec.trim().to_string();
    if cmd_str.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", &cmd_str]);
        if std::env::var_os("DISPLAY").is_none()
            && let Ok(rt) = std::env::var("XDG_RUNTIME_DIR")
            && let Ok(disp) = std::fs::read_to_string(PathBuf::from(rt).join("corten-x11-display"))
        {
            let disp = disp.trim();
            if !disp.is_empty() {
                cmd.env("DISPLAY", disp);
            }
        }
        let _ = cmd.spawn();
    });
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
    show_app_drawer: bool,
    drawer_search: String,
    drawer_category: String,
    drawer_apps: Vec<DrawerAppEntry>,
    drawer_loading: bool,
    drawer_last_loaded: Option<std::time::Instant>,
    drawer_transition: Transition,
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
    pinned_clipboard: Vec<String>,
    clipboard_search: String,
    clipboard_filter: String,
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
    custom_accent: String,
    brightness_pct: u8,
    show_desktop_context_hub: bool,
    audio_vu_level: (f32, f32),
    last_dock_width: u32,
    last_layer_size: (u32, u32),
    hovered_dock_item: Option<String>,
    launch_bounce: Option<(ActiveApp, u8)>,
    eq_tick: u8,
    osd_display_level: f32,
    last_tick_sec: u64,
}

fn check_system_integrity(
    sentinel_mtimes: &mut std::collections::HashMap<std::path::PathBuf, u64>,
) -> Option<String> {
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
        if let Ok(meta) = std::fs::metadata(target)
            && let Ok(modified) = meta.modified()
        {
            let mtime = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Some(&prev_mtime) = sentinel_mtimes.get(target) {
                if mtime > prev_mtime {
                    sentinel_mtimes.insert(target.clone(), mtime);
                    let file_name = target.file_name().unwrap_or_default().to_string_lossy();
                    alert = Some(format!(
                        "Güvenlik: {} dosyasında değişiklik tespit edildi",
                        file_name
                    ));
                    tracing::warn!(
                        "Corten Security Sentinel: Sensitive file modification detected on {:?}",
                        target
                    );
                }
            } else {
                sentinel_mtimes.insert(target.clone(), mtime);
            }
        }
    }
    alert
}

fn is_custom_command_safe(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    let forbidden_patterns = [
        "sudo ",
        "doas ",
        "pkexec ",
        "su ",
        "rm -rf",
        "rm -fr",
        "rm -r -f",
        ":(){ :|:& };:", // fork bomb
        "mkfs",
        "dd if=",
        "| bash",
        "| sh",
        "| zsh",
        "| python",
        "> /dev/sd",
        "> /dev/nvme",
        "/etc/shadow",
        "/etc/passwd",
    ];
    for pattern in forbidden_patterns {
        if lower.contains(pattern) {
            return false;
        }
    }
    true
}

fn convert_svg_to_png(svg_path: &std::path::Path, out_size: u32) -> Option<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let cache_dir = std::env::var("XDG_CACHE_HOME")
        .ok()
        .map(|d| PathBuf::from(d).join("corten").join("icons"))
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".cache/corten/icons"))
        })
        .or_else(|| {
            std::env::var("XDG_RUNTIME_DIR")
                .ok()
                .map(|d| PathBuf::from(d).join("corten_icon_cache"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/corten_icon_cache"));
    let _ = std::fs::create_dir_all(&cache_dir);

    let file_stem = svg_path.file_stem()?.to_string_lossy();
    let mut hasher = DefaultHasher::new();
    svg_path.hash(&mut hasher);
    let path_hash = hasher.finish();
    let out_file = cache_dir.join(format!(
        "{}_{:08x}_{}x{}.png",
        file_stem, path_hash as u32, out_size, out_size
    ));
    if out_file.exists() {
        return Some(out_file);
    }

    let tmp_file = cache_dir.join(format!(
        "{}_{:08x}_{}x{}.png.tmp",
        file_stem, path_hash as u32, out_size, out_size
    ));
    if let Ok(status) = std::process::Command::new("rsvg-convert")
        .args([
            "-w",
            &out_size.to_string(),
            "-h",
            &out_size.to_string(),
            svg_path.to_str()?,
            "-o",
            tmp_file.to_str()?,
        ])
        .status()
        && status.success()
        && tmp_file.exists()
    {
        if std::fs::rename(&tmp_file, &out_file).is_ok() {
            return Some(out_file);
        }
        let _ = std::fs::remove_file(&tmp_file);
    }

    if rasterize_svg_pure_rust(svg_path, &out_file, out_size) {
        Some(out_file)
    } else {
        None
    }
}

fn rasterize_svg_pure_rust(
    svg_path: &std::path::Path,
    out_file: &std::path::Path,
    out_size: u32,
) -> bool {
    let Ok(svg_text) = std::fs::read_to_string(svg_path) else {
        return false;
    };
    let mut accent = (217u8, 107u8, 67u8);
    for chunk in svg_text.split('#').skip(1) {
        let hex: String = chunk
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if hex.len() == 6
            && let (Ok(r), Ok(g), Ok(b)) = (
                u8::from_str_radix(&hex[0..2], 16),
                u8::from_str_radix(&hex[2..4], 16),
                u8::from_str_radix(&hex[4..6], 16),
            )
            && (r as u16 + g as u16 + b as u16) > 45
        {
            accent = (r, g, b);
            break;
        }
    }
    let size = out_size.clamp(16, 128);
    let mut raw_scanlines = Vec::with_capacity(((size * 4 + 1) * size) as usize);
    let center = (size as f32 - 1.0) * 0.5;
    let radius = center * 0.85;
    for y in 0..size {
        raw_scanlines.push(0u8);
        for x in 0..size {
            let dx = (x as f32 - center).abs();
            let dy = (y as f32 - center).abs();
            if (dx * dx + dy * dy).sqrt() <= radius {
                raw_scanlines.extend_from_slice(&[accent.0, accent.1, accent.2, 240]);
            } else {
                raw_scanlines.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    let write_chunk = |out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let mut crc = 0xFFFF_FFFFu32;
        for &b in tag.iter().chain(data.iter()) {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if (crc & 1) != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        out.extend_from_slice(&(!crc).to_be_bytes());
    };

    let mut png = Vec::new();
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut png, b"IHDR", &ihdr);

    let mut zlib = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw_scanlines.chunks(65535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        zlib.push(if i + 1 == chunks.len() { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in &raw_scanlines {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    write_chunk(&mut png, b"IDAT", &zlib);
    write_chunk(&mut png, b"IEND", &[]);
    std::fs::write(out_file, png).is_ok()
}

fn pinned_clipboard_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".config/corten/pinned_clipboard.json")
}

fn load_pinned_clipboard() -> Vec<String> {
    let path = pinned_clipboard_path();
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

fn save_pinned_clipboard(items: &[String]) {
    let path = pinned_clipboard_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(items) {
        let _ = std::fs::write(path, json);
    }
}

fn is_hex_color_entry(s: &str) -> bool {
    parse_hex_swatch_color(s).is_some()
}

fn parse_hex_swatch_color(s: &str) -> Option<iced::Color> {
    let trimmed = s.trim();
    if trimmed.starts_with('#') && trimmed.len() == 7 {
        let r = u8::from_str_radix(&trimmed[1..3], 16).ok()?;
        let g = u8::from_str_radix(&trimmed[3..5], 16).ok()?;
        let b = u8::from_str_radix(&trimmed[5..7], 16).ok()?;
        return Some(iced::Color::from_rgb8(r, g, b));
    }
    None
}

fn for_clip_tab<'a>(
    label: &'a str,
    key: &'static str,
    active: &str,
    palette: Palette,
) -> Element<'a, Message> {
    let is_active = active == key;
    button(text(label).size(10.5).color(if is_active {
        palette.on_primary().to_iced()
    } else {
        palette.text_muted.to_iced()
    }))
    .padding([3, 8])
    .style(move |_, _| button::Style {
        background: Some(iced::Background::Color(if is_active {
            palette.primary.to_iced()
        } else {
            palette.surface_hover.to_iced()
        })),
        border: iced::Border {
            radius: iced::border::Radius::from(6.0),
            ..Default::default()
        },
        ..Default::default()
    })
    .on_press(Message::SetClipboardFilter(key.into()))
    .into()
}

fn resolve_system_icon(app_id: &str) -> Option<PathBuf> {
    let lower = app_id.to_lowercase();
    let clean = lower.strip_suffix(".desktop").unwrap_or(&lower);

    // 0. Bespoke Corten icons direct check
    let bespoke_paths = [
        format!("assets/icons/hicolor/64x64/apps/{}.png", clean),
        format!("assets/icons/hicolor/48x48/apps/{}.png", clean),
        format!("assets/icons/hicolor/scalable/apps/{}.svg", clean),
        format!("/usr/share/icons/hicolor/64x64/apps/{}.png", clean),
        format!("/usr/share/icons/hicolor/48x48/apps/{}.png", clean),
    ];
    for bp in &bespoke_paths {
        let path = PathBuf::from(bp);
        if path.exists() {
            if bp.ends_with(".svg") {
                if let Some(png) = convert_svg_to_png(&path, 48) {
                    return Some(png);
                }
            } else {
                return Some(path);
            }
        }
    }

    let candidates: &[&str] =
        if lower.contains("corten-ai") || lower.contains("ai") || lower.contains("assistant") {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-ai.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-ai.png",
                "assets/icons/hicolor/64x64/apps/corten-ai.png",
                "assets/icons/hicolor/48x48/apps/corten-ai.png",
                "/usr/share/pixmaps/corten-ai.png",
                "assets/logos/ai_concept1_aura_core.png",
                "/usr/share/pixmaps/corten.png",
                "assets/logo.png",
            ]
        } else if lower.contains("corten-terminal")
            || lower.contains("term")
            || lower.contains("alacritty")
            || lower.contains("kitty")
            || lower.contains("foot")
            || lower.contains("konsole")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-terminal.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-terminal.png",
                "assets/icons/hicolor/64x64/apps/corten-terminal.png",
                "assets/icons/hicolor/48x48/apps/corten-terminal.png",
                "/usr/share/icons/oxygen/base/48x48/apps/utilities-terminal.png",
                "/usr/share/icons/breeze/apps/48/utilities-terminal.svg",
                "/usr/share/icons/AdwaitaLegacy/48x48/legacy/utilities-terminal.png",
                "/usr/share/icons/hicolor/48x48/apps/alacritty.png",
            ]
        } else if lower.contains("corten-files")
            || lower.contains("file")
            || lower.contains("nautilus")
            || lower.contains("dolphin")
            || lower.contains("thunar")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-files.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-files.png",
                "assets/icons/hicolor/64x64/apps/corten-files.png",
                "assets/icons/hicolor/48x48/apps/corten-files.png",
                "/usr/share/icons/oxygen/base/48x48/apps/system-file-manager.png",
                "/usr/share/icons/breeze/apps/48/system-file-manager.svg",
                "/usr/share/icons/AdwaitaLegacy/48x48/legacy/system-file-manager.png",
            ]
        } else if lower.contains("corten-settings")
            || lower.contains("setting")
            || lower.contains("config")
            || lower.contains("control")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-settings.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-settings.png",
                "assets/icons/hicolor/64x64/apps/corten-settings.png",
                "assets/icons/hicolor/48x48/apps/corten-settings.png",
                "/usr/share/icons/oxygen/base/48x48/categories/preferences-system.png",
                "/usr/share/icons/breeze/preferences/32/preferences-system.svg",
                "/usr/share/icons/AdwaitaLegacy/48x48/legacy/preferences-system.png",
            ]
        } else if lower.contains("corten-monitor")
            || lower.contains("monitor")
            || lower.contains("htop")
            || lower.contains("btop")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-monitor.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-monitor.png",
                "assets/icons/hicolor/64x64/apps/corten-monitor.png",
                "assets/icons/hicolor/48x48/apps/corten-monitor.png",
                "/usr/share/icons/oxygen/base/48x48/apps/utilities-system-monitor.png",
                "/usr/share/icons/breeze/apps/48/utilities-system-monitor.svg",
            ]
        } else if lower.contains("corten-welcome") || lower.contains("welcome") {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-welcome.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-welcome.png",
                "assets/icons/hicolor/64x64/apps/corten-welcome.png",
                "assets/icons/hicolor/48x48/apps/corten-welcome.png",
            ]
        } else if lower.contains("editor")
            || lower.contains("code")
            || lower.contains("nvim")
            || lower.contains("rust")
            || lower.contains("text")
            || lower.contains("kate")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-editor.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-editor.png",
                "assets/icons/hicolor/64x64/apps/corten-editor.png",
                "assets/icons/hicolor/48x48/apps/corten-editor.png",
                "/usr/share/icons/oxygen/base/48x48/apps/accessories-text-editor.png",
                "/usr/share/icons/breeze/apps/48/accessories-text-editor.svg",
                "/usr/share/icons/AdwaitaLegacy/48x48/legacy/accessories-text-editor.png",
                "/usr/share/pixmaps/vscode.png",
            ]
        } else if lower.contains("browser")
            || lower.contains("firefox")
            || lower.contains("chrom")
            || lower.contains("brave")
            || lower.contains("zen")
            || lower.contains("web")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-browser.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-browser.png",
                "assets/icons/hicolor/64x64/apps/corten-browser.png",
                "assets/icons/hicolor/48x48/apps/corten-browser.png",
                "/usr/share/icons/hicolor/48x48/apps/firefox.png",
                "/usr/share/icons/oxygen/base/48x48/apps/internet-web-browser.png",
                "/usr/share/icons/breeze/apps/48/internet-web-browser.svg",
            ]
        } else if lower.contains("corten-forge")
            || lower.contains("forge")
            || lower.contains("store")
            || lower.contains("software")
            || lower.contains("package")
        {
            &[
                "/usr/share/icons/hicolor/64x64/apps/corten-forge.png",
                "/usr/share/icons/hicolor/48x48/apps/corten-forge.png",
                "assets/icons/hicolor/64x64/apps/corten-forge.png",
                "assets/icons/hicolor/48x48/apps/corten-forge.png",
                "/usr/share/icons/oxygen/base/48x48/apps/system-software-install.png",
                "/usr/share/icons/breeze/apps/48/system-software-install.svg",
                "/usr/share/icons/AdwaitaLegacy/48x48/legacy/system-software-install.png",
                "assets/logo.png",
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
    if b_path.exists()
        && let Some(png) = convert_svg_to_png(&b_path, 48)
    {
        return Some(png);
    }

    // Flatpak exported icons (user + system), matching exact ID or reverse-DNS stem
    // (e.g., "brave-browser" -> "com.brave.Browser.png", "spotify" -> "com.spotify.Client.png")
    let mut flatpak_icon_roots = vec![PathBuf::from(
        "/var/lib/flatpak/exports/share/icons/hicolor",
    )];
    if let Ok(home) = std::env::var("HOME") {
        flatpak_icon_roots.insert(
            0,
            PathBuf::from(home).join(".local/share/flatpak/exports/share/icons/hicolor"),
        );
    }
    let key_token = clean
        .split(['-', '_', '.'])
        .find(|s| s.len() >= 3)
        .unwrap_or(clean);
    for root in &flatpak_icon_roots {
        for size_dir in ["128x128/apps", "64x64/apps", "48x48/apps", "scalable/apps"] {
            let dir = root.join(size_dir);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let stem_lower = stem.to_ascii_lowercase();
                    if stem_lower == clean
                        || stem_lower.contains(clean)
                        || (key_token.len() >= 3 && stem_lower.contains(key_token))
                    {
                        if path.extension().and_then(|e| e.to_str()) == Some("svg") {
                            if let Some(png) = convert_svg_to_png(&path, 48) {
                                return Some(png);
                            }
                        } else if path.extension().and_then(|e| e.to_str()) == Some("png") {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Corten's own system apps stay fixed in the dock's first app group;
/// everything else (pinned third-party apps and running windows) goes to the second group.
fn is_corten_system_app(app_id: &str) -> bool {
    let lower = app_id.to_ascii_lowercase();
    lower.starts_with("corten-")
        || matches!(
            lower.as_str(),
            "files" | "settings" | "terminal" | "forge" | "store" | "software" | "ai" | "monitor"
        )
}

/// Wraps a dock popover so it grows upward out of the dock exactly like the App Drawer.
fn reveal_card<'a>(card: impl Into<Element<'a, Message>>, max_h: f32) -> Element<'a, Message> {
    container(card).max_height(max_h).clip(true).into()
}

fn app_icon_char(app_id: &str) -> char {
    let lower = app_id.to_lowercase();
    if lower.contains("term")
        || lower.contains("alacritty")
        || lower.contains("kitty")
        || lower.contains("foot")
        || lower.contains("bash")
    {
        Icon::TERMINAL
    } else if lower.contains("firefox")
        || lower.contains("chrom")
        || lower.contains("brave")
        || lower.contains("zen")
        || lower.contains("web")
        || lower.contains("browser")
    {
        Icon::GLOBE
    } else if lower.contains("code")
        || lower.contains("rust")
        || lower.contains("nvim")
        || lower.contains("emacs")
        || lower.contains("editor")
    {
        Icon::CODE
    } else if lower.contains("music")
        || lower.contains("spotify")
        || lower.contains("vlc")
        || lower.contains("mpv")
    {
        Icon::MUSIC
    } else if lower.contains("file")
        || lower.contains("corten-files")
        || lower.contains("thunar")
        || lower.contains("nautilus")
    {
        Icon::FOLDER
    } else if lower.contains("setting")
        || lower.contains("corten-settings")
        || lower.contains("control")
    {
        Icon::SETTINGS
    } else if lower.contains("ai") || lower.contains("corten-ai") || lower.contains("assistant") {
        Icon::CPU
    } else if lower.contains("forge")
        || lower.contains("corten-forge")
        || lower.contains("store")
        || lower.contains("software")
    {
        Icon::DOWNLOAD
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
                || (lower_id.is_empty()
                    && (lower_title.contains("corten terminal")
                        || lower_title.ends_with("terminal")
                        || lower_title.starts_with("terminal")))
        }
        ActiveApp::Files => {
            lower_id.contains("corten-files")
                || lower_id.contains("corten_files")
                || lower_id == "thunar"
                || lower_id == "dolphin"
                || lower_id == "nautilus"
                || lower_id.starts_with("org.gnome.nautilus")
                || lower_id.starts_with("org.kde.dolphin")
                || (lower_id.is_empty()
                    && (lower_title == "corten files"
                        || lower_title.starts_with("corten files")
                        || lower_title == "files"
                        || lower_title.starts_with("thunar")
                        || lower_title.starts_with("dolphin")))
        }
        ActiveApp::Settings => {
            lower_id.contains("corten-settings")
                || lower_id.contains("corten_settings")
                || lower_id == "gnome-control-center"
                || lower_id == "systemsettings"
                || lower_id.starts_with("org.gnome.settings")
                || (lower_id.is_empty()
                    && (lower_title == "corten settings"
                        || lower_title.starts_with("corten settings")
                        || lower_title == "settings"
                        || lower_title == "system settings"))
        }
        ActiveApp::Browser => {
            lower_id.contains("firefox")
                || lower_id.contains("chrome")
                || lower_id.contains("brave")
                || lower_id.contains("chromium")
                || lower_id.contains("zen")
                || lower_id.contains("opera")
                || (lower_id.is_empty()
                    && (lower_title.contains("firefox")
                        || lower_title.contains("chrome")
                        || lower_title.contains("brave")))
        }
        ActiveApp::Editor => {
            lower_id.contains("code")
                || lower_id.contains("antigravity")
                || lower_id == "kate"
                || lower_id == "gedit"
                || lower_id == "nvim"
                || lower_id == "zed"
                || (lower_id.is_empty()
                    && (lower_title.contains("visual studio code")
                        || lower_title.contains("antigravity")))
        }
        ActiveApp::Ai => {
            lower_id.contains("corten-ai")
                || lower_id.contains("corten_ai")
                || lower_id.contains("corten-assistant")
                || (lower_id.is_empty()
                    && (lower_title.contains("corten ai") || lower_title.starts_with("corten ai")))
        }
        ActiveApp::Forge => {
            lower_id.contains("corten-forge")
                || lower_id.contains("corten_forge")
                || lower_id.contains("corten-store")
                || (lower_id.is_empty()
                    && (lower_title.contains("corten forge")
                        || lower_title.starts_with("corten forge")))
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
        "corten-forge" | "forge" | "store" | "software" => ActiveApp::Forge,
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
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
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
    items
        .into_iter()
        .take(5)
        .map(|(n, s, p, _)| (n, s, p))
        .collect()
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
    fn update_preview(&mut self, message: Message) -> Task<Message> {
        <Self as Application>::update(self, message)
    }

    fn view_preview(&self) -> Element<'_, Message> {
        let fg = container(<Self as Application>::view(self))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(20)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Bottom);

        let wallpaper_path = PathBuf::from("assets/wallpapers/corten_dark_landscape.jpg");
        if wallpaper_path.exists() {
            let bg = image(image::Handle::from_path(wallpaper_path))
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover);
            iced::widget::stack![bg, fg].into()
        } else {
            fg.style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.07, 0.08, 0.10,
                ))),
                ..Default::default()
            })
            .into()
        }
    }

    fn subscription_preview(&self) -> Subscription<Message> {
        <Self as Application>::subscription(self)
    }

    fn calculate_dock_width(&self) -> u32 {
        // 1. Left Island: Launcher + App Drawer + dynamic workspace pills
        let mut ws_set = std::collections::BTreeSet::from([1usize, 2usize, self.active_workspace]);
        for &occ in &self.occupied_workspaces {
            let occ_u = occ as usize;
            if (1..=9).contains(&occ_u) {
                ws_set.insert(occ_u);
            }
        }
        let ws_w = 24 + (ws_set.len().saturating_sub(1) as u32) * 16 + 16;
        let left_island_w = 16 + 44 + 40 + ws_w + 16;

        // 2. Center Island: Corten system apps | pinned third-party + running apps
        let pinned_count = self.config.pinned_apps.len() as u32;
        let user_pinned = self
            .config
            .pinned_apps
            .iter()
            .filter(|p| !is_corten_system_app(p))
            .count() as u32;
        let mut seen = std::collections::HashSet::new();
        let mut dyn_unpinned_count = 0u32;
        for (_, app_id, _title, _, _, _) in &self.compositor_windows {
            let effective_id = if !app_id.trim().is_empty() {
                app_id.as_str()
            } else if !_title.trim().is_empty() {
                _title.as_str()
            } else {
                continue;
            };
            let is_pinned = self
                .config
                .pinned_apps
                .iter()
                .any(|pin| is_matching_pinned(effective_id, pin));
            if !is_pinned && seen.insert(effective_id.to_string()) {
                dyn_unpinned_count += 1;
            }
        }
        let total_app_slots = pinned_count + dyn_unpinned_count;
        let has_user_group = user_pinned + dyn_unpinned_count > 0;
        let center_divider_w = if has_user_group { 20 } else { 0 };
        let center_island_w = 28 + total_app_slots * 58 + center_divider_w;

        // 3. Right Island: Tray + Volume + Wi-Fi (+ battery) + Clock
        let tray_w = (self.tray_items.len() as u32) * 32;
        let battery_w = if self.battery_status.is_some() { 60 } else { 0 };
        let right_island_w = 28 + 136 + battery_w + tray_w;

        // Two 10px gaps between the 3 islands + generous slack so islands never squeeze
        let total_content_w = left_island_w + center_island_w + right_island_w + 64;

        let min_w = if self.show_app_drawer || self.active_window_previews.is_some() {
            720
        } else {
            580
        };
        total_content_w.clamp(min_w, 1800)
    }

    fn close_all_popovers(&mut self) {
        self.show_control_center = false;
        self.show_power_menu = false;
        self.show_calendar = false;
        self.show_clipboard_modal = false;
        self.show_notification_center = false;
        self.show_downloads_stack = false;
        self.show_mini_task_manager = false;
        self.show_net_speed_details = false;
        self.show_desktop_context_hub = false;
        self.show_app_drawer = false;
        self.show_wifi_modal = false;
        self.dock_context_menu = None;
        self.active_window_previews = None;
        self.pending_power_action = None;
    }

    fn layer_config_task(&mut self) -> Task<Message> {
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
            || self.show_desktop_context_hub
            || self.show_app_drawer;

        let height = if !self.panel_visible {
            1
        } else if popover_open {
            560
        } else if self.osd_state.is_some() {
            138
        } else {
            82
        };

        let target = (dynamic_width, height);
        if self.last_layer_size == target {
            return Task::none();
        }
        self.last_layer_size = target;
        Task::done(Message::LayerAction(LayershellCustomActions::SizeChange(
            target,
        )))
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
        let night_light_enabled = corten_config::ConfigManager::<
            corten_config::DisplaySettingsConfig,
        >::load_or_default("displays")
        .map(|c| c.get().night_light)
        .unwrap_or(false);

        let (active_patina, custom_accent, theme_mode, language) =
            corten_config::ConfigManager::<corten_config::AppearanceConfig>::load_or_default(
                "appearance",
            )
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let a = c.get().accent_color.clone();
                let m = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                let l = Language::from_code(&c.get().language);
                (p, a, m, l)
            })
            .unwrap_or_else(|_| {
                (
                    PatinaVariant::Rust,
                    String::new(),
                    ThemeMode::Dark,
                    Language::detect_system(),
                )
            });

        let mut app = Self {
            config_manager: flags.config_manager,
            config,
            _config_watcher: watcher,
            ipc_rx,
            theme_mode,
            active_patina,
            custom_accent,
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
            show_app_drawer: false,
            drawer_search: String::new(),
            drawer_category: "all".into(),
            drawer_apps: Vec::new(),
            drawer_loading: false,
            drawer_last_loaded: None,
            drawer_transition: Transition::completed(Motion::DURATION_EMPHASIZED_DECELERATE),
            popover_transition: Transition::new(Motion::DURATION_EMPHASIZED_DECELERATE),
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
            clipboard_history: load_pinned_clipboard(),
            pinned_clipboard: load_pinned_clipboard(),
            clipboard_search: String::new(),
            clipboard_filter: "all".into(),
            osd_state: None,
            show_notification_center: false,
            notification_history: vec![(
                "Corten Masaüstü".into(),
                "Sistem hazır ve optimize durumda.".into(),
                "Şimdi".into(),
            )],
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
            last_layer_size: (0, 0),
            hovered_dock_item: None,
            launch_bounce: None,
            eq_tick: 0,
            osd_display_level: 50.0,
            last_tick_sec: 0,
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
                    async {
                        tokio::task::spawn_blocking(corten_services::network::scan_wifi)
                            .await
                            .unwrap_or_default()
                    },
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
        let palette = Palette::for_mode(self.theme_mode)
            .with_patina(self.active_patina)
            .with_custom_accent(&self.custom_accent);
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: palette.text_primary.to_iced(),
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen().map(Message::IcedEvent);
        // Battery-friendly cadence: 1000ms idle tick (instead of 500ms), 500ms only when OSD is actively fading
        let tick_ms = if self.osd_state.is_some() { 500 } else { 1000 };
        let timer = iced::time::every(Duration::from_millis(tick_ms)).map(|_| Message::Tick);

        let needs_fast_animation = !self.popover_transition.is_complete()
            || !self.drawer_transition.is_complete()
            || self.launch_bounce.is_some()
            || self
                .osd_state
                .as_ref()
                .is_some_and(|(_, lvl, _)| (self.osd_display_level - *lvl as f32).abs() > 0.2);
        let needs_vu_animation = self.show_control_center
            && (self
                .media_players
                .iter()
                .any(|p| p.playback_status == corten_services::mpris::PlaybackStatus::Playing)
                || !self.app_streams.is_empty()
                || self.audio_vu_level.0 > 0.01
                || self.audio_vu_level.1 > 0.01);

        let mut subs = vec![events, timer];
        if needs_fast_animation {
            subs.push(iced::time::every(Duration::from_millis(16)).map(|_| Message::AnimationTick));
        } else if needs_vu_animation {
            subs.push(iced::time::every(Duration::from_millis(33)).map(|_| Message::AnimationTick));
        }

        // Standard UNIX domain event stream subscription (Hyprland / Sway model)
        let event_sock_path = corten_ipc::event_socket_path();
        let event_stream = futures::stream::unfold(
            None,
            move |mut opt_lines: Option<
                tokio::io::Lines<tokio::io::BufReader<tokio::net::UnixStream>>,
            >| {
                let sock_path = event_sock_path.clone();
                async move {
                    loop {
                        if opt_lines.is_none() {
                            match tokio::net::UnixStream::connect(&sock_path).await {
                                Ok(s) => {
                                    use tokio::io::AsyncBufReadExt;
                                    opt_lines = Some(tokio::io::BufReader::new(s).lines());
                                }
                                _ => {
                                    tokio::time::sleep(Duration::from_millis(300)).await;
                                    continue;
                                }
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
            },
        );
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
                | Message::ToggleAppDrawer
                | Message::OpenDockContextMenu(_)
                | Message::ToggleClipboardModal
                | Message::ToggleNotificationCenter
                | Message::ToggleDownloadsStack
                | Message::ToggleMiniTaskManager
                | Message::ToggleNetSpeedDetails
                | Message::ToggleWindowPreviews(_)
        ) {
            self.popover_transition.restart();
        }
        match message {
            Message::AnimationTick => {
                self.popover_transition.tick(16);
                self.drawer_transition.tick(16);
                if let Some((_, target_lvl, _)) = &self.osd_state {
                    self.osd_display_level =
                        Transition::smooth_lerp(self.osd_display_level, *target_lvl as f32, 0.28);
                }
                if let Some((b_app, ticks)) = self.launch_bounce {
                    if ticks > 1 {
                        self.launch_bounce = Some((b_app, ticks - 1));
                    } else {
                        self.launch_bounce = None;
                    }
                }
                let is_audio_playing =
                    self.media_players.iter().any(|p| {
                        p.playback_status == corten_services::mpris::PlaybackStatus::Playing
                    }) || !self.app_streams.is_empty();
                if is_audio_playing {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    self.eq_tick = ((now / 180) % 4) as u8;
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
                    if let Ok(app_cfg) = corten_config::ConfigManager::<
                        corten_config::AppearanceConfig,
                    >::load_or_default("appearance")
                    {
                        self.language = Language::from_code(&app_cfg.get().language);
                        self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                        self.custom_accent = app_cfg.get().accent_color.clone();
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
                            if self.show_clipboard_modal
                                && let Some(txt) = SystemInfo::read_clipboard_text()
                                && self.clipboard_history.first() != Some(&txt)
                            {
                                self.clipboard_history.retain(|item| item != &txt);
                                self.clipboard_history.insert(0, txt);
                                if self.clipboard_history.len() > 30 {
                                    self.clipboard_history.truncate(30);
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
                                stripped.trim().parse::<u8>().unwrap_or_else(|_| {
                                    self.audio_status.map(|a| a.0).unwrap_or(50)
                                })
                            } else {
                                self.audio_status.map(|a| a.0).unwrap_or(50)
                            };
                            self.osd_state = Some((OsdKind::Volume, vol, 3));
                        }
                        cmd if cmd.starts_with("osd:brightness") => {
                            let bri = if let Some(stripped) = cmd.strip_prefix("osd:brightness:") {
                                stripped
                                    .trim()
                                    .parse::<u8>()
                                    .unwrap_or_else(|_| SystemInfo::get_brightness_percent())
                            } else {
                                SystemInfo::get_brightness_percent()
                            };
                            self.osd_state = Some((OsdKind::Brightness, bri, 3));
                        }
                        "osd:switcher" => {
                            let (_, _, windows) = SystemInfo::get_compositor_state_for_output(
                                self.output_name.as_deref(),
                            );
                            let list: Vec<(String, String, bool)> = windows
                                .into_iter()
                                .map(|(_, app_id, title, _, is_active, _)| {
                                    (app_id, title, is_active)
                                })
                                .collect();
                            if !list.is_empty() {
                                self.osd_state = Some((OsdKind::Switcher(list), 100, 3));
                            }
                        }
                        cmd if cmd.starts_with("osd:toast:") => {
                            let msg = cmd.trim_start_matches("osd:toast:").to_string();
                            self.osd_state = Some((OsdKind::Toast(msg), 100, 5));
                        }
                        cmd if cmd.starts_with("toast:") => {
                            let msg = cmd.trim_start_matches("toast:").to_string();
                            self.osd_state = Some((OsdKind::Toast(msg), 100, 5));
                        }
                        cmd if cmd.starts_with("osd:workspace:") => {
                            if let Ok(ws) = cmd
                                .trim_start_matches("osd:workspace:")
                                .trim()
                                .parse::<usize>()
                            {
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
                                let _ = std::process::Command::new("pkill")
                                    .args(["-TERM", "-x", "corten-launcher"])
                                    .spawn();
                            } else {
                                let bin = find_binary("corten-launcher");
                                if let Err(error) =
                                    std::process::Command::new(bin).arg("--start-menu").spawn()
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
                if let Some((active_ws, occupied_ws, windows)) =
                    SystemInfo::try_get_compositor_state_for_output(self.output_name.as_deref())
                {
                    if active_ws > 0 && self.active_workspace != active_ws {
                        self.active_workspace = active_ws;
                        self.osd_state = Some((OsdKind::Workspace(active_ws), active_ws as u8, 3));
                    }
                    self.occupied_workspaces = occupied_ws;
                    self.compositor_windows = windows;
                    self.running_apps =
                        SystemInfo::get_running_apps_from_windows(&self.compositor_windows);
                    self.other_apps =
                        SystemInfo::get_other_running_apps_from_windows(&self.compositor_windows);
                }
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
                // Guard periodic system probes so sub-second OSD ticks never double-fire fork/exec work
                let is_new_sec = current_secs != self.last_tick_sec;
                if is_new_sec {
                    self.last_tick_sec = current_secs;
                }

                let (time, date) =
                    SystemInfo::get_datetime(&self.config.time_format, &self.config.date_format);
                self.current_time = time;
                self.current_date = date;
                if let Some((active_ws, occupied_ws, windows)) =
                    SystemInfo::try_get_compositor_state_for_output(self.output_name.as_deref())
                {
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
                    self.running_apps =
                        SystemInfo::get_running_apps_from_windows(&self.compositor_windows);
                    self.other_apps =
                        SystemInfo::get_other_running_apps_from_windows(&self.compositor_windows);
                }
                let mut tick_tasks: Vec<Task<Message>> = Vec::new();
                let mut needs_layer_sync = false;
                let current_w = self.calculate_dock_width();
                if current_w != self.last_dock_width {
                    self.last_dock_width = current_w;
                    needs_layer_sync = true;
                }

                // Smart On-Demand & Battery-Friendly Cadence Polling (Zero-Excess-Fork)
                if is_new_sec
                    && (self.show_volume_slider
                        || self.show_control_center
                        || current_secs.is_multiple_of(5))
                {
                    self.audio_status = pipewire::get_volume();
                }
                if is_new_sec && (self.show_control_center || current_secs.is_multiple_of(5)) {
                    self.mic_active = SystemInfo::is_mic_active();
                    self.network_status = SystemInfo::get_network_details();
                    self.battery_status = SystemInfo::get_battery_info();
                }
                if is_new_sec && self.show_control_center && current_secs.is_multiple_of(3) {
                    tick_tasks.push(Task::perform(
                        async {
                            tokio::task::spawn_blocking(move || {
                                corten_services::mpris::list_players().unwrap_or_default()
                            })
                            .await
                            .unwrap_or_default()
                        },
                        Message::MediaPlayersUpdated,
                    ));
                }
                if is_new_sec
                    && (self.show_volume_slider || self.show_control_center)
                    && current_secs.is_multiple_of(3)
                {
                    self.audio_sinks = pipewire::list_sinks();
                }
                if is_new_sec && self.show_control_center && current_secs.is_multiple_of(4) {
                    self.listening_ports = SystemInfo::get_listening_dev_ports();
                    self.app_streams = corten_services::pipewire::list_app_streams();
                }
                if is_new_sec && self.show_mini_task_manager && current_secs.is_multiple_of(3) {
                    self.top_processes = SystemInfo::get_top_memory_processes(6);
                }
                if is_new_sec
                    && (self.show_net_speed_details
                        || self.show_control_center
                        || current_secs.is_multiple_of(2))
                {
                    let (_rx_s, _tx_s, rx_fmt, tx_fmt) = self.net_speed_tracker.sample_speed();
                    self.net_speed_display = (rx_fmt, tx_fmt);
                }
                if is_new_sec && current_secs.is_multiple_of(5) {
                    self.tray_items = SystemInfo::get_active_tray_items();
                }

                // Gentle trash polling (every 10 seconds or on-demand)
                if is_new_sec && current_secs.is_multiple_of(10) {
                    self.trash_item_count = get_trash_count();
                }

                if self.config.autohide && self.config.autohide_mode == "dodge" {
                    let has_windows_on_ws = self
                        .compositor_windows
                        .iter()
                        .any(|(_, _, _, ws, _, _)| *ws as usize == self.active_workspace);
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
                        || self.show_desktop_context_hub
                        || self.show_app_drawer;
                    let target_visible = !has_windows_on_ws || popover_open;
                    if self.panel_visible != target_visible {
                        self.panel_visible = target_visible;
                        needs_layer_sync = true;
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
                        if current_secs.is_multiple_of(interval)
                            && !self.running_custom_scripts.contains(&idx)
                        {
                            let cmd_str = script.command.clone();
                            if !is_custom_command_safe(&cmd_str) {
                                tracing::warn!(
                                    "Security: Blocked dangerous custom script '{}': {}",
                                    script.name,
                                    cmd_str
                                );
                                if idx < self.custom_script_outputs.len() {
                                    self.custom_script_outputs[idx].text =
                                        "Güvenlik Engeli".to_string();
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
                                        Err(_) => "Zaman Aşımı".to_string(),
                                    }
                                },
                                move |res| Message::CustomScriptResult(idx, res),
                            ));
                        }
                    }
                }

                // Removable USB / disk auto-detection & notification (every 5 seconds)
                if is_new_sec
                    && current_secs.is_multiple_of(5)
                    && let Ok(devices) = corten_services::udisks2::list_devices()
                {
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
                                format!("{} ({:.1} GB) bağlandı: {}", label, size_gb, mp.display())
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

                            self.notification_history
                                .insert(0, ("Harici Disk Algılandı".into(), body, "Şimdi".into()));
                        }
                    }
                    let current_paths: std::collections::HashSet<String> =
                        removable.into_iter().map(|d| d.object_path).collect();
                    self.known_block_devices
                        .retain(|p| current_paths.contains(p));
                }

                // Critical system file integrity watcher / tripwire (every 10 seconds)
                if is_new_sec
                    && current_secs.is_multiple_of(10)
                    && let Some(alert) = check_system_integrity(&mut self.sentinel_mtimes)
                {
                    self.osd_state = Some((OsdKind::Toast(alert.clone()), 100, 4));
                    self.notification_history
                        .insert(0, ("Güvenlik Bekçisi".into(), alert, "Şimdi".into()));
                }

                // Clipboard history polling (gentle on-demand & 5s periodic, zero-excess-fork)
                if is_new_sec
                    && (self.show_clipboard_modal || current_secs.is_multiple_of(5))
                    && let Some(txt) = SystemInfo::read_clipboard_text()
                    && self.clipboard_history.first() != Some(&txt)
                {
                    self.clipboard_history.retain(|item| item != &txt);
                    self.clipboard_history.insert(0, txt);
                    if self.clipboard_history.len() > 30 {
                        self.clipboard_history.truncate(30);
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
                tick_tasks.extend(custom_script_tasks);
                if needs_layer_sync
                    || current_w != self.last_dock_width
                    || self.osd_state.is_some() != had_osd
                {
                    self.last_dock_width = current_w;
                    tick_tasks.push(self.layer_config_task());
                }
                if !tick_tasks.is_empty() {
                    return Task::batch(tick_tasks);
                }
            }
            Message::CustomScriptResult(idx, raw_output) => {
                self.running_custom_scripts.remove(&idx);
                if idx < self.custom_script_outputs.len() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw_output) {
                        let text = json
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&raw_output)
                            .trim()
                            .chars()
                            .take(48)
                            .collect::<String>();
                        let tooltip = json
                            .get("tooltip")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let class = json
                            .get("class")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let percentage = json
                            .get("percentage")
                            .and_then(|v| v.as_u64())
                            .map(|p| p as u8);
                        self.custom_script_outputs[idx].text = text;
                        self.custom_script_outputs[idx].tooltip = tooltip;
                        self.custom_script_outputs[idx].class = class;
                        self.custom_script_outputs[idx].percentage = percentage;
                    } else {
                        let first_line = raw_output
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .chars()
                            .take(48)
                            .collect::<String>();
                        self.custom_script_outputs[idx].text = first_line;
                        self.custom_script_outputs[idx].tooltip = None;
                        self.custom_script_outputs[idx].class = None;
                        self.custom_script_outputs[idx].percentage = None;
                    }
                }
            }
            Message::ClickCustomScript(idx) => {
                if let Some(script) = self.config.custom_scripts.get(idx)
                    && let Some(ref cmd) = script.on_click
                    && is_custom_command_safe(cmd)
                {
                    let _ = std::process::Command::new("sh").args(["-c", cmd]).spawn();
                }
            }
            Message::RightClickCustomScript(idx) => {
                if let Some(script) = self.config.custom_scripts.get(idx)
                    && let Some(ref cmd) = script.on_right_click
                    && is_custom_command_safe(cmd)
                {
                    let _ = std::process::Command::new("sh").args(["-c", cmd]).spawn();
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
                self.launch_bounce = Some((app, 18));
                corten_services::acoustic::play(
                    corten_services::acoustic::AcousticCue::ToggleClick,
                );
                let existing_id =
                    self.compositor_windows
                        .iter()
                        .find_map(|(id, app_id, title, _, _, _)| {
                            if is_matching_window(app_id, title, app) {
                                Some(*id)
                            } else {
                                None
                            }
                        });
                if let Some(win_id) = existing_id
                    && SystemInfo::focus_window(win_id)
                {
                    return Task::none();
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
                        if !spawned
                            && std::process::Command::new("konsole")
                                .args(["--profile", "Corten"])
                                .spawn()
                                .is_ok()
                        {
                            spawned = true;
                        }
                        if !spawned
                            && std::process::Command::new(
                                "/var/lib/flatpak/exports/bin/org.kde.konsole",
                            )
                            .args(["--profile", "Corten"])
                            .spawn()
                            .is_ok()
                        {
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
                    ActiveApp::Forge => {
                        let bin = find_binary("corten-forge");
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
                        self.osd_state =
                            Some((OsdKind::Toast("Dock ayarı güncellendi".to_string()), 1, 3));
                    }
                    DockAction::MoveLeft(idx) => {
                        if idx > 0 && idx < self.config.pinned_apps.len() {
                            self.config.pinned_apps.swap(idx, idx - 1);
                            let _ = self.config_manager.update(self.config.clone());
                            self.osd_state = Some((
                                OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()),
                                100,
                                3,
                            ));
                        }
                    }
                    DockAction::MoveRight(idx) => {
                        if idx + 1 < self.config.pinned_apps.len() {
                            self.config.pinned_apps.swap(idx, idx + 1);
                            let _ = self.config_manager.update(self.config.clone());
                            self.osd_state = Some((
                                OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()),
                                100,
                                3,
                            ));
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
                    self.osd_state = Some((
                        OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()),
                        100,
                        3,
                    ));
                    return self.layer_config_task();
                }
            }
            Message::MoveDockItemRight(idx) => {
                if idx + 1 < self.config.pinned_apps.len() {
                    self.config.pinned_apps.swap(idx, idx + 1);
                    let _ = self.config_manager.update(self.config.clone());
                    self.osd_state = Some((
                        OsdKind::Toast("🔄 Dock sırası güncellendi".to_string()),
                        100,
                        3,
                    ));
                    return self.layer_config_task();
                }
            }
            Message::DockHover(hover_app_opt) => {
                if self.hovered_dock_item != hover_app_opt {
                    self.hovered_dock_item = hover_app_opt;
                }
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
                    "Akıllı Gizlenme (Dodge) Aktif".to_string()
                } else {
                    "Dock Daima Görünür".to_string()
                };
                self.osd_state = Some((OsdKind::Toast(toast), 100, 3));
                return self.layer_config_task();
            }
            Message::PinAppToDock(app_id) => {
                if !self.config.pinned_apps.contains(&app_id) {
                    self.config.pinned_apps.push(app_id);
                    let _ = self.config_manager.update(self.config.clone());
                    self.osd_state = Some((
                        OsdKind::Toast("Uygulama Dock'a sabitlendi".to_string()),
                        100,
                        3,
                    ));
                    return self.layer_config_task();
                }
            }
            Message::UnpinAppFromDock(app_id) => {
                self.config.pinned_apps.retain(|a| a != &app_id);
                let _ = self.config_manager.update(self.config.clone());
                self.osd_state = Some((
                    OsdKind::Toast("Uygulama Dock'tan kaldırıldı".to_string()),
                    100,
                    3,
                ));
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
                let will_open = !self.show_downloads_stack;
                self.close_all_popovers();
                self.show_downloads_stack = will_open;
                if will_open {
                    self.downloads_items = get_recent_downloads();
                }
                return self.layer_config_task();
            }
            Message::OpenFilePath(path) => {
                let _ = std::process::Command::new("xdg-open").arg(path).spawn();
                self.show_downloads_stack = false;
                return self.layer_config_task();
            }
            Message::OpenTrash => {
                let _ = std::process::Command::new("corten-files")
                    .arg("trash:///")
                    .spawn();
            }
            Message::EmptyTrash => {
                if let Ok(home) = std::env::var("HOME") {
                    // FreeDesktop Trash Spec: clear BOTH Trash/files and Trash/info off the UI thread.
                    let trash_root = PathBuf::from(home).join(".local/share/Trash");
                    std::thread::spawn(move || {
                        for sub in ["files", "info"] {
                            let dir = trash_root.join(sub);
                            let _ = std::fs::remove_dir_all(&dir);
                            let _ = std::fs::create_dir_all(&dir);
                        }
                    });
                    self.trash_item_count = 0;
                    self.osd_state = Some((OsdKind::Toast("Çöp kutusu boşaltıldı".into()), 100, 3));
                }
            }
            Message::OpenLauncher | Message::ToggleLauncherModal => {
                std::thread::spawn(|| {
                    if let Some(cmdline) = get_corten_launcher_cmdline() {
                        let _ = std::process::Command::new("pkill")
                            .args(["-TERM", "-x", "corten-launcher"])
                            .status();
                        if cmdline.contains("--dashboard") {
                            let bin = find_binary("corten-launcher");
                            let _ = std::process::Command::new(bin).arg("--start-menu").spawn();
                        }
                    } else {
                        let bin = find_binary("corten-launcher");
                        let _ = std::process::Command::new(bin).arg("--start-menu").spawn();
                    }
                });
            }
            Message::LauncherSearchChanged(query) => {
                self.launcher_search = query;
            }
            Message::LaunchCommand(cmd) => {
                self.show_launcher_modal = false;
                let _ = launch_fallback_chain(&cmd);
            }
            Message::OpenDashboard => {
                std::thread::spawn(|| {
                    if let Some(cmdline) = get_corten_launcher_cmdline() {
                        let _ = std::process::Command::new("pkill")
                            .args(["-TERM", "-x", "corten-launcher"])
                            .status();
                        if !cmdline.contains("--dashboard") {
                            let bin = find_binary("corten-launcher");
                            let _ = std::process::Command::new(bin).arg("--dashboard").spawn();
                        }
                    } else {
                        let bin = find_binary("corten-launcher");
                        let _ = std::process::Command::new(bin).arg("--dashboard").spawn();
                    }
                });
            }
            Message::ToggleAudioMute => {
                let _ = pipewire::toggle_mute("@DEFAULT_AUDIO_SINK@");
                self.audio_status = pipewire::get_volume();
                self.mic_active = SystemInfo::is_mic_active();
            }
            Message::ToggleAudioSlider => {
                let will_open = !self.show_control_center;
                self.close_all_popovers();
                self.show_control_center = will_open;
                if will_open {
                    self.popover_transition.restart();
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
                let (prim, _) = patina.colors();
                let hex = format!(
                    "#{:02X}{:02X}{:02X}",
                    (prim.r * 255.0) as u8,
                    (prim.g * 255.0) as u8,
                    (prim.b * 255.0) as u8
                );
                self.custom_accent = hex.clone();
                if let Ok(mut app_mgr) =
                    ConfigManager::<corten_config::AppearanceConfig>::load_or_default("appearance")
                {
                    let mut cfg = app_mgr.get().clone();
                    cfg.patina = patina.code().to_string();
                    cfg.accent_color = hex;
                    let _ = app_mgr.update(cfg);
                }
            }
            Message::ToggleDesktopContextHub => {
                let will_open = !self.show_desktop_context_hub;
                self.close_all_popovers();
                self.show_desktop_context_hub = will_open;
                if will_open {
                    self.popover_transition.restart();
                }
                return self.layer_config_task();
            }
            Message::CloseDesktopContextHub => {
                self.show_desktop_context_hub = false;
                return self.layer_config_task();
            }
            Message::TriggerWallpaperCycle => {
                let _ = Command::new("pkill")
                    .args(["-USR1", "-x", "corten-wallpaper"])
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
                    self.dock_context_menu = None;
                    self.active_window_previews = None;
                    self.show_net_speed_details = false;
                    self.show_downloads_stack = false;
                    self.show_mini_task_manager = false;
                    self.show_desktop_context_hub = false;
                    self.popover_transition.restart();
                }
                return self.layer_config_task();
            }
            Message::ScanWifi => {
                return Task::perform(
                    async {
                        tokio::task::spawn_blocking(corten_services::network::scan_wifi)
                            .await
                            .unwrap_or_default()
                    },
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
                corten_services::acoustic::play(
                    corten_services::acoustic::AcousticCue::ToggleClick,
                );
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
                corten_services::acoustic::play(
                    corten_services::acoustic::AcousticCue::ToggleClick,
                );
                let _ = corten_services::power_profiles::set_active_profile(&profile);
                self.active_power_profile = profile;
            }
            Message::ToggleDnd => {
                self.dnd_enabled = !self.dnd_enabled;
                corten_services::acoustic::play(
                    corten_services::acoustic::AcousticCue::ToggleClick,
                );
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
                    "Rahatsız Etme: Açık".to_string()
                } else {
                    "Rahatsız Etme: Kapalı".to_string()
                };
                self.osd_state = Some((OsdKind::Toast(toast), 100, 3));
            }
            Message::ToggleNightLight => {
                self.night_light_enabled = !self.night_light_enabled;
                corten_services::acoustic::play(
                    corten_services::acoustic::AcousticCue::ToggleClick,
                );
                if let Ok(mut cm) = corten_config::ConfigManager::<
                    corten_config::DisplaySettingsConfig,
                >::load_or_default("displays")
                {
                    let mut cfg = cm.get().clone();
                    cfg.night_light = self.night_light_enabled;
                    let _ = cm.update(cfg);
                }
                let toast = if self.night_light_enabled {
                    "Gece Işığı: Açık".to_string()
                } else {
                    "Gece Işığı: Kapalı".to_string()
                };
                self.osd_state = Some((OsdKind::Toast(toast), 100, 3));
            }
            Message::ToggleControlCenter => {
                let will_open = !self.show_control_center;
                self.close_all_popovers();
                self.show_control_center = will_open;
                if self.show_control_center {
                    self.popover_transition.restart();
                    return Task::batch(vec![
                        self.layer_config_task(),
                        Task::perform(
                            async {
                                tokio::task::spawn_blocking(move || {
                                    corten_services::mpris::list_players().unwrap_or_default()
                                })
                                .await
                                .unwrap_or_default()
                            },
                            Message::MediaPlayersUpdated,
                        ),
                    ]);
                }
                return self.layer_config_task();
            }
            Message::MprisPlayPause(ref bus) => {
                let bus_c = bus.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let _ = corten_services::mpris::play_pause(&bus_c);
                            corten_services::mpris::list_players().unwrap_or_default()
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::MediaPlayersUpdated,
                );
            }
            Message::MprisNext(ref bus) => {
                let bus_c = bus.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let _ = corten_services::mpris::next(&bus_c);
                            corten_services::mpris::list_players().unwrap_or_default()
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::MediaPlayersUpdated,
                );
            }
            Message::MprisPrevious(ref bus) => {
                let bus_c = bus.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let _ = corten_services::mpris::previous(&bus_c);
                            corten_services::mpris::list_players().unwrap_or_default()
                        })
                        .await
                        .unwrap_or_default()
                    },
                    Message::MediaPlayersUpdated,
                );
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
            Message::RouteAppStreamToNextSink(id) => {
                if self.audio_sinks.is_empty() {
                    self.audio_sinks = pipewire::list_sinks();
                }
                if !self.audio_sinks.is_empty() {
                    let current_sink_id = self
                        .app_streams
                        .iter()
                        .find(|s| s.id == id)
                        .and_then(|s| s.target_sink_id)
                        .or_else(|| self.audio_sinks.iter().find(|k| k.is_default).map(|k| k.id));
                    let next_sink = if let Some(cur_id) = current_sink_id {
                        let pos = self
                            .audio_sinks
                            .iter()
                            .position(|k| k.id == cur_id)
                            .unwrap_or(0);
                        &self.audio_sinks[(pos + 1) % self.audio_sinks.len()]
                    } else {
                        &self.audio_sinks[0]
                    };
                    let sink_id = next_sink.id;
                    let sink_name = next_sink.name.clone();
                    let _ = corten_services::pipewire::move_stream_to_sink(id, sink_id);
                    if let Some(s) = self.app_streams.iter_mut().find(|s| s.id == id) {
                        s.target_sink_id = Some(sink_id);
                    }
                    self.osd_state = Some((
                        OsdKind::Toast(format!("Çıkış yönlendirildi: {}", sink_name)),
                        100,
                        3,
                    ));
                }
            }
            Message::TogglePowerMenu => {
                let will_open = !self.show_power_menu;
                self.close_all_popovers();
                self.show_power_menu = will_open;
                return self.layer_config_task();
            }
            Message::ToggleCalendar => {
                let will_open = !self.show_calendar;
                self.close_all_popovers();
                self.show_calendar = will_open;
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
                    self.osd_state = Some((
                        OsdKind::Toast("Ekran görüntüsü panoya kopyalandı".to_string()),
                        100,
                        3,
                    ));
                }
                Err(error) => {
                    warn!(%error, "Screenshot portal unavailable; using native fallback");
                    SystemInfo::take_screenshot_fallback(region);
                    self.osd_state = Some((
                        OsdKind::Toast("Ekran görüntüsü kaydedildi".to_string()),
                        100,
                        3,
                    ));
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
                corten_services::acoustic::play(
                    corten_services::acoustic::AcousticCue::ToggleClick,
                );
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
                        if let Ok(conn) = zbus::Connection::session().await
                            && let Ok(proxy) = zbus::Proxy::new(
                                &conn,
                                bus_name.as_str(),
                                obj_path.as_str(),
                                "org.kde.StatusNotifierItem",
                            )
                            .await
                        {
                            let _ = proxy.call::<_, _, ()>("Activate", &(0i32, 0i32)).await;
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
                        if let Ok(conn) = zbus::Connection::session().await
                            && let Ok(proxy) = zbus::Proxy::new(
                                &conn,
                                bus_name.as_str(),
                                obj_path.as_str(),
                                "org.kde.StatusNotifierItem",
                            )
                            .await
                        {
                            let _ = proxy.call::<_, _, ()>("ContextMenu", &(0i32, 0i32)).await;
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
                        let lock_bin = std::env::current_exe()
                            .ok()
                            .and_then(|p| p.parent().map(|d| d.join("corten-lock")))
                            .filter(|p| p.exists())
                            .unwrap_or_else(|| std::path::PathBuf::from("corten-lock"));
                        if let Err(error) = std::process::Command::new(lock_bin).spawn() {
                            warn!(%error, "Failed to start lock screen");
                        }
                    }
                    SystemPowerAction::Suspend => {
                        if let Err(error) = std::process::Command::new("systemctl")
                            .arg("suspend")
                            .spawn()
                        {
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
                let will_open = !self.show_clipboard_modal;
                self.close_all_popovers();
                self.show_clipboard_modal = will_open;
                if will_open
                    && let Some(txt) = SystemInfo::read_clipboard_text()
                    && self.clipboard_history.first() != Some(&txt)
                {
                    self.clipboard_history.retain(|item| item != &txt);
                    self.clipboard_history.insert(0, txt);
                    if self.clipboard_history.len() > 30 {
                        self.clipboard_history.truncate(30);
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
            Message::TogglePinClipboardItem(txt) => {
                if let Some(pos) = self.pinned_clipboard.iter().position(|p| p == &txt) {
                    self.pinned_clipboard.remove(pos);
                } else {
                    self.pinned_clipboard.insert(0, txt);
                }
                save_pinned_clipboard(&self.pinned_clipboard);
            }
            Message::SetClipboardFilter(f) => {
                self.clipboard_filter = f;
            }
            Message::DeleteClipboardItem(idx) => {
                if idx < self.clipboard_history.len() {
                    let removed = self.clipboard_history.remove(idx);
                    self.pinned_clipboard.retain(|p| p != &removed);
                    save_pinned_clipboard(&self.pinned_clipboard);
                }
            }
            Message::ClearClipboardHistory => {
                self.clipboard_history = self.pinned_clipboard.clone();
            }
            Message::PickScreenColor => {
                self.show_control_center = false;
                let _ = Command::new("busctl")
                    .args([
                        "--user",
                        "call",
                        "org.corten.Compositor",
                        "/org/corten/Compositor",
                        "org.corten.Compositor",
                        "DispatchAction",
                        "s",
                        "color-picker",
                    ])
                    .spawn();
                return self.layer_config_task();
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
                let will_open = !self.show_notification_center;
                self.close_all_popovers();
                self.show_notification_center = will_open;
                return self.layer_config_task();
            }
            Message::ClearNotifications => {
                self.notification_history.clear();
            }
            Message::ToggleAppDrawer => {
                let will_open = !self.show_app_drawer;
                self.close_all_popovers();
                self.show_app_drawer = will_open;
                if will_open {
                    self.drawer_transition.restart();
                    self.drawer_search.clear();

                    let cache_stale = self
                        .drawer_last_loaded
                        .is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
                    if (self.drawer_apps.is_empty() || cache_stale) && !self.drawer_loading {
                        self.drawer_loading = true;
                        let scan_task = Task::perform(
                            async {
                                tokio::task::spawn_blocking(scan_installed_drawer_apps)
                                    .await
                                    .unwrap_or_default()
                            },
                            Message::DrawerAppsLoaded,
                        );
                        return Task::batch(vec![self.layer_config_task(), scan_task]);
                    }
                }
                return self.layer_config_task();
            }
            Message::DrawerAppsLoaded(apps) => {
                self.drawer_apps = apps;
                self.drawer_loading = false;
                self.drawer_last_loaded = Some(std::time::Instant::now());
            }
            Message::DrawerSearchChanged(q) => {
                self.drawer_search = q;
            }
            Message::DrawerCategorySelected(cat) => {
                self.drawer_category = cat;
            }
            Message::LaunchDrawerApp(exec) => {
                self.show_app_drawer = false;
                self.drawer_search.clear();
                launch_drawer_exec(&exec);
                return self.layer_config_task();
            }
            Message::IcedEvent(Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            })) if self.show_control_center
                || self.show_power_menu
                || self.show_calendar
                || self.show_clipboard_modal
                || self.show_notification_center
                || self.show_desktop_context_hub
                || self.show_app_drawer
                || self.dock_context_menu.is_some() =>
            {
                self.show_control_center = false;
                self.show_power_menu = false;
                self.show_calendar = false;
                self.show_clipboard_modal = false;
                self.show_notification_center = false;
                self.show_desktop_context_hub = false;
                self.show_app_drawer = false;
                self.dock_context_menu = None;
                self.pending_power_action = None;
                return self.layer_config_task();
            }
            Message::IcedEvent(_) | Message::LayerAction(_) | Message::NoOp => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode)
            .with_patina(self.active_patina)
            .with_custom_accent(&self.custom_accent);
        // Shared Material 3 emphasized-decelerate upward reveal for all dock popovers.
        let reveal_eased = self
            .popover_transition
            .emphasized_decelerate()
            .clamp(0.0, 1.0);
        let popover_alpha = (0.20 + 0.80 * reveal_eased).clamp(0.0, 1.0);
        let popover_lift_gap = Spacing::XS;
        let reveal_max_h = (48.0 + 572.0 * reveal_eased).round();

        // ==================== ISLAND 1 (LEFT): LAUNCHER, APP DRAWER & WORKSPACES ====================
        let island_style = move |_: &iced::Theme| {
            let mut bg = palette.surface_container_low().to_iced();
            bg.a = 0.94;
            container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::LG),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.34),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 14.0,
                },
                ..Default::default()
            }
        };

        let start_btn = button(
            container(
                image(image::Handle::from_bytes(LOGO_PNG.to_vec()))
                    .width(Length::Fixed(26.0))
                    .height(Length::Fixed(26.0)),
            )
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        )
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .padding(0)
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered | button::Status::Pressed => Some(iced::Background::Color(
                    palette.surface_container_high().to_iced(),
                )),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            }
        })
        .on_press(Message::OpenLauncher);

        let app_drawer_active = self.show_app_drawer;
        let app_drawer_btn = button(
            container(text(Icon::APP_WINDOW).font(LUCIDE_FONT).size(16.0).color(
                if app_drawer_active {
                    palette.on_primary_container().to_iced()
                } else {
                    palette.text_muted.to_iced()
                },
            ))
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
        )
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(36.0))
        .padding(0)
        .style(move |_, status| {
            let bg = if app_drawer_active {
                Some(iced::Background::Color(
                    palette.primary_container().to_iced(),
                ))
            } else if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                Some(iced::Background::Color(
                    palette.surface_container_high().to_iced(),
                ))
            } else {
                None
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::MD),
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleAppDrawer);

        let t = self.language.t();
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

        // Dynamic GNOME/COSMIC-style workspace pills (elongated primary pill for active, crisp dot for inactive)
        let mut spaces_row = row![].spacing(6).align_y(Alignment::Center);
        for ws_num in visible_workspaces {
            let is_current = self.active_workspace == ws_num;
            let is_occupied = self.occupied_workspaces.contains(&(ws_num as u32));
            let pill_w = if is_current { 22.0 } else { 8.0 };
            let pill_h = 8.0;

            let ws_dot = container(row![])
                .width(Length::Fixed(pill_w))
                .height(Length::Fixed(pill_h))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(if is_current {
                        palette.primary.to_iced()
                    } else if is_occupied {
                        palette.text_muted.to_iced()
                    } else {
                        palette.surface_container_highest().to_iced()
                    })),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(Radius::FULL),
                    },
                    ..Default::default()
                });

            let ws_btn = button(
                container(ws_dot)
                    .height(Length::Fixed(28.0))
                    .center_y(Length::Fill),
            )
            .padding([0, 2])
            .style(|_, _| button::Style {
                background: None,
                ..Default::default()
            })
            .on_press(Message::SwitchWorkspace(ws_num));
            spaces_row = spaces_row.push(ws_btn);
        }

        let make_divider = || {
            container(row![])
                .width(Length::Fixed(1.0))
                .height(Length::Fixed(24.0))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(palette.outline_variant().to_iced())),
                    ..Default::default()
                })
        };

        let mut left_items = row![].spacing(4).align_y(Alignment::Center).padding([0, 8]);
        if self.config.show_launcher_button {
            left_items = left_items.push(start_btn);
        }
        left_items = left_items
            .push(app_drawer_btn)
            .push(container(spaces_row).padding([0, 6]));

        let left_island = container(left_items)
            .width(Length::Shrink)
            .height(Length::Fixed(56.0))
            .center_y(Length::Fixed(56.0))
            .style(island_style);

        // ==================== SECTION 2: SYSTEM APPS (FIXED) + PINNED/RUNNING APPS ====================
        let mut sys_row = row![].spacing(6).align_y(Alignment::Center);
        let mut user_row = row![].spacing(6).align_y(Alignment::Center);
        let mut user_count = 0usize;
        let hovered_pinned_idx = self
            .hovered_dock_item
            .as_ref()
            .and_then(|h| self.config.pinned_apps.iter().position(|p| p == h));
        let is_build_active = self.top_processes.iter().any(|p| {
            let n = p.name.to_lowercase();
            n.contains("cargo")
                || n.contains("rustc")
                || n.contains("make")
                || n.contains("gcc")
                || n.contains("clang")
                || n.contains("npm")
        }) || self
            .compositor_windows
            .iter()
            .any(|(_, _, title, _, _, _)| {
                let t = title.to_lowercase();
                t.contains("cargo ") || t.contains("compiling") || t.contains("building")
            });

        for (idx, app_name) in self.config.pinned_apps.iter().enumerate() {
            let app = match app_name.as_str() {
                "terminal" | "corten-terminal" => ActiveApp::Terminal,
                "files" | "corten-files" => ActiveApp::Files,
                "settings" | "corten-settings" => ActiveApp::Settings,
                "editor" => ActiveApp::Editor,
                "browser" => ActiveApp::Browser,
                "corten-ai" | "ai" => ActiveApp::Ai,
                "corten-forge" | "forge" | "store" | "software" => ActiveApp::Forge,
                _ => ActiveApp::Terminal,
            };

            let proc_running = match app {
                ActiveApp::Terminal => self.running_apps.0,
                ActiveApp::Files => self.running_apps.1,
                ActiveApp::Browser => self.running_apps.2,
                ActiveApp::Editor => self.running_apps.3,
                ActiveApp::Settings => false,
                ActiveApp::Ai => false,
                ActiveApp::Forge => false,
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

            // Subtle hover magnification inside a fixed 48x48 slot (~70% vertical fill ratio)
            let hover_dist = hovered_pinned_idx.map(|h| h.abs_diff(idx));
            let (icon_size, glyph_size, base_lift) = match hover_dist {
                Some(0) => (40.0, 28.0, 2.0),
                Some(1) => (38.0, 26.0, 1.0),
                _ => (36.0, 24.0, 0.0),
            };
            let bounce_lift = match self.launch_bounce {
                Some((b_app, ticks)) if b_app == app => {
                    (ticks as f32 * 0.65).sin().abs() * (ticks as f32 / 18.0) * 4.0
                }
                _ => 0.0,
            };
            let total_lift = (base_lift + bounce_lift).clamp(0.0, 4.0);
            let top_pad = 4.0 - total_lift;
            let bottom_pad = total_lift;

            let real_icon = resolve_system_icon(app_name);
            let icon_view: Element<'_, Self::Message> = if let Some(ref icon_path) = real_icon {
                iced::widget::image(image::Handle::from_path(icon_path))
                    .width(Length::Fixed(icon_size))
                    .height(Length::Fixed(icon_size))
                    .into()
            } else {
                match app {
                    ActiveApp::Files => container(
                        text(Icon::FOLDER)
                            .font(LUCIDE_FONT)
                            .size(glyph_size * 0.80)
                            .color(iced::Color::from_rgb(1.0, 0.96, 0.90)),
                    )
                    .width(Length::Fixed(icon_size))
                    .height(Length::Fixed(icon_size))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| {
                        let grad = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                            .add_stop(0.0, iced::Color::from_rgb(0.95, 0.63, 0.30))
                            .add_stop(1.0, iced::Color::from_rgb(0.82, 0.45, 0.18));
                        container::Style {
                            background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                                grad,
                            ))),
                            border: iced::Border {
                                width: 0.0,
                                radius: iced::border::Radius::from(9.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .into(),
                    ActiveApp::Settings => container(
                        text(Icon::SETTINGS)
                            .font(LUCIDE_FONT)
                            .size(glyph_size * 0.84)
                            .color(iced::Color::from_rgb(0.86, 0.88, 0.93)),
                    )
                    .width(Length::Fixed(icon_size))
                    .height(Length::Fixed(icon_size))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| {
                        let grad = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                            .add_stop(0.0, iced::Color::from_rgb(0.28, 0.30, 0.36))
                            .add_stop(1.0, iced::Color::from_rgb(0.17, 0.18, 0.23));
                        container::Style {
                            background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                                grad,
                            ))),
                            border: iced::Border {
                                width: 0.0,
                                radius: iced::border::Radius::from(9.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .into(),
                    ActiveApp::Terminal => container(
                        text(">_")
                            .font(corten_theme::NOTO_SANS_BOLD)
                            .size(glyph_size * 0.56)
                            .color(iced::Color::from_rgb(0.93, 0.55, 0.34)),
                    )
                    .width(Length::Fixed(icon_size))
                    .height(Length::Fixed(icon_size))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| {
                        let grad = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                            .add_stop(0.0, iced::Color::from_rgb(0.20, 0.21, 0.26))
                            .add_stop(1.0, iced::Color::from_rgb(0.11, 0.12, 0.15));
                        container::Style {
                            background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                                grad,
                            ))),
                            border: iced::Border {
                                width: 0.0,
                                radius: iced::border::Radius::from(9.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .into(),
                    ActiveApp::Forge => container(
                        text(Icon::DOWNLOAD)
                            .font(LUCIDE_FONT)
                            .size(glyph_size * 0.78)
                            .color(iced::Color::from_rgb(0.96, 0.62, 0.40)),
                    )
                    .width(Length::Fixed(icon_size))
                    .height(Length::Fixed(icon_size))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_| {
                        let grad = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                            .add_stop(0.0, iced::Color::from_rgb(0.32, 0.21, 0.17))
                            .add_stop(1.0, iced::Color::from_rgb(0.18, 0.12, 0.10));
                        container::Style {
                            background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                                grad,
                            ))),
                            border: iced::Border {
                                width: 0.0,
                                radius: iced::border::Radius::from(9.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .into(),
                    _ => {
                        let icon_char = match app {
                            ActiveApp::Editor => Icon::CODE,
                            ActiveApp::Browser => Icon::GLOBE,
                            ActiveApp::Ai => Icon::CPU,
                            _ => Icon::APP_WINDOW,
                        };
                        text(icon_char)
                            .font(LUCIDE_FONT)
                            .size(glyph_size)
                            .color(if is_active {
                                palette.primary.to_iced()
                            } else if is_running {
                                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.95)
                            } else {
                                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.75)
                            })
                            .into()
                    }
                }
            };

            let icon_container = container(icon_view)
                .width(Length::Fixed(46.0))
                .height(Length::Fixed(40.0))
                .center_x(Length::Fill)
                .center_y(Length::Fill);

            let elevated_icon = iced::widget::column![
                vertical_space().height(top_pad),
                icon_container,
                vertical_space().height(bottom_pad),
            ]
            .align_x(Alignment::Center);

            let show_build_badge = app == ActiveApp::Terminal && is_build_active;
            let bar_color = if show_build_badge {
                palette.warning.to_iced()
            } else {
                palette.primary.to_iced()
            };

            // Crisp running indicator (zero blur/glow)
            let indicator: Element<'_, Self::Message> = if is_active || show_build_badge {
                container(row![])
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(3.0))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(bar_color)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
            } else if win_count >= 2 {
                let make_dot = || {
                    container(row![])
                        .width(Length::Fixed(4.0))
                        .height(Length::Fixed(4.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.text_muted.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
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
                    .width(Length::Fixed(4.0))
                    .height(Length::Fixed(4.0))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(palette.text_muted.to_iced())),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::FULL),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .into()
            } else {
                container(row![])
                    .width(Length::Fixed(4.0))
                    .height(Length::Fixed(3.0))
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
                    .spacing(1)
                    .align_x(Alignment::Center),
            )
            .width(Length::Fixed(50.0))
            .height(Length::Fixed(48.0))
            .padding(0)
            .style(move |_, status| match status {
                button::Status::Hovered => button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, 0.06,
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(10.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                button::Status::Pressed => button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, 0.14,
                    ))),
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
                .on_right_press(Message::OpenDockContextMenu(DockMenuTarget::Pinned(
                    app, idx,
                )))
                .on_enter(Message::DockHover(Some(app_hover_name)))
                .on_middle_press(if idx + 1 < self.config.pinned_apps.len() {
                    Message::MoveDockItemRight(idx)
                } else {
                    Message::MoveDockItemLeft(idx)
                });
            if is_corten_system_app(app_name) {
                sys_row = sys_row.push(app_area);
            } else {
                user_row = user_row.push(app_area);
                user_count += 1;
            }
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

            let is_pinned = self
                .config
                .pinned_apps
                .iter()
                .any(|pin| is_matching_pinned(effective_id, pin));

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

                let icon_size = if is_hovered { 40.0 } else { 36.0 };
                let icon_view: Element<'_, Self::Message> = if let Some(ref icon_path) = real_icon {
                    iced::widget::image(image::Handle::from_path(icon_path))
                        .width(Length::Fixed(icon_size))
                        .height(Length::Fixed(icon_size))
                        .into()
                } else {
                    let icon_char = app_icon_char(effective_id);
                    text(icon_char)
                        .font(LUCIDE_FONT)
                        .size(if is_hovered { 28.0 } else { 24.0 })
                        .color(if is_act {
                            palette.primary.to_iced()
                        } else {
                            palette.text_primary.to_iced()
                        })
                        .into()
                };

                let icon_container = container(icon_view)
                    .width(Length::Fixed(46.0))
                    .height(Length::Fixed(40.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill);

                let elevated_icon = iced::widget::column![
                    vertical_space().height(if is_hovered { 2.0 } else { 4.0 }),
                    icon_container,
                    vertical_space().height(if is_hovered { 2.0 } else { 0.0 }),
                ]
                .align_x(Alignment::Center);

                let indicator: Element<'_, Self::Message> = if is_act {
                    container(row![])
                        .width(Length::Fixed(16.0))
                        .height(Length::Fixed(3.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.primary.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
                            },
                            ..Default::default()
                        })
                        .into()
                } else if app_win_count >= 2 {
                    let make_dot = || {
                        container(row![])
                            .width(Length::Fixed(4.0))
                            .height(Length::Fixed(4.0))
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(
                                    palette.text_muted.to_iced(),
                                )),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::FULL),
                                    ..Default::default()
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
                        .width(Length::Fixed(4.0))
                        .height(Length::Fixed(4.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(palette.text_muted.to_iced())),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
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
                        .spacing(1)
                        .align_x(Alignment::Center),
                )
                .width(Length::Fixed(50.0))
                .height(Length::Fixed(48.0))
                .padding(0)
                .style(|_, status| {
                    let bg = match status {
                        button::Status::Hovered => Some(iced::Background::Color(
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        )),
                        button::Status::Pressed => Some(iced::Background::Color(
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.14),
                        )),
                        _ => Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                    };
                    button::Style {
                        background: bg,
                        border: iced::Border {
                            radius: iced::border::Radius::from(10.0),
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
                    .on_enter(Message::DockHover(Some(dyn_hover_id)));
                user_row = user_row.push(dyn_area);
                user_count += 1;
            }
        }

        let sys_section = mouse_area(sys_row).on_exit(Message::DockHover(None));
        let user_section = mouse_area(user_row).on_exit(Message::DockHover(None));

        // ==================== SECTION 3: UNIFIED CONTROL CENTER PILL & CLOCK ====================
        let is_muted = self.audio_status.map(|(_, m)| m).unwrap_or(false);
        let (is_online, _net_name, _) = &self.network_status;

        let active_playing = self
            .media_players
            .iter()
            .any(|p| p.playback_status == corten_services::mpris::PlaybackStatus::Playing);

        let mut status_pill_items = row![].spacing(10).align_y(Alignment::Center);
        if active_playing {
            let wave = ["▂▅▃", "▅▃▆", "▃▆▂", "▆▂▅"][(self.eq_tick as usize) % 4];
            status_pill_items =
                status_pill_items.push(text(wave).size(11.0).color(palette.primary.to_iced()));
        }
        if self.recording_active || self.mic_active {
            status_pill_items =
                status_pill_items.push(text("⏺").size(11.0).color(if self.recording_active {
                    palette.danger.to_iced()
                } else {
                    palette.warning.to_iced()
                }));
        }
        status_pill_items =
            status_pill_items.push(text(Icon::VOLUME).font(LUCIDE_FONT).size(14.5).color(
                if is_muted {
                    palette.danger.to_iced()
                } else {
                    iced::Color::from_rgba(0.92, 0.93, 0.96, 0.90)
                },
            ));
        status_pill_items =
            status_pill_items.push(text(Icon::WIFI).font(LUCIDE_FONT).size(14.0).color(
                if *is_online {
                    iced::Color::from_rgba(0.92, 0.93, 0.96, 0.90)
                } else {
                    palette.warning.to_iced()
                },
            ));
        if let Some((pct, _charging)) = self.battery_status {
            status_pill_items = status_pill_items.push(
                row![
                    text(Icon::BATTERY)
                        .font(LUCIDE_FONT)
                        .size(13.5)
                        .color(if pct <= 20 {
                            palette.danger.to_iced()
                        } else {
                            palette.primary.to_iced()
                        }),
                    text(format!("{}%", pct))
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(3)
                .align_y(Alignment::Center),
            );
        }

        let cc_active = self.show_control_center;
        let control_center_pill = button(
            container(status_pill_items)
                .padding([5, 8])
                .center_y(Length::Shrink),
        )
        .padding(0)
        .style(move |_, status| {
            let _ = cc_active;
            let bg = if matches!(status, button::Status::Hovered) {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
            } else {
                iced::Color::TRANSPARENT
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(10.0),
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleControlCenter);

        let _ = &self.current_date;
        let clock_column = button(
            text(&self.current_time)
                .size(14.0)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(iced::Color::from_rgba(0.94, 0.95, 0.98, 0.94)),
        )
        .padding([4, 6])
        .style(|_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.10,
                ))),
                button::Status::Pressed => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.16,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    width: 0.0,
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleCalendar);

        let mut right_row = row![control_center_pill]
            .spacing(4)
            .align_y(Alignment::Center);

        // StatusNotifierItem (SNI) / Background App Tray
        if !self.tray_items.is_empty() {
            let mut tray_row = row![].spacing(4).align_y(Alignment::Center);
            for item in &self.tray_items {
                let id = item.id.clone();
                let id_right = item.id.clone();
                let tray_glyph = app_icon_char(&format!("{} {}", item.id, item.title));
                let tray_btn = button(
                    text(tray_glyph)
                        .font(LUCIDE_FONT)
                        .size(14.0)
                        .color(palette.text_primary.to_iced()),
                )
                .padding([3, 5])
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
                let interactive_tray =
                    mouse_area(tray_btn).on_right_press(Message::RightClickTrayItem(id_right));
                tray_row = tray_row.push(interactive_tray);
            }
            right_row = right_row.push(tray_row);
        }

        let _ = (self.show_downloads_stack, self.trash_item_count);
        let right_section = row![right_row, clock_column]
            .spacing(4)
            .align_y(Alignment::Center);

        // ==================== ISLAND 2 (CENTER): PURE APP DOCK ====================
        let mut center_items = row![sys_section]
            .spacing(Spacing::XS)
            .align_y(Alignment::Center)
            .padding([0, 10]);
        if user_count > 0 {
            center_items = center_items.push(make_divider()).push(user_section);
        }

        let center_island = container(center_items)
            .width(Length::Shrink)
            .height(Length::Fixed(56.0))
            .center_y(Length::Fixed(56.0))
            .style(island_style);

        // ==================== ISLAND 3 (RIGHT): STATUS & CLOCK ====================
        let right_island = container(right_section)
            .width(Length::Shrink)
            .height(Length::Fixed(56.0))
            .padding([0, 10])
            .center_y(Length::Fixed(56.0))
            .style(island_style);

        // Modular 3 floating islands with identical 56px height and Radius::LG corners
        let dock_cluster = row![left_island, center_island, right_island]
            .spacing(10)
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
                .style(move |_: &iced::Theme, status| {
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

            let quick_power_row: Element<'_, Self::Message> = row![
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
            .width(Length::Fill)
            .into();

            // Volume and screen brightness sliders inside control center
            let current_vol = self.audio_status.map(|(v, _)| v).unwrap_or(50);
            let vol_muted = self.audio_status.map(|(_, m)| m).unwrap_or(false);
            let cc_slider = slider(0..=100, current_vol, Message::SetVolume)
                .width(Length::Fill)
                .step(1)
                .style(corten_theme::iced_styles::capsule_slider_style(palette));

            let bri_slider = slider(5..=100, self.brightness_pct, Message::SetBrightness)
                .width(Length::Fill)
                .step(1)
                .style(corten_theme::iced_styles::capsule_slider_style(palette));

            let mute_toggle_btn = button(
                text(if vol_muted {
                    Icon::VOLUME_X
                } else {
                    Icon::VOLUME
                })
                .font(LUCIDE_FONT)
                .size(16.0)
                .color(if vol_muted {
                    palette.danger.to_iced()
                } else {
                    palette.text_primary.to_iced()
                }),
            )
            .padding([4, 6])
            .style(move |_, status| {
                let bg = match status {
                    button::Status::Hovered => Some(iced::Background::Color(
                        palette.surface_container_high().to_iced(),
                    )),
                    _ => None,
                };
                button::Style {
                    background: bg,
                    border: iced::Border {
                        width: 0.0,
                        radius: iced::border::Radius::from(Radius::SM),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ToggleAudioMute);

            let sliders_box = iced::widget::column![
                row![
                    mute_toggle_btn,
                    cc_slider,
                    text(format!("{}%", current_vol))
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                        .width(Length::Fixed(32.0))
                        .align_x(iced::alignment::Horizontal::Right),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
                row![
                    container(
                        text(Icon::SUN)
                            .font(LUCIDE_FONT)
                            .size(16.0)
                            .color(palette.text_primary.to_iced())
                    )
                    .padding([4, 6]),
                    bri_slider,
                    text(format!("{}%", self.brightness_pct))
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                        .width(Length::Fixed(32.0))
                        .align_x(iced::alignment::Horizontal::Right),
                ]
                .spacing(Spacing::XS)
                .align_y(Alignment::Center),
            ]
            .spacing(Spacing::SM)
            .width(Length::Fill);

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
                    let mut bat_row = row![text(Icon::BATTERY).font(LUCIDE_FONT).size(14.0).color(
                        if pct <= 20 && !is_charging {
                            palette.danger.to_iced()
                        } else {
                            palette.primary.to_iced()
                        }
                    ),]
                    .spacing(4)
                    .align_y(Alignment::Center);
                    if is_charging {
                        bat_row = bat_row.push(
                            text(Icon::ZAP)
                                .font(LUCIDE_FONT)
                                .size(12.0)
                                .color(palette.warning.to_iced()),
                        );
                    }
                    bat_row = bat_row.push(
                        text(format!("{pct}%"))
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    );
                    bat_row.into()
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

            let profiles = [
                ("power-saver", "Tasarruf"),
                ("balanced", "Dengeli"),
                ("performance", "Performans"),
            ];
            let mut power_profile_row = row![
                text(Icon::GAUGE)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                text("Güç:").size(11.0).color(palette.text_muted.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            for (p_id, p_label) in profiles {
                let is_active = self.active_power_profile == p_id;
                let prof_btn = button(text(p_label).size(11.0).color(if is_active {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }))
                .padding([4, 9])
                .style(move |_, status| {
                    corten_theme::iced_styles::bento_tile_button_style(palette, is_active, status)
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
                text(Icon::SHIELD)
                    .font(LUCIDE_FONT)
                    .size(12.0)
                    .color(palette.primary.to_iced()),
                text(if self.mic_active {
                    "Mikrofon Aktif"
                } else {
                    "Mikrofon Boşta"
                })
                .size(11.0)
                .color(if self.mic_active {
                    palette.warning.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
            ]
            .spacing(6)
            .width(Length::Fill)
            .align_y(Alignment::Center);

            // Material 3 / GNOME-style 2x2 horizontal quick-setting pills
            let (bt_powered, bt_name) = &self.bluetooth_status;

            let quick_pill = |icon_char: char,
                              title: &'static str,
                              subtitle: String,
                              is_active: bool,
                              msg: Message|
             -> Element<'_, Self::Message> {
                let fg = if is_active {
                    palette.on_primary().to_iced()
                } else {
                    palette.text_primary.to_iced()
                };
                let sub_fg = if is_active {
                    let on = palette.on_primary().to_iced();
                    iced::Color::from_rgba(on.r, on.g, on.b, 0.80)
                } else {
                    palette.text_muted.to_iced()
                };
                button(
                    container(
                        row![
                            text(icon_char).font(LUCIDE_FONT).size(18.0).color(fg),
                            iced::widget::column![
                                text(title)
                                    .size(12.0)
                                    .font(corten_theme::NOTO_SANS_SEMIBOLD)
                                    .color(fg),
                                text(subtitle).size(10.5).color(sub_fg),
                            ]
                            .spacing(1)
                            .width(Length::Fill),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(56.0))
                    .padding([0, 12])
                    .center_y(Length::Fill),
                )
                .padding(0)
                .width(Length::FillPortion(1))
                .height(Length::Fixed(56.0))
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    let bg = if is_active {
                        if hovered {
                            palette.primary_hover.to_iced()
                        } else {
                            palette.primary.to_iced()
                        }
                    } else if hovered {
                        palette.surface_container_highest().to_iced()
                    } else {
                        palette.surface_container_high().to_iced()
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            color: iced::Color::TRANSPARENT,
                            width: 0.0,
                            radius: iced::border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    }
                })
                .on_press(msg)
                .into()
            };

            let wifi_sub = if *is_online {
                let name = net_name.trim();
                if name.is_empty() {
                    "Bağlı".to_string()
                } else if name.chars().count() > 12 {
                    format!("{}…", name.chars().take(11).collect::<String>())
                } else {
                    name.to_string()
                }
            } else {
                "Kapalı".to_string()
            };
            let bt_sub = if *bt_powered {
                bt_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .map(|s| {
                        if s.chars().count() > 12 {
                            format!("{}…", s.chars().take(11).collect::<String>())
                        } else {
                            s.to_string()
                        }
                    })
                    .unwrap_or_else(|| "Açık".to_string())
            } else {
                "Kapalı".to_string()
            };
            let game_active = self.dnd_enabled || self.active_power_profile == "performance";
            let game_sub = if game_active { "Açık" } else { "Kapalı" }.to_string();
            let night_sub = if self.night_light_enabled {
                "Açık"
            } else {
                "Kapalı"
            }
            .to_string();

            let bento_quick_grid = iced::widget::column![
                row![
                    quick_pill(Icon::WIFI, "Wi-Fi", wifi_sub, *is_online, Message::ScanWifi),
                    quick_pill(
                        Icon::BLUETOOTH,
                        "Bluetooth",
                        bt_sub,
                        *bt_powered,
                        Message::ToggleBluetooth,
                    ),
                ]
                .spacing(Spacing::XS)
                .width(Length::Fill),
                row![
                    quick_pill(
                        Icon::GAMEPAD,
                        "Game Mode",
                        game_sub,
                        game_active,
                        Message::ToggleDnd,
                    ),
                    quick_pill(
                        Icon::SUN,
                        "Night Light",
                        night_sub,
                        self.night_light_enabled,
                        Message::ToggleNightLight,
                    ),
                ]
                .spacing(Spacing::XS)
                .width(Length::Fill),
            ]
            .spacing(Spacing::XS)
            .width(Length::Fill);

            let header_circle_btn = |icon_char: char, msg: Message| -> Element<'_, Self::Message> {
                button(
                    container(
                        text(icon_char)
                            .font(LUCIDE_FONT)
                            .size(14.0)
                            .color(palette.text_primary.to_iced()),
                    )
                    .width(Length::Fixed(32.0))
                    .height(Length::Fixed(32.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill),
                )
                .width(Length::Fixed(32.0))
                .height(Length::Fixed(32.0))
                .padding(0)
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered | button::Status::Pressed => {
                            palette.surface_container_highest().to_iced()
                        }
                        _ => palette.surface_container_high().to_iced(),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            color: iced::Color::TRANSPARENT,
                            width: 0.0,
                            radius: iced::border::Radius::from(Radius::FULL),
                        },
                        ..Default::default()
                    }
                })
                .on_press(msg)
                .into()
            };

            let status_chip_text = if let Some((pct, charging)) = self.battery_status {
                if charging {
                    format!("{}% • Şarjda", pct)
                } else {
                    format!("{}%", pct)
                }
            } else {
                "Şebeke Bağlı".to_string()
            };
            let cc_header_row = row![
                container(
                    row![
                        text(if self.battery_status.is_some() {
                            Icon::BATTERY
                        } else {
                            Icon::MONITOR
                        })
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.text_muted.to_iced()),
                        text(status_chip_text)
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                )
                .padding([6, 10])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(
                        palette.surface_container_high().to_iced(),
                    )),
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius::from(Radius::FULL),
                    },
                    ..Default::default()
                }),
                horizontal_space(),
                header_circle_btn(Icon::CAMERA, Message::TakeScreenshot(true)),
                header_circle_btn(Icon::SETTINGS, Message::SelectApp(ActiveApp::Settings)),
                header_circle_btn(Icon::POWER, Message::TogglePowerMenu),
            ]
            .spacing(6)
            .width(Length::Fill)
            .align_y(Alignment::Center);

            let screenshot_btn = button(
                row![
                    text(Icon::CAMERA)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.primary.to_iced()),
                    text("Ekran Kesiti")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 10])
            .width(Length::FillPortion(1))
            .style(move |_, status| {
                corten_theme::iced_styles::bento_tile_button_style(palette, false, status)
            })
            .on_press(Message::TakeScreenshot(true));

            let recording_btn = button(
                row![
                    text(Icon::ACTIVITY).font(LUCIDE_FONT).size(13.0).color(
                        if self.recording_active {
                            palette.danger.to_iced()
                        } else {
                            palette.primary.to_iced()
                        }
                    ),
                    text(if self.recording_active {
                        "Kaydı Durdur"
                    } else {
                        "Ekran Kaydı"
                    })
                    .size(11.0)
                    .color(if self.recording_active {
                        palette.danger.to_iced()
                    } else {
                        palette.text_primary.to_iced()
                    }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 10])
            .width(Length::FillPortion(1))
            .style(move |_, status| {
                corten_theme::iced_styles::bento_tile_button_style(
                    palette,
                    self.recording_active,
                    status,
                )
            })
            .on_press(Message::ToggleRecording);

            let color_picker_btn = button(
                row![
                    text(Icon::DROPLET)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.primary.to_iced()),
                    text("Renk Seçici")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([6, 10])
            .width(Length::FillPortion(1))
            .style(move |_, status| {
                corten_theme::iced_styles::bento_tile_button_style(palette, false, status)
            })
            .on_press(Message::PickScreenColor);

            let quick_tools_row = row![screenshot_btn, recording_btn, color_picker_btn]
                .spacing(8)
                .width(Length::Fill)
                .align_y(Alignment::Center);

            // PipeWire Audio Output Sink Selector
            let mut sinks_col = iced::widget::column![
                text(t.audio_output_device)
                    .size(11.0)
                    .color(palette.text_muted.to_iced())
            ]
            .spacing(4);

            for sink in self.audio_sinks.iter().take(3) {
                let is_sel = sink.is_default;
                let sink_id = sink.id;
                let sink_btn = button(
                    row![
                        text(if is_sel { "✓" } else { " " })
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
            let mut wifi_col = iced::widget::column![
                row![
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
                .align_y(Alignment::Center)
            ]
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
                            text(if is_sec { Icon::LOCK } else { Icon::GLOBE })
                                .font(LUCIDE_FONT)
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
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
                            button(
                                text(Icon::X)
                                    .font(LUCIDE_FONT)
                                    .size(11.0)
                                    .color(palette.text_muted.to_iced())
                            )
                            .padding([1, 4])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::CloseWifiModal),
                        ]
                        .width(Length::Fill)
                        .align_y(Alignment::Center),
                        text_input("Wi-Fi parolasını girin...", &self.wifi_password_input)
                            .on_input(Message::WifiPasswordChanged)
                            .on_submit(Message::ConnectWifiSubmit)
                            .padding([6, 10])
                            .size(11.0)
                            .secure(true)
                            .style(move |_, status| {
                                corten_theme::iced_styles::corten_text_input_style(palette, status)
                            }),
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

            let media_section: Element<'_, Self::Message> =
                if let Some(player) = self.media_players.first() {
                    let bus = player.bus_name.clone();
                    let is_playing =
                        player.playback_status == corten_services::mpris::PlaybackStatus::Playing;
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

                    let play_icon = if is_playing { Icon::PAUSE } else { Icon::PLAY };

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
                                button(
                                    text(Icon::SKIP_BACK)
                                        .font(LUCIDE_FONT)
                                        .size(13.0)
                                        .color(palette.text_primary.to_iced())
                                )
                                .padding([4, 8])
                                .style(|_, status| {
                                    let bg = match status {
                                        button::Status::Hovered => {
                                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                                        }
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
                                button(
                                    text(play_icon)
                                        .font(LUCIDE_FONT)
                                        .size(13.0)
                                        .color(palette.on_primary().to_iced())
                                )
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
                                button(
                                    text(Icon::SKIP_FORWARD)
                                        .font(LUCIDE_FONT)
                                        .size(13.0)
                                        .color(palette.text_primary.to_iced())
                                )
                                .padding([4, 8])
                                .style(|_, status| {
                                    let bg = match status {
                                        button::Status::Hovered => {
                                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                                        }
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
                        .align_y(Alignment::Center),
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
                        button(
                            text(Icon::SHUFFLE)
                                .font(LUCIDE_FONT)
                                .size(11.0)
                                .color(palette.primary.to_iced())
                        )
                        .padding([2, 5])
                        .style(|_, _| button::Style::default())
                        .on_press(Message::RouteAppStreamToNextSink(id)),
                        button(
                            text(if is_muted {
                                Icon::VOLUME_X
                            } else {
                                Icon::VOLUME
                            })
                            .font(LUCIDE_FONT)
                            .size(12.0)
                            .color(palette.text_muted.to_iced())
                        )
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
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            0.0, 0.0, 0.0, 0.20,
                        ))),
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

            let patina_chip =
                |variant: PatinaVariant, label: &'static str, is_active: bool, pal: Palette| {
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
                            container(text(""))
                                .width(Length::Fixed(8.0))
                                .height(Length::Fixed(8.0))
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(v_color.to_iced())),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(4.0),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                            text(label).size(11.0).color(if is_active {
                                pal.text_primary.to_iced()
                            } else {
                                pal.text_muted.to_iced()
                            }),
                        ]
                        .spacing(5)
                        .align_y(Alignment::Center),
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
                patina_chip(
                    PatinaVariant::Rust,
                    "Pas",
                    self.active_patina == PatinaVariant::Rust,
                    palette
                ),
                patina_chip(
                    PatinaVariant::Verdigris,
                    "Zümrüt",
                    self.active_patina == PatinaVariant::Verdigris,
                    palette
                ),
                patina_chip(
                    PatinaVariant::Titanium,
                    "Titanyum",
                    self.active_patina == PatinaVariant::Titanium,
                    palette
                ),
                patina_chip(
                    PatinaVariant::Molten,
                    "Akkor",
                    self.active_patina == PatinaVariant::Molten,
                    palette
                ),
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
                .spacing(6),
            )
            .padding([8, 10])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.22,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            let telemetry_row = container(
                row![
                    text(Icon::ACTIVITY)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.primary.to_iced()),
                    text(format!(
                        "↓ {} · ↑ {}",
                        self.net_speed_display.0, self.net_speed_display.1
                    ))
                    .size(11.0)
                    .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    button(
                        row![
                            text(Icon::CPU)
                                .font(LUCIDE_FONT)
                                .size(12.0)
                                .color(palette.primary.to_iced()),
                            text(format!(
                                "RAM {:.0}% ({} MB)",
                                self.memory_summary.2, self.memory_summary.0
                            ))
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(4)
                        .align_y(Alignment::Center),
                    )
                    .padding([2, 6])
                    .style(|_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
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
                    .on_press(Message::ToggleMiniTaskManager),
                ]
                .spacing(6)
                .width(Length::Fill)
                .align_y(Alignment::Center),
            )
            .padding([6, 10])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.22,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            let _ = &t.control_center_title;
            let _unused_cc_sections: [Element<'_, Self::Message>; 11] = [
                telemetry_row.into(),
                media_section,
                sinks_col.into(),
                app_streams_col,
                sys_status_row.into(),
                wifi_col.into(),
                quick_tools_row.into(),
                patina_box.into(),
                power_profile_row.into(),
                dev_ports_row.into(),
                quick_power_row,
            ];

            let mut deck_col = iced::widget::column![cc_header_row, bento_quick_grid, sliders_box]
                .spacing(Spacing::MD)
                .width(Length::Fill);

            if self.show_wifi_modal {
                deck_col = deck_col.push(wifi_modal_element);
            }

            let cc_card = container(deck_col)
                .width(Length::Fixed(328.0))
                .padding(Spacing::MD)
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, Radius::LG);
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

            // Column shrinks to the dock width; End alignment pins the deck above the dock's right edge
            iced::widget::column![reveal_card(cc_card, reveal_max_h), dock_cluster]
                .spacing(popover_lift_gap)
                .align_x(Alignment::End)
                .into()
        } else if self.show_power_menu {
            let t = self.language.t();
            // Elevated Corten Güç & Oturum Merkezi (Power & Session Hub)
            let action_card = |icon_char: char,
                               label: &'static str,
                               desc: &'static str,
                               action: SystemPowerAction,
                               is_danger: bool| {
                button(
                    row![
                        container(text(icon_char).font(LUCIDE_FONT).size(16.0).color(
                            if is_danger {
                                palette.danger.to_iced()
                            } else {
                                palette.primary.to_iced()
                            }
                        ))
                        .padding([6, 8])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(if is_danger {
                                iced::Color::from_rgba(
                                    palette.danger.r,
                                    palette.danger.g,
                                    palette.danger.b,
                                    0.15,
                                )
                            } else {
                                iced::Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.15,
                                )
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
                                iced::Color::from_rgba(
                                    palette.danger.r,
                                    palette.danger.g,
                                    palette.danger.b,
                                    0.3,
                                )
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
                text(Icon::ZAP)
                    .font(LUCIDE_FONT)
                    .size(14.0)
                    .color(palette.primary.to_iced()),
                text(t.power_hub_title)
                    .size(14.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                container(text("Super+X").size(10.0).color(palette.primary.to_iced()))
                    .padding([2, 6])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            palette.primary.r,
                            palette.primary.g,
                            palette.primary.b,
                            0.15
                        ))),
                        border: iced::Border {
                            radius: iced::border::Radius::from(4.0),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                button(
                    text(Icon::X)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.text_muted.to_iced())
                )
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::TogglePowerMenu),
            ]
            .spacing(6)
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
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            palette.danger.r,
                            palette.danger.g,
                            palette.danger.b,
                            0.1
                        ))),
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
                        button(
                            text(t.btn_confirm)
                                .size(12.0)
                                .color(palette.on_primary().to_iced())
                        )
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
                    action_card(
                        Icon::LOCK,
                        t.power_lock,
                        t.power_lock_desc,
                        SystemPowerAction::Lock,
                        false
                    ),
                    action_card(
                        Icon::MOON,
                        t.power_suspend,
                        t.power_suspend_desc,
                        SystemPowerAction::Suspend,
                        false
                    ),
                    action_card(
                        Icon::USER,
                        t.power_logout,
                        t.power_logout_desc,
                        SystemPowerAction::Logout,
                        false
                    ),
                    action_card(
                        Icon::REFRESH,
                        t.power_reboot,
                        t.power_reboot_desc,
                        SystemPowerAction::Reboot,
                        false
                    ),
                    action_card(
                        Icon::POWER,
                        t.power_poweroff,
                        t.power_poweroff_desc,
                        SystemPowerAction::Poweroff,
                        true
                    ),
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

            iced::widget::column![reveal_card(power_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_calendar {
            let t = self.language.t();
            // Corten Takvim & Hava Durumu Modülü
            let header = row![
                text(Icon::CALENDAR)
                    .font(LUCIDE_FONT)
                    .size(14.0)
                    .color(palette.primary.to_iced()),
                text(t.weather_calendar_title)
                    .size(14.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(
                    text(Icon::X)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.text_muted.to_iced())
                )
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::ToggleCalendar),
            ]
            .spacing(6)
            .width(Length::Fill)
            .align_y(Alignment::Center);

            // Weather card
            let temp_display = format!(
                "{}{}\u{00B0}C",
                if self.weather_report.temp_c > 0 {
                    "+"
                } else {
                    ""
                },
                self.weather_report.temp_c
            );
            let weather_card = container(
                column![
                    row![
                        column![
                            row![
                                text(&self.weather_report.city)
                                    .size(13.0)
                                    .color(palette.text_muted.to_iced()),
                                text("•").size(10.0).color(palette.text_disabled.to_iced()),
                                text(&self.weather_report.condition)
                                    .size(12.0)
                                    .color(palette.text_primary.to_iced()),
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                            row![
                                text(
                                    if self.weather_report.condition.contains("Bulut")
                                        || self.weather_report.condition.contains("Yağmur")
                                    {
                                        Icon::CLOUD
                                    } else {
                                        Icon::SUN
                                    }
                                )
                                .font(LUCIDE_FONT)
                                .size(24.0)
                                .color(palette.primary.to_iced()),
                                text(temp_display)
                                    .size(24.0)
                                    .color(palette.primary.to_iced()),
                            ]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        ]
                        .spacing(2),
                        horizontal_space(),
                        column![
                            row![
                                text(Icon::DROPLET)
                                    .font(LUCIDE_FONT)
                                    .size(11.0)
                                    .color(palette.primary.to_iced()),
                                text(format!(
                                    "{}: %{}",
                                    t.weather_humidity, self.weather_report.humidity
                                ))
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                            row![
                                text(Icon::WIND)
                                    .font(LUCIDE_FONT)
                                    .size(11.0)
                                    .color(palette.text_muted.to_iced()),
                                text(format!(
                                    "{}: {} km/s",
                                    t.weather_wind, self.weather_report.wind_kmh
                                ))
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                        ]
                        .spacing(4),
                    ]
                    .align_y(Alignment::Center),
                    // 3-day forecast
                    row(self.weather_report.forecast.iter().map(|f| {
                        container(
                            row![
                                text(&f.day_name)
                                    .size(10.0)
                                    .color(palette.text_muted.to_iced()),
                                text(Icon::CLOUD)
                                    .font(LUCIDE_FONT)
                                    .size(12.0)
                                    .color(palette.text_muted.to_iced()),
                                text(format!("{}°", f.temp_max))
                                    .size(11.0)
                                    .color(palette.text_primary.to_iced()),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                        )
                        .padding([3, 6])
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                1.0, 1.0, 1.0, 0.04,
                            ))),
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
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.03,
                ))),
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
                container(text(w).size(11.0).color(palette.text_muted.to_iced()))
                    .width(Length::Fixed(40.0))
                    .align_x(Alignment::Center)
                    .into()
            }))
            .spacing(4);

            // Compute first day weekday offset and days in month
            let first_day_date = chrono::NaiveDate::from_ymd_opt(current_year, current_month, 1)
                .unwrap_or(now.date_naive());
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
                        .height(Length::Fixed(24.0)),
                );
                day_cell_idx += 1;
            }

            // Days 1..=days_in_month
            for d in 1..=days_in_month {
                let is_today = d == current_day;
                let day_chip = container(text(format!("{}", d)).size(11.0).color(if is_today {
                    palette.on_primary().to_iced()
                } else {
                    palette.text_primary.to_iced()
                }))
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
                        text(month_title)
                            .size(13.0)
                            .color(palette.primary.to_iced()),
                        horizontal_space(),
                        text(now.format("%H:%M").to_string())
                            .size(13.0)
                            .color(palette.text_muted.to_iced()),
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
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.02,
                ))),
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

            iced::widget::column![reveal_card(calendar_card, reveal_max_h), dock_cluster]
                .spacing(popover_lift_gap)
                .align_x(Alignment::End)
                .into()
        } else if self.show_clipboard_modal {
            let title_row = row![
                text("Pano Geçmişi (Super+V)")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(
                    text("Tümünü Temizle")
                        .size(11.0)
                        .color(palette.danger.to_iced())
                )
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
                .size(12.0)
                .style(move |_, status| {
                    corten_theme::iced_styles::corten_text_input_style(palette, status)
                });

            let filter_tabs = row![
                for_clip_tab("Tümü", "all", &self.clipboard_filter, palette),
                for_clip_tab("Sabitlenen", "pinned", &self.clipboard_filter, palette),
                for_clip_tab("Renkler", "color", &self.clipboard_filter, palette),
                for_clip_tab("Görseller", "image", &self.clipboard_filter, palette),
            ]
            .spacing(4);

            let mut items_col = iced::widget::column![filter_tabs].spacing(6);
            let mut filtered_items: Vec<(usize, &String, bool)> = self
                .clipboard_history
                .iter()
                .enumerate()
                .filter(|(_, text)| {
                    let q_ok = self.clipboard_search.is_empty()
                        || text
                            .to_lowercase()
                            .contains(&self.clipboard_search.to_lowercase());
                    let is_pin = self.pinned_clipboard.contains(text);
                    let is_hex = is_hex_color_entry(text);
                    let is_img = text.starts_with("[IMG:") || text.ends_with(".png");
                    let tab_ok = match self.clipboard_filter.as_str() {
                        "pinned" => is_pin,
                        "color" => is_hex,
                        "image" => is_img,
                        _ => true,
                    };
                    q_ok && tab_ok
                })
                .map(|(idx, text)| (idx, text, self.pinned_clipboard.contains(text)))
                .collect();
            filtered_items.sort_by_key(|(_, _, is_pinned)| !*is_pinned);

            if filtered_items.is_empty() {
                items_col = items_col.push(corten_theme::iced_styles::empty_state_card(
                    Icon::CLIPBOARD,
                    "Pano öğesi bulunamadı",
                    "Kopyaladığınız metinler, renkler ve görseller burada görünür",
                    palette,
                ));
            } else {
                for (idx, text_entry, is_pinned) in filtered_items {
                    let display_text = if text_entry.chars().count() > 42 {
                        format!("{}...", text_entry.chars().take(42).collect::<String>())
                    } else {
                        text_entry.clone()
                    };
                    let copy_val = text_entry.clone();
                    let pin_val = text_entry.clone();
                    let pin_color = if is_pinned {
                        palette.primary.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    };
                    let mut entry_content = row![].spacing(6).align_y(Alignment::Center);
                    if let Some(swatch) = parse_hex_swatch_color(text_entry) {
                        entry_content = entry_content.push(
                            container(row![])
                                .width(Length::Fixed(14.0))
                                .height(Length::Fixed(14.0))
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(swatch)),
                                    border: iced::Border {
                                        color: palette.border.to_iced(),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(4.0),
                                    },
                                    ..Default::default()
                                }),
                        );
                    }
                    entry_content = entry_content.push(
                        text(display_text)
                            .size(12.0)
                            .color(palette.text_primary.to_iced()),
                    );
                    let item_row = row![
                        button(
                            text(Icon::PIN)
                                .font(LUCIDE_FONT)
                                .size(12.0)
                                .color(pin_color)
                        )
                        .padding([4, 6])
                        .style(|_, _| button::Style::default())
                        .on_press(Message::TogglePinClipboardItem(pin_val)),
                        button(entry_content)
                            .width(Length::Fill)
                            .padding([5, 8])
                            .style(move |_, status| {
                                let bg = match status {
                                    button::Status::Hovered => palette.surface_hover.to_iced(),
                                    _ if is_pinned => iced::Color::from_rgba(
                                        palette.primary.r,
                                        palette.primary.g,
                                        palette.primary.b,
                                        0.14,
                                    ),
                                    _ => palette.surface.to_iced(),
                                };
                                button::Style {
                                    background: Some(iced::Background::Color(bg)),
                                    border: iced::Border {
                                        color: if is_pinned {
                                            iced::Color::from_rgba(
                                                palette.primary.r,
                                                palette.primary.g,
                                                palette.primary.b,
                                                0.35,
                                            )
                                        } else {
                                            palette.border.to_iced()
                                        },
                                        width: 1.0,
                                        radius: iced::border::Radius::from(6.0),
                                    },
                                    ..Default::default()
                                }
                            })
                            .on_press(Message::CopyClipboardItem(copy_val)),
                        button(
                            text(Icon::X)
                                .font(LUCIDE_FONT)
                                .size(11.0)
                                .color(palette.text_muted.to_iced())
                        )
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
                iced::widget::column![title_row, search_input, clipboard_scroll].spacing(10),
            )
            .width(Length::Fixed(380.0))
            .padding(Spacing::MD)
            .style(move |_| {
                let mut style = acrylic_glass_style(palette, Radius::LG);
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

            iced::widget::column![reveal_card(clipboard_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_notification_center {
            let title_row = row![
                text("Bildirim Merkezi")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(
                    text("Tümünü Temizle")
                        .size(11.0)
                        .color(palette.danger.to_iced())
                )
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
                    container(
                        text("Yeni bildirim yok.")
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    )
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
                                text(time_str)
                                    .size(10.0)
                                    .color(palette.text_muted.to_iced()),
                            ]
                            .align_y(Alignment::Center),
                            text(body).size(11.0).color(palette.text_primary.to_iced()),
                        ]
                        .spacing(2),
                    )
                    .padding([8, 12])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(
                            palette.surface_container_high().to_iced(),
                        )),
                        border: iced::Border {
                            color: iced::Color::TRANSPARENT,
                            width: 0.0,
                            radius: iced::border::Radius::from(Radius::MD),
                        },
                        ..Default::default()
                    })
                    .width(Length::Fill);

                    notes_col = notes_col.push(note_item);
                }
            }

            let notif_scroll = scrollable(notes_col).height(Length::Fixed(240.0));
            let notif_card = container(iced::widget::column![title_row, notif_scroll].spacing(10))
                .width(Length::Fixed(360.0))
                .padding(Spacing::MD)
                .style(move |_| {
                    let mut style = acrylic_glass_style(palette, Radius::LG);
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

            iced::widget::column![reveal_card(notif_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_downloads_stack {
            let header = row![
                text(Icon::DOWNLOAD)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                text("İndirilenler")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(
                    text("Tümünü Göster")
                        .size(11.0)
                        .color(palette.primary.to_iced())
                )
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::SelectApp(ActiveApp::Files)),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleDownloadsStack),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            let mut dl_col = iced::widget::column![].spacing(6);
            if self.downloads_items.is_empty() {
                dl_col = dl_col.push(
                    container(
                        text("İndirilen dosya bulunamadı.")
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .padding([20, 10])
                    .center_x(Length::Fill),
                );
            } else {
                for (name, size_str, path) in &self.downloads_items {
                    let path_clone = path.clone();
                    let item = button(
                        row![
                            text(Icon::FOLDER)
                                .font(LUCIDE_FONT)
                                .size(14.0)
                                .color(palette.primary.to_iced()),
                            column![
                                text(name.clone())
                                    .size(12.0)
                                    .color(palette.text_primary.to_iced()),
                                text(size_str.clone())
                                    .size(10.0)
                                    .color(palette.text_muted.to_iced()),
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

            let dl_card = container(iced::widget::column![header, dl_col].spacing(10))
                .padding([14, 18])
                .width(Length::Fixed(360.0))
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

            iced::widget::column![reveal_card(dl_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if let Some(target) = &self.dock_context_menu {
            let (disp_title, app_id_str, win_id_opt, is_running, cmd_launch, pin_idx_opt) =
                match target {
                    DockMenuTarget::Pinned(app, idx) => {
                        let title = match app {
                            ActiveApp::Terminal => "Terminal",
                            ActiveApp::Files => "Dosya Yöneticisi",
                            ActiveApp::Settings => "Sistem Ayarları",
                            ActiveApp::Browser => "Web Tarayıcısı",
                            ActiveApp::Editor => "Kod Düzenleyici",
                            ActiveApp::Ai => "Corten AI",
                            ActiveApp::Forge => "Corten Forge",
                        };
                        let running = match app {
                            ActiveApp::Terminal => self.running_apps.0,
                            ActiveApp::Files => self.running_apps.1,
                            ActiveApp::Settings => false,
                            ActiveApp::Browser => self.running_apps.2,
                            ActiveApp::Editor => self.running_apps.3,
                            ActiveApp::Ai => false,
                            ActiveApp::Forge => false,
                        };
                        let cmd = match app {
                            ActiveApp::Terminal => {
                                "corten-terminal || alacritty || foot || konsole || xterm"
                                    .to_string()
                            }
                            ActiveApp::Files => {
                                "corten-files || nautilus || dolphin || thunar".to_string()
                            }
                            ActiveApp::Settings => {
                                "corten-settings || gnome-control-center || systemsettings"
                                    .to_string()
                            }
                            ActiveApp::Browser => {
                                "firefox || google-chrome || brave || chromium || zen".to_string()
                            }
                            ActiveApp::Editor => "code || gedit || kate || nvim".to_string(),
                            ActiveApp::Ai => "corten-ai".to_string(),
                            ActiveApp::Forge => "corten-forge".to_string(),
                        };
                        (
                            title.to_string(),
                            format!("{:?}", app).to_lowercase(),
                            None,
                            running,
                            cmd,
                            Some(*idx),
                        )
                    }
                    DockMenuTarget::Dynamic {
                        win_id,
                        app_id,
                        title,
                    } => {
                        let dt = if title.is_empty() {
                            app_id.clone()
                        } else {
                            title.clone()
                        };
                        (
                            dt,
                            app_id.clone(),
                            Some(*win_id),
                            true,
                            app_id.clone(),
                            None,
                        )
                    }
                };

            let status_badge = container(
                text(if is_running {
                    "Çalışıyor"
                } else {
                    "Hazır"
                })
                .size(9.5)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(if is_running {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
            )
            .padding([2, 8])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(if is_running {
                    iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.14,
                    )
                } else {
                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.04)
                })),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::FULL),
                    ..Default::default()
                },
                ..Default::default()
            });

            let header = row![
                text(Icon::APP_WINDOW)
                    .font(LUCIDE_FONT)
                    .size(14.0)
                    .color(palette.primary.to_iced()),
                text(disp_title)
                    .size(13.0)
                    .font(corten_theme::NOTO_SANS_SEMIBOLD)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                status_badge,
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::CloseDockContextMenu),
            ]
            .spacing(6)
            .width(Length::Fill)
            .align_y(Alignment::Center);

            let make_sep = || {
                container(iced::widget::Space::new(Length::Fill, Length::Fixed(1.0))).style(
                    move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            1.0, 1.0, 1.0, 0.08,
                        ))),
                        ..Default::default()
                    },
                )
            };

            let mut menu_items = column![].spacing(4);

            // List open windows matching this app (Exposé quick jump)
            let app_windows: Vec<_> = self
                .compositor_windows
                .iter()
                .filter(|(wid, aid, title, _, _, _)| {
                    if let Some(target_wid) = win_id_opt {
                        *wid == target_wid
                    } else {
                        aid.to_lowercase().contains(&app_id_str)
                            || title.to_lowercase().contains(&app_id_str)
                    }
                })
                .take(4)
                .collect();

            if !app_windows.is_empty() {
                menu_items = menu_items.push(
                    text("AÇIK PENCERELER")
                        .size(9.5)
                        .color(palette.text_muted.to_iced()),
                );
                for (wid, aid, wtitle, ws, is_act, _) in app_windows {
                    let w_id = *wid;
                    let w_label = if wtitle.is_empty() { aid } else { wtitle };
                    let w_short = if w_label.chars().count() > 24 {
                        format!("{}…", w_label.chars().take(22).collect::<String>())
                    } else {
                        w_label.clone()
                    };
                    let w_active = *is_act;
                    let win_row_btn = button(
                        row![
                            text(if w_active { "✓" } else { " " })
                                .size(10.0)
                                .color(if w_active {
                                    palette.primary.to_iced()
                                } else {
                                    palette.text_muted.to_iced()
                                }),
                            text(w_short)
                                .size(11.5)
                                .color(palette.text_primary.to_iced())
                                .width(Length::Fill),
                            text(format!("M{ws}"))
                                .size(9.5)
                                .font(iced::Font::MONOSPACE)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(6)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([5, 8])
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                            _ if w_active => iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.14,
                            ),
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
                    .on_press(Message::DockMenuAction(DockAction::Focus(w_id)));
                    menu_items = menu_items.push(win_row_btn);
                }
                menu_items = menu_items.push(make_sep());
            }

            let new_win_btn = button(
                row![
                    text(Icon::APP_WINDOW)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.primary.to_iced()),
                    text("Yeni Pencere Aç")
                        .size(12.0)
                        .color(palette.text_primary.to_iced()),
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
                        text(Icon::SEARCH)
                            .font(LUCIDE_FONT)
                            .size(13.0)
                            .color(palette.text_muted.to_iced()),
                        text("Öne Getir / Odaklan")
                            .size(12.0)
                            .color(palette.text_primary.to_iced()),
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
            }

            if let Some(idx) = pin_idx_opt {
                let can_left = idx > 0;
                let can_right = idx + 1 < self.config.pinned_apps.len();
                let reorder_row = row![
                    button(
                        row![
                            text("◀").size(11.0),
                            text(" Sola")
                                .size(11.0)
                                .color(palette.text_primary.to_iced())
                        ]
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
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press_maybe(if can_left {
                        Some(Message::MoveDockItemLeft(idx))
                    } else {
                        None
                    }),
                    button(
                        row![
                            text("Sağa ")
                                .size(11.0)
                                .color(palette.text_primary.to_iced()),
                            text("▶").size(11.0)
                        ]
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
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press_maybe(if can_right {
                        Some(Message::MoveDockItemRight(idx))
                    } else {
                        None
                    }),
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
                    text(if is_pinned {
                        "Dock'tan Kaldır"
                    } else {
                        "Dock'a Sabitle"
                    })
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
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
                    text(Icon::SHIELD)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.text_muted.to_iced()),
                    text(if is_dodge {
                        "Akıllı Gizlenme: Açık"
                    } else {
                        "Akıllı Gizlenme: Kapalı"
                    })
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
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

            if is_running {
                let focus_id = win_id_opt.unwrap_or(0);
                menu_items = menu_items.push(make_sep());
                let close_btn = button(
                    row![
                        text(Icon::X)
                            .font(LUCIDE_FONT)
                            .size(13.0)
                            .color(palette.danger.to_iced()),
                        text("Pencereyi Kapat / Sonlandır")
                            .size(12.0)
                            .color(palette.danger.to_iced())
                            .width(Length::Fill),
                        text("Alt+F4")
                            .size(9.5)
                            .font(iced::Font::MONOSPACE)
                            .color(palette.text_muted.to_iced()),
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
                .on_press(Message::DockMenuAction(DockAction::CloseWindow(
                    focus_id,
                    app_id_str.clone(),
                )));
                menu_items = menu_items.push(close_btn);
            }

            let menu_card = container(column![header, make_sep(), menu_items].spacing(8))
                .padding([12, 14])
                .width(Length::Fixed(276.0))
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

            let menu_area = mouse_area(
                container(reveal_card(
                    mouse_area(menu_card).on_press(Message::NoOp),
                    reveal_max_h,
                ))
                .width(Length::Fill)
                .align_x(Alignment::Center),
            )
            .on_press(Message::CloseDockContextMenu);
            iced::widget::column![menu_area, dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_mini_task_manager {
            let header = row![
                text(Icon::ACTIVITY)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                text("Mini Görev Yöneticisi")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(
                    text("Tümünü Aç ↗")
                        .size(11.0)
                        .color(palette.primary.to_iced())
                )
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::OpenFullTaskManager),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleMiniTaskManager),
            ]
            .spacing(6)
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
                    container(
                        text("Süreç bilgisi yükleniyor...")
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .padding([15, 10])
                    .center_x(Length::Fill),
                );
            } else {
                for p in &self.top_processes {
                    let pid = p.pid;
                    let proc_row = row![
                        column![
                            text(&p.name)
                                .size(12.0)
                                .color(palette.text_primary.to_iced()),
                            text(format!("PID: {} • {} MB RAM", p.pid, p.mem_mb))
                                .size(10.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .width(Length::Fill),
                        button(
                            text("✕ Sonlandır")
                                .size(10.0)
                                .color(palette.danger.to_iced())
                        )
                        .padding([3, 7])
                        .style(|_, status| {
                            let bg = match status {
                                button::Status::Hovered => {
                                    iced::Color::from_rgba(1.0, 0.2, 0.2, 0.25)
                                }
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

                    proc_col = proc_col.push(container(proc_row).padding([5, 8]).style(|_| {
                        container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                1.0, 1.0, 1.0, 0.04,
                            ))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(6.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    }));
                }
            }

            let task_card = container(column![header, mem_bar, proc_col].spacing(10))
                .padding([14, 18])
                .width(Length::Fixed(360.0))
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

            iced::widget::column![reveal_card(task_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if let Some(ref target_app_id) = self.active_window_previews {
            let matching_windows: Vec<_> = self
                .compositor_windows
                .iter()
                .filter(|(_, aid, title, _, _, _)| {
                    aid == target_app_id
                        || is_matching_window(
                            aid,
                            title,
                            match target_app_id.as_str() {
                                "terminal" | "corten-terminal" => ActiveApp::Terminal,
                                "files" | "corten-files" => ActiveApp::Files,
                                "settings" | "corten-settings" => ActiveApp::Settings,
                                "browser" => ActiveApp::Browser,
                                "editor" => ActiveApp::Editor,
                                "corten-ai" | "ai" => ActiveApp::Ai,
                                "corten-forge" | "forge" | "store" | "software" => ActiveApp::Forge,
                                _ => ActiveApp::Terminal,
                            },
                        )
                })
                .collect();

            let real_app_icon = resolve_system_icon(target_app_id);
            let app_icon_view: Element<'_, Self::Message> =
                if let Some(ref icon_path) = real_app_icon {
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
                    .font(corten_theme::NOTO_SANS_BOLD)
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
                    container(
                        text("Açık pencere bulunamadı.")
                            .size(12.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .padding([15, 10])
                    .center_x(Length::Fill),
                );
            } else {
                for (win_id, aid, title, ws, is_active, _) in matching_windows.iter().take(3) {
                    let win_id_val = *win_id;
                    let aid_val = (*aid).clone();
                    let title_text = if title.is_empty() {
                        aid.as_str()
                    } else {
                        title.as_str()
                    };
                    let title_clipped = if title_text.chars().count() > 22 {
                        format!("{}...", title_text.chars().take(20).collect::<String>())
                    } else {
                        title_text.to_string()
                    };

                    // Miniature window interior simulation
                    let lower_aid = aid.to_lowercase();
                    let mini_content: Element<'_, Self::Message> = if lower_aid.contains("term") {
                        column![
                            text("corten@desktop:~$ corten-term")
                                .size(9.0)
                                .color(iced::Color::from_rgb(0.3, 0.9, 0.4)),
                            text("> Ready (60 FPS Wayland)")
                                .size(8.0)
                                .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.7)),
                            row![
                                text("▍")
                                    .size(9.0)
                                    .color(iced::Color::from_rgb(0.3, 0.9, 0.4)),
                            ],
                        ]
                        .spacing(2)
                        .padding(6)
                        .into()
                    } else if lower_aid.contains("file") {
                        row![
                            column![
                                text(Icon::FOLDER)
                                    .font(LUCIDE_FONT)
                                    .size(14.0)
                                    .color(palette.primary.to_iced()),
                                text("Belgeler")
                                    .size(7.0)
                                    .color(palette.text_muted.to_iced())
                            ]
                            .align_x(Alignment::Center),
                            column![
                                text(Icon::FOLDER)
                                    .font(LUCIDE_FONT)
                                    .size(14.0)
                                    .color(palette.primary.to_iced()),
                                text("İndirilen")
                                    .size(7.0)
                                    .color(palette.text_muted.to_iced())
                            ]
                            .align_x(Alignment::Center),
                            column![
                                text(Icon::FOLDER)
                                    .font(LUCIDE_FONT)
                                    .size(14.0)
                                    .color(palette.primary.to_iced()),
                                text("Resimler")
                                    .size(7.0)
                                    .color(palette.text_muted.to_iced())
                            ]
                            .align_x(Alignment::Center),
                        ]
                        .spacing(10)
                        .padding(10)
                        .align_y(Alignment::Center)
                        .into()
                    } else if lower_aid.contains("setting") {
                        column![
                            row![
                                text("Ses:").size(8.0),
                                horizontal_space(),
                                text("80%").size(8.0).color(palette.primary.to_iced())
                            ],
                            container(row![])
                                .width(Length::Fixed(120.0))
                                .height(Length::Fixed(3.0))
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        palette.primary.to_iced()
                                    )),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(2.0),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                            row![
                                text("Parlaklık:").size(8.0),
                                horizontal_space(),
                                text("100%").size(8.0).color(palette.primary.to_iced())
                            ],
                        ]
                        .spacing(4)
                        .padding(6)
                        .into()
                    } else {
                        column![
                            text(title_clipped.clone())
                                .size(9.0)
                                .color(palette.text_primary.to_iced()),
                            text(format!("Pencere ID #{}", win_id_val))
                                .size(8.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(3)
                        .padding(6)
                        .into()
                    };

                    let is_win_active = *is_active;
                    let mini_indicator = container(row![])
                        .width(Length::Fixed(6.0))
                        .height(Length::Fixed(6.0))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(if is_win_active {
                                palette.primary.to_iced()
                            } else {
                                palette.text_muted.to_iced()
                            })),
                            border: iced::Border {
                                radius: iced::border::Radius::from(2.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        });

                    let mini_titlebar = row![
                        mini_indicator,
                        text(title_clipped)
                            .size(10.0)
                            .color(palette.text_primary.to_iced()),
                        horizontal_space(),
                        button(text("✕").size(9.0).color(palette.text_muted.to_iced()))
                            .padding([1, 4])
                            .style(|_, _| button::Style::default())
                            .on_press(Message::CloseWindowById(win_id_val, aid_val)),
                    ]
                    .spacing(5)
                    .padding([4, 6])
                    .align_y(Alignment::Center);
                    let mini_window_card = container(column![
                        mini_titlebar,
                        container(mini_content)
                            .width(Length::Fill)
                            .height(Length::Fixed(70.0))
                            .style(|_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(
                                    0.02, 0.03, 0.05, 0.95
                                ))),
                                ..Default::default()
                            }),
                        row![
                            text(format!("Masaüstü {}", ws))
                                .size(9.0)
                                .color(palette.text_muted.to_iced()),
                            horizontal_space(),
                            text("Odaklan").size(9.0).color(palette.primary.to_iced()),
                        ]
                        .padding([3, 6])
                        .align_y(Alignment::Center),
                    ])
                    .width(Length::Fixed(190.0))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            0.08, 0.10, 0.14, 0.95,
                        ))),
                        border: iced::Border {
                            color: if is_win_active {
                                palette.primary.to_iced()
                            } else {
                                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                            },
                            width: if is_win_active { 1.5 } else { 1.0 },
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    });

                    let clickable_thumb = button(mini_window_card)
                        .padding(0)
                        .style(|_, status| {
                            let border_color = match status {
                                button::Status::Hovered => {
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.4)
                                }
                                _ => iced::Color::TRANSPARENT,
                            };
                            button::Style {
                                border: iced::Border {
                                    color: border_color,
                                    width: 1.0,
                                    radius: iced::border::Radius::from(8.0),
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::FocusWindow(win_id_val));

                    win_row = win_row.push(clickable_thumb);
                }
            }

            let card_w =
                ((matching_windows.len().max(1) as f32) * 205.0 + 28.0).clamp(240.0, 680.0);
            let preview_card = container(column![header, win_row].spacing(10))
                .padding([12, 14])
                .width(Length::Fixed(card_w))
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

            iced::widget::column![reveal_card(preview_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_net_speed_details {
            let (is_online, net_name, ip_addr) = &self.network_status;
            let header = row![
                text(Icon::GLOBE)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                text("Canlı Ağ ve Trafik Detayı")
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                horizontal_space(),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 5])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleNetSpeedDetails),
            ]
            .spacing(6)
            .align_y(Alignment::Center);

            let status_row = row![
                text(if *is_online {
                    "Bağlı"
                } else {
                    "Çevrimdışı"
                })
                .size(12.0)
                .font(corten_theme::NOTO_SANS_MEDIUM)
                .color(if *is_online {
                    palette.primary.to_iced()
                } else {
                    palette.danger.to_iced()
                }),
                text(format!("{} ({})", net_name, ip_addr))
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(8)
            .align_y(Alignment::Center);

            let speed_boxes = row![
                container(
                    column![
                        text("İNDİRME (RX)")
                            .size(10.0)
                            .color(palette.text_muted.to_iced()),
                        text(&self.net_speed_display.0)
                            .size(16.0)
                            .color(palette.primary.to_iced()),
                    ]
                    .spacing(2)
                )
                .width(Length::Fill)
                .padding([8, 12])
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, 0.04
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(8.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                container(
                    column![
                        text("YÜKLEME (TX)")
                            .size(10.0)
                            .color(palette.text_muted.to_iced()),
                        text(&self.net_speed_display.1)
                            .size(16.0)
                            .color(palette.warning.to_iced()),
                    ]
                    .spacing(2)
                )
                .width(Length::Fill)
                .padding([8, 12])
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, 0.04
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(8.0),
                        ..Default::default()
                    },
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
                            color.r,
                            color.g,
                            color.b,
                            color.a * popover_alpha,
                        )));
                    }
                    style
                });

            iced::widget::column![reveal_card(net_card, reveal_max_h), dock_cluster]
                .spacing(12)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_desktop_context_hub {
            let hub_action_btn =
                |icon_char: char, label_text: &'static str, msg: Message, pal: Palette| {
                    button(
                        column![
                            text(icon_char)
                                .font(LUCIDE_FONT)
                                .size(18.0)
                                .color(pal.primary.to_iced()),
                            text(label_text)
                                .size(10.0)
                                .color(pal.text_primary.to_iced()),
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
                    .font(corten_theme::NOTO_SANS_BOLD)
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
                    text(Icon::MONITOR)
                        .font(LUCIDE_FONT)
                        .size(16.0)
                        .color(palette.primary.to_iced()),
                    column![
                        text("Duvarkağıdı & Ambiance Uyum")
                            .size(12.0)
                            .color(palette.text_primary.to_iced()),
                        text("Canlı Renk & Atmosferik Senkronizasyon")
                            .size(9.0)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(1),
                    horizontal_space(),
                    button(
                        text("‹ Önceki")
                            .size(11.0)
                            .color(palette.text_primary.to_iced())
                    )
                    .padding([4, 8])
                    .style(|_, status| {
                        let bg = match status {
                            button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                            _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
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
                    .on_press(Message::TriggerWallpaperCycle),
                    button(
                        text("Sonraki ›")
                            .size(11.0)
                            .color(palette.on_primary().to_iced())
                    )
                    .padding([4, 8])
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
                    .on_press(Message::TriggerWallpaperCycle),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            )
            .padding([8, 12])
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.0, 0.0, 0.0, 0.22,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            });

            let tools_grid = row![
                hub_action_btn(
                    Icon::CAMERA,
                    "Ekran Kesiti",
                    Message::LaunchCommand("corten-snipper".into()),
                    palette
                ),
                hub_action_btn(
                    Icon::TERMINAL,
                    "Terminal",
                    Message::LaunchCommand("corten-terminal || alacritty || foot".into()),
                    palette
                ),
                hub_action_btn(
                    Icon::FOLDER,
                    "Dosyalar",
                    Message::LaunchCommand("corten-files".into()),
                    palette
                ),
                hub_action_btn(
                    Icon::ACTIVITY,
                    "Monitör",
                    Message::OpenFullTaskManager,
                    palette
                ),
            ]
            .spacing(8)
            .width(Length::Fill);

            let widget_gallery_row = button(
                row![
                    text(Icon::APP_WINDOW)
                        .font(LUCIDE_FONT)
                        .size(14.0)
                        .color(palette.primary.to_iced()),
                    text("Bileşen Galerisi (Widget Çekmecesi)")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    container(text("GALERİ").size(9.0).color(palette.primary.to_iced()))
                        .padding([2, 5])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.20
                            ))),
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
            .on_press(Message::LaunchCommand("corten-widgets --drawer".into()));

            let zen_toggle_row = button(
                row![
                    text(Icon::EYE)
                        .font(LUCIDE_FONT)
                        .size(14.0)
                        .color(palette.primary.to_iced()),
                    text("Masaüstü HUD Göstergelerini Değiştir (Zen Modu)")
                        .size(11.0)
                        .color(palette.text_primary.to_iced()),
                    horizontal_space(),
                    container(text("HUD").size(9.0).color(palette.primary.to_iced()))
                        .padding([2, 5])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.20
                            ))),
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
            .on_press(Message::LaunchCommand(
                "pkill -USR1 -x corten-widgets || true".into(),
            ));

            let hub_card = container(
                column![
                    header,
                    wallpaper_row,
                    tools_grid,
                    widget_gallery_row,
                    zen_toggle_row
                ]
                .spacing(10),
            )
            .padding([14, 18])
            .width(Length::Fixed(360.0))
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

            iced::widget::column![reveal_card(hub_card, reveal_max_h), dock_cluster]
                .spacing(popover_lift_gap)
                .align_x(Alignment::Center)
                .into()
        } else if self.show_app_drawer {
            let query = self.drawer_search.trim().to_ascii_lowercase();
            let active_cat = self.drawer_category.as_str();
            let drawer_eased = self
                .drawer_transition
                .emphasized_decelerate()
                .clamp(0.0, 1.0);
            let animated_drawer_h = (48.0 + 372.0 * drawer_eased).round();

            let header_row = row![
                text(Icon::SEARCH)
                    .font(LUCIDE_FONT)
                    .size(14.0)
                    .color(palette.primary.to_iced()),
                text_input("Uygulama ara...", &self.drawer_search)
                    .on_input(Message::DrawerSearchChanged)
                    .padding([7, 12])
                    .size(13.0)
                    .width(Length::Fill)
                    .style(move |_, status| {
                        let focused = matches!(status, text_input::Status::Focused);
                        let hovered = matches!(status, text_input::Status::Hovered);
                        text_input::Style {
                            background: iced::Background::Color(iced::Color::from_rgba(
                                1.0,
                                1.0,
                                1.0,
                                if focused {
                                    0.10
                                } else if hovered {
                                    0.08
                                } else {
                                    0.06
                                },
                            )),
                            border: iced::Border {
                                color: if focused {
                                    palette.primary.to_iced()
                                } else {
                                    iced::Color::from_rgba(1.0, 1.0, 1.0, 0.14)
                                },
                                width: if focused { 1.5 } else { 1.0 },
                                radius: iced::border::Radius::from(10.0),
                            },
                            icon: palette.text_muted.to_iced(),
                            placeholder: palette.text_muted.to_iced(),
                            value: palette.text_primary.to_iced(),
                            selection: iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.35,
                            ),
                        }
                    }),
                button(
                    text(Icon::X)
                        .font(LUCIDE_FONT)
                        .size(13.0)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([6, 9])
                .style(|_, status| button::Style {
                    background: match status {
                        button::Status::Hovered => Some(iced::Background::Color(
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                        )),
                        _ => None,
                    },
                    border: iced::Border {
                        radius: iced::border::Radius::from(8.0),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .on_press(Message::ToggleAppDrawer),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

            let cats = [
                ("all", "Tümü"),
                ("internet", "İnternet"),
                ("dev", "Geliştirme"),
                ("media", "Medya"),
                ("system", "Sistem"),
            ];
            let mut cat_row = row![].spacing(6).align_y(Alignment::Center);
            for (code, label) in cats {
                let is_sel = active_cat == code;
                cat_row = cat_row.push(
                    button(text(label).size(11.0).color(if is_sel {
                        palette.on_primary().to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }))
                    .padding([4, 10])
                    .style(move |_, status| {
                        let bg = if is_sel {
                            palette.primary.to_iced()
                        } else if matches!(status, button::Status::Hovered) {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12)
                        } else {
                            iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05)
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                radius: iced::border::Radius::from(8.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(Message::DrawerCategorySelected(code.to_string())),
                );
            }

            let filtered: Vec<&DrawerAppEntry> = self
                .drawer_apps
                .iter()
                .filter(|app| {
                    let cat_ok = active_cat == "all" || app.category == active_cat;
                    let q_ok = query.is_empty()
                        || app.name.to_ascii_lowercase().contains(&query)
                        || app.exec.to_ascii_lowercase().contains(&query);
                    cat_ok && q_ok
                })
                .collect();

            let grid_content: Element<'_, Self::Message> =
                if self.drawer_loading && self.drawer_apps.is_empty() {
                    container(
                        text("Uygulamalar yükleniyor...")
                            .size(13.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(280.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
                } else if filtered.is_empty() {
                    container(
                        text("Eşleşen uygulama bulunamadı")
                            .size(13.0)
                            .color(palette.text_muted.to_iced()),
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(280.0))
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
                } else {
                    // Explicit fixed tile dimensions inside scrollable so Iced 0.13 never collapses rows!
                    let mut rows_col = iced::widget::column![].spacing(8);
                    for chunk in filtered.chunks(5) {
                        let mut grid_row = row![].spacing(8).align_y(Alignment::Center);
                        for app in chunk {
                            let icon_elem: Element<'_, Self::Message> =
                                if let Some(ref p) = app.icon_path {
                                    iced::widget::image(image::Handle::from_path(p))
                                        .width(Length::Fixed(34.0))
                                        .height(Length::Fixed(34.0))
                                        .into()
                                } else {
                                    text(app.icon_fallback)
                                        .font(LUCIDE_FONT)
                                        .size(26.0)
                                        .color(palette.primary.to_iced())
                                        .into()
                                };
                            let short_name = if app.name.chars().count() > 13 {
                                format!("{}…", app.name.chars().take(12).collect::<String>())
                            } else {
                                app.name.clone()
                            };
                            let exec_clone = app.exec.clone();
                            let tile = button(
                                iced::widget::column![
                                    container(icon_elem)
                                        .width(Length::Fixed(40.0))
                                        .height(Length::Fixed(40.0))
                                        .center_x(Length::Fixed(40.0))
                                        .center_y(Length::Fixed(40.0)),
                                    text(short_name)
                                        .size(11.0)
                                        .color(palette.text_primary.to_iced()),
                                ]
                                .spacing(5)
                                .align_x(Alignment::Center),
                            )
                            .width(Length::Fixed(116.0))
                            .height(Length::Fixed(78.0))
                            .padding([8, 4])
                            .style(|_, status| {
                                let bg = match status {
                                    button::Status::Pressed => {
                                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.18)
                                    }
                                    button::Status::Hovered => {
                                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10)
                                    }
                                    _ => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03),
                                };
                                button::Style {
                                    background: Some(iced::Background::Color(bg)),
                                    border: iced::Border {
                                        color: iced::Color::from_rgba(
                                            1.0,
                                            1.0,
                                            1.0,
                                            if matches!(status, button::Status::Hovered) {
                                                0.16
                                            } else {
                                                0.05
                                            },
                                        ),
                                        width: 1.0,
                                        radius: iced::border::Radius::from(10.0),
                                    },
                                    ..Default::default()
                                }
                            })
                            .on_press(Message::LaunchDrawerApp(exec_clone));
                            grid_row = grid_row.push(tile);
                        }
                        rows_col = rows_col.push(grid_row);
                    }
                    iced::widget::scrollable(rows_col)
                        .height(Length::Fixed(316.0))
                        .width(Length::Fill)
                        .into()
                };

            let drawer_card = container(
                iced::widget::column![header_row, cat_row, grid_content]
                    .spacing(10)
                    .width(Length::Fill),
            )
            .padding([14, 16])
            .width(Length::Fixed(660.0))
            .height(Length::Fixed(animated_drawer_h))
            .clip(true)
            .style(move |_| {
                let mut style = acrylic_glass_style(palette, 18.0);
                if let Some(iced::Background::Color(color)) = style.background {
                    style.background = Some(iced::Background::Color(iced::Color::from_rgba(
                        color.r,
                        color.g,
                        color.b,
                        color.a * (0.25 + 0.75 * drawer_eased),
                    )));
                }
                style
            });

            let drawer_handle_down = button(
                container(
                    text(Icon::ARROW_DOWN)
                        .font(LUCIDE_FONT)
                        .size(10.0)
                        .color(palette.primary.to_iced()),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::Fixed(48.0))
            .height(Length::Fixed(14.0))
            .padding(0)
            .style(move |_, status| button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09,
                    0.10,
                    0.13,
                    if matches!(status, button::Status::Hovered) {
                        0.96
                    } else {
                        0.84
                    },
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.45,
                    ),
                    width: 1.0,
                    radius: iced::border::Radius::from(7.0),
                },
                ..Default::default()
            })
            .on_press(Message::ToggleAppDrawer);

            let drawer_area = mouse_area(
                container(
                    iced::widget::column![
                        mouse_area(drawer_card).on_press(Message::NoOp),
                        drawer_handle_down
                    ]
                    .spacing(4)
                    .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .align_x(Alignment::Center),
            )
            .on_press(Message::ToggleAppDrawer);
            iced::widget::column![drawer_area, dock_cluster]
                .spacing(4)
                .align_x(Alignment::Center)
                .into()
        } else {
            let mut bar_content = iced::widget::column![]
                .spacing(8)
                .align_x(Alignment::Center);
            if let Some((kind, lvl, _)) = &self.osd_state {
                let osd_pill = match kind {
                    OsdKind::Workspace(ws) => {
                        let mut ws_chips = row![].spacing(6).align_y(Alignment::Center);
                        for i in 1..=5 {
                            let is_curr = i == *ws;
                            let chip =
                                container(text(format!("{}", i)).size(12.0).color(if is_curr {
                                    palette.primary.to_iced()
                                } else {
                                    palette.text_muted.to_iced()
                                }))
                                .padding([2, 8])
                                .style(move |_| {
                                    container::Style {
                                        background: Some(iced::Background::Color(if is_curr {
                                            iced::Color::from_rgba(
                                                palette.primary.r,
                                                palette.primary.g,
                                                palette.primary.b,
                                                0.22,
                                            )
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
                                    }
                                });
                            ws_chips = ws_chips.push(chip);
                        }

                        container(
                            row![
                                text("⊞").size(14.0).color(palette.primary.to_iced()),
                                text(format!("Masaüstü {}", ws))
                                    .size(12.0)
                                    .color(palette.text_primary.to_iced()),
                                ws_chips,
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                        )
                        .padding([6, 16])
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                0.09, 0.10, 0.12, 0.95,
                            ))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.55,
                                ),
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
                    OsdKind::Toast(msg) => container(
                        row![text(msg).size(12.0).color(palette.text_primary.to_iced()),]
                            .spacing(8)
                            .align_y(Alignment::Center),
                    )
                    .padding([6, 18])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            0.09, 0.10, 0.12, 0.96,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.6,
                            ),
                            width: 1.5,
                            radius: iced::border::Radius::from(16.0),
                        },
                        shadow: iced::Shadow {
                            color: iced::Color::from_rgba(
                                palette.primary.r,
                                palette.primary.g,
                                palette.primary.b,
                                0.25,
                            ),
                            offset: iced::Vector::new(0.0, 4.0),
                            blur_radius: 18.0,
                        },
                        ..Default::default()
                    }),
                    OsdKind::MicMute(is_muted) => {
                        let (icon, label, color) = if *is_muted {
                            (Icon::SHIELD, "Mikrofon Susturuldu", palette.danger)
                        } else {
                            (Icon::MIC, "Mikrofon Aktif", palette.success)
                        };
                        container(
                            row![
                                text(icon)
                                    .font(LUCIDE_FONT)
                                    .size(15.0)
                                    .color(color.to_iced()),
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
                    OsdKind::BatteryWarning(pct) => container(
                        row![
                            text(Icon::ZAP)
                                .font(LUCIDE_FONT)
                                .size(15.0)
                                .color(palette.warning.to_iced()),
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
                    }),
                    OsdKind::Volume | OsdKind::Brightness => {
                        let icon = match kind {
                            OsdKind::Volume => Icon::VOLUME,
                            OsdKind::Brightness => Icon::SUN,
                            _ => Icon::VOLUME,
                        };
                        let label = match kind {
                            OsdKind::Volume => "Ses",
                            OsdKind::Brightness => "Parlaklık",
                            _ => "",
                        };
                        let fill_pct = (self.osd_display_level / 100.0).clamp(0.0, 1.0);
                        let display_lvl = self.osd_display_level.round() as u8;
                        let bar_width = 110.0;
                        let filled_width = (fill_pct * bar_width).clamp(0.5, bar_width);

                        let liquid_bar = container(row![
                            container(text(""))
                                .width(Length::Fixed(filled_width))
                                .height(Length::Fixed(6.0))
                                .style(move |_| container::Style {
                                    background: Some(iced::Background::Color(
                                        palette.primary.to_iced()
                                    )),
                                    border: iced::Border {
                                        radius: iced::border::Radius::from(3.0),
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                }),
                        ])
                        .width(Length::Fixed(bar_width))
                        .height(Length::Fixed(6.0))
                        .style(|_| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                1.0, 1.0, 1.0, 0.12,
                            ))),
                            border: iced::Border {
                                radius: iced::border::Radius::from(3.0),
                                ..Default::default()
                            },
                            ..Default::default()
                        });

                        container(
                            row![
                                text(icon)
                                    .font(LUCIDE_FONT)
                                    .size(15.0)
                                    .color(palette.primary.to_iced()),
                                text(label).size(12.0).color(palette.text_muted.to_iced()),
                                liquid_bar,
                                text(format!("{}%", display_lvl.max(*lvl)))
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
                            let app_name = if app_id.is_empty() {
                                "Uygulama"
                            } else {
                                app_id.as_str()
                            };
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
                                    text(short_title)
                                        .size(10.0)
                                        .color(palette.text_muted.to_iced()),
                                ]
                                .spacing(2)
                                .align_x(Alignment::Center),
                            )
                            .padding([6, 12])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(if is_curr {
                                    iced::Color::from_rgba(
                                        palette.primary.r,
                                        palette.primary.g,
                                        palette.primary.b,
                                        0.25,
                                    )
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
                            background: Some(iced::Background::Color(iced::Color::from_rgba(
                                0.09, 0.10, 0.12, 0.96,
                            ))),
                            border: iced::Border {
                                color: iced::Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.6,
                                ),
                                width: 1.5,
                                radius: iced::border::Radius::from(16.0),
                            },
                            shadow: iced::Shadow {
                                color: iced::Color::from_rgba(
                                    palette.primary.r,
                                    palette.primary.g,
                                    palette.primary.b,
                                    0.25,
                                ),
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
        } else if self.show_control_center
            || self.show_power_menu
            || self.show_calendar
            || self.show_clipboard_modal
            || self.show_notification_center
            || self.show_downloads_stack
            || self.show_mini_task_manager
            || self.dock_context_menu.is_some()
            || self.active_window_previews.is_some()
            || self.show_net_speed_details
            || self.show_desktop_context_hub
            || self.show_app_drawer
        {
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
    use super::{SystemPowerAction, launch_tray_application};

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
        use super::{ActiveApp, is_matching_window};

        // Files windows with various app_id and titles
        assert!(is_matching_window(
            "corten-files",
            "Corten Files",
            ActiveApp::Files
        ));
        assert!(!is_matching_window(
            "corten-files",
            "Corten Files",
            ActiveApp::Settings
        ));
        assert!(is_matching_window("", "Corten Files", ActiveApp::Files));
        assert!(!is_matching_window("", "Corten Files", ActiveApp::Settings));

        // Settings windows with various app_id and titles
        assert!(is_matching_window(
            "corten-settings",
            "Corten Settings",
            ActiveApp::Settings
        ));
        assert!(!is_matching_window(
            "corten-settings",
            "Corten Settings",
            ActiveApp::Files
        ));
        assert!(is_matching_window(
            "",
            "Corten Settings",
            ActiveApp::Settings
        ));
        assert!(!is_matching_window("", "Corten Settings", ActiveApp::Files));

        // Folder or file named settings opened in corten-files must NOT match Settings!
        assert!(is_matching_window(
            "corten-files",
            "settings.toml — Corten Files",
            ActiveApp::Files
        ));
        assert!(!is_matching_window(
            "corten-files",
            "settings.toml — Corten Files",
            ActiveApp::Settings
        ));
        assert!(!is_matching_window(
            "",
            "settings.toml — Corten Files",
            ActiveApp::Settings
        ));
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

    #[test]
    fn test_dock_width_stability_and_instant_control_center() {
        use super::*;
        let config_manager = ConfigManager::<PanelConfig>::load_or_default("panel").unwrap();
        let flags = PanelAppFlags {
            config_manager,
            ipc_rx: None,
            output_name: None,
        };
        let (mut app, _) = CortenPanelApp::new(flags);
        let base_width = app.calculate_dock_width();

        // Changing audio VU, volume slider, Wi-Fi details, network speed, or opening Control Center
        // must NEVER alter the dock width (preventing Wayland SizeChange jitter and dropped clicks)
        app.audio_vu_level = (0.85, 0.92);
        assert_eq!(app.calculate_dock_width(), base_width);

        app.show_volume_slider = true;
        assert_eq!(app.calculate_dock_width(), base_width);

        app.show_wifi_details = true;
        assert_eq!(app.calculate_dock_width(), base_width);

        app.net_speed_display = ("980.4 MB/s".into(), "412.9 MB/s".into());
        assert_eq!(app.calculate_dock_width(), base_width);

        let _ = app.update(Message::ToggleControlCenter);
        assert!(app.show_control_center);
        assert_eq!(app.calculate_dock_width(), base_width);
        let _ = app.view();
    }
}
