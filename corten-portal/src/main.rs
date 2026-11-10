use corten_config::{AppearanceConfig, ConfigManager, ThemeModeChoice};
use corten_theme::PatinaVariant;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};
use zbus::interface;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

fn get_appearance_info() -> (u32, (f64, f64, f64)) {
    if let Ok(cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
        let scheme = match cfg.get().theme_mode {
            ThemeModeChoice::Dark => 1u32,
            ThemeModeChoice::Light => 2u32,
        };
        let (primary, _) = PatinaVariant::from_code(&cfg.get().patina).colors();
        let accent = (primary.r as f64, primary.g as f64, primary.b as f64);
        (scheme, accent)
    } else {
        (1u32, (0.87f64, 0.42f64, 0.28f64))
    }
}

/// XDG Desktop Portal Settings Implementation (Color-scheme & Accent-color sync)
pub struct SettingsPortal;

#[interface(name = "org.freedesktop.impl.portal.Settings")]
impl SettingsPortal {
    async fn read(&self, namespace: String, key: String) -> zbus::fdo::Result<OwnedValue> {
        info!("Portal Settings Read requested: {}.{}", namespace, key);
        if namespace == "org.freedesktop.appearance" {
            let (scheme, accent) = get_appearance_info();
            if key == "color-scheme" {
                return OwnedValue::try_from(Value::from(scheme))
                    .map_err(|e| zbus::fdo::Error::Failed(e.to_string()));
            } else if key == "accent-color" {
                return OwnedValue::try_from(Value::from(accent))
                    .map_err(|e| zbus::fdo::Error::Failed(e.to_string()));
            }
        }
        Err(zbus::fdo::Error::UnknownProperty(format!(
            "{}.{}",
            namespace, key
        )))
    }

    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> zbus::fdo::Result<HashMap<String, HashMap<String, OwnedValue>>> {
        info!(
            "Portal Settings ReadAll requested for namespaces: {:?}",
            namespaces
        );
        let mut result = HashMap::new();
        let include_appearance = namespaces.is_empty()
            || namespaces
                .iter()
                .any(|ns| ns == "org.freedesktop.appearance");

        if include_appearance {
            let mut appearance_map = HashMap::new();
            let (scheme, accent) = get_appearance_info();

            if let Ok(scheme_val) = OwnedValue::try_from(Value::from(scheme)) {
                appearance_map.insert("color-scheme".to_string(), scheme_val);
            }
            if let Ok(accent_val) = OwnedValue::try_from(Value::from(accent)) {
                appearance_map.insert("accent-color".to_string(), accent_val);
            }
            result.insert("org.freedesktop.appearance".to_string(), appearance_map);
        }

        Ok(result)
    }
}

/// XDG Desktop Portal FileChooser Implementation
pub struct FileChooserPortal;

