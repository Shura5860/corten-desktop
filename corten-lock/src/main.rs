mod auth;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use auth::{AuthOutcome, PamAuth};
use chrono::{Datelike, Local};
use corten_auth::AuthController;
use corten_config::{AppearanceConfig, ConfigManager, InputSettingsConfig};
use corten_theme::iced_styles::acrylic_glass_style;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, ThemeMode, Transition, LUCIDE_FONT, LUCIDE_FONT_BYTES,
    NOTO_SANS, NOTO_SANS_BYTES,
};
use iced::widget::{button, container, image, row, stack, text};
use iced::{event, Alignment, Element, Event, Length, Padding, Subscription, Task};
use iced_sessionlock::actions::UnLockAction;
use iced_sessionlock::settings::Settings;
use iced_sessionlock::MultiApplication;
use tracing::{info, warn};

const MAX_VISIBLE_PASSWORD_CHARS: usize = 22;
const POWER_CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);
const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(5);

pub fn main() -> Result<(), iced_sessionlock::Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_hal::vulkan::conv=error,wgpu_core=info".into()),
        )
        .init();
    info!("Starting corten-lock (Wayland ext-session-lock-v1)...");

    if std::env::var_os("ICED_PRESENT_MODE").is_none() {
        std::env::set_var("ICED_PRESENT_MODE", "mailbox");
    }

    let current_user = users::get_current_username()
        .map(|u| u.to_string_lossy().to_string())
        .unwrap_or_else(|| "user".into());

    CortenLockApp::run(Settings {
        id: Some("corten-lock".into()),
        flags: current_user,
        fonts: vec![
            Cow::Borrowed(LUCIDE_FONT_BYTES),
            Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
    })?;

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkState {
    Wifi,
    Ethernet,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Suspend,
    Reboot,
    Poweroff,
}

impl PowerAction {
    fn systemctl_arg(self) -> &'static str {
        match self {
            Self::Suspend => "suspend",
            Self::Reboot => "reboot",
            Self::Poweroff => "poweroff",
        }
    }

    fn requires_confirmation(self) -> bool {
        matches!(self, Self::Reboot | Self::Poweroff)
    }

    fn confirm_prompt(self) -> &'static str {
        match self {
            Self::Suspend => "Uyku moduna geçilsin mi?",
            Self::Reboot => "Yeniden başlatılsın mı?",
            Self::Poweroff => "Bilgisayar kapatılsın mı?",
        }
    }
}

pub struct CortenLockApp {
    auth: AuthController,
    auth_error_msg: Option<String>,
    show_password: bool,
    show_virtual_keyboard: bool,
    virtual_shift: bool,
    current_time: String,
    current_date: String,
    caps_lock_on: bool,
    keyboard_layout: String,
    avatar_handle: Option<image::Handle>,
    wallpaper_handle: Option<image::Handle>,
    media_track: Option<corten_services::mpris::MediaPlayer>,
    battery_info: Option<(u8, bool)>,
    network_state: NetworkState,
    pending_power_confirm: Option<(PowerAction, Instant)>,
    primary_window: Option<iced::window::Id>,
    caret_visible: bool,
    shake_transition: Transition,
    entrance_transition: Transition,
    last_tick: Instant,
    last_status_poll: Instant,
    active_patina: PatinaVariant,
}

impl CortenLockApp {
    fn format_turkish_date<T: Datelike>(dt: &T) -> String {
        let weekday = match dt.weekday() {
            chrono::Weekday::Mon => "Pazartesi",
            chrono::Weekday::Tue => "Salı",
            chrono::Weekday::Wed => "Çarşamba",
            chrono::Weekday::Thu => "Perşembe",
            chrono::Weekday::Fri => "Cuma",
            chrono::Weekday::Sat => "Cumartesi",
            chrono::Weekday::Sun => "Pazar",
        };
        let month = match dt.month() {
            1 => "Ocak",
            2 => "Şubat",
            3 => "Mart",
            4 => "Nisan",
            5 => "Mayıs",
            6 => "Haziran",
            7 => "Temmuz",
            8 => "Ağustos",
            9 => "Eylül",
            10 => "Ekim",
            11 => "Kasım",
            12 => "Aralık",
            _ => "",
        };
        format!("{weekday}, {} {month}", dt.day())
    }

