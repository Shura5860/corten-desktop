use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct MusicTrack {
    pub title: String,
    pub artist: String,
    #[allow(dead_code)]
    pub album: String,
    pub is_playing: bool,
}

impl Default for MusicTrack {
    fn default() -> Self {
        Self {
            title: "No Media Playing".to_string(),
            artist: "Corten Sound".to_string(),
            album: String::new(),
            is_playing: false,
        }
    }
}

pub struct MusicService;

impl MusicService {
    /// Polls playerctl asynchronously to get current playing track metadata without stalling the UI thread
    pub async fn fetch_current_track_async() -> MusicTrack {
        if let Ok(output) = tokio::process::Command::new("playerctl")
            .args([
                "metadata",
                "--format",
                "{{title}}|{{artist}}|{{album}}|{{status}}",
            ])
            .output()
            .await
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let parts: Vec<&str> = text.split('|').collect();
                if parts.len() >= 4 {
                    let title = parts[0].trim().to_string();
                    let artist = parts[1].trim().to_string();
                    let album = parts[2].trim().to_string();
                    let status = parts[3].trim().to_lowercase();
                    let is_playing = status == "playing";

                    return MusicTrack {
                        title: if title.is_empty() {
                            "Unknown Title".into()
                        } else {
                            title
                        },
                        artist: if artist.is_empty() {
                            "Unknown Artist".into()
                        } else {
                            artist
                        },
                        album,
                        is_playing,
                    };
                }
            }
        }

        MusicTrack::default()
    }

    pub fn play_pause() {
        let _ = Command::new("playerctl").arg("play-pause").spawn();
    }

    pub fn next() {
        let _ = Command::new("playerctl").arg("next").spawn();
    }

    pub fn previous() {
        let _ = Command::new("playerctl").arg("previous").spawn();
    }
}