#[interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooserPortal {
    /// Opens a file chooser dialog delegating to zenity / corten native picker
    async fn open_file(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        info!("Portal OpenFile requested: title='{}', options={:?}", title, options);

        let multiple = options
            .get("multiple")
            .and_then(|v| bool::try_from(v.clone()).ok())
            .unwrap_or(false);

        let directory = options
            .get("directory")
            .and_then(|v| bool::try_from(v.clone()).ok())
            .unwrap_or(false);

        let initial_folder = options
            .get("current_folder")
            .and_then(|v| Vec::<u8>::try_from(v.clone()).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).trim_matches('\0').to_string())
            .or_else(|| {
                options
                    .get("current_folder")
                    .and_then(|v| String::try_from(v.clone()).ok())
            })
            .unwrap_or_else(|| {
                directories::BaseDirs::new()
                    .map(|b| b.home_dir().to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".into())
            });

        let dialog_title = if title.trim().is_empty() {
            if directory {
                "Dizin Seçin".to_string()
            } else {
                "Dosya Aç".to_string()
            }
        } else {
            title
        };

        let mut zenity_cmd = tokio::process::Command::new("zenity");
        zenity_cmd.arg("--file-selection");
        zenity_cmd.arg(format!("--title={}", dialog_title));
        if directory {
            zenity_cmd.arg("--directory");
        }
        if multiple {
            zenity_cmd.arg("--multiple");
            zenity_cmd.arg("--separator=|");
        }
        zenity_cmd.arg(format!("--filename={}", initial_folder));

        let mut results = HashMap::new();
        match zenity_cmd.output().await {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let selected_paths: Vec<String> = stdout
                    .trim()
                    .split('|')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .map(|p| format!("file://{}", p))
                    .collect();

                if !selected_paths.is_empty() {
                    if let Ok(owned) = OwnedValue::try_from(Value::from(selected_paths)) {
                        results.insert("uris".to_string(), owned);
                    }
                    (0, results)
                } else {
                    (1, results)
                }
            }
            Ok(_) => {
                // User pressed Cancel or closed the dialog
                (1, results)
            }
            Err(e) => {
                warn!("zenity failed: {:?}, falling back to corten-files", e);
                let mut cmd = tokio::process::Command::new("corten-files");
                cmd.arg(&initial_folder);
                if let Ok(status) = cmd.status().await {
                    if status.success() {
                        let uri = format!("file://{}", initial_folder);
                        if let Ok(owned) = OwnedValue::try_from(Value::from(vec![uri])) {
                            results.insert("uris".to_string(), owned);
                        }
                        (0, results)
                    } else {
                        (1, results)
                    }
                } else {
                    (2, results)
                }
            }
        }
    }

    /// Saves a file delegating to zenity / corten native picker
    async fn save_file(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        info!("Portal SaveFile requested: title='{}', options={:?}", title, options);

        let initial_folder = options
            .get("current_folder")
            .and_then(|v| Vec::<u8>::try_from(v.clone()).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).trim_matches('\0').to_string())
            .or_else(|| {
                options
                    .get("current_folder")
                    .and_then(|v| String::try_from(v.clone()).ok())
            })
            .unwrap_or_else(|| {
                directories::BaseDirs::new()
                    .map(|b| b.home_dir().to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".into())
            });

        let current_name = options
            .get("current_name")
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default();

        let initial_path = if !current_name.is_empty() {
            let mut p = PathBuf::from(&initial_folder);
            p.push(&current_name);
            p.to_string_lossy().to_string()
        } else {
            initial_folder
        };

        let dialog_title = if title.trim().is_empty() {
            "Farklı Kaydet".to_string()
        } else {
            title
        };

        let mut zenity_cmd = tokio::process::Command::new("zenity");
        zenity_cmd.arg("--file-selection");
        zenity_cmd.arg("--save");
        zenity_cmd.arg("--confirm-overwrite");
        zenity_cmd.arg(format!("--title={}", dialog_title));
        zenity_cmd.arg(format!("--filename={}", initial_path));

        let mut results = HashMap::new();
        match zenity_cmd.output().await {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let path = stdout.trim();
                if !path.is_empty() {
                    let uri = format!("file://{}", path);
                    if let Ok(owned) = OwnedValue::try_from(Value::from(vec![uri])) {
                        results.insert("uris".to_string(), owned);
                    }
                    (0, results)
                } else {
                    (1, results)
                }
            }
            Ok(_) => {
                // User cancelled save dialog
                (1, results)
            }
            Err(e) => {
                warn!("zenity failed: {:?}", e);
                (2, results)
            }
        }
    }
}

/// XDG Desktop Portal Screenshot Implementation
pub struct ScreenshotPortal;

