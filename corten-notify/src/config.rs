use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    TopRight,
    TopLeft,
    #[default]
    BottomRight,
    BottomLeft,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotifyConfig {
    /// Screen corner position for notification popups
    pub position: Position,
    /// Default display duration in milliseconds before auto-dismissal
    pub default_timeout_ms: u64,
    /// Maximum number of notification cards visible concurrently
    pub max_visible: usize,
    /// Width of the notification cards in pixels
    pub card_width: u32,
    /// Margin from screen edge in pixels
    pub margin: u32,
    /// Whether Do-Not-Disturb is enabled
    #[serde(default)]
    pub dnd: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            position: Position::BottomRight,
            default_timeout_ms: 5000,
            max_visible: 5,
            card_width: 360,
            margin: 16,
            dnd: false,
        }
    }
}
