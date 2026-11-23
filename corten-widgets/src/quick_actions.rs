use std::process::Command;

#[derive(Debug, Clone)]
pub struct QuickActionItem {
    #[allow(dead_code)]
    pub id: &'static str,
    pub label: &'static str,
    pub icon_char: char,
    pub command: &'static str,
}

pub struct QuickActions;

impl QuickActions {
    pub const ACTIONS: &'static [QuickActionItem] = &[
        QuickActionItem {
            id: "terminal",
            label: "Terminal",
            icon_char: corten_theme::Icon::TERMINAL,
            command: "corten-terminal || alacritty || foot || xterm",
        },
        QuickActionItem {
            id: "snipper",
            label: "Ekran Kesiti",
            icon_char: corten_theme::Icon::CAMERA,
            command: "corten-snipper",
        },
        QuickActionItem {
            id: "files",
            label: "Dosyalar",
            icon_char: corten_theme::Icon::FOLDER,
            command: "corten-files",
        },
        QuickActionItem {
            id: "settings",
            label: "Ayarlar",
            icon_char: corten_theme::Icon::SETTINGS,
            command: "corten-settings",
        },
        QuickActionItem {
            id: "monitor",
            label: "Monitör",
            icon_char: corten_theme::Icon::ACTIVITY,
            command: "corten-monitor",
        },
        QuickActionItem {
            id: "browser",
            label: "Web",
            icon_char: corten_theme::Icon::GLOBE,
            command: "google-chrome || chromium || firefox",
        },
    ];

    pub fn launch(command: &str) {
        let _ = Command::new("sh").arg("-c").arg(command).spawn();
    }
}
