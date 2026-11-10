use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

use ::image as image_crate;
use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::{Palette, PatinaVariant, ThemeMode};
use iced::widget::{container, image as iced_image};
use iced::{Element, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting Corten Wallpaper Daemon (corten-wallpaper)...");

    let args: Vec<String> = std::env::args().collect();
    let mut target_output: Option<String> = None;
    let mut slideshow_dir: Option<PathBuf> = None;
    let mut interval_secs: u64 = 300;
    let mut single_path: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--output" => {
                if i + 1 < args.len() {
                    target_output = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--slideshow" => {
                if i + 1 < args.len() {
                    slideshow_dir = Some(PathBuf::from(&args[i + 1]));
                    i += 1;
                }
            }
            "--interval" => {
                if i + 1 < args.len() {
                    if let Ok(val) = args[i + 1].parse::<u64>() {
                        interval_secs = val.max(5);
                    }
                    i += 1;
                }
            }
            arg if !arg.starts_with("--") => {
                single_path = Some(PathBuf::from(arg));
            }
            _ => {}
        }
        i += 1;
    }

    let (wallpapers, slideshow_enabled) = if let Some(dir) = slideshow_dir {
        let list = collect_wallpapers(&dir);
        (list, true)
    } else if let Some(path) = single_path {
        if path.is_dir() {
            let list = collect_wallpapers(&path);
            (list, true)
        } else {
            (vec![path], false)
        }
    } else {
        let resolved = resolve_wallpapers();
        (resolved, false)
    };

    info!(
        "Loaded {} wallpaper(s), slideshow: {}, bound output: {:?}",
        wallpapers.len(),
        slideshow_enabled,
        target_output
    );

    let flags = WallpaperFlags {
        wallpapers,
        interval_secs,
        slideshow_enabled,
    };

    CortenWallpaperApp::run(Settings {
        id: Some("corten-wallpaper".into()),
        flags,
        fonts: vec![],
        layer_settings: LayerShellSettings {
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            layer: Layer::Background, // Sits strictly behind all normal windows and widgets
            margin: (0, 0, 0, 0),
            exclusive_zone: 0, // Neutral: Never collapse or reduce usable screen area for other layers
            size: Some((0, 0)), // 0 signals full fill across all 4 anchors
            keyboard_interactivity: KeyboardInteractivity::None,
            binded_output_name: target_output,
        },
        default_font: iced::Font::default(),
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

pub fn collect_wallpapers(path: &Path) -> Vec<PathBuf> {
    if !path.exists() {
        return Vec::new();
    }
    if path.is_file() {
        return vec![path.to_path_buf()];
    }

    let mut images = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_lowercase();
                if matches!(ext_lower.as_str(), "png" | "jpg" | "jpeg" | "webp") {
                    images.push(p);
                }
            }
        }
    }
    images.sort();
    images
}

pub fn resolve_wallpapers() -> Vec<PathBuf> {
    // 0. Check AppearanceConfig for user configured wallpaper
    if let Ok(app_mgr) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
        let path_str = app_mgr.get().wallpaper_path.trim();
        if !path_str.is_empty() {
            let p = PathBuf::from(path_str);
            if p.exists() {
                if p.is_dir() {
                    let list = collect_wallpapers(&p);
                    if !list.is_empty() {
                        return list;
                    }
                } else if p.is_file() {
                    return vec![p];
                }
            }
        }
    }

    // 1. Check XDG user pictures or config
    if let Ok(home) = std::env::var("HOME") {
        let custom = Path::new(&home).join(".config/corten/wallpaper.png");
        if custom.exists() {
            return vec![custom];
        }
        let custom_jpg = Path::new(&home).join(".config/corten/wallpaper.jpg");
        if custom_jpg.exists() {
            return vec![custom_jpg];
        }
    }

    // 2. Check workspace assets folder
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    let search_roots = [
        PathBuf::from("/usr/share/backgrounds/corten"),
        PathBuf::from("assets/wallpapers"),
        exe_dir
            .map(|d| d.join("../../../assets/wallpapers"))
            .unwrap_or_else(|| PathBuf::from("assets/wallpapers")),
    ];

    for root in &search_roots {
        let list = collect_wallpapers(root);
        if !list.is_empty() {
            return list;
        }
    }

    // 3. System wallpapers fallback
    let fallback_system = [
        "/usr/share/wallpapers/Next/contents/images_dark/5120x2880.png",
        "/usr/share/wallpapers/Next/contents/images/5120x2880.png",
        "/usr/share/wallpapers/Air/contents/images/5120x2880.png",
    ];

    for path in fallback_system {
        let p = PathBuf::from(path);
        if p.exists() {
            return vec![p];
        }
    }

    Vec::new()
}

pub struct WallpaperFlags {
    pub wallpapers: Vec<PathBuf>,
    pub interval_secs: u64,
    pub slideshow_enabled: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    NextWallpaper,
    ReloadConfig,
    CheckPeriodic,
    LayerAction(LayershellCustomActions),
}

impl TryInto<LayershellCustomActions> for Message {
    type Error = Self;

    fn try_into(self) -> Result<LayershellCustomActions, Self::Error> {
        match self {
            Message::LayerAction(action) => Ok(action),
            _ => Err(self),
        }
    }
}

