//! # Corten Config (`corten-config`)
//!
//! Standardized configuration loader, serializer, and live file watcher
//! for Corten Desktop components.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, info, warn};

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("XDG config directory could not be determined")]
    NoConfigDir,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("TOML deserialization error: {0}")]
    Deserialize(#[from] toml::de::Error),
    #[error("File watcher error: {0}")]
    Watch(#[from] notify::Error),
}

/// Helper function to resolve the standard config path for a component:
/// `~/.config/corten/<component_name>.toml`
pub fn get_config_path(component_name: &str) -> Result<PathBuf, ConfigError> {
    let base_dirs = directories::BaseDirs::new().ok_or(ConfigError::NoConfigDir)?;
    let mut path = base_dirs.config_dir().to_path_buf();
    path.push("corten");
    path.push(format!("{}.toml", component_name));
    Ok(path)
}

/// Validates that a configuration file is securely protected against unauthorized modification
/// by checking that it is not group- or world-writable (`0o022`).
pub fn is_secure_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mode = meta.permissions().mode();
            if mode & 0o022 != 0 {
                return false;
            }
        }
    }
    true
}

/// Generic configuration manager supporting typed load, save, and live watch
pub struct ConfigManager<T: Serialize + DeserializeOwned + Default + Clone + Send + 'static> {
    path: PathBuf,
    current: T,
}

impl<T: Serialize + DeserializeOwned + Default + Clone + Send + 'static> ConfigManager<T> {
    /// Loads configuration from file; if file does not exist, writes default config and returns it.
    pub fn load_or_default(component_name: &str) -> Result<Self, ConfigError> {
        let path = get_config_path(component_name)?;
        Self::load_or_default_from_path(&path)
    }

    /// Loads configuration from a specific path
    pub fn load_or_default_from_path(path: &Path) -> Result<Self, ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }

        let current = if path.exists() {
            debug!("Loading configuration from {:?}", path);
            let contents = fs::read_to_string(path)?;
            match toml::from_str(&contents) {
                Ok(cfg) => cfg,
                Err(err) => {
                    warn!(
                        "Failed to parse config file {:?}: {}. Falling back to default.",
                        path, err
                    );
                    let default_cfg = T::default();
                    let serialized = toml::to_string_pretty(&default_cfg)?;
                    fs::write(path, serialized)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
                    }
                    default_cfg
                }
            }
        } else {
            info!(
                "Config file {:?} not found. Creating default configuration.",
                path
            );
            let default_cfg = T::default();
            let serialized = toml::to_string_pretty(&default_cfg)?;
            fs::write(path, serialized)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
            }
            default_cfg
        };

        Ok(Self {
            path: path.to_path_buf(),
            current,
        })
    }

    /// Returns a reference to the currently loaded configuration
    pub fn get(&self) -> &T {
        &self.current
    }

    /// Saves the current configuration to disk
    pub fn save(&self) -> Result<(), ConfigError> {
        let serialized = toml::to_string_pretty(&self.current)?;
        fs::write(&self.path, serialized)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        info!("Saved configuration to {:?}", self.path);
        Ok(())
    }

    /// Updates the configuration in memory and writes it to disk
    pub fn update(&mut self, new_config: T) -> Result<(), ConfigError> {
        self.current = new_config;
        self.save()
    }

    /// Spawns a background file watcher and returns a receiver channel that emits updated config instances
    pub fn spawn_watcher(&self) -> Result<(RecommendedWatcher, Receiver<T>), ConfigError> {
        let (tx, rx) = channel();
        let path = self.path.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        debug!("Config file modified: {:?}", path);
                        // Brief pause to prevent reading while writer is flushing
                        std::thread::sleep(Duration::from_millis(50));
                        if let Ok(contents) = fs::read_to_string(&path) {
                            match toml::from_str::<T>(&contents) {
                                Ok(new_cfg) => {
                                    let _ = tx.send(new_cfg);
                                }
                                Err(err) => {
                                    error!("Failed to reload config on change: {}", err);
                                }
                            }
                        }
                    }
                }
                Err(e) => error!("File watch error: {}", e),
            },
            Config::default().with_poll_interval(Duration::from_secs(1)),
        )?;

        watcher.watch(&self.path, RecursiveMode::NonRecursive)?;
        info!(
            "Watching configuration file {:?} for live changes",
            self.path
        );
        Ok((watcher, rx))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeModeChoice {
    #[default]
    Dark,
    Light,
}