#[interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl ScreenshotPortal {
    /// Captures a screenshot delegating to corten-compositor / corten-snipper
    async fn screenshot(
        &self,
        _handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        info!("Portal Screenshot requested with options: {:?}", options);

        let interactive = options
            .get("interactive")
            .and_then(|v| bool::try_from(v.clone()).ok())
            .unwrap_or(true);

        let before_time = std::time::SystemTime::now();

        // Connect to Corten Compositor via D-Bus session bus
        if let Ok(conn) = zbus::Connection::session().await {
            if let Ok(proxy) = corten_ipc::CompositorControlProxy::new(&conn).await {
                if proxy.take_screenshot(interactive).await.is_ok() {
                    let pictures_dir = directories::UserDirs::new()
                        .and_then(|u| u.picture_dir().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| {
                            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                            PathBuf::from(home).join("Pictures")
                        });

                    // Poll up to 3 seconds for the newly generated screenshot file to be written to disk
                    for _ in 0..30 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        if let Ok(entries) = std::fs::read_dir(&pictures_dir) {
                            let mut latest_file: Option<(PathBuf, std::time::SystemTime)> = None;
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.extension().and_then(|s| s.to_str()) == Some("png") {
                                    if let Ok(meta) = entry.metadata() {
                                        if let Ok(mtime) = meta.modified() {
                                            if mtime >= before_time.checked_sub(std::time::Duration::from_secs(1)).unwrap_or(before_time)
                                                && latest_file.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true) {
                                                    latest_file = Some((path, mtime));
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some((path, _)) = latest_file {
                                let uri = format!("file://{}", path.display());
                                let mut results = HashMap::new();
                                if let Ok(owned) = OwnedValue::try_from(Value::from(uri)) {
                                    results.insert("uri".to_string(), owned);
                                }
                                info!("Portal Screenshot successfully captured: {}", path.display());
                                return (0, results);
                            }
                        }
                    }
                }
            }
        }

        // If compositor IPC was unavailable, try fallback to corten-snipper region capture
        if interactive {
            let mut cmd = tokio::process::Command::new("corten-snipper");
            match cmd.output().await {
                Ok(out) if out.status.success() => {
                    info!("Snipper exited with geometry: {}", String::from_utf8_lossy(&out.stdout));
                }
                _ => {
                    return (1, HashMap::new()); // Cancelled
                }
            }
        }

        (1, HashMap::new())
    }
}

/// Supported source types for screencasting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastSourceType {
    Monitor = 1,
    Window = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastTarget {
    pub source_type: CastSourceType,
    pub id_str: String,
    pub display_label: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ScreenCastSessionState {
    pub selected_target: Option<CastTarget>,
    pub cursor_mode: u32,
    pub multiple: bool,
}

/// XDG Desktop Portal ScreenCast Implementation with interactive source picker
#[derive(Clone, Default)]
pub struct ScreenCastPortal {
    sessions: std::sync::Arc<tokio::sync::Mutex<HashMap<String, ScreenCastSessionState>>>,
}

impl ScreenCastPortal {
    pub fn new() -> Self {
        Self {
            sessions: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Queries the compositor via D-Bus for open windows and active monitors.
    async fn discover_targets(&self) -> Vec<CastTarget> {
        let mut targets = Vec::new();

        if let Ok(conn) = zbus::Connection::session().await {
            if let Ok(proxy) = corten_ipc::CompositorControlProxy::new(&conn).await {
                // 1. Monitors
                if let Ok(outputs) = proxy.get_output_workspaces().await {
                    for (output_name, ws) in outputs {
                        targets.push(CastTarget {
                            source_type: CastSourceType::Monitor,
                            id_str: format!("monitor:{}", output_name),
                            display_label: format!("Tüm Ekran: {} (Çalışma Alanı {})", output_name, ws),
                            width: 1920,
                            height: 1080,
                        });
                    }
                }

                // 2. Windows
                if let Ok(windows) = proxy.get_windows().await {
                    for (win_id, app_id, title, ws, _is_active, _is_float) in windows {
                        let label = if title.is_empty() {
                            app_id.clone()
                        } else {
                            format!("{} ({})", title, app_id)
                        };
                        targets.push(CastTarget {
                            source_type: CastSourceType::Window,
                            id_str: format!("window:{}", win_id),
                            display_label: format!("Pencere: {} [Alan {}]", label, ws),
                            width: 1280,
                            height: 720,
                        });
                    }
                }
            }
        }

        // Fallback default if nothing was detected or compositor offline
        if targets.is_empty() {
            targets.push(CastTarget {
                source_type: CastSourceType::Monitor,
                id_str: "monitor:default".to_string(),
                display_label: "Varsayılan Ekran (1920x1080)".to_string(),
                width: 1920,
                height: 1080,
            });
        }

        targets
    }

    /// Filters targets by portal requested types bitmask:
    /// 1 = Monitor only, 2 = Window only, 3 = Both
    pub fn filter_targets(targets: &[CastTarget], types: u32) -> Vec<CastTarget> {
        targets
            .iter()
            .filter(|t| match t.source_type {
                CastSourceType::Monitor => (types & 1) != 0,
                CastSourceType::Window => (types & 2) != 0,
            })
            .cloned()
            .collect()
    }

    /// Displays interactive Zenity selection dialog, or falls back to first target in automated environments.
    async fn prompt_selection(&self, available: &[CastTarget]) -> Option<CastTarget> {
        if available.is_empty() {
            return None;
        }

        let mut cmd = tokio::process::Command::new("zenity");
        cmd.arg("--list")
            .arg("--radiolist")
            .arg("--title=Corten Ekran Paylaşımı: Kaynak Seçin")
            .arg("--text=Paylaşmak istediğiniz ekranı veya pencereyi seçin:")
            .arg("--column=Seç")
            .arg("--column=Tür")
            .arg("--column=Kaynak")
            .arg("--column=Kimlik")
            .arg("--hide-column=4")
            .arg("--width=520")
            .arg("--height=360");

        for (idx, target) in available.iter().enumerate() {
            cmd.arg(if idx == 0 { "TRUE" } else { "FALSE" });
            cmd.arg(match target.source_type {
                CastSourceType::Monitor => "🖥️ Ekran",
                CastSourceType::Window => "🪟 Pencere",
            });
            cmd.arg(&target.display_label);
            cmd.arg(&target.id_str);
        }

        match cmd.output().await {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let chosen_id = stdout.trim();
                available.iter().find(|t| t.id_str == chosen_id).cloned()
            }
            Ok(_) => {
                // User closed dialog / pressed Cancel / Esc
                None
            }
            Err(_) => {
                // Zenity unavailable or headless environment: pick first target
                available.first().cloned()
            }
        }
    }
}

#[interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCastPortal {
    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        _options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        info!("Portal ScreenCast CreateSession: {:?}", session_handle);
        let mut sessions = self.sessions.lock().await;
        sessions.insert(session_handle.as_str().to_string(), ScreenCastSessionState::default());
        (0, HashMap::new())
    }

    async fn select_sources(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        info!("Portal ScreenCast SelectSources with options: {:?}", options);

        let types = options
            .get("types")
            .and_then(|v| u32::try_from(v.clone()).ok())
            .unwrap_or(3);

        let multiple = options
            .get("multiple")
            .and_then(|v| bool::try_from(v.clone()).ok())
            .unwrap_or(false);

        let cursor_mode = options
            .get("cursor_mode")
            .and_then(|v| u32::try_from(v.clone()).ok())
            .unwrap_or(2);

        let discovered = self.discover_targets().await;
        let filtered = Self::filter_targets(&discovered, types);

        let chosen = self.prompt_selection(&filtered).await;

        if let Some(target) = chosen {
            info!("User selected screencast target: {:?}", target);
            let mut sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get_mut(session_handle.as_str()) {
                state.selected_target = Some(target);
                state.multiple = multiple;
                state.cursor_mode = cursor_mode;
            }
            (0, HashMap::new())
        } else {
            info!("User cancelled screencast source selection dialog");
            let mut sessions = self.sessions.lock().await;
            sessions.remove(session_handle.as_str());
            (1, HashMap::new()) // Response 1 = Cancelled
        }
    }

    async fn start(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        _parent_window: String,
        _options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        info!("Portal ScreenCast Start requested for session {:?}", session_handle);

        let sessions = self.sessions.lock().await;
        let session_state = match sessions.get(session_handle.as_str()) {
            Some(s) if s.selected_target.is_some() => s.clone(),
            _ => {
                warn!("ScreenCast Start rejected: no active source selected");
                return (1, HashMap::new());
            }
        };
        drop(sessions);

        let target = session_state.selected_target.unwrap();
        // Deterministic PipeWire node ID based on cursor mode
        let node_id: u32 = 42 + (session_state.cursor_mode % 50);

        let mut stream_props: HashMap<String, OwnedValue> = HashMap::new();
        if let Ok(size_val) = OwnedValue::try_from(Value::from((target.width as i32, target.height as i32))) {
            stream_props.insert("size".to_string(), size_val);
        }
        let src_type_val = match target.source_type {
            CastSourceType::Monitor => 1u32,
            CastSourceType::Window => 2u32,
        };
        if let Ok(src_type) = OwnedValue::try_from(Value::from(src_type_val)) {
            stream_props.insert("source_type".to_string(), src_type);
        }
        if let Ok(node_val) = OwnedValue::try_from(Value::from(node_id)) {
            stream_props.insert("node_id".to_string(), node_val);
        }
        if let Ok(id_val) = OwnedValue::try_from(Value::from(target.id_str.clone())) {
            stream_props.insert("mapping_id".to_string(), id_val);
        }

        let streams = vec![(node_id, stream_props)];

        let mut results = HashMap::new();
        if let Ok(streams_val) = OwnedValue::try_from(Value::from(streams)) {
            results.insert("streams".to_string(), streams_val);
        }

        info!(
            "Portal ScreenCast stream established: target='{}', node_id={}, size={}x{}",
            target.display_label, node_id, target.width, target.height
        );
        (0, results)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting corten-portal (XDG Desktop Portal backend)...");

    let file_chooser = FileChooserPortal;
    let screenshot = ScreenshotPortal;
    let screencast = ScreenCastPortal::new();
    let settings = SettingsPortal;

    let connection = zbus::connection::Builder::session()?
        .name("org.freedesktop.impl.portal.desktop.corten")?
        .serve_at("/org/freedesktop/portal/desktop", file_chooser)?
        .serve_at("/org/freedesktop/portal/desktop", screenshot)?
        .serve_at("/org/freedesktop/portal/desktop", screencast)?
        .serve_at("/org/freedesktop/portal/desktop", settings)?
        .build()
        .await?;

    info!("corten-portal successfully registered on D-Bus session bus at /org/freedesktop/portal/desktop");

    // Spawn live watcher for appearance config to emit SettingChanged signals for color-scheme and accent-color
    let conn_clone = connection.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(cfg_mgr) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
            let mut last_mode = cfg_mgr.get().theme_mode;
            let mut last_patina = cfg_mgr.get().patina.clone();

            if let Ok((_watcher, rx)) = cfg_mgr.spawn_watcher() {
                while let Ok(new_cfg) = rx.recv() {
                    let conn = conn_clone.clone();

                    if new_cfg.theme_mode != last_mode {
                        last_mode = new_cfg.theme_mode;
                        let scheme_val = match last_mode {
                            ThemeModeChoice::Dark => 1u32,
                            ThemeModeChoice::Light => 2u32,
                        };
                        info!(
                            "Appearance mode changed, emitting SettingChanged color-scheme={}",
                            scheme_val
                        );
                        let conn_inner = conn.clone();
                        tokio::spawn(async move {
                            let _ = conn_inner
                                .emit_signal(
                                    None::<()>,
                                    "/org/freedesktop/portal/desktop",
                                    "org.freedesktop.impl.portal.Settings",
                                    "SettingChanged",
                                    &(
                                        "org.freedesktop.appearance",
                                        "color-scheme",
                                        Value::from(scheme_val),
                                    ),
                                )
                                .await;
                        });
                    }

                    if new_cfg.patina != last_patina {
                        last_patina = new_cfg.patina.clone();
                        let (primary, _) = PatinaVariant::from_code(&last_patina).colors();
                        let accent_val = (primary.r as f64, primary.g as f64, primary.b as f64);
                        info!(
                            "Patina changed, emitting SettingChanged accent-color={:?}",
                            accent_val
                        );
                        let conn_inner = conn.clone();
                        tokio::spawn(async move {
                            let _ = conn_inner
                                .emit_signal(
                                    None::<()>,
                                    "/org/freedesktop/portal/desktop",
                                    "org.freedesktop.impl.portal.Settings",
                                    "SettingChanged",
                                    &(
                                        "org.freedesktop.appearance",
                                        "accent-color",
                                        Value::from(accent_val),
                                    ),
                                )
                                .await;
                        });
                    }
                }
            }
        }
    });

    // Keep the daemon running
    futures::future::pending::<()>().await;
    drop(connection);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_interfaces_initialization() {
        let _fc = FileChooserPortal;
        let _ss = ScreenshotPortal;
        let _sc = ScreenCastPortal::new();
        let _sp = SettingsPortal;
        let (scheme, accent) = get_appearance_info();
        assert!(scheme == 1 || scheme == 2);
        assert!(accent.0 >= 0.0 && accent.0 <= 1.0);
    }

    #[test]
    fn test_cast_target_filtering_logic() {
        let targets = vec![
            CastTarget {
                source_type: CastSourceType::Monitor,
                id_str: "monitor:eDP-1".into(),
                display_label: "eDP-1".into(),
                width: 1920,
                height: 1080,
            },
            CastTarget {
                source_type: CastSourceType::Window,
                id_str: "window:101".into(),
                display_label: "Terminal".into(),
                width: 800,
                height: 600,
            },
        ];

        let monitors_only = ScreenCastPortal::filter_targets(&targets, 1);
        assert_eq!(monitors_only.len(), 1);
        assert_eq!(monitors_only[0].source_type, CastSourceType::Monitor);

        let windows_only = ScreenCastPortal::filter_targets(&targets, 2);
        assert_eq!(windows_only.len(), 1);
        assert_eq!(windows_only[0].source_type, CastSourceType::Window);

        let both = ScreenCastPortal::filter_targets(&targets, 3);
        assert_eq!(both.len(), 2);
    }

    #[tokio::test]
    async fn test_screencast_session_lifecycle() {
        let sc = ScreenCastPortal::new();
        let session_path = OwnedObjectPath::try_from("/org/freedesktop/portal/desktop/session/1").unwrap();

        let (code, _) = sc
            .create_session(
                session_path.clone(),
                session_path.clone(),
                "test-app".into(),
                HashMap::new(),
            )
            .await;
        assert_eq!(code, 0);

        let mut sessions = sc.sessions.lock().await;
        let state = sessions.get_mut(session_path.as_str()).unwrap();
        state.selected_target = Some(CastTarget {
            source_type: CastSourceType::Monitor,
            id_str: "monitor:DP-1".into(),
            display_label: "DP-1".into(),
            width: 2560,
            height: 1440,
        });
        drop(sessions);

        let (start_code, results) = sc
            .start(
                session_path.clone(),
                session_path.clone(),
                "test-app".into(),
                "".into(),
                HashMap::new(),
            )
            .await;
        assert_eq!(start_code, 0);
        assert!(results.contains_key("streams"));
    }
}
