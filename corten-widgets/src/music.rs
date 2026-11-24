use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use zbus::zvariant::OwnedValue;

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
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            is_playing: false,
            player_kind: PlayerKind::Generic,
            cover_path: None,
            position_secs: 0,
            length_secs: 0,
        }
    }
}

impl MusicTrack {
    pub fn progress(&self) -> f32 {
        if self.length_secs == 0 {
            0.0
        } else {
            (self.position_secs as f32 / self.length_secs as f32).clamp(0.0, 1.0)
        }
    }

    pub fn formatted_position(&self) -> String {
        Self::fmt_mm_ss(self.position_secs)
    }

    pub fn formatted_length(&self) -> String {
        Self::fmt_mm_ss(self.length_secs)
    }

    fn fmt_mm_ss(secs: u64) -> String {
        let m = secs / 60;
        let s = secs % 60;
        format!("{:02}:{:02}", m, s)
    }
}

static LAST_ART_CACHE: OnceLock<Mutex<(String, Option<PathBuf>)>> = OnceLock::new();
static ACTIVE_BUS_NAME: OnceLock<Mutex<Option<String>>> = OnceLock::new();

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

    fn value_to_string(v: &OwnedValue) -> Option<String> {
        if let Ok(s) = <&str>::try_from(v) {
            return Some(s.to_string());
        }
        if let Ok(arr) = Vec::<String>::try_from(v.try_clone().ok()?) {
            let joined = arr.join(", ");
            if !joined.is_empty() {
                return Some(joined);
            }
        }
        None
    }

    fn value_to_u64(v: &OwnedValue) -> u64 {
        if let Ok(n) = u64::try_from(v) {
            n
        } else if let Ok(n) = i64::try_from(v) {
            n.max(0) as u64
        } else if let Ok(n) = u32::try_from(v) {
            n as u64
        } else {
            0
        }
    }

    /// Queries session D-Bus (`org.mpris.MediaPlayer2.*`) directly so Flatpak Spotify,
    /// Brave, YouTube Music, and native players sync reliably without requiring `playerctl`.
    async fn fetch_via_dbus() -> Option<MusicTrack> {
        let conn = zbus::Connection::session().await.ok()?;
        let dbus_proxy = zbus::fdo::DBusProxy::new(&conn).await.ok()?;
        let names = dbus_proxy.list_names().await.ok()?;

        let mut players: Vec<String> = names
            .into_iter()
            .map(|n| n.to_string())
            .filter(|n| n.starts_with("org.mpris.MediaPlayer2."))
            .collect();
        players.sort();

        if players.is_empty() {
            return None;
        }

        let mut best_paused: Option<(String, MusicTrack)> = None;

        for bus_name in players {
            let Ok(proxy) = zbus::Proxy::new(
                &conn,
                bus_name.as_str(),
                "/org/mpris/MediaPlayer2",
                "org.mpris.MediaPlayer2.Player",
            )
            .await
            else {
                continue;
            };

            let status: String = proxy
                .get_property("PlaybackStatus")
                .await
                .unwrap_or_default();
            let status_lower = status.to_lowercase();
            let is_playing = status_lower == "playing";

            let metadata: HashMap<String, OwnedValue> =
                proxy.get_property("Metadata").await.unwrap_or_default();

            let title = metadata
                .get("xesam:title")
                .and_then(Self::value_to_string)
                .unwrap_or_default()
                .trim()
                .to_string();
            let artist = metadata
                .get("xesam:artist")
                .and_then(Self::value_to_string)
                .unwrap_or_default()
                .trim()
                .to_string();
            let album = metadata
                .get("xesam:album")
                .and_then(Self::value_to_string)
                .unwrap_or_default()
                .trim()
                .to_string();
            let art_url = metadata
                .get("mpris:artUrl")
                .and_then(Self::value_to_string)
                .unwrap_or_default();
            let track_url = metadata
                .get("xesam:url")
                .and_then(Self::value_to_string)
                .unwrap_or_default();
            let len_us = metadata
                .get("mpris:length")
                .map(Self::value_to_u64)
                .unwrap_or(0);
            let pos_us: i64 = proxy.get_property("Position").await.unwrap_or(0);

            if title.is_empty() && !is_playing && status_lower != "paused" {
                continue;
            }

            let player_kind = PlayerKind::detect(&bus_name, &track_url, &art_url);
            let cover_path = Self::resolve_art_url(&art_url).await;

            let track = MusicTrack {
                title: if title.is_empty() {
                    "Bilinmeyen Parça".to_string()
                } else {
                    title
                },
                artist: if artist.is_empty() {
                    player_kind.display_name().to_string()
                } else {
                    artist
                },
                album,
                is_playing,
                player_kind,
                cover_path,
                position_secs: (pos_us.max(0) as u64) / 1_000_000,
                length_secs: len_us / 1_000_000,
            };

            if is_playing {
                if let Ok(mut guard) = ACTIVE_BUS_NAME.get_or_init(|| Mutex::new(None)).lock() {
                    *guard = Some(bus_name);
                }
                return Some(track);
            } else if best_paused.is_none() {
                best_paused = Some((bus_name, track));
            }
        }

        if let Some((bus_name, track)) = best_paused {
            if let Ok(mut guard) = ACTIVE_BUS_NAME.get_or_init(|| Mutex::new(None)).lock() {
                *guard = Some(bus_name);
            }
            return Some(track);
        }

        None
    }

    /// Fetches active MPRIS player track via native D-Bus first, falling back to `playerctl`.
    pub async fn fetch_current_track_async() -> MusicTrack {
        if let Some(track) = Self::fetch_via_dbus().await {
            return track;
        }

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
                            "Bilinmeyen Parça".into()
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

    fn call_mpris_method(method: &'static str, playerctl_fallback: &'static str) {
        let target_bus = ACTIVE_BUS_NAME
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|g| g.clone());

        if let Some(bus) = target_bus {
            let ok = Command::new("busctl")
                .args([
                    "--user",
                    "call",
                    &bus,
                    "/org/mpris/MediaPlayer2",
                    "org.mpris.MediaPlayer2.Player",
                    method,
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return;
            }
        }
        let _ = Command::new("playerctl").arg(playerctl_fallback).spawn();
    }

    pub fn play_pause() {
        Self::call_mpris_method("PlayPause", "play-pause");
    }

    pub fn next() {
        Self::call_mpris_method("Next", "next");
    }

    pub fn previous() {
        Self::call_mpris_method("Previous", "previous");
    }
}
