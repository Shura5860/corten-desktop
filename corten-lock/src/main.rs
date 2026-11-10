mod auth;

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use auth::PamAuth;
use chrono::Local;
use corten_auth::AuthController;
use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::{Icon, Palette, PatinaVariant, Radius, ThemeMode, LUCIDE_FONT, LUCIDE_FONT_BYTES};
use iced::widget::{button, container, image, row, stack, text};
use iced::{event, Alignment, Element, Event, Length, Padding, Subscription, Task};
use iced_sessionlock::actions::UnLockAction;
use iced_sessionlock::settings::Settings;
use iced_sessionlock::MultiApplication;
use tracing::info;

pub fn main() -> Result<(), iced_sessionlock::Error> {
    // Diagnostics: wgpu_hal/wgpu_core at info level reveal which GPU adapter is selected
    // (e.g. NVIDIA vs llvmpipe software fallback). RUST_LOG still overrides this default.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_hal::vulkan::conv=error,wgpu_core=info".into()),
        )
        .init();
    info!("Starting corten-lock (Faz 5: Screen Locker)...");

    // NVIDIA proprietary Vulkan WSI on Wayland blocks inside vkQueuePresent/vkAcquireNextImage
    // (wl_display_dispatch_queue_timeout) for 0.5-10 s when presenting with FIFO/vsync against our
    // compositor, freezing the whole event loop. Mailbox presentation (the driver only offers Mailbox and Fifo) avoids that wait.
    // The user can still override it via ICED_PRESENT_MODE.
    if std::env::var_os("ICED_PRESENT_MODE").is_none() {
        std::env::set_var("ICED_PRESENT_MODE", "mailbox");
    }

    let current_user = users::get_current_username()
        .map(|u| u.to_string_lossy().to_string())
        .unwrap_or_else(|| "user".into());

    CortenLockApp::run(Settings {
        id: Some("corten-lock".into()),
        flags: current_user,
        fonts: vec![Cow::Borrowed(LUCIDE_FONT_BYTES)],
        default_font: iced::Font::default(),
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
    })?;

    Ok(())
}

pub struct CortenLockApp {
    auth: AuthController,
    show_password: bool,
    show_virtual_keyboard: bool,
    virtual_shift: bool,
    current_time: String,
    current_date: String,
    caps_lock_on: bool,
    wallpaper_handle: Option<image::Handle>,
    media_track: Option<corten_services::mpris::MediaPlayer>,
    battery_info: Option<(u8, bool)>,
    network_online: bool,
    shake_count: u8,
    tick_count: u32,
    active_patina: PatinaVariant,
}