    /// Reads the hardware Caps Lock LED state from sysfs so the indicator matches reality on launch.
    fn detect_caps_lock_led() -> bool {
        if let Ok(entries) = std::fs::read_dir("/sys/class/leds") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().ends_with("::capslock") {
                    let brightness = entry.path().join("brightness");
                    if let Ok(val) = std::fs::read_to_string(brightness) {
                        if val.trim() == "1" {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn resolve_keyboard_layout() -> String {
        ConfigManager::<InputSettingsConfig>::load_or_default("input")
            .ok()
            .map(|c| c.get().keyboard_layout.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "TR".to_string())
    }

    fn resolve_user_avatar(username: &str) -> Option<PathBuf> {
        if let Ok(home) = std::env::var("HOME") {
            let home_path = Path::new(&home);
            for candidate in [
                home_path.join(".face"),
                home_path.join(".face.icon"),
                home_path.join(".config/corten/avatar.png"),
                home_path.join(".config/corten/avatar.jpg"),
            ] {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        let acct_icon = PathBuf::from(format!("/var/lib/AccountsService/icons/{username}"));
        if acct_icon.is_file() {
            return Some(acct_icon);
        }
        None
    }

    fn get_media_status() -> Option<corten_services::mpris::MediaPlayer> {
        corten_services::mpris::list_players()
            .ok()
            .and_then(|players| players.into_iter().find(|p| !p.title.is_empty()))
    }

    fn get_battery_info() -> Option<(u8, bool)> {
        if let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name.to_string_lossy().starts_with("BAT") {
                    let cap_path = entry.path().join("capacity");
                    let status_path = entry.path().join("status");
                    let cap = std::fs::read_to_string(cap_path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u8>().ok());
                    let status = std::fs::read_to_string(status_path).unwrap_or_default();
                    let is_charging = status.trim().eq_ignore_ascii_case("Charging")
                        || status.trim().eq_ignore_ascii_case("Full");
                    if let Some(c) = cap {
                        return Some((c, is_charging));
                    }
                }
            }
        }
        None
    }

    fn get_network_state() -> NetworkState {
        let mut has_ethernet = false;
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str == "lo"
                    || name_str.starts_with("docker")
                    || name_str.starts_with("virbr")
                    || name_str.starts_with("veth")
                {
                    continue;
                }
                let operstate = entry.path().join("operstate");
                if let Ok(state) = std::fs::read_to_string(operstate) {
                    if state.trim() == "up" {
                        if entry.path().join("wireless").exists() || name_str.starts_with("wl") {
                            return NetworkState::Wifi;
                        }
                        has_ethernet = true;
                    }
                }
            }
        }
        if has_ethernet {
            NetworkState::Ethernet
        } else {
            NetworkState::Offline
        }
    }

    fn resolve_source_wallpaper() -> Option<PathBuf> {
        if let Ok(path_str) = std::env::var("CORTEN_WALLPAPER") {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
            }
        }

        if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
            let configured = app_cfg.get().wallpaper_path.trim().to_string();
            if !configured.is_empty() {
                let p = PathBuf::from(configured);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        if let Ok(home) = std::env::var("HOME") {
            let home_path = Path::new(&home);
            for ext in ["png", "jpg", "jpeg", "webp"] {
                let p = home_path.join(format!(".config/corten/wallpaper.{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }

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
            for file in ["corten_dark_landscape.jpg", "corten_dark_portrait.jpg"] {
                let p = root.join(file);
                if p.is_file() {
                    return Some(p);
                }
            }
        }

        for path in [
            "/usr/share/wallpapers/Next/contents/images_dark/5120x2880.png",
            "/usr/share/wallpapers/Next/contents/images/5120x2880.png",
            "/usr/share/wallpapers/Air/contents/images/5120x2880.png",
        ] {
            let p = PathBuf::from(path);
            if p.is_file() {
                return Some(p);
            }
        }

        None
    }

    fn blur_cache_path() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".cache/corten/lock_wallpaper_blur.png"))
    }

    fn is_blur_cache_fresh(src: &Path, cache: &Path) -> bool {
        if !cache.is_file() {
            return false;
        }
        let cache_mtime = cache.metadata().and_then(|m| m.modified()).ok();
        let src_mtime = src.metadata().and_then(|m| m.modified()).ok();
        match (cache_mtime, src_mtime) {
            (Some(c), Some(s)) => c >= s,
            _ => false,
        }
    }

    /// Generates a Gaussian-blurred lock wallpaper in pure Rust (zero external ffmpeg dependency)
    /// and atomically renames it into place so concurrent readers never see a half-written file.
    async fn generate_blurred_wallpaper_async(src: PathBuf, cache: PathBuf) -> Option<PathBuf> {
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = cache.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let img = ::image::open(&src).ok()?;
            // Downscale before Gaussian blur for fast (~30ms) high-quality frosted backdrop
            let small = img.thumbnail(960, 540);
            let blurred = small.blur(16.0);
            let tmp = cache.with_extension("png.tmp");
            if let Err(err) = blurred.save_with_format(&tmp, ::image::ImageFormat::Png) {
                warn!("corten-lock: failed to save blurred wallpaper: {err}");
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            if let Err(err) = std::fs::rename(&tmp, &cache) {
                warn!("corten-lock: failed to atomically rename blur cache: {err}");
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            Some(cache)
        })
        .await
        .ok()
        .flatten()
    }

    /// Formats the password field with UTF-8 character count and horizontal truncation so
    /// long passwords never wrap onto multiple lines inside the fixed input box.
    fn format_password_display(secret: &str, show_password: bool) -> String {
        let count = secret.chars().count();
        if count == 0 {
            return String::new();
        }
        if show_password {
            if count > MAX_VISIBLE_PASSWORD_CHARS {
                let tail: String = secret
                    .chars()
                    .skip(count - MAX_VISIBLE_PASSWORD_CHARS)
                    .collect();
                format!("…{tail}")
            } else {
                secret.to_string()
            }
        } else if count > MAX_VISIBLE_PASSWORD_CHARS {
            format!("… {}", "● ".repeat(MAX_VISIBLE_PASSWORD_CHARS - 1).trim_end())
        } else {
            "● ".repeat(count).trim_end().to_string()
        }
    }

    /// Resolves the actual character emitted by a virtual keyboard key:
    /// - `virtual_shift` shifts both letters and symbols.
    /// - `caps_lock_on` ONLY uppercases alphabetic characters (never turns '1' into '!').
    fn resolve_virtual_key_char(
        normal: char,
        shifted: char,
        virtual_shift: bool,
        caps_lock_on: bool,
    ) -> char {
        if normal.is_alphabetic() {
            if virtual_shift ^ caps_lock_on {
                shifted
            } else {
                normal
            }
        } else if virtual_shift {
            shifted
        } else {
            normal
        }
    }

    fn render_virtual_keyboard(&self, palette: &Palette) -> Element<'_, Message> {
        let pal = *palette;
        let v_shift = self.virtual_shift;
        let caps = self.caps_lock_on;

        let make_key = move |normal: char, shifted: char| -> Element<'_, Message> {
            let ch = Self::resolve_virtual_key_char(normal, shifted, v_shift, caps);
            button(
                text(ch.to_string())
                    .size(13.0)
                    .color(pal.text_primary.to_iced()),
            )
            .width(Length::Fixed(35.0))
            .height(Length::Fixed(34.0))
            .style(move |_, status| {
                let bg_alpha = match status {
                    button::Status::Pressed => 0.24,
                    button::Status::Hovered => 0.16,
                    _ => 0.08,
                };
                button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, bg_alpha,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.14),
                        width: 1.0,
                        radius: iced::border::Radius::from(6.0),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::VirtualKeyPress(ch))
            .into()
        };

        // Row 1: Numbers + backtick/backslash + Backspace
        let r1_chars = [
            ('`', '~'),
            ('1', '!'),
            ('2', '@'),
            ('3', '#'),
            ('4', '$'),
            ('5', '%'),
            ('6', '^'),
            ('7', '&'),
            ('8', '*'),
            ('9', '('),
            ('0', ')'),
            ('-', '_'),
            ('=', '+'),
            ('\\', '|'),
        ];
        let mut row1 = row![].spacing(4).align_y(Alignment::Center);
        for (n, s) in r1_chars {
            row1 = row1.push(make_key(n, s));
        }
        let backspace_btn = button(
            text(Icon::X)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(pal.warning.to_iced()),
        )
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(34.0))
        .style(move |_, status| {
            let alpha = match status {
                button::Status::Pressed => 0.32,
                button::Status::Hovered => 0.24,
                _ => 0.15,
            };
            button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    pal.warning.r,
                    pal.warning.g,
                    pal.warning.b,
                    alpha,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(pal.warning.r, pal.warning.g, pal.warning.b, 0.45),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            }
        })
        .on_press(Message::VirtualKeyBackspace);
        row1 = row1.push(backspace_btn);

        // Row 2: QWERTY + Turkish ğ/ü + brackets
        let r2_chars = [
            ('q', 'Q'),
            ('w', 'W'),
            ('e', 'E'),
            ('r', 'R'),
            ('t', 'T'),
            ('y', 'Y'),
            ('u', 'U'),
            ('i', 'İ'),
            ('o', 'O'),
            ('p', 'P'),
            ('ğ', 'Ğ'),
            ('ü', 'Ü'),
            ('[', '{'),
            (']', '}'),
        ];
        let mut row2 = row![].spacing(4).align_y(Alignment::Center);
        for (n, s) in r2_chars {
            row2 = row2.push(make_key(n, s));
        }

        // Row 3: Shift + ASDF + Turkish ş/ı + punctuation + Enter
        let r3_chars = [
            ('a', 'A'),
            ('s', 'S'),
            ('d', 'D'),
            ('f', 'F'),
            ('g', 'G'),
            ('h', 'H'),
            ('j', 'J'),
            ('k', 'K'),
            ('l', 'L'),
            ('ş', 'Ş'),
            ('ı', 'I'),
            (';', ':'),
            ('\'', '"'),
        ];
        let mut row3 = row![].spacing(4).align_y(Alignment::Center);
        let shift_active = self.virtual_shift;
        let shift_btn = button(
            text(Icon::ARROW_UP)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(if shift_active {
                    pal.primary.to_iced()
                } else {
                    pal.text_muted.to_iced()
                }),
        )
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(34.0))
        .style(move |_, status| {
            let bg = if shift_active {
                iced::Color::from_rgba(pal.primary.r, pal.primary.g, pal.primary.b, 0.28)
            } else if matches!(status, button::Status::Hovered) {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.16)
            } else {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::from_rgba(pal.primary.r, pal.primary.g, pal.primary.b, 0.45),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            }
        })
        .on_press(Message::VirtualKeyShift);
        row3 = row3.push(shift_btn);
        for (n, s) in r3_chars {
            row3 = row3.push(make_key(n, s));
        }
        let enter_btn = button(
            text(Icon::CHECK)
                .font(LUCIDE_FONT)
                .size(15.0)
                .color(pal.on_primary().to_iced()),
        )
        .width(Length::Fixed(48.0))
        .height(Length::Fixed(34.0))
        .style(move |_, status| {
            let alpha = match status {
                button::Status::Pressed => 0.80,
                button::Status::Hovered => 0.92,
                _ => 1.0,
            };
            button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    pal.primary.r,
                    pal.primary.g,
                    pal.primary.b,
                    alpha,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::SubmitPassword);
        row3 = row3.push(enter_btn);

        // Row 4: ZXCV + Turkish ö/ç + Space + Clear
        let r4_chars = [
            ('z', 'Z'),
            ('x', 'X'),
            ('c', 'C'),
            ('v', 'V'),
            ('b', 'B'),
            ('n', 'N'),
            ('m', 'M'),
            ('ö', 'Ö'),
            ('ç', 'Ç'),
            (',', '<'),
            ('.', '>'),
            ('/', '?'),
        ];
        let mut row4 = row![].spacing(4).align_y(Alignment::Center);
        for (n, s) in r4_chars {
            row4 = row4.push(make_key(n, s));
        }
        let space_btn = button(
            text("Boşluk")
                .size(12.0)
                .color(pal.text_primary.to_iced()),
        )
        .width(Length::Fixed(86.0))
        .height(Length::Fixed(34.0))
        .style(move |_, status| {
            let alpha = match status {
                button::Status::Pressed => 0.24,
                button::Status::Hovered => 0.16,
                _ => 0.10,
            };
            button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, alpha,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            }
        })
        .on_press(Message::VirtualKeySpace);
        row4 = row4.push(space_btn);

        let clear_btn = button(
            text("Temizle")
                .size(11.0)
                .color(pal.danger.to_iced()),
        )
        .width(Length::Fixed(58.0))
        .height(Length::Fixed(34.0))
        .style(move |_, status| {
            let alpha = match status {
                button::Status::Pressed => 0.28,
                button::Status::Hovered => 0.20,
                _ => 0.12,
            };
            button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    pal.danger.r,
                    pal.danger.g,
                    pal.danger.b,
                    alpha,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(pal.danger.r, pal.danger.g, pal.danger.b, 0.35),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            }
        })
        .on_press(Message::VirtualKeyClear);
        row4 = row4.push(clear_btn);

        container(
            iced::widget::column![row1, row2, row3, row4]
                .spacing(6)
                .align_x(Alignment::Center),
        )
        .padding([12, 16])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.08, 0.09, 0.11, 0.95,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(pal.primary.r, pal.primary.g, pal.primary.b, 0.40),
                width: 1.5,
                radius: iced::border::Radius::from(14.0),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.60),
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        })
        .into()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    WallpaperReady(Option<PathBuf>),
    TogglePasswordReveal,
    ToggleVirtualKeyboard,
    VirtualKeyPress(char),
    VirtualKeyBackspace,
    VirtualKeyClear,
    VirtualKeyShift,
    VirtualKeySpace,
    SubmitPassword,
    AuthResult(AuthOutcome),
    Unlock,
    LockMediaPlayPause(String),
    LockMediaNext(String),
    LockMediaPrevious(String),
    RequestPowerAction(PowerAction),
    ConfirmPowerAction,
    CancelPowerAction,
    IcedEvent(iced::window::Id, Event),
}

