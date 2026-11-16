use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WidgetPosition {
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetsConfig {
    #[serde(default = "default_true")]
    pub music_enabled: bool,

    #[serde(default = "default_true")]
    pub sysinfo_enabled: bool,

    #[serde(default = "default_true")]
    pub weather_enabled: bool,

    #[serde(default = "default_true")]
    pub scratchpad_enabled: bool,

    #[serde(default = "default_true")]
    pub network_enabled: bool,

    #[serde(default = "default_true")]
    pub clock_enabled: bool,

    #[serde(default = "default_true")]
    pub gpu_enabled: bool,

    #[serde(default = "default_true")]
    pub weather_forecast_enabled: bool,

    #[serde(default = "default_true")]
    pub quick_actions_enabled: bool,

    #[serde(default = "default_true")]
    pub disk_enabled: bool,

    #[serde(default = "default_true")]
    pub pomodoro_enabled: bool,

    #[serde(default = "default_position")]
    pub position: WidgetPosition,

    #[serde(default = "default_margin")]
    pub margin: (i32, i32, i32, i32), // top, right, bottom, left
}

fn default_true() -> bool {
    true
}

fn default_position() -> WidgetPosition {
    WidgetPosition::TopRight
}

fn default_margin() -> (i32, i32, i32, i32) {
    (30, 24, 76, 24)
}

impl Default for WidgetsConfig {
    fn default() -> Self {
        Self {
            music_enabled: true,
            sysinfo_enabled: true,
            weather_enabled: true,
            scratchpad_enabled: true,
            network_enabled: true,
            clock_enabled: true,
            gpu_enabled: true,
            weather_forecast_enabled: true,
            quick_actions_enabled: true,
            disk_enabled: true,
            pomodoro_enabled: true,
            position: default_position(),
            margin: default_margin(),
        }
    }
}