fn default_language() -> String {
    "tr".to_string()
}

fn default_patina() -> String {
    "rust".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppearanceConfig {
    pub theme_mode: ThemeModeChoice,
    pub accent_color: String,
    pub font_family: String,
    pub font_size: u16,
    pub wallpaper_path: String,
    pub blur_radius: u32,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_patina")]
    pub patina: String,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme_mode: ThemeModeChoice::Dark,
            accent_color: "#DE6B48".to_string(), // Corten Rust
            font_family: "Inter".to_string(),
            font_size: 14,
            wallpaper_path: "".to_string(),
            blur_radius: 24,
            language: default_language(),
            patina: default_patina(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputSettingsConfig {
    pub keyboard_layout: String,
    pub natural_scrolling: bool,
    pub tap_to_click: bool,
    pub pointer_speed: i32,
    pub remap_caps_to_ctrl: bool,
    pub virtual_keyboard_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutMode {
    #[default]
    Floating,
    MasterStack,
    Dwindle,
    Bsp,
    Monocle,
}

impl LayoutMode {
    pub fn next(self) -> Self {
        match self {
            Self::Floating => Self::MasterStack,
            Self::MasterStack => Self::Dwindle,
            Self::Dwindle => Self::Bsp,
            Self::Bsp => Self::Monocle,
            Self::Monocle => Self::Floating,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Floating => "Floating (Serbest/Yüzen)",
            Self::MasterStack => "Master-Stack",
            Self::Dwindle => "Dwindle (Spiral)",
            Self::Bsp => "BSP (Ağaç Bölme)",
            Self::Monocle => "Monocle (Tekli)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowRuleConfig {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub floating: Option<bool>,
    pub workspace: Option<usize>,
    pub fullscreen: Option<bool>,
    pub opacity_percent: Option<u8>,
    pub blur: Option<bool>,
}

impl WindowRuleConfig {
    pub fn opacity_float(&self) -> Option<f32> {
        self.opacity_percent.map(|p| (p.clamp(10, 100) as f32) / 100.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    pub terminal: String,
    pub file_manager: String,
    pub task_manager: String,
    pub launcher: String,
    pub dashboard: String,
    pub settings: String,
    pub close_window: String,
    pub focus_next: String,
    pub focus_previous: String,
    pub decrease_master: String,
    pub increase_master: String,
    pub toggle_floating: String,
    pub cycle_layout: String,
    pub screenshot: String,
    pub move_next_output: String,
    pub move_previous_output: String,
    pub toggle_fullscreen: String,
    pub minimize: String,
    pub lock_screen: String,
    pub workspace_1: String,
    pub workspace_2: String,
    pub workspace_3: String,
    pub workspace_4: String,
    pub workspace_5: String,
    pub workspace_6: String,
    pub workspace_7: String,
    pub workspace_8: String,
    pub workspace_9: String,
    pub move_to_workspace_1: String,
    pub move_to_workspace_2: String,
    pub move_to_workspace_3: String,
    pub move_to_workspace_4: String,
    pub move_to_workspace_5: String,
    pub move_to_workspace_6: String,
    pub move_to_workspace_7: String,
    pub move_to_workspace_8: String,
    pub move_to_workspace_9: String,
    pub swap_next: String,
    pub swap_previous: String,
    pub swap_master: String,
    pub ai_assistant: String,
    pub power_menu: String,
    pub window_overview: String,
    pub cheat_sheet: String,
    pub snap_left: String,
    pub snap_right: String,
    pub snap_up: String,
    pub snap_down: String,
    pub split_horizontal: String,
    pub split_vertical: String,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            terminal: "Super+Return".into(),
            file_manager: "Super+E".into(),
            task_manager: "Super+Escape".into(),
            launcher: "Super+Space".into(),
            dashboard: "Super+A".into(),
            ai_assistant: "Super+Alt+Space".into(),
            power_menu: "Super+X".into(),
            window_overview: "Super+W".into(),
            cheat_sheet: "Super+slash".into(),
            snap_left: "Super+Left".into(),
            snap_right: "Super+Right".into(),
            snap_up: "Super+Up".into(),
            snap_down: "Super+Down".into(),
            split_horizontal: "Super+B".into(),
            split_vertical: "Super+V".into(),
            settings: "Super+I".into(),
            close_window: "Super+Q".into(),
            focus_next: "Super+J".into(),
            focus_previous: "Super+K".into(),
            decrease_master: "Super+H".into(),
            increase_master: "Super+L".into(),
            toggle_floating: "Super+Shift+Space".into(),
            cycle_layout: "Super+Tab".into(),
            screenshot: "Super+Shift+S".into(),
            move_next_output: "Super+Shift+Right".into(),
            move_previous_output: "Super+Shift+Left".into(),
            toggle_fullscreen: "Super+F".into(),
            minimize: "Super+M".into(),
            lock_screen: "Super+Ctrl+L".into(),
            workspace_1: "Super+1".into(),
            workspace_2: "Super+2".into(),
            workspace_3: "Super+3".into(),
            workspace_4: "Super+4".into(),
            workspace_5: "Super+5".into(),
            workspace_6: "Super+6".into(),
            workspace_7: "Super+7".into(),
            workspace_8: "Super+8".into(),
            workspace_9: "Super+9".into(),
            move_to_workspace_1: "Super+Shift+1".into(),
            move_to_workspace_2: "Super+Shift+2".into(),
            move_to_workspace_3: "Super+Shift+3".into(),
            move_to_workspace_4: "Super+Shift+4".into(),
            move_to_workspace_5: "Super+Shift+5".into(),
            move_to_workspace_6: "Super+Shift+6".into(),
            move_to_workspace_7: "Super+Shift+7".into(),
            move_to_workspace_8: "Super+Shift+8".into(),
            move_to_workspace_9: "Super+Shift+9".into(),
            swap_next: "Super+Shift+J".into(),
            swap_previous: "Super+Shift+K".into(),
            swap_master: "Super+Shift+Return".into(),
        }
    }
}

impl KeybindingsConfig {
    /// Match a compositor key event against a named configured action.
    /// Chords use the stable human-readable form `Super+Shift+S`.
    pub fn matches(
        &self,
        action: &str,
        key: &str,
        logo: bool,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> bool {
        let Some(binding) = self.binding(action) else {
            return false;
        };
        let mut expected_logo = false;
        let mut expected_ctrl = false;
        let mut expected_alt = false;
        let mut expected_shift = false;
        let mut expected_key: Option<String> = None;
        for part in binding.split('+') {
            let normalized = part.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "super" | "logo" | "meta" => expected_logo = true,
                "ctrl" | "control" => expected_ctrl = true,
                "alt" | "mod1" => expected_alt = true,
                "shift" => expected_shift = true,
                value if !value.is_empty() => expected_key = Some(value.to_string()),
                _ => {}
            }
        }
        expected_key == Some(key.to_ascii_lowercase())
            && expected_logo == logo
            && expected_ctrl == ctrl
            && expected_alt == alt
            && expected_shift == shift
    }

    fn binding(&self, action: &str) -> Option<&str> {
        Some(match action {
            "terminal" => &self.terminal,
            "file_manager" => &self.file_manager,
            "task_manager" => &self.task_manager,
            "launcher" => &self.launcher,
            "dashboard" => &self.dashboard,
            "ai_assistant" => &self.ai_assistant,
            "settings" => &self.settings,
            "close_window" => &self.close_window,
            "focus_next" => &self.focus_next,
            "focus_previous" => &self.focus_previous,
            "decrease_master" => &self.decrease_master,
            "increase_master" => &self.increase_master,
            "toggle_floating" => &self.toggle_floating,
            "cycle_layout" => &self.cycle_layout,
            "screenshot" => &self.screenshot,
            "move_next_output" => &self.move_next_output,
            "move_previous_output" => &self.move_previous_output,
            "toggle_fullscreen" => &self.toggle_fullscreen,
            "minimize" => &self.minimize,
            "lock_screen" => &self.lock_screen,
            "workspace_1" => &self.workspace_1,
            "workspace_2" => &self.workspace_2,
            "workspace_3" => &self.workspace_3,
            "workspace_4" => &self.workspace_4,
            "workspace_5" => &self.workspace_5,
            "workspace_6" => &self.workspace_6,
            "workspace_7" => &self.workspace_7,
            "workspace_8" => &self.workspace_8,
            "workspace_9" => &self.workspace_9,
            "move_to_workspace_1" => &self.move_to_workspace_1,
            "move_to_workspace_2" => &self.move_to_workspace_2,
            "move_to_workspace_3" => &self.move_to_workspace_3,
            "move_to_workspace_4" => &self.move_to_workspace_4,
            "move_to_workspace_5" => &self.move_to_workspace_5,
            "move_to_workspace_6" => &self.move_to_workspace_6,
            "move_to_workspace_7" => &self.move_to_workspace_7,
            "move_to_workspace_8" => &self.move_to_workspace_8,
            "move_to_workspace_9" => &self.move_to_workspace_9,
            "swap_next" => &self.swap_next,
            "swap_previous" => &self.swap_previous,
            "swap_master" => &self.swap_master,
            "power_menu" => &self.power_menu,
            "window_overview" => &self.window_overview,
            "cheat_sheet" => &self.cheat_sheet,
            "snap_left" => &self.snap_left,
            "snap_right" => &self.snap_right,
            "snap_up" => &self.snap_up,
            "snap_down" => &self.snap_down,
            "split_horizontal" => &self.split_horizontal,
            "split_vertical" => &self.split_vertical,
            _ => return None,
        })
    }

    pub fn set_binding(&mut self, action: &str, chord: String) -> bool {
        let target = match action {
            "terminal" => &mut self.terminal,
            "file_manager" => &mut self.file_manager,
            "task_manager" => &mut self.task_manager,
            "launcher" => &mut self.launcher,
            "dashboard" => &mut self.dashboard,
            "ai_assistant" => &mut self.ai_assistant,
            "power_menu" => &mut self.power_menu,
            "window_overview" => &mut self.window_overview,
            "settings" => &mut self.settings,
            "close_window" => &mut self.close_window,
            "focus_next" => &mut self.focus_next,
            "focus_previous" => &mut self.focus_previous,
            "decrease_master" => &mut self.decrease_master,
            "increase_master" => &mut self.increase_master,
            "toggle_floating" => &mut self.toggle_floating,
            "cycle_layout" => &mut self.cycle_layout,
            "screenshot" => &mut self.screenshot,
            "move_next_output" => &mut self.move_next_output,
            "move_previous_output" => &mut self.move_previous_output,
            "toggle_fullscreen" => &mut self.toggle_fullscreen,
            "minimize" => &mut self.minimize,
            "lock_screen" => &mut self.lock_screen,
            "workspace_1" => &mut self.workspace_1,
            "workspace_2" => &mut self.workspace_2,
            "workspace_3" => &mut self.workspace_3,
            "workspace_4" => &mut self.workspace_4,
            "workspace_5" => &mut self.workspace_5,
            "workspace_6" => &mut self.workspace_6,
            "workspace_7" => &mut self.workspace_7,
            "workspace_8" => &mut self.workspace_8,
            "workspace_9" => &mut self.workspace_9,
            "move_to_workspace_1" => &mut self.move_to_workspace_1,
            "move_to_workspace_2" => &mut self.move_to_workspace_2,
            "move_to_workspace_3" => &mut self.move_to_workspace_3,
            "move_to_workspace_4" => &mut self.move_to_workspace_4,
            "move_to_workspace_5" => &mut self.move_to_workspace_5,
            "move_to_workspace_6" => &mut self.move_to_workspace_6,
            "move_to_workspace_7" => &mut self.move_to_workspace_7,
            "move_to_workspace_8" => &mut self.move_to_workspace_8,
            "move_to_workspace_9" => &mut self.move_to_workspace_9,
            "swap_next" => &mut self.swap_next,
            "swap_previous" => &mut self.swap_previous,
            "swap_master" => &mut self.swap_master,
            "snap_left" => &mut self.snap_left,
            "snap_right" => &mut self.snap_right,
            "snap_up" => &mut self.snap_up,
            "snap_down" => &mut self.snap_down,
            "split_horizontal" => &mut self.split_horizontal,
            "split_vertical" => &mut self.split_vertical,
            _ => return false,
        };
        *target = chord;
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettingsConfig {
    pub resolution: String,
    pub scale_pct: u32,
    pub night_light: bool,
    pub auto_night_light: bool,
    pub brightness_pct: u8,
    pub secondary_output: String,
    pub secondary_scale_pct: u32,
    pub arrangement: String,
    #[serde(default)]
    pub offset_x: i32,
    #[serde(default)]
    pub offset_y: i32,
    pub vrr: bool,
    pub allow_tearing: bool,
    pub icc_profile: Option<String>,
    pub hdr_enabled: bool,
    pub color_space: String,
    pub max_luminance_nits: Option<u32>,
}

impl Default for DisplaySettingsConfig {
    fn default() -> Self {
        Self {
            resolution: "1920x1080@60Hz".into(),
            scale_pct: 100,
            night_light: false,
            auto_night_light: false,
            brightness_pct: 100,
            secondary_output: "None".into(),
            secondary_scale_pct: 100,
            arrangement: "extend-right".into(),
            offset_x: 0,
            offset_y: 0,
            vrr: false,
            allow_tearing: false,
            icc_profile: None,
            hdr_enabled: false,
            color_space: "srgb".into(),
            max_luminance_nits: Some(400),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerSettingsConfig {
    pub screen_sleep_mins: u32,
    pub suspend_mins: u32,
    pub power_profile: String,
}

impl Default for PowerSettingsConfig {
    fn default() -> Self {
        Self {
            screen_sleep_mins: 15,
            suspend_mins: 30,
            power_profile: "balanced".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettingsConfig {
    pub enabled: bool,
    pub model_tier: String, // "eco", "balanced", "pro", "custom"
    pub backend_url: String,
    pub custom_model_path: String,
    pub auto_gpu_release_on_game: bool,
    pub idle_unload_mins: u32,
}

impl Default for AiSettingsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model_tier: "eco".into(),
            backend_url: "http://127.0.0.1:8000".into(),
            custom_model_path: String::new(),
            auto_gpu_release_on_game: true,
            idle_unload_mins: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ColorFilterChoice {
    #[default]
    None,
    Protanopia,
    Deuteranopia,
    Tritanopia,
    Monochrome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessibilityConfig {
    pub high_contrast: bool,
    pub color_filter: ColorFilterChoice,
    pub font_scale: f32,
    pub large_cursor: bool,
    pub high_vis_focus_ring: bool,
    pub sound_feedback: bool,
    pub screen_reader: bool,
}

impl Default for AccessibilityConfig {
    fn default() -> Self {
        Self {
            high_contrast: false,
            color_filter: ColorFilterChoice::None,
            font_scale: 1.0,
            large_cursor: false,
            high_vis_focus_ring: false,
            sound_feedback: false,
            screen_reader: false,
        }
    }
}

impl Default for InputSettingsConfig {
    fn default() -> Self {
        Self {
            keyboard_layout: "tr".to_string(),
            natural_scrolling: true,
            tap_to_click: true,
            pointer_speed: 0,
            remap_caps_to_ctrl: true,
            virtual_keyboard_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
    struct SampleConfig {
        name: String,
        count: u32,
    }

    #[test]
    fn test_load_and_save() {
        let tmp_dir = std::env::temp_dir().join("corten_test_config");
        let path = tmp_dir.join("sample.toml");

        let manager = ConfigManager::<SampleConfig>::load_or_default_from_path(&path).unwrap();
        assert_eq!(manager.get().name, "");
        assert_eq!(manager.get().count, 0);

        let updated = SampleConfig {
            name: "corten".into(),
            count: 42,
        };
        let mut manager = manager;
        manager.update(updated).unwrap();

        let reloaded = ConfigManager::<SampleConfig>::load_or_default_from_path(&path).unwrap();
        assert_eq!(reloaded.get().name, "corten");
        assert_eq!(reloaded.get().count, 42);

        let _ = fs::remove_dir_all(tmp_dir);
    }

    #[test]
    fn keybindings_match_configured_chords() {
        let bindings = KeybindingsConfig::default();
        assert!(bindings.matches("launcher", "space", true, false, false, false));
        assert!(bindings.matches("screenshot", "s", true, false, false, true));
        assert!(!bindings.matches("launcher", "space", false, false, false, false));
        assert!(!bindings.matches("launcher", "a", true, false, false, false));
    }

    #[test]
    fn keybindings_support_non_logo_modifiers() {
        let bindings = KeybindingsConfig {
            terminal: "Ctrl+Alt+Return".into(),
            ..Default::default()
        };
        assert!(bindings.matches("terminal", "return", false, true, true, false));
        assert!(!bindings.matches("terminal", "return", true, true, true, false));
    }

    #[test]
    fn test_accessibility_config_defaults_and_serde() {
        let a11y = AccessibilityConfig::default();
        assert!(!a11y.high_contrast);
        assert_eq!(a11y.font_scale, 1.0);
        assert_eq!(a11y.color_filter, ColorFilterChoice::None);

        let serialized = toml::to_string(&a11y).unwrap();
        let deserialized: AccessibilityConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(a11y, deserialized);
    }
}
