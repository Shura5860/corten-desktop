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
    #[serde(default)]
    pub music_enabled: bool,

    #[serde(default = "default_true")]
    pub sysinfo_enabled: bool,

    #[serde(default = "default_true")]
    pub weather_enabled: bool,

    #[serde(default)]
    pub scratchpad_enabled: bool,

    #[serde(default)]
    pub network_enabled: bool,

    #[serde(default = "default_true")]
    pub clock_enabled: bool,

    #[serde(default)]
    pub gpu_enabled: bool,

    #[serde(default)]
    pub weather_forecast_enabled: bool,

    #[serde(default = "default_true")]
    pub quick_actions_enabled: bool,

    #[serde(default)]
    pub disk_enabled: bool,

    #[serde(default)]
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
            music_enabled: false,
            sysinfo_enabled: true,
            weather_enabled: true,
            scratchpad_enabled: false,
            network_enabled: false,
            clock_enabled: true,
            gpu_enabled: false,
            weather_forecast_enabled: false,
            quick_actions_enabled: true,
            disk_enabled: false,
            pomodoro_enabled: false,
            position: default_position(),
            margin: default_margin(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_widgets_clean_preset() {
        let cfg = WidgetsConfig::default();
        assert!(cfg.clock_enabled);
        assert!(cfg.sysinfo_enabled);
        assert!(cfg.weather_enabled);
        assert!(cfg.quick_actions_enabled);
        assert!(!cfg.pomodoro_enabled);
        assert!(!cfg.disk_enabled);
        assert!(!cfg.gpu_enabled);
    }
}
