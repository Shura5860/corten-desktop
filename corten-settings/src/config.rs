use corten_config::{KeybindingsConfig, LayoutMode, WindowRuleConfig};
pub use corten_config::{
    AccessibilityConfig, AiSettingsConfig, AppearanceConfig, ColorFilterChoice, DisplaySettingsConfig, InputSettingsConfig, PowerSettingsConfig,
    ThemeModeChoice,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PanelPositionChoice {
    Top,
    #[default]
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PanelStyleChoice {
    #[default]
    Floating,
    Islands,
    Flush,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomScriptSettingsConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_script_interval")]
    pub interval_secs: u64,
    #[serde(default)]
    pub icon: Option<String>,
}

fn default_script_interval() -> u64 {
    30
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelSettingsConfig {
    pub height: u32,
    pub position: PanelPositionChoice,
    pub style: PanelStyleChoice,
    #[serde(default = "default_time_format")]
    pub time_format: String,
    #[serde(default = "default_date_format")]
    pub date_format: String,
    pub show_launcher_button: bool,
    pub show_audio: bool,
    pub show_network: bool,
    pub show_battery: bool,
    pub autohide: bool,
    #[serde(default = "default_autohide_mode")]
    pub autohide_mode: String,
    #[serde(default = "default_pinned_apps")]
    pub pinned_apps: Vec<String>,
    #[serde(default)]
    pub custom_scripts: Vec<CustomScriptSettingsConfig>,
}

fn default_time_format() -> String {
    "%H:%M".into()
}

fn default_date_format() -> String {
    "%a, %d %b".into()
}

fn default_autohide_mode() -> String {
    "none".into()
}

fn default_pinned_apps() -> Vec<String> {
    vec!["settings".into(), "files".into(), "corten-ai".into()]
}

impl Default for PanelSettingsConfig {
    fn default() -> Self {
        Self {
            height: 78,
            position: PanelPositionChoice::Bottom,
            style: PanelStyleChoice::Floating,
            time_format: default_time_format(),
            date_format: default_date_format(),
            show_launcher_button: true,
            show_audio: true,
            show_network: true,
            show_battery: false,
            autohide: false,
            autohide_mode: default_autohide_mode(),
            pinned_apps: default_pinned_apps(),
            custom_scripts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LauncherSettingsConfig {
    #[serde(default = "default_launcher_style", alias = "default_style")]
    pub style: String, // "spotlight", "dashboard", or "startmenu"
    pub width: u32,
    pub max_results: usize,
    #[serde(default = "default_placeholder")]
    pub placeholder: String,
    pub enable_clipboard_history: bool,
    #[serde(default = "default_clipboard_limit")]
    pub clipboard_history_limit: usize,
}

fn default_launcher_style() -> String {
    "startmenu".to_string()
}

fn default_placeholder() -> String {
    "Uygulama, komut veya panoda ara...".to_string()
}

fn default_clipboard_limit() -> usize {
    50
}

impl Default for LauncherSettingsConfig {
    fn default() -> Self {
        Self {
            style: "startmenu".to_string(),
            width: 480,
            max_results: 16,
            placeholder: default_placeholder(),
            enable_clipboard_history: true,
            clipboard_history_limit: 50,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NotifyPositionChoice {
    TopRight,
    TopLeft,
    #[default]
    BottomRight,
    BottomLeft,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifySettingsConfig {
    #[serde(default)]
    pub position: NotifyPositionChoice,
    pub default_timeout_ms: u64,
    pub max_visible: usize,
    pub card_width: u32,
    #[serde(default = "default_notify_margin")]
    pub margin: u32,
}

fn default_notify_margin() -> u32 {
    16
}

impl Default for NotifySettingsConfig {
    fn default() -> Self {
        Self {
            position: NotifyPositionChoice::BottomRight,
            default_timeout_ms: 5000,
            max_visible: 5,
            card_width: 360,
            margin: default_notify_margin(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompositorSettingsConfig {
    pub gap: i32,
    pub master_ratio: f32,
    #[serde(default)]
    pub layout_mode: LayoutMode,
    #[serde(default)]
    pub window_rules: Vec<WindowRuleConfig>,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

impl Default for CompositorSettingsConfig {
    fn default() -> Self {
        Self {
            gap: 12,
            master_ratio: 0.55,
            layout_mode: LayoutMode::Floating,
            window_rules: Vec::new(),
            keybindings: KeybindingsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WidgetsPositionChoice {
    #[default]
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetsSettingsConfig {
    #[serde(default = "default_true")]
    pub music_enabled: bool,
    #[serde(default = "default_true")]
    pub sysinfo_enabled: bool,
    #[serde(default = "default_true")]
    pub scratchpad_enabled: bool,
    #[serde(default = "default_true")]
    pub weather_enabled: bool,
    #[serde(default)]
    pub position: WidgetsPositionChoice,
    #[serde(default = "default_widgets_margin")]
    pub margin: (i32, i32, i32, i32), // top, right, bottom, left
}

fn default_true() -> bool {
    true
}

fn default_widgets_margin() -> (i32, i32, i32, i32) {
    (30, 30, 30, 30)
}

impl Default for WidgetsSettingsConfig {
    fn default() -> Self {
        Self {
            music_enabled: true,
            sysinfo_enabled: true,
            scratchpad_enabled: true,
            weather_enabled: true,
            position: WidgetsPositionChoice::TopRight,
            margin: default_widgets_margin(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundSettingsConfig {
    pub output_volume: u8,
    pub input_volume: u8,
    pub output_device: String,
    pub mute_output: bool,
}

impl Default for SoundSettingsConfig {
    fn default() -> Self {
        Self {
            output_volume: 75,
            input_volume: 80,
            output_device: "Default Audio Device".to_string(),
            mute_output: false,
        }
    }
}