impl TryInto<UnLockAction> for Message {
    type Error = Self;

    fn try_into(self) -> Result<UnLockAction, Self::Error> {
        match self {
            Message::Unlock => Ok(UnLockAction),
            other => Err(other),
        }
    }
}

impl MultiApplication for CortenLockApp {
    type Message = Message;
    type Flags = String;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let now = Local::now();
        let current_time = now.format("%H:%M").to_string();
        let current_date = Self::format_turkish_date(&now);

        let src_wallpaper = Self::resolve_source_wallpaper();
        let blur_cache = Self::blur_cache_path();

        let (initial_wp, blur_task) = match (src_wallpaper, blur_cache) {
            (Some(src), Some(cache)) if Self::is_blur_cache_fresh(&src, &cache) => {
                (Some(image::Handle::from_path(cache)), Task::none())
            }
            (Some(src), Some(cache)) => {
                let fallback = if cache.is_file() {
                    Some(image::Handle::from_path(&cache))
                } else {
                    Some(image::Handle::from_path(&src))
                };
                let task = Task::perform(
                    Self::generate_blurred_wallpaper_async(src, cache),
                    Message::WallpaperReady,
                );
                (fallback, task)
            }
            (Some(src), None) => (Some(image::Handle::from_path(src)), Task::none()),
            _ => (None, Task::none()),
        };

