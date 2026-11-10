use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelStyle {
    /// Floating bar with screen margins, rounded corners, and frosted translucent background
    Floating,
    /// Transparent root with individually floating module pill capsules (macOS Tahoe/Vision style)
    Islands,
    /// Classic edge-to-edge full width panel
    Flush,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomScriptConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub on_click: Option<String>,
    #[serde(default)]
    pub on_right_click: Option<String>,
}

fn default_interval() -> u64 {
    30
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelConfig {
    /// Height of the panel/dock in pixels
    pub height: u32,
    /// Position on screen (top or bottom)
    pub position: PanelPosition,
    /// Visual style: "floating", "islands", or "flush"
    pub style: PanelStyle,
    /// Time format for clock (e.g. "%H:%M")
    pub time_format: String,
    /// Date format for calendar tooltip / display (e.g. "%a, %d %b")
    pub date_format: String,
    /// Whether to show the app launcher trigger icon
    pub show_launcher_button: bool,
    /// Whether to show audio volume indicator
    pub show_audio: bool,
    /// Whether to show network connectivity
    pub show_network: bool,
    /// Whether to show battery percentage
    pub show_battery: bool,
    /// Whether dock autohides when mouse leaves bottom area
    pub autohide: bool,
    /// Autohide behavior: "none", "always", or "dodge"
    #[serde(default = "default_autohide_mode")]
    pub autohide_mode: String,
    /// List of pinned application IDs
    #[serde(default = "default_pinned_apps")]
    pub pinned_apps: Vec<String>,
    /// Custom dynamic script applets (like waybar/i3blocks custom modules)
    #[serde(default)]
    pub custom_scripts: Vec<CustomScriptConfig>,
}

fn default_autohide_mode() -> String {
    "none".into()
}

fn default_pinned_apps() -> Vec<String> {
    vec![
        "settings".into(),
        "files".into(),
        "corten-ai".into(),
    ]
}

impl Default for PanelConfig {
    fn default() -> Self {
        Self {
            height: 78,
            position: PanelPosition::Bottom,
            style: PanelStyle::Floating,
            time_format: "%H:%M".into(),
            date_format: "%a, %d %b".into(),
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
