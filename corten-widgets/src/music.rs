use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerKind {
    Spotify,
    YouTubeMusic,
    Brave,
    Generic,
}

impl PlayerKind {
    pub fn display_name(self) -> &'static str {
        match self {
            PlayerKind::Spotify => "Spotify",
            PlayerKind::YouTubeMusic => "YouTube Music",
            PlayerKind::Brave => "Brave",
            PlayerKind::Generic => "Media",
        }
    }

    pub fn detect(player_name: &str, track_url: &str, art_url: &str) -> Self {
        let p = player_name.to_lowercase();
        let u = track_url.to_lowercase();
        let a = art_url.to_lowercase();

        if p.contains("spotify") || u.contains("spotify.com") || a.contains("scdn.co") {
            PlayerKind::Spotify
        } else if p.contains("youtube")
            || p.contains("ytmdesktop")
            || u.contains("music.youtube.com")
            || u.contains("youtube.com")
            || u.contains("youtu.be")
            || a.contains("ytimg.com")
            || a.contains("ggpht.com")
        {
            PlayerKind::YouTubeMusic
        } else if p.contains("brave") {
            PlayerKind::Brave
        } else {
            PlayerKind::Generic
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MusicTrack {
    pub title: String,
    pub artist: String,
    #[allow(dead_code)]
    pub album: String,
    pub is_playing: bool,
    pub player_kind: PlayerKind,
    pub cover_path: Option<PathBuf>,
    pub position_secs: u64,
    pub length_secs: u64,
}

impl Default for MusicTrack {
    fn default() -> Self {
        Self {
            title: "Midnight Dunes".to_string(),
            artist: "Solar Fields".to_string(),
            album: String::new(),
            is_playing: false,
            player_kind: PlayerKind::Spotify,
            cover_path: None,
            position_secs: 68,
            length_secs: 222,
        }
    }
}

impl MusicTrack {
    pub fn progress(&self) -> f32 {
        if self.length_secs == 0 {
            if self.is_playing { 0.42 } else { 0.31 }
        } else {
            (self.position_secs as f32 / self.length_secs as f32).clamp(0.0, 1.0)
        }
    }

    pub fn formatted_position(&self) -> String {
        Self::fmt_mm_ss(self.position_secs)
    }

    pub fn formatted_length(&self) -> String {
        if self.length_secs == 0 {
            "03:42".to_string()
        } else {
            Self::fmt_mm_ss(self.length_secs)
        }
    }

    fn fmt_mm_ss(secs: u64) -> String {
        let m = secs / 60;
        let s = secs % 60;
        format!("{:02}:{:02}", m, s)
    }
}

static LAST_ART_CACHE: OnceLock<Mutex<(String, Option<PathBuf>)>> = OnceLock::new();

pub struct MusicService;

impl MusicService {
    async fn resolve_art_url(art_url: &str) -> Option<PathBuf> {
        let trimmed = art_url.trim();
        if trimmed.is_empty() {
            return None;
        }

        if let Some(rest) = trimmed.strip_prefix("file://") {
            let path = PathBuf::from(rest);
            if path.exists() {
                return Some(path);
            }
            return None;
        }

        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let cache_mutex = LAST_ART_CACHE.get_or_init(|| Mutex::new((String::new(), None)));
            if let Ok(guard) = cache_mutex.lock()
                && guard.0 == trimmed
                && let Some(ref p) = guard.1
                && p.exists()
            {
                return Some(p.clone());
            }

            let cache_file = std::env::temp_dir().join("corten_mpris_cover.jpg");
            let status = tokio::process::Command::new("curl")
                .args([
                    "-fsSL",
                    "--max-time",
                    "2",
                    "-o",
                    &cache_file.to_string_lossy(),
                    trimmed,
                ])
                .status()
                .await;

            if let Ok(s) = status
                && s.success()
                && cache_file.exists()
            {
                if let Ok(mut guard) = cache_mutex.lock() {
                    *guard = (trimmed.to_string(), Some(cache_file.clone()));
                }
                return Some(cache_file);
            }
        }

        None
    }

    /// Polls playerctl asynchronously to get current playing track metadata, player identity, and cover art
    pub async fn fetch_current_track_async() -> MusicTrack {
        if let Ok(output) = tokio::process::Command::new("playerctl")
            .args([
                "metadata",
                "--format",
                "{{title}}|{{artist}}|{{album}}|{{status}}|{{playerName}}|{{mpris:artUrl}}|{{xesam:url}}|{{position}}|{{mpris:length}}",
            ])
            .output()
            .await
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let parts: Vec<&str> = text.split('|').collect();
            if parts.len() >= 4 {
                let title = parts[0].trim().to_string();
                let artist = parts[1].trim().to_string();
                let album = parts[2].trim().to_string();
                let status = parts[3].trim().to_lowercase();
                let player_name = parts.get(4).copied().unwrap_or("").trim();
                let art_url = parts.get(5).copied().unwrap_or("").trim();
                let track_url = parts.get(6).copied().unwrap_or("").trim();
                let pos_us = parts
                    .get(7)
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                let len_us = parts
                    .get(8)
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(0);

                if !title.is_empty() || status == "playing" || status == "paused" {
                    let is_playing = status == "playing";
                    let player_kind = PlayerKind::detect(player_name, track_url, art_url);
                    let cover_path = Self::resolve_art_url(art_url).await;

                    return MusicTrack {
                        title: if title.is_empty() {
                            "Unknown Title".into()
                        } else {
                            title
                        },
                        artist: if artist.is_empty() {
                            player_kind.display_name().into()
                        } else {
                            artist
                        },
                        album,
                        is_playing,
                        player_kind,
                        cover_path,
                        position_secs: pos_us / 1_000_000,
                        length_secs: len_us / 1_000_000,
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