impl CortenLockApp {
    fn get_media_status() -> Option<corten_services::mpris::MediaPlayer> {
        corten_services::mpris::list_players().ok().and_then(|players| {
            players.into_iter().find(|p| !p.title.is_empty())
        })
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
                    let is_charging = status.trim().eq_ignore_ascii_case("Charging");
                    if let Some(c) = cap {
                        return Some((c, is_charging));
                    }
                }
            }
        }
        None
    }

    fn get_network_status() -> bool {
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if name == "lo" {
                    continue;
                }
                let operstate = entry.path().join("operstate");
                if let Ok(state) = std::fs::read_to_string(operstate) {
                    if state.trim() == "up" {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn resolve_source_wallpaper() -> Option<PathBuf> {
        if let Ok(path_str) = std::env::var("CORTEN_WALLPAPER") {
            let p = PathBuf::from(path_str);
            if p.exists() {
                return Some(p);
            }
        }

        let home = std::env::var("HOME").ok()?;
        let home_path = std::path::Path::new(&home);

        let corten_wp_png = home_path.join(".config/corten/wallpaper.png");
        if corten_wp_png.exists() {
            return Some(corten_wp_png);
        }
        let corten_wp_jpg = home_path.join(".config/corten/wallpaper.jpg");
        if corten_wp_jpg.exists() {
            return Some(corten_wp_jpg);
        }

        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()));
        let search_roots = [
            home_path.join("Projects/WaylandDesktopProject/assets/wallpapers"),
            PathBuf::from("/usr/share/backgrounds/corten"),
            PathBuf::from("assets/wallpapers"),
            exe_dir
                .map(|d| d.join("../../../assets/wallpapers"))
                .unwrap_or_else(|| PathBuf::from("assets/wallpapers")),
        ];

        for root in &search_roots {
            let landscape = root.join("corten_dark_landscape.jpg");
            if landscape.exists() {
                return Some(landscape);
            }
            let portrait = root.join("corten_dark_portrait.jpg");
            if portrait.exists() {
                return Some(portrait);
            }
        }

        let fallback_system = [
            "/usr/share/wallpapers/Next/contents/images_dark/5120x2880.png",
            "/usr/share/wallpapers/Next/contents/images/5120x2880.png",
            "/usr/share/wallpapers/Air/contents/images/5120x2880.png",
        ];

        for path in fallback_system {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }

        None
    }

    fn ensure_blurred_wallpaper() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let cache_dir = PathBuf::from(&home).join(".cache/corten");
        let blur_cache = cache_dir.join("lock_wallpaper_blur.png");

        let src = Self::resolve_source_wallpaper()?;

        let is_cache_fresh = if blur_cache.exists() {
            let cache_mtime = blur_cache.metadata().and_then(|m| m.modified()).ok();
            let src_mtime = src.metadata().and_then(|m| m.modified()).ok();
            match (cache_mtime, src_mtime) {
                (Some(c), Some(s)) => c >= s,
                _ => true,
            }
        } else {
            false
        };

        if is_cache_fresh {
            return Some(blur_cache);
        }

        let _ = std::fs::create_dir_all(&cache_dir);

        // Spawn background blur generation so lock screen displays immediately without freezing UI thread
        let src_clone = src.clone();
        let cache_clone = blur_cache.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-i",
                    &src_clone.to_string_lossy(),
                    "-vf",
                    "scale=1920:1080:force_original_aspect_ratio=increase,crop=1920:1080,gblur=sigma=30",
                    &cache_clone.to_string_lossy(),
                ])
                .status();
        });

        // If stale blur cache exists, use it temporarily; otherwise return raw src wallpaper
        if blur_cache.exists() {
            Some(blur_cache)
        } else {
            Some(src)
        }
    }

    fn render_virtual_keyboard(&self, palette: &Palette) -> Element<'_, Message> {
        let is_upper = self.virtual_shift || self.caps_lock_on;
        let pal = *palette;

        let make_key = move |normal: char, shifted: char| -> Element<'_, Message> {
            let ch = if is_upper { shifted } else { normal };
            let label = ch.to_string();
            button(
                text(label)
                    .size(13.0)
                    .color(pal.text_primary.to_iced()),
            )
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(34.0))
            .style(move |_, _| button::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            })
            .on_press(Message::VirtualKeyPress(ch))
            .into()
        };

        // Row 1: Numbers
        let r1_chars = [
            ('1', '!'), ('2', '@'), ('3', '#'), ('4', '$'), ('5', '%'),
            ('6', '^'), ('7', '&'), ('8', '*'), ('9', '('), ('0', ')'),
            ('-', '_'), ('=', '+'),
        ];
        let mut row1 = row![].spacing(4).align_y(Alignment::Center);
        for (n, s) in r1_chars {
            row1 = row1.push(make_key(n, s));
        }
        let backspace_btn = button(
            text("⌫")
                .size(14.0)
                .color(pal.warning.to_iced()),
        )
        .width(Length::Fixed(46.0))
        .height(Length::Fixed(34.0))
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 0.4, 0.2, 0.15))),
            border: iced::Border {
                color: iced::Color::from_rgba(pal.warning.r, pal.warning.g, pal.warning.b, 0.4),
                width: 1.0,
                radius: iced::border::Radius::from(6.0),
            },
            ..Default::default()
        })
        .on_press(Message::VirtualKeyBackspace);
        row1 = row1.push(backspace_btn);

        // Row 2: QWERTY
        let r2_chars = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']'];
        let mut row2 = row![].spacing(4).align_y(Alignment::Center);
        for c in r2_chars {
            let s = match c {
                '[' => '{',
                ']' => '}',
                _ => c.to_ascii_uppercase(),
            };
            row2 = row2.push(make_key(c, s));
        }

        // Row 3: ASDF
        let r3_chars = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', '\''];
        let mut row3 = row![].spacing(4).align_y(Alignment::Center);
        let shift_active = self.virtual_shift || self.caps_lock_on;
        let shift_btn = button(
            text("⇧")
                .size(15.0)
                .color(if shift_active { pal.primary.to_iced() } else { pal.text_muted.to_iced() }),
        )
        .width(Length::Fixed(48.0))
        .height(Length::Fixed(34.0))
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(if shift_active {
                iced::Color::from_rgba(pal.primary.r, pal.primary.g, pal.primary.b, 0.25)
            } else {
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08)
            })),
            border: iced::Border {
                color: iced::Color::from_rgba(pal.primary.r, pal.primary.g, pal.primary.b, 0.45),
                width: 1.0,
                radius: iced::border::Radius::from(6.0),
            },
            ..Default::default()
        })
        .on_press(Message::VirtualKeyShift);
        row3 = row3.push(shift_btn);
        for c in r3_chars {
            let s = match c {
                ';' => ':',
                '\'' => '"',
                _ => c.to_ascii_uppercase(),
            };
            row3 = row3.push(make_key(c, s));
        }
        let enter_btn = button(
            text("↵")
                .size(15.0)
                .color(pal.background.to_iced()),
        )
        .width(Length::Fixed(50.0))
        .height(Length::Fixed(34.0))
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(pal.primary.to_iced())),
            border: iced::Border {
                radius: iced::border::Radius::from(6.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Message::SubmitPassword);
        row3 = row3.push(enter_btn);

        // Row 4: ZXCV & Space & Clear
        let r4_chars = ['z', 'x', 'c', 'v', 'b', 'n', 'm', ',', '.', '/'];
        let mut row4 = row![].spacing(4).align_y(Alignment::Center);
        for c in r4_chars {
            let s = match c {
                ',' => '<',
                '.' => '>',
                '/' => '?',
                _ => c.to_ascii_uppercase(),
            };
            row4 = row4.push(make_key(c, s));
        }
        let space_btn = button(
            text("Boşluk")
                .size(12.0)
                .color(pal.text_primary.to_iced()),
        )
        .width(Length::Fixed(100.0))
        .height(Length::Fixed(34.0))
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                width: 1.0,
                radius: iced::border::Radius::from(6.0),
            },
            ..Default::default()
        })
        .on_press(Message::VirtualKeySpace);
        row4 = row4.push(space_btn);

        let clear_btn = button(
            text("Temizle")
                .size(11.0)
                .color(pal.danger.to_iced()),
        )
        .width(Length::Fixed(60.0))
        .height(Length::Fixed(34.0))
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 0.2, 0.2, 0.12))),
            border: iced::Border {
                color: iced::Color::from_rgba(pal.danger.r, pal.danger.g, pal.danger.b, 0.35),
                width: 1.0,
                radius: iced::border::Radius::from(6.0),
            },
            ..Default::default()
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
            background: Some(iced::Background::Color(iced::Color::from_rgba(0.08, 0.09, 0.11, 0.95))),
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
    TogglePasswordReveal,
    ToggleVirtualKeyboard,
    VirtualKeyPress(char),
    VirtualKeyBackspace,
    VirtualKeyClear,
    VirtualKeyShift,
    VirtualKeySpace,
    SubmitPassword,
    AuthResult(bool),
    Unlock,
    LockMediaPlayPause(String),
    LockMediaNext(String),
    LockMediaPrevious(String),
    SystemPowerAction(&'static str),
    IcedEvent(Event),
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
        let current_date = now.format("%A, %d %B").to_string();

        let wallpaper_path = Self::ensure_blurred_wallpaper();
        let wallpaper_handle = wallpaper_path.as_ref().map(image::Handle::from_path);
        let media_track = Self::get_media_status();
        let battery_info = Self::get_battery_info();
        let network_online = Self::get_network_status();

        let load_font_task = iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::Tick);
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        (
            Self {
                auth: AuthController::new(flags),
                show_password: false,
                show_virtual_keyboard: false,
                virtual_shift: false,
                current_time,
                current_date,
                caps_lock_on: false,
                wallpaper_handle,
                media_track,
                battery_info,
                network_online,
                shake_count: 0,
                tick_count: 0,
                active_patina,
            },
            load_font_task,
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
        let events = event::listen().map(Message::IcedEvent);
        let timer = iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick);
        Subscription::batch(vec![events, timer])
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Tick => {
                self.tick_count = self.tick_count.wrapping_add(1);
                if self.shake_count > 0 {
                    self.shake_count -= 1;
                }
                if self.tick_count.is_multiple_of(10) {
                    let now = Local::now();
                    self.current_time = now.format("%H:%M").to_string();
                    self.current_date = now.format("%A, %d %B").to_string();
                    self.media_track = Self::get_media_status();
                    self.battery_info = Self::get_battery_info();
                    self.network_online = Self::get_network_status();
                    if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                        let new_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                        if new_patina != self.active_patina {
                            self.active_patina = new_patina;
                        }
                    }
                }
            }
            Message::TogglePasswordReveal => {
                self.show_password = !self.show_password;
            }
            Message::ToggleVirtualKeyboard => {
                self.show_virtual_keyboard = !self.show_virtual_keyboard;
            }
            Message::VirtualKeyPress(ch) => {
                let actual_char = if self.virtual_shift || self.caps_lock_on {
                    ch.to_ascii_uppercase()
                } else {
                    ch.to_ascii_lowercase()
                };
                let mut secret = self.auth.secret().to_string();
                secret.push(actual_char);
                self.auth.set_secret(secret);
                if self.virtual_shift && !self.caps_lock_on {
                    self.virtual_shift = false;
                }
            }
            Message::VirtualKeyBackspace => {
                let mut secret = self.auth.secret().to_string();
                secret.pop();
                self.auth.set_secret(secret);
            }
            Message::VirtualKeyClear => {
                self.auth.clear_secret();
            }
            Message::VirtualKeyShift => {
                self.virtual_shift = !self.virtual_shift;
            }
            Message::VirtualKeySpace => {
                let mut secret = self.auth.secret().to_string();
                secret.push(' ');
                self.auth.set_secret(secret);
            }
            Message::SubmitPassword => {
                let Some((user, pass)) = self.auth.begin() else {
                    return Task::none();
                };

                return Task::perform(
                    async move { PamAuth::authenticate(&user, &pass) },
                    Message::AuthResult,
                );
            }
            Message::AuthResult(success) => {
                self.auth.complete(success);
                if success {
                    info!("Authentication successful, unlocking Corten session!");
                    return Task::done(Message::Unlock);
                } else {
                    self.shake_count = 6;
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
            Message::SystemPowerAction(action) => {
                let _ = std::process::Command::new("systemctl").arg(action).spawn();
            }
            Message::IcedEvent(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                modified_key,
                modifiers,
                ..
            })) => {
                if modifiers.control() {
                    match modified_key {
                        iced::keyboard::Key::Character(ref sm)
                            if sm == "u" || sm == "U" || sm == "c" || sm == "C" =>
                        {
                            self.auth.clear_secret();
                        }
                        iced::keyboard::Key::Character(ref sm) if sm == "h" || sm == "H" => {
                            let mut secret = self.auth.secret().to_string();
                            secret.pop();
                            self.auth.set_secret(secret);
                        }
                        _ => {}
                    }
                } else {
                    match modified_key {
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
                            self.auth.cancel();
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::CapsLock) => {
                            self.caps_lock_on = !self.caps_lock_on;
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter) => {
                            return Task::done(Message::SubmitPassword);
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace) => {
                            let mut secret = self.auth.secret().to_string();
                            secret.pop();
                            self.auth.set_secret(secret);
                        }
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete) => {
                            self.auth.clear_secret();
                        }
                        iced::keyboard::Key::Character(ref sm) => {
                            for ch in sm.chars() {
                                if !ch.is_control() {
                                    let mut secret = self.auth.secret().to_string();
                                    secret.push(ch);
                                    self.auth.set_secret(secret);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Message::IcedEvent(_) | Message::Unlock => {}
        }
        Task::none()
    }

    fn view(&self, _window: iced::window::Id) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(ThemeMode::Dark).with_patina(self.active_patina);

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

        // ==================== CENTER CARD: AVATAR & CREDENTIALS ====================
        let initial_char = self
            .auth
            .username()
            .chars()
            .next()
            .unwrap_or('U')
            .to_uppercase();
        let avatar = container(
            text(initial_char.to_string())
                .size(32.0)
                .color(palette.primary.to_iced()),
        )
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

        // Direct password display: 100% reliable, zero-latency, no layer-shell focus issues
        let pass_display = if self.auth.secret().is_empty() {
            text("Parola girin...")
                .size(14.0)
                .color(palette.text_muted.to_iced())
        } else if self.show_password {
            text(self.auth.secret())
                .size(15.0)
                .color(palette.text_primary.to_iced())
        } else {
            text("● ".repeat(self.auth.secret().len()))
                .size(13.0)
                .color(palette.text_primary.to_iced())
        };

        let eye_icon = if self.show_password { "●" } else { "○" };
        let reveal_btn = button(
            text(eye_icon)
                .size(15.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, _| button::Style::default())
        .on_press(Message::TogglePasswordReveal);

        let kbd_btn = button(
            text("⌨")
                .size(16.0)
                .color(if self.show_virtual_keyboard {
                    palette.primary.to_iced()
                } else {
                    palette.text_muted.to_iced()
                }),
        )
        .padding([4, 8])
        .style(|_, _| button::Style::default())
        .on_press(Message::ToggleVirtualKeyboard);

        let submit_btn = button(
            text(if self.auth.is_pending() { "…" } else { "↵" })
                .size(18.0)
                .color(palette.background.to_iced()),
        )
        .padding([8, 14])
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(palette.primary.to_iced())),
            border: iced::Border {
                radius: iced::border::Radius::from(8.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Message::SubmitPassword);

        let input_box = container(
            row![
                container(pass_display).width(Length::Fill).padding([8, 12]),
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

        let shake_offset: f32 = match self.shake_count {
            5 | 1 => 10.0,
            4 | 2 => -10.0,
            3 => 6.0,
            _ => 0.0,
        };

        let shaken_input_box = container(input_box).padding(Padding {
            top: 0.0,
            right: (10.0 - shake_offset).max(0.0),
            bottom: 0.0,
            left: (10.0 + shake_offset).max(0.0),
        });

        let status_feedback = if self.auth.is_pending() {
            text("Doğrulanıyor...")
                .size(13.0)
                .color(palette.text_muted.to_iced())
        } else if self.auth.has_failed() {
            text("Hatalı parola, lütfen tekrar deneyin")
                .size(13.0)
                .color(palette.danger.to_iced())
        } else if self.caps_lock_on {
            text("⇪ Caps Lock Açık")
                .size(12.0)
                .color(palette.warning.to_iced())
        } else {
            text("").size(13.0).color(iced::Color::TRANSPARENT)
        };

        let center_card = container(
            iced::widget::column![avatar, welcome_text, shaken_input_box, status_feedback,]
                .spacing(18)
                .align_x(Alignment::Center),
        )
        .padding([28, 36])
        .width(Length::Fixed(380.0))
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.09, 0.10, 0.12, 0.88,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(
                    palette.primary.r,
                    palette.primary.g,
                    palette.primary.b,
                    0.35,
                ),
                width: 1.5,
                radius: iced::border::Radius::from(24.0),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.55),
                offset: iced::Vector::new(0.0, 10.0),
                blur_radius: 32.0,
            },
            ..Default::default()
        });

        // ==================== BOTTOM ROW ====================
        let left_media: Element<'_, Self::Message> = if let Some(ref player) = self.media_track {
            let is_playing = player.playback_status == corten_services::mpris::PlaybackStatus::Playing;
            let play_icon = if is_playing { "⏸" } else { "▶" };
            let bus = player.bus_name.clone();
            let bus_prev = bus.clone();
            let bus_next = bus.clone();

            let title_display = if player.title.chars().count() > 22 {
                format!("{}…", player.title.chars().take(22).collect::<String>())
            } else {
                player.title.clone()
            };

            let prev_btn = button(text("⏮").size(13.0).color(palette.text_primary.to_iced()))
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::LockMediaPrevious(bus_prev));

            let play_btn = button(text(play_icon).size(14.0).color(palette.primary.to_iced()))
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::LockMediaPlayPause(bus));

            let next_btn = button(text("⏭").size(13.0).color(palette.text_primary.to_iced()))
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::LockMediaNext(bus_next));

            let mut info_col = iced::widget::column![
                text(title_display).size(12.0).color(palette.text_primary.to_iced())
            ];

            if !player.artist.is_empty() {
                let artist_display = if player.artist.chars().count() > 20 {
                    format!("{}…", player.artist.chars().take(20).collect::<String>())
                } else {
                    player.artist.clone()
                };
                info_col = info_col.push(
                    text(artist_display).size(10.0).color(palette.text_muted.to_iced())
                );
            }

            container(
                row![
                    text(Icon::MUSIC)
                        .font(LUCIDE_FONT)
                        .size(15.0)
                        .color(palette.primary.to_iced()),
                    info_col.spacing(2),
                    row![prev_btn, play_btn, next_btn].spacing(2).align_y(Alignment::Center),
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

        let mut status_row = row![].spacing(14).align_y(Alignment::Center);

        if let Some((cap, _charging)) = self.battery_info {
            status_row = status_row.push(
                row![
                    text(Icon::BATTERY)
                        .font(LUCIDE_FONT)
                        .size(15.0)
                        .color(palette.text_primary.to_iced()),
                    text(format!("%{}", cap))
                        .size(12.0)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            );
        }

        if self.network_online {
            status_row = status_row.push(
                text(Icon::WIFI)
                    .font(LUCIDE_FONT)
                    .size(15.0)
                    .color(palette.text_primary.to_iced()),
            );
        }

        let reboot_btn = button(
            text(Icon::CLOCK)
                .font(LUCIDE_FONT)
                .size(15.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 6])
        .style(|_, _| button::Style::default())
        .on_press(Message::SystemPowerAction("reboot"));

        let power_btn = button(
            text(Icon::POWER)
                .font(LUCIDE_FONT)
                .size(15.0)
                .color(palette.danger.to_iced()),
        )
        .padding([4, 6])
        .style(|_, _| button::Style::default())
        .on_press(Message::SystemPowerAction("poweroff"));

        status_row = status_row.push(reboot_btn);
        status_row = status_row.push(power_btn);

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

        let bottom_bar = row![left_media, iced::widget::horizontal_space(), right_status,]
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

    #[test]
    fn test_auth_controller_flow() {
        let mut auth = AuthController::new("testuser");
        assert_eq!(auth.username(), "testuser");
        assert_eq!(auth.secret(), "");

        auth.set_secret("my_secure_password");
        assert_eq!(auth.secret(), "my_secure_password");

        let creds = auth.begin();
        assert!(creds.is_some());
        let (u, p) = creds.unwrap();
        assert_eq!(u, "testuser");
        assert_eq!(p, "my_secure_password");

        auth.clear_secret();
        assert_eq!(auth.secret(), "");
    }

    #[test]
    fn test_resolve_source_wallpaper_non_panicking() {
        // Should not panic even if no custom wallpaper is set
        let _ = CortenLockApp::resolve_source_wallpaper();
    }
}
