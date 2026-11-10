use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LauncherStyle {
    Spotlight,
    Dashboard,
    #[default]
    StartMenu,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LauncherConfig {
    /// Style of launcher: compact Spotlight or expanded application dashboard
    pub style: LauncherStyle,
    /// Width of the modal in pixels
    pub width: u32,
    /// Maximum number of search results to display
    pub max_results: usize,
    /// Placeholder text for the search box
    pub placeholder: String,
    /// Whether to enable clipboard history integration mode (cliphist)
    pub enable_clipboard_history: bool,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            style: LauncherStyle::StartMenu,
            width: 740,
            max_results: 16,
            placeholder: "Uygulama, komut veya panoda ara...".into(),
            enable_clipboard_history: true,
        }
    }
}
