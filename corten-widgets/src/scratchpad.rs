use directories::ProjectDirs;
use std::fs;
use std::path::PathBuf;

pub struct ScratchpadService;

impl ScratchpadService {
    fn notes_path() -> PathBuf {
        if let Some(dirs) = ProjectDirs::from("org", "corten", "corten-desktop") {
            let dir = dirs.data_dir();
            let _ = fs::create_dir_all(dir);
            dir.join("scratchpad.txt")
        } else {
            PathBuf::from("/tmp/corten_scratchpad.txt")
        }
    }

    pub fn load_note() -> String {
        let path = Self::notes_path();
        if let Ok(content) = fs::read_to_string(&path) {
            content
        } else {
            "• Welcome to Corten Widgets\n• Press Enter to edit notes\n• Zero CPU, pure Wayland desklets".to_string()
        }
    }

    pub fn save_note(text: &str) {
        let path = Self::notes_path();
        let _ = fs::write(path, text);
    }
}