/// Extracts the dominant and vibrant ambiance color from a wallpaper image file
pub fn extract_ambiance_color(path: &Path) -> Option<(u8, u8, u8, PatinaVariant)> {
    let img = image_crate::io::Reader::open(path).ok()?.decode().ok()?;
    let rgb = img.thumbnail(64, 64).to_rgb8();
    let mut total_r: u64 = 0;
    let mut total_g: u64 = 0;
    let mut total_b: u64 = 0;
    let mut count: u64 = 0;

    for pixel in rgb.pixels() {
        let [r, g, b] = pixel.0;
        let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        // Ignore near-black and near-white
        if lum > 30.0 && lum < 230.0 {
            total_r += r as u64;
            total_g += g as u64;
            total_b += b as u64;
            count += 1;
        }
    }

    if count == 0 {
        return None;
    }

    let avg_r = (total_r / count) as u8;
    let avg_g = (total_g / count) as u8;
    let avg_b = (total_b / count) as u8;
    let patina = PatinaVariant::match_from_rgb(avg_r, avg_g, avg_b);
    Some((avg_r, avg_g, avg_b, patina))
}

pub fn sync_ambiance_to_config(path: &Path) {
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        if let Some((r, g, b, _patina)) = extract_ambiance_color(&p) {
            if let Ok(mut app_mgr) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                let mut cfg = app_mgr.get().clone();
                cfg.accent_color = format!("#{:02X}{:02X}{:02X}", r, g, b);
                let _ = app_mgr.update(cfg);
                info!("Wallpaper Ambiance Synced in background: #{:02X}{:02X}{:02X}", r, g, b);
            }
        }
    });
}

pub struct CortenWallpaperApp {
    wallpapers: Vec<PathBuf>,
    current_index: usize,
    interval_secs: u64,
    slideshow_enabled: bool,
    palette: Palette,
}

impl Application for CortenWallpaperApp {
    type Message = Message;
    type Flags = WallpaperFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        if let Some(first) = flags.wallpapers.first() {
            sync_ambiance_to_config(first);
        }
        (
            Self {
                wallpapers: flags.wallpapers,
                current_index: 0,
                interval_secs: flags.interval_secs,
                slideshow_enabled: flags.slideshow_enabled,
                palette: Palette::for_mode(ThemeMode::Dark),
            },
            Task::none(),
        )
    }

    fn namespace(&self) -> String {
        "corten-wallpaper".into()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: self.palette.text_primary.to_iced(),
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::NextWallpaper => {
                if !self.wallpapers.is_empty() {
                    self.current_index = (self.current_index + 1) % self.wallpapers.len();
                    if let Some(wp) = self.wallpapers.get(self.current_index) {
                        sync_ambiance_to_config(wp);
                    }
                }
            }
            Message::ReloadConfig | Message::CheckPeriodic => {
                if let Ok(app_mgr) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    let path_str = app_mgr.get().wallpaper_path.trim();
                    if !path_str.is_empty() {
                        let new_path = PathBuf::from(path_str);
                        if new_path.exists() && new_path.is_file() {
                            let is_different = self.wallpapers.get(self.current_index) != Some(&new_path);
                            if is_different {
                                info!("corten-wallpaper: Updating wallpaper to {}", new_path.display());
                                self.wallpapers = vec![new_path.clone()];
                                self.current_index = 0;
                                sync_ambiance_to_config(&new_path);
                            }
                        }
                    }
                }
            }
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if let Some(path) = self.wallpapers.get(self.current_index) {
            let handle = iced_image::Handle::from_path(path);
            container(
                iced_image(handle)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::Cover),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.08, 0.08, 0.10,
                ))),
                ..Default::default()
            })
            .into()
        } else {
            // Elegant fallback dark canvas
            container(iced::widget::Space::new(Length::Fill, Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(self.palette.background.to_iced())),
                    ..Default::default()
                })
                .into()
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subs = Vec::new();

        if self.slideshow_enabled && self.wallpapers.len() > 1 && self.interval_secs > 0 {
            subs.push(
                iced::time::every(Duration::from_secs(self.interval_secs))
                    .map(|_| Message::NextWallpaper),
            );
        }

        let sigusr1_sub = Subscription::run_with_id(
            "wallpaper-sigusr1-stream",
            futures::stream::unfold(None, |mut sig_opt: Option<tokio::signal::unix::Signal>| async move {
                loop {
                    if sig_opt.is_none() {
                        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
                            Ok(s) => sig_opt = Some(s),
                            Err(_) => {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                continue;
                            }
                        }
                    }

                    if let Some(ref mut sig) = sig_opt {
                        sig.recv().await;
                        return Some((Message::ReloadConfig, sig_opt));
                    }
                }
            }),
        );
        subs.push(sigusr1_sub);

        subs.push(
            iced::time::every(Duration::from_millis(1000)).map(|_| Message::CheckPeriodic),
        );

        Subscription::batch(subs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_wallpapers_empty_or_nonexistent() {
        let list = collect_wallpapers(Path::new("/nonexistent_folder_xyz"));
        assert!(list.is_empty());
    }

    #[test]
    fn test_collect_wallpapers_single_file() {
        let tmp_file = PathBuf::from("Cargo.toml");
        let list = collect_wallpapers(&tmp_file);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], tmp_file);
    }

    #[test]
    fn test_extract_ambiance_color_nonexistent() {
        let res = extract_ambiance_color(Path::new("/nonexistent_img.png"));
        assert!(res.is_none());
    }

    #[test]
    fn test_resolve_wallpapers_does_not_panic() {
        let list = resolve_wallpapers();
        // Should safely run and return a vec (empty or resolved)
        assert!(list.is_empty() || !list.is_empty());
    }

    #[test]
    fn test_wallpaper_flags_creation() {
        let flags = WallpaperFlags {
            wallpapers: vec![PathBuf::from("/tmp/test.png")],
            interval_secs: 60,
            slideshow_enabled: true,
        };
        assert_eq!(flags.wallpapers.len(), 1);
        assert_eq!(flags.interval_secs, 60);
        assert!(flags.slideshow_enabled);
    }
}