        let avatar_handle = Self::resolve_user_avatar(&flags).map(image::Handle::from_path);
        let media_track = Self::get_media_status();
        let battery_info = Self::get_battery_info();
        let network_state = Self::get_network_state();
        let caps_lock_on = Self::detect_caps_lock_led();
        let keyboard_layout = Self::resolve_keyboard_layout();
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        let now_inst = Instant::now();
        (
            Self {
                auth: AuthController::new(flags),
                auth_error_msg: None,
                show_password: false,
                show_virtual_keyboard: false,
                virtual_shift: false,
                current_time,
                current_date,
                caps_lock_on,
                keyboard_layout,
                avatar_handle,
                wallpaper_handle: initial_wp,
                media_track,
                battery_info,
                network_state,
                pending_power_confirm: None,
                primary_window: None,
                caret_visible: true,
                shake_transition: Transition::completed(360),
                entrance_transition: Transition::new(260),
                last_tick: now_inst,
                last_status_poll: now_inst,
                active_patina,
            },
            blur_task,
        )
    }

    fn namespace(&self) -> String {
        "corten-lock".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_sessionlock::Appearance {
        iced_sessionlock::Appearance {
            background_color: iced::Color::from_rgb(0.04, 0.05, 0.06),
            text_color: iced::Color::WHITE,
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen_with(|ev, _status, id| Some(Message::IcedEvent(id, ev)));
        let animating =
            !self.shake_transition.is_complete() || !self.entrance_transition.is_complete();
        let interval = if animating {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(1000)
        };
        let timer = iced::time::every(interval).map(|_| Message::Tick);
        Subscription::batch(vec![events, timer])
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Tick => {
                let now_inst = Instant::now();
                let dt_ms = now_inst
                    .duration_since(self.last_tick)
                    .as_millis()
                    .clamp(1, 1000) as u32;
                self.last_tick = now_inst;

                if !self.shake_transition.is_complete() {
                    self.shake_transition.tick(dt_ms);
                }
                if !self.entrance_transition.is_complete() {
                    self.entrance_transition.tick(dt_ms);
                }

                // Auto-dismiss power confirmation after timeout
                if let Some((_, asked_at)) = self.pending_power_confirm {
                    if asked_at.elapsed() >= POWER_CONFIRM_TIMEOUT {
                        self.pending_power_confirm = None;
                    }
                }

                // Only update clock/caret on ~1s cadence (not every 16ms animation frame)
                if dt_ms >= 400 {
                    self.caret_visible = !self.caret_visible;
                    let now = Local::now();
                    self.current_time = now.format("%H:%M").to_string();
                    self.current_date = Self::format_turkish_date(&now);
                }

                // Poll external system state at most once every 5 seconds
                if now_inst.duration_since(self.last_status_poll) >= STATUS_POLL_INTERVAL {
                    self.last_status_poll = now_inst;
                    self.media_track = Self::get_media_status();
                    self.battery_info = Self::get_battery_info();
                    self.network_state = Self::get_network_state();
                    self.caps_lock_on = Self::detect_caps_lock_led();
                    if let Ok(app_cfg) =
                        ConfigManager::<AppearanceConfig>::load_or_default("appearance")
                    {
                        let new_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                        if new_patina != self.active_patina {
                            self.active_patina = new_patina;
                        }
                    }
                }
            }
            Message::WallpaperReady(Some(path)) => {
                self.wallpaper_handle = Some(image::Handle::from_path(path));
            }
            Message::WallpaperReady(None) => {}
            Message::TogglePasswordReveal => {
                self.show_password = !self.show_password;
            }
            Message::ToggleVirtualKeyboard => {
                self.show_virtual_keyboard = !self.show_virtual_keyboard;
            }
            Message::VirtualKeyPress(ch) => {
                if self.auth.push_char(ch) {
                    self.auth_error_msg = None;
                    self.caret_visible = true;
                    if self.virtual_shift {
                        self.virtual_shift = false;
                    }
                }
            }
            Message::VirtualKeyBackspace => {
                if self.auth.pop_char() {
                    self.auth_error_msg = None;
                    self.caret_visible = true;
                }
            }
            Message::VirtualKeyClear => {
                self.auth.cancel_input();
                self.auth_error_msg = None;
            }
            Message::VirtualKeyShift => {
                self.virtual_shift = !self.virtual_shift;
            }
            Message::VirtualKeySpace => {
                if self.auth.push_char(' ') {
                    self.auth_error_msg = None;
                    self.caret_visible = true;
                }
            }
            Message::SubmitPassword => {
                let Some((user, pass)) = self.auth.begin_zeroized() else {
                    return Task::none();
                };
                self.auth_error_msg = None;
                return Task::perform(
                    PamAuth::authenticate_async(user, pass),
                    Message::AuthResult,
                );
            }
            Message::AuthResult(outcome) => {
                let success = outcome == AuthOutcome::Success;
                self.auth.complete(success);
                if success {
                    self.auth_error_msg = None;
                    corten_services::acoustic::play(corten_services::acoustic::AcousticCue::Unlock);
                    info!("Authentication successful, unlocking Corten session!");
                    return Task::done(Message::Unlock);
                } else {
                    let attempts = self.auth.failed_attempts();
                    self.auth_error_msg = Some(match outcome {
                        AuthOutcome::AccountLocked => {
                            "Hesap geçici olarak kilitlendi (çok fazla hatalı deneme)".into()
                        }
                        AuthOutcome::ServiceUnavailable(detail) => detail,
                        AuthOutcome::InvalidCredentials => {
                            if attempts >= 2 {
                                format!("Hatalı parola ({attempts}. başarısız deneme)")
                            } else {
                                "Hatalı parola, lütfen tekrar deneyin".into()
                            }
                        }
                        AuthOutcome::Success => String::new(),
                    });
                    corten_services::acoustic::play(
                        corten_services::acoustic::AcousticCue::AuthFail,
                    );
                    self.shake_transition.restart();
                    self.last_tick = Instant::now();
                }
            }
            Message::LockMediaPlayPause(bus) => {
                let _ = corten_services::mpris::play_pause(&bus);
                self.media_track = Self::get_media_status();
            }
            Message::LockMediaNext(bus) => {
                let _ = corten_services::mpris::next(&bus);
                self.media_track = Self::get_media_status();
            }
            Message::LockMediaPrevious(bus) => {
                let _ = corten_services::mpris::previous(&bus);
                self.media_track = Self::get_media_status();
            }
            Message::RequestPowerAction(action) => {
                if action.requires_confirmation() {
                    self.pending_power_confirm = Some((action, Instant::now()));
                } else {
                    self.pending_power_confirm = None;
                    let _ = std::process::Command::new("systemctl")
                        .arg(action.systemctl_arg())
                        .spawn();
                }
            }
            Message::ConfirmPowerAction => {
                if let Some((action, _)) = self.pending_power_confirm.take() {
                    let _ = std::process::Command::new("systemctl")
                        .arg(action.systemctl_arg())
                        .spawn();
                }
            }
            Message::CancelPowerAction => {
                self.pending_power_confirm = None;
            }
            Message::IcedEvent(window_id, Event::Window(iced::window::Event::Opened { .. })) => {
                if self.primary_window.is_none() {
                    self.primary_window = Some(window_id);
                }
            }
            Message::IcedEvent(
                _window_id,
                Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    modified_key,
                    modifiers,
                    ..
                }),
            ) => {
                if modifiers.control() {
                    match modified_key {
                        iced::keyboard::Key::Character(ref sm)
                            if sm == "u" || sm == "U" || sm == "c" || sm == "C" =>
                        {
                            self.auth.cancel_input();
                            self.auth_error_msg = None;
                        }
                        iced::keyboard::Key::Character(ref sm) if sm == "h" || sm == "H" => {
                            if self.auth.pop_char() {
                                self.auth_error_msg = None;
                            }
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace)
                            if self.auth.pop_word() =>
                        {
                            self.auth_error_msg = None;
                        }
                        _ => {}
                    }
                } else {
                    match modified_key {
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
                            if self.pending_power_confirm.is_some() {
                                self.pending_power_confirm = None;
                            } else {
                                self.auth.cancel_input();
                                self.auth_error_msg = None;
                            }
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::CapsLock) => {
                            self.caps_lock_on = !self.caps_lock_on;
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) => {
                            return Task::done(Message::SubmitPassword);
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace) => {
                            if self.auth.pop_char() {
                                self.auth_error_msg = None;
                                self.caret_visible = true;
                            }
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete) => {
                            self.auth.cancel_input();
                            self.auth_error_msg = None;
                        }
                        iced::keyboard::Key::Character(ref sm) => {
                            for ch in sm.chars() {
                                if self.auth.push_char(ch) {
                                    self.auth_error_msg = None;
                                    self.caret_visible = true;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Message::IcedEvent(_, _) | Message::Unlock => {}
        }
        Task::none()
    }

    fn view(&self, window: iced::window::Id) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(ThemeMode::Dark).with_patina(self.active_patina);
        let is_primary = self.primary_window.is_none_or(|id| id == window);

        // ==================== TOP: CLOCK & DATE ====================
        let clock = text(&self.current_time)
            .size(80.0)
            .color(iced::Color::from_rgb(0.96, 0.97, 0.98));

        let date = text(&self.current_date)
            .size(19.0)
            .color(iced::Color::from_rgb(0.82, 0.84, 0.88));

        let time_section = iced::widget::column![clock, date]
            .spacing(4)
            .align_x(Alignment::Center);

        // Secondary monitors stay minimal and clean: blurred backdrop + clock/date only.
        if !is_primary {
            let secondary_overlay = container(time_section)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.05, 0.06, 0.08, 0.70,
                    ))),
                    ..Default::default()
                });
            return if let Some(ref handle) = self.wallpaper_handle {
                let bg_img = image(handle.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::Cover);
                stack![bg_img, secondary_overlay].into()
            } else {
                secondary_overlay.into()
            };
        }

        // ==================== CENTER CARD: AVATAR & CREDENTIALS ====================
        let avatar_content: Element<'_, Self::Message> = if let Some(ref handle) = self.avatar_handle
        {
            image(handle.clone())
                .width(Length::Fixed(68.0))
                .height(Length::Fixed(68.0))
                .content_fit(iced::ContentFit::Cover)
                .into()
        } else {
            let initial_char = self
                .auth
                .username()
                .chars()
                .next()
                .unwrap_or('U')
                .to_uppercase();
            text(initial_char.to_string())
                .size(32.0)
                .color(palette.primary.to_iced())
                .into()
        };

        let avatar = container(avatar_content)
            .width(Length::Fixed(72.0))
            .height(Length::Fixed(72.0))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.12, 0.95,
                ))),
                border: iced::Border {
                    color: palette.primary.to_iced(),
                    width: 2.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.40,
                    ),
                    offset: iced::Vector::new(0.0, 0.0),
                    blur_radius: 14.0,
                },
                ..Default::default()
            });

        let welcome_text = text(format!("Tekrar Hoş Geldiniz, {}", self.auth.username()))
            .size(17.0)
            .color(palette.text_primary.to_iced());

        let formatted_pass = Self::format_password_display(self.auth.secret(), self.show_password);
        let caret = if !self.auth.is_pending() && self.caret_visible {
            "|"
        } else {
            ""
        };
        let pass_display = if formatted_pass.is_empty() {
            text(format!("Parola girin…{caret}"))
                .size(14.0)
                .color(palette.text_muted.to_iced())
        } else if self.show_password {
            text(format!("{formatted_pass}{caret}"))
                .size(15.0)
                .color(palette.text_primary.to_iced())
        } else {
            text(format!("{formatted_pass}{caret}"))
                .size(13.0)
                .color(palette.text_primary.to_iced())
        };

        let eye_icon = if self.show_password {
            Icon::EYE_OFF
        } else {
            Icon::EYE
        };
        let show_pw = self.show_password;
        let reveal_btn = button(
            text(eye_icon)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(if show_pw {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .padding([6, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.10,
                ))),
                button::Status::Pressed => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.16,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::TogglePasswordReveal);

        let vkbd_open = self.show_virtual_keyboard;
        let kbd_btn = button(
            text(Icon::KEYBOARD)
                .font(LUCIDE_FONT)
                .size(15.0)
                .color(if vkbd_open {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .padding([6, 8])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.10,
                ))),
                button::Status::Pressed => Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.16,
                ))),
                _ => None,
            };
            button::Style {
                background: bg,
                border: iced::Border {
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::ToggleVirtualKeyboard);

        let is_pending = self.auth.is_pending();
        let submit_icon = if is_pending {
            Icon::CLOCK
        } else {
            Icon::CHEVRON_RIGHT
        };
        let submit_btn = button(
            text(submit_icon)
                .font(LUCIDE_FONT)
                .size(16.0)
                .color(palette.on_primary().to_iced()),
        )
        .padding([8, 12])
        .style(move |_, status| {
            let alpha = if is_pending {
                0.55
            } else {
                match status {
                    button::Status::Pressed => 0.78,
                    button::Status::Hovered => 0.90,
                    _ => 1.0,
                }
            };
            button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    alpha,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(8.0),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press_maybe((!is_pending).then_some(Message::SubmitPassword));

        let input_box = container(
            row![
                container(pass_display)
                    .width(Length::Fill)
                    .padding([8, 12])
                    .clip(true),
                reveal_btn,
                kbd_btn,
                submit_btn,
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .width(Length::Fixed(350.0))
        .padding(4)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                1.0, 1.0, 1.0, 0.08,
            ))),
            border: iced::Border {
                color: if self.auth.has_failed() {
                    palette.danger.to_iced()
                } else {
                    iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.45,
                    )
                },
                width: if self.auth.has_failed() { 1.5 } else { 1.0 },
                radius: iced::border::Radius::from(14.0),
            },
            shadow: iced::Shadow {
                color: if self.auth.has_failed() {
                    iced::Color::from_rgba(0.9, 0.2, 0.2, 0.35)
                } else {
                    iced::Color::from_rgba(0.0, 0.0, 0.0, 0.30)
                },
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 12.0,
            },
            ..Default::default()
        });

        let shake_offset: f32 = if !self.shake_transition.is_complete() {
            self.shake_transition.damped_shake(14.0)
        } else {
            0.0
        };

        let shaken_input_box = container(input_box).padding(Padding {
            top: 0.0,
            right: (14.0 - shake_offset).max(0.0),
            bottom: 0.0,
            left: (14.0 + shake_offset).max(0.0),
        });

        let status_feedback: Element<'_, Self::Message> = if self.auth.is_pending() {
            text("Doğrulanıyor…")
                .size(13.0)
                .color(palette.text_muted.to_iced())
                .into()
        } else if let Some(ref err_msg) = self.auth_error_msg {
            row![
                text(Icon::SHIELD)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.danger.to_iced()),
                text(err_msg)
                    .size(13.0)
                    .color(palette.danger.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        } else if self.caps_lock_on {
            container(
                row![
                    text(Icon::SHIELD)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.warning.to_iced()),
                    text("Caps Lock Açık — Büyük Harf Etkin")
                        .size(12.0)
                        .color(palette.warning.to_iced()),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .padding([3, 10])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    palette.warning.r,
                    palette.warning.g,
                    palette.warning.b,
                    0.15,
                ))),
                border: iced::Border {
                    color: palette.warning.to_iced(),
                    width: 1.0,
                    radius: iced::border::Radius::from(8.0),
                },
                ..Default::default()
            })
            .into()
        } else {
            row![
                text(Icon::LOCK)
                    .font(LUCIDE_FONT)
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
                text("PAM Oturum Kilidi Aktif")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(5)
            .align_y(Alignment::Center)
            .into()
        };

        // Smooth entrance slide-up offset powered by entrance_transition
        let entrance_offset: f32 = (1.0 - self.entrance_transition.eased()) * 22.0;

        let center_card = container(
            container(
                iced::widget::column![avatar, welcome_text, shaken_input_box, status_feedback]
                    .spacing(18)
                    .align_x(Alignment::Center),
            )
            .padding([28, 36])
            .width(Length::Fixed(396.0))
            .style(move |_| acrylic_glass_style(palette, 24.0)),
        )
        .padding(Padding {
            top: entrance_offset.max(0.0),
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        });

        // ==================== BOTTOM ROW ====================
        let left_media: Element<'_, Self::Message> = if let Some(ref player) = self.media_track {
            let is_playing =
                player.playback_status == corten_services::mpris::PlaybackStatus::Playing;
            let play_icon = if is_playing { Icon::PAUSE } else { Icon::PLAY };
            let bus = player.bus_name.clone();
            let bus_prev = bus.clone();
            let bus_next = bus.clone();

            let title_display = if player.title.chars().count() > 22 {
                format!("{}…", player.title.chars().take(22).collect::<String>())
            } else {
                player.title.clone()
            };

            let media_btn_style = |_: &iced::Theme, status: button::Status| button::Style {
                background: match status {
                    button::Status::Hovered => Some(iced::Background::Color(
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    )),
                    button::Status::Pressed => Some(iced::Background::Color(
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.20),
                    )),
                    _ => None,
                },
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            };

            let prev_btn = button(
                text(Icon::SKIP_BACK)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([4, 6])
            .style(media_btn_style)
            .on_press(Message::LockMediaPrevious(bus_prev));

            let play_btn = button(
                text(play_icon)
                    .font(LUCIDE_FONT)
                    .size(14.0)
                    .color(palette.primary.to_iced()),
            )
            .padding([4, 6])
            .style(media_btn_style)
            .on_press(Message::LockMediaPlayPause(bus));

            let next_btn = button(
                text(Icon::SKIP_FORWARD)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([4, 6])
            .style(media_btn_style)
            .on_press(Message::LockMediaNext(bus_next));

            let mut info_col = iced::widget::column![
                text(title_display)
                    .size(12.0)
                    .color(palette.text_primary.to_iced())
            ];

            if !player.artist.is_empty() {
                let artist_display = if player.artist.chars().count() > 20 {
                    format!("{}…", player.artist.chars().take(20).collect::<String>())
                } else {
                    player.artist.clone()
                };
                info_col = info_col.push(
                    text(artist_display)
                        .size(10.0)
                        .color(palette.text_muted.to_iced()),
                );
            }

            container(
                row![
                    text(Icon::MUSIC)
                        .font(LUCIDE_FONT)
                        .size(15.0)
                        .color(palette.primary.to_iced()),
                    info_col.spacing(2),
                    row![prev_btn, play_btn, next_btn]
                        .spacing(2)
                        .align_y(Alignment::Center),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .padding([6, 14])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.12, 0.85,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    width: 1.0,
                    radius: iced::border::Radius::from(14.0),
                },
                ..Default::default()
            })
            .into()
        } else {
            row![].into()
        };

        let mut status_row = row![].spacing(12).align_y(Alignment::Center);

        // Keyboard layout badge
        status_row = status_row.push(
            container(
                text(&self.keyboard_layout)
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([2, 6])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0, 1.0, 1.0, 0.08,
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(5.0),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );

        // Battery with charging bolt & low-battery alert
        if let Some((cap, charging)) = self.battery_info {
            let bat_color = if charging {
                palette.primary.to_iced()
            } else if cap <= 20 {
                palette.danger.to_iced()
            } else {
                palette.text_primary.to_iced()
            };
            let mut bat_row = row![
                text(Icon::BATTERY)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(bat_color),
            ]
            .spacing(3)
            .align_y(Alignment::Center);
            if charging {
                bat_row = bat_row.push(
                    text(Icon::ZAP)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.warning.to_iced()),
                );
            }
            bat_row = bat_row.push(text(format!("%{cap}")).size(12.0).color(bat_color));
            status_row = status_row.push(bat_row);
        }

        // Network state (Wi-Fi / Ethernet / Offline)
        let (net_icon, net_color) = match self.network_state {
            NetworkState::Wifi => (Icon::WIFI, palette.text_primary.to_iced()),
            NetworkState::Ethernet => (Icon::GLOBE, palette.text_primary.to_iced()),
            NetworkState::Offline => (Icon::GLOBE, iced::Color::from_rgba(0.6, 0.6, 0.65, 0.40)),
        };
        status_row = status_row.push(
            text(net_icon)
                .font(LUCIDE_FONT)
                .size(15.0)
                .color(net_color),
        );

        // Power controls OR inline 2-step confirmation pill
        if let Some((action, _)) = self.pending_power_confirm {
            let confirm_btn = button(
                text("Evet")
                    .size(12.0)
                    .color(palette.on_primary().to_iced()),
            )
            .padding([4, 10])
            .style(move |_, status| {
                let alpha = if matches!(status, button::Status::Hovered) {
                    0.88
                } else {
                    1.0
                };
                button::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        palette.danger.r,
                        palette.danger.g,
                        palette.danger.b,
                        alpha,
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(6.0),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::ConfirmPowerAction);

            let cancel_btn = button(
                text("İptal")
                    .size(12.0)
                    .color(palette.text_primary.to_iced()),
            )
            .padding([4, 8])
            .style(|_, status| button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    1.0,
                    1.0,
                    1.0,
                    if matches!(status, button::Status::Hovered) {
                        0.18
                    } else {
                        0.10
                    },
                ))),
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::CancelPowerAction);

            status_row = status_row.push(
                row![
                    text(action.confirm_prompt())
                        .size(12.0)
                        .color(palette.warning.to_iced()),
                    confirm_btn,
                    cancel_btn,
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            );
        } else {
            let icon_btn_style = |_: &iced::Theme, status: button::Status| button::Style {
                background: match status {
                    button::Status::Hovered => Some(iced::Background::Color(
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    )),
                    button::Status::Pressed => Some(iced::Background::Color(
                        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.20),
                    )),
                    _ => None,
                },
                border: iced::Border {
                    radius: iced::border::Radius::from(6.0),
                    ..Default::default()
                },
                ..Default::default()
            };

            let suspend_btn = button(
                text(Icon::MOON)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([5, 7])
            .style(icon_btn_style)
            .on_press(Message::RequestPowerAction(PowerAction::Suspend));

            let reboot_btn = button(
                text(Icon::REFRESH)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(palette.text_muted.to_iced()),
            )
            .padding([5, 7])
            .style(icon_btn_style)
            .on_press(Message::RequestPowerAction(PowerAction::Reboot));

            let power_btn = button(
                text(Icon::POWER)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(palette.danger.to_iced()),
            )
            .padding([5, 7])
            .style(icon_btn_style)
            .on_press(Message::RequestPowerAction(PowerAction::Poweroff));

            status_row = status_row
                .push(suspend_btn)
                .push(reboot_btn)
                .push(power_btn);
        }

        let right_status =
            container(status_row)
                .padding([8, 16])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.09, 0.10, 0.12, 0.85,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                        width: 1.0,
                        radius: iced::border::Radius::from(12.0),
                    },
                    ..Default::default()
                });

        let bottom_bar = row![left_media, iced::widget::horizontal_space(), right_status]
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .padding(Padding {
                top: 0.0,
                right: 32.0,
                bottom: 24.0,
                left: 32.0,
            });

        let mut main_items = vec![
            iced::widget::vertical_space().into(),
            time_section.into(),
            center_card.into(),
        ];

        if self.show_virtual_keyboard {
            main_items.push(self.render_virtual_keyboard(&palette));
        }

        main_items.push(iced::widget::vertical_space().into());
        main_items.push(bottom_bar.into());

        let main_column = iced::widget::Column::with_children(main_items)
            .spacing(if self.show_virtual_keyboard { 14 } else { 28 })
            .align_x(Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fill);

        let ui_overlay = container(main_column)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.05, 0.06, 0.08, 0.65,
                ))),
                ..Default::default()
            });

        if let Some(ref handle) = self.wallpaper_handle {
            let bg_img = image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover);

            stack![bg_img, ui_overlay].into()
        } else {
            ui_overlay.into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_auth_controller_flow() {
        let mut auth = AuthController::new("testuser");
        assert_eq!(auth.username(), "testuser");
        assert_eq!(auth.secret(), "");

        for ch in "şifre123".chars() {
            assert!(auth.push_char(ch));
        }
        assert_eq!(auth.char_count(), 8);

        let creds = auth.begin_zeroized();
        assert!(creds.is_some());
        let (u, p) = creds.unwrap();
        assert_eq!(u, "testuser");
        assert_eq!(p.as_str(), "şifre123");

        // Mutations must be ignored while pending
        assert!(!auth.push_char('!'));
        assert!(!auth.pop_char());
        auth.cancel_input();
        assert!(auth.is_pending());

        auth.complete(false);
        assert_eq!(auth.secret(), "");
        assert_eq!(auth.failed_attempts(), 1);
    }

    #[test]
    fn test_password_display_utf8_and_truncation() {
        // UTF-8 multi-byte Turkish characters must produce 1 dot per character, not per byte
        assert_eq!(
            CortenLockApp::format_password_display("şğü", false),
            "● ● ●"
        );
        assert_eq!(
            CortenLockApp::format_password_display("şğü", true),
            "şğü"
        );

        let long_pw = "a".repeat(30);
        let masked = CortenLockApp::format_password_display(&long_pw, false);
        assert!(masked.starts_with('…'));
        assert_eq!(masked.matches('●').count(), MAX_VISIBLE_PASSWORD_CHARS - 1);

        let revealed = CortenLockApp::format_password_display(&long_pw, true);
        assert!(revealed.starts_with('…'));
        assert_eq!(revealed.chars().count(), MAX_VISIBLE_PASSWORD_CHARS + 1);
    }

    #[test]
    fn test_virtual_key_caps_lock_vs_shift() {
        // Caps Lock uppercases letters but must NOT shift numbers/punctuation
        assert_eq!(
            CortenLockApp::resolve_virtual_key_char('a', 'A', false, true),
            'A'
        );
        assert_eq!(
            CortenLockApp::resolve_virtual_key_char('1', '!', false, true),
            '1'
        );
        assert_eq!(
            CortenLockApp::resolve_virtual_key_char('1', '!', true, false),
            '!'
        );
        assert_eq!(
            CortenLockApp::resolve_virtual_key_char('ş', 'Ş', false, true),
            'Ş'
        );
        // Shift + Caps Lock cancels letter case (standard keyboard behavior)
        assert_eq!(
            CortenLockApp::resolve_virtual_key_char('a', 'A', true, true),
            'a'
        );
    }

    #[test]
    fn test_turkish_date_formatting() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        assert_eq!(CortenLockApp::format_turkish_date(&d), "Pazartesi, 23 Kasım");
        let d2 = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(CortenLockApp::format_turkish_date(&d2), "Perşembe, 1 Ocak");
    }

    #[test]
    fn test_power_action_confirmation_rules() {
        assert!(!PowerAction::Suspend.requires_confirmation());
        assert!(PowerAction::Reboot.requires_confirmation());
        assert!(PowerAction::Poweroff.requires_confirmation());
    }

    #[test]
    fn test_resolve_source_wallpaper_non_panicking() {
        let _ = CortenLockApp::resolve_source_wallpaper();
    }
}
