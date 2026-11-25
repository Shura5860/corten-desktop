mod ipc;
mod power;
mod sessions;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Datelike, Local};
use corten_auth::AuthController;
use corten_config::AppearanceConfig;
use corten_theme::iced_styles::acrylic_glass_style;
use corten_theme::{
    Icon, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS, NOTO_SANS_BYTES, NOTO_SANS_SEMIBOLD, Palette,
    PatinaVariant, Radius, ThemeMode,
};
use iced::keyboard::Key;
use iced::keyboard::key::Named;
use iced::widget::{button, container, horizontal_space, image, row, stack, text, text_input};
use iced::{Alignment, Element, Event, Length, Padding, Subscription, Task, event};
use ipc::GreetdClient;
use power::PowerActions;
use sessions::{SessionEntry, SessionManager, UserAccount};
use tracing::info;

const POWER_CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();
    info!("Starting corten-greeter...");

    iced::application(
        "Corten Login Screen",
        CortenGreeterApp::update,
        CortenGreeterApp::view,
    )
    .subscription(CortenGreeterApp::subscription)
    .theme(|_| iced::Theme::Dark)
    .font(LUCIDE_FONT_BYTES)
    .font(NOTO_SANS_BYTES)
    .default_font(NOTO_SANS)
    .window(iced::window::Settings {
        size: iced::Size::new(1280.0, 800.0),
        min_size: Some(iced::Size::new(800.0, 600.0)),
        resizable: true,
        decorations: false,
        ..Default::default()
    })
    .antialiasing(true)
    .run_with(CortenGreeterApp::boot)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Suspend,
    Reboot,
    Shutdown,
}

impl PowerAction {
    fn requires_confirmation(self) -> bool {
        matches!(self, Self::Reboot | Self::Shutdown)
    }

    fn confirm_prompt(self) -> &'static str {
        match self {
            Self::Suspend => "Uyku moduna geçilsin mi?",
            Self::Reboot => "Yeniden başlatılsın mı?",
            Self::Shutdown => "Bilgisayar kapatılsın mı?",
        }
    }
}

pub struct CortenGreeterApp {
    auth: AuthController,
    human_users: Vec<UserAccount>,
    selected_user_idx: usize,
    current_time: String,
    current_date: String,
    sessions: Vec<SessionEntry>,
    selected_session_idx: usize,
    show_password: bool,
    caps_lock_on: bool,
    keyboard_layout: String,
    wallpaper_handle: Option<image::Handle>,
    pending_power_confirm: Option<(PowerAction, Instant)>,
    error_message: Option<String>,
    active_patina: PatinaVariant,
    user_input_id: text_input::Id,
    pass_input_id: text_input::Id,
    password_focused: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    UsernameChanged(String),
    PasswordChanged(String),
    TogglePasswordReveal,
    CycleUser,
    CycleSession,
    SubmitLogin,
    LoginResult(Result<(), String>),
    RequestPowerAction(PowerAction),
    ConfirmPowerAction,
    CancelPowerAction,
    IcedEvent(Event),
}

impl CortenGreeterApp {
    fn boot() -> (Self, Task<Message>) {
        let app = Self::default();
        let focus_id = if app.auth.username().trim().is_empty() {
            app.user_input_id.clone()
        } else {
            app.pass_input_id.clone()
        };
        (app, text_input::focus(focus_id))
    }

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

    fn detect_caps_lock_led() -> bool {
        if let Ok(entries) = std::fs::read_dir("/sys/class/leds") {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with("::capslock")
                    && let Ok(val) = std::fs::read_to_string(entry.path().join("brightness"))
                    && val.trim() == "1"
                {
                    return true;
                }
            }
        }
        false
    }

    fn detect_keyboard_layout() -> String {
        if let Ok(layout) = std::env::var("XKB_DEFAULT_LAYOUT") {
            let trimmed = layout.trim();
            if !trimmed.is_empty() {
                return trimmed.split(',').next().unwrap_or(trimmed).to_uppercase();
            }
        }
        if let Ok(vconsole) = std::fs::read_to_string("/etc/vconsole.conf") {
            for line in vconsole.lines() {
                if let Some(km) = line.trim().strip_prefix("KEYMAP=") {
                    let clean = km.trim().trim_matches('"').trim_matches('\'');
                    if clean.starts_with("tr") {
                        return "TR".to_string();
                    }
                    if !clean.is_empty() {
                        return clean.chars().take(2).collect::<String>().to_uppercase();
                    }
                }
            }
        }
        "TR".to_string()
    }

    fn resolve_wallpaper() -> Option<PathBuf> {
        if let Ok(path_str) = std::env::var("CORTEN_WALLPAPER") {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                return Some(p);
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
        None
    }

    fn load_patina_for_home(home_dir: Option<&str>) -> PatinaVariant {
        let mut candidates = Vec::new();
        if let Some(home) = home_dir {
            candidates.push(Path::new(home).join(".config/corten/appearance.toml"));
        }
        candidates.push(PathBuf::from("/etc/corten/appearance.toml"));

        for path in candidates {
            if let Ok(raw) = std::fs::read_to_string(path)
                && let Ok(cfg) = toml::from_str::<AppearanceConfig>(&raw)
            {
                return PatinaVariant::from_code(&cfg.patina);
            }
        }
        PatinaVariant::Rust
    }

    fn subscription(&self) -> Subscription<Message> {
        let tick = iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick);
        let events = event::listen_with(|ev, _status, _id| match ev {
            Event::Keyboard(iced::keyboard::Event::KeyPressed { .. }) => {
                Some(Message::IcedEvent(ev))
            }
            _ => None,
        });
        Subscription::batch(vec![tick, events])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let now = Local::now();
                let new_time = now.format("%H:%M").to_string();
                if new_time != self.current_time {
                    self.current_time = new_time;
                    self.current_date = Self::format_turkish_date(&now);
                }
                self.caps_lock_on = Self::detect_caps_lock_led();
                if let Some((_, asked_at)) = self.pending_power_confirm
                    && asked_at.elapsed() >= POWER_CONFIRM_TIMEOUT
                {
                    self.pending_power_confirm = None;
                }
            }
            Message::UsernameChanged(name) => {
                self.auth.set_username(name);
                self.error_message = None;
            }
            Message::PasswordChanged(pass) => {
                self.auth.set_secret(pass);
                self.error_message = None;
            }
            Message::TogglePasswordReveal => {
                self.show_password = !self.show_password;
            }
            Message::CycleUser => {
                if !self.human_users.is_empty() && !self.auth.is_pending() {
                    self.selected_user_idx = (self.selected_user_idx + 1) % self.human_users.len();
                    let next = &self.human_users[self.selected_user_idx];
                    self.auth.set_username(next.username.clone());
                    self.auth.cancel_input();
                    self.error_message = None;
                    self.active_patina = Self::load_patina_for_home(Some(&next.home_dir));
                    self.password_focused = true;
                    return text_input::focus(self.pass_input_id.clone());
                }
            }
            Message::CycleSession => {
                if !self.sessions.is_empty() {
                    self.selected_session_idx =
                        (self.selected_session_idx + 1) % self.sessions.len();
                }
            }
            Message::SubmitLogin => {
                if self.auth.is_pending() {
                    return Task::none();
                }
                if self.auth.username().trim().is_empty() {
                    self.error_message = Some("Lütfen geçerli bir kullanıcı adı girin.".into());
                    self.password_focused = false;
                    return text_input::focus(self.user_input_id.clone());
                }

                let Some((user, pass)) = self.auth.begin_zeroized() else {
                    self.error_message = Some("Lütfen parolanızı girin.".into());
                    self.password_focused = true;
                    return text_input::focus(self.pass_input_id.clone());
                };

                self.error_message = None;

                let (session_cmd, is_wayland) = self
                    .sessions
                    .get(self.selected_session_idx)
                    .map(|s| (s.exec.clone(), s.is_wayland))
                    .unwrap_or_else(|| (vec!["corten-session".into()], true));

                // Preview mode must be explicit; production must never fake authentication.
                if std::env::var("GREETD_SOCK").is_err()
                    && std::env::var("CORTEN_GREETER_PREVIEW").as_deref() != Ok("1")
                {
                    self.auth.cancel();
                    self.error_message = Some(
                        "greetd bağlantısı bulunamadı. Gerçek oturum açmak için greeter'ı greetd ile çalıştırın."
                            .into(),
                    );
                    return Task::none();
                }

                if std::env::var("GREETD_SOCK").is_err() {
                    return Task::perform(
                        async move {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            info!(
                                "Running in preview/test mode (GREETD_SOCK unset). Simulating login success for user '{}'",
                                user
                            );
                            Ok(())
                        },
                        Message::LoginResult,
                    );
                }

                return Task::perform(
                    GreetdClient::login_and_start_session_async(
                        user,
                        pass,
                        session_cmd,
                        is_wayland,
                    ),
                    Message::LoginResult,
                );
            }
            Message::LoginResult(result) => {
                self.auth.complete(result.is_ok());
                match result {
                    Ok(_) => {
                        return iced::window::get_latest()
                            .then(|id| id.map(iced::window::close).unwrap_or_else(Task::none));
                    }
                    Err(err) => {
                        self.error_message = Some(err);
                        self.password_focused = true;
                        return text_input::focus(self.pass_input_id.clone());
                    }
                }
            }
            Message::RequestPowerAction(action) => {
                if action.requires_confirmation() {
                    self.pending_power_confirm = Some((action, Instant::now()));
                } else {
                    self.pending_power_confirm = None;
                    PowerActions::suspend();
                }
            }
            Message::ConfirmPowerAction => {
                if let Some((action, _)) = self.pending_power_confirm.take() {
                    match action {
                        PowerAction::Suspend => PowerActions::suspend(),
                        PowerAction::Reboot => PowerActions::reboot(),
                        PowerAction::Shutdown => PowerActions::shutdown(),
                    }
                }
            }
            Message::CancelPowerAction => {
                self.pending_power_confirm = None;
            }
            Message::IcedEvent(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(Named::Tab),
                ..
            })) => {
                self.password_focused = !self.password_focused;
                let target = if self.password_focused {
                    self.pass_input_id.clone()
                } else {
                    self.user_input_id.clone()
                };
                return text_input::focus(target);
            }
            Message::IcedEvent(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            })) => {
                if self.pending_power_confirm.is_some() {
                    self.pending_power_confirm = None;
                } else {
                    self.auth.cancel_input();
                    self.error_message = None;
                }
            }
            Message::IcedEvent(Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(Named::CapsLock),
                ..
            })) => {
                self.caps_lock_on = !self.caps_lock_on;
            }
            Message::IcedEvent(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(ThemeMode::Dark).with_patina(self.active_patina);

        // ==================== TOP BAR: SESSION, LAYOUT & POWER ====================
        let session_name = self
            .sessions
            .get(self.selected_session_idx)
            .map(|s| s.name.as_str())
            .unwrap_or("Corten Desktop");

        let session_btn = button(
            row![
                text(Icon::LAYERS)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.primary.to_iced()),
                text(session_name)
                    .size(13.0)
                    .color(palette.text_primary.to_iced()),
                text(Icon::ARROW_DOWN)
                    .font(LUCIDE_FONT)
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([7, 14])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => palette.surface_hover.to_iced(),
                _ => iced::Color::from_rgba(
                    palette.surface.r,
                    palette.surface.g,
                    palette.surface.b,
                    0.82,
                ),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::FULL),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::CycleSession);

        let layout_badge = container(
            text(&self.keyboard_layout)
                .size(11.5)
                .color(palette.text_muted.to_iced()),
        )
        .padding([6, 10])
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                palette.surface.r,
                palette.surface.g,
                palette.surface.b,
                0.78,
            ))),
            border: iced::Border {
                radius: iced::border::Radius::from(Radius::FULL),
                ..Default::default()
            },
            ..Default::default()
        });

        let power_controls: Element<'_, Message> =
            if let Some((action, _)) = self.pending_power_confirm {
                container(
                    row![
                        text(action.confirm_prompt())
                            .size(12.5)
                            .color(palette.warning.to_iced()),
                        button(
                            text("Evet")
                                .size(12.0)
                                .color(palette.on_primary().to_iced()),
                        )
                        .padding([5, 11])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => {
                                    iced::Color::from_rgba(0.90, 0.28, 0.22, 0.95)
                                }
                                _ => palette.danger.to_iced(),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::ConfirmPowerAction),
                        button(
                            text("İptal")
                                .size(12.0)
                                .color(palette.text_primary.to_iced()),
                        )
                        .padding([5, 10])
                        .style(move |_, status| {
                            let bg = match status {
                                button::Status::Hovered => palette.surface_hover.to_iced(),
                                _ => palette.surface.to_iced(),
                            };
                            button::Style {
                                background: Some(iced::Background::Color(bg)),
                                border: iced::Border {
                                    radius: iced::border::Radius::from(Radius::FULL),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .on_press(Message::CancelPowerAction),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding([5, 12])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        palette.surface.r,
                        palette.surface.g,
                        palette.surface.b,
                        0.90,
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            } else {
                let power_pill_btn = |icon: char, danger: bool, msg: Message| {
                    button(text(icon).font(LUCIDE_FONT).size(15.0).color(if danger {
                        palette.danger.to_iced()
                    } else {
                        palette.text_muted.to_iced()
                    }))
                    .padding([7, 10])
                    .style(move |_, status| {
                        let bg = match status {
                            button::Status::Hovered if danger => {
                                iced::Color::from_rgba(0.85, 0.25, 0.20, 0.28)
                            }
                            button::Status::Hovered => palette.surface_hover.to_iced(),
                            _ => iced::Color::TRANSPARENT,
                        };
                        button::Style {
                            background: Some(iced::Background::Color(bg)),
                            border: iced::Border {
                                radius: iced::border::Radius::from(Radius::FULL),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .on_press(msg)
                };

                container(
                    row![
                        power_pill_btn(
                            Icon::MOON,
                            false,
                            Message::RequestPowerAction(PowerAction::Suspend),
                        ),
                        power_pill_btn(
                            Icon::REFRESH,
                            false,
                            Message::RequestPowerAction(PowerAction::Reboot),
                        ),
                        power_pill_btn(
                            Icon::POWER,
                            true,
                            Message::RequestPowerAction(PowerAction::Shutdown),
                        ),
                    ]
                    .spacing(2)
                    .align_y(Alignment::Center),
                )
                .padding([2, 4])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        palette.surface.r,
                        palette.surface.g,
                        palette.surface.b,
                        0.82,
                    ))),
                    border: iced::Border {
                        radius: iced::border::Radius::from(Radius::FULL),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
            };

        let top_bar = row![
            session_btn,
            horizontal_space(),
            layout_badge,
            power_controls,
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        let top_bar_container = container(top_bar).width(Length::Fill).padding(Padding {
            top: 20.0,
            right: 28.0,
            bottom: 0.0,
            left: 28.0,
        });

        // ==================== CENTER LOGIN CARD ====================
        let clock = text(&self.current_time)
            .size(76.0)
            .color(palette.text_primary.to_iced());

        let date = text(&self.current_date)
            .size(18.0)
            .color(palette.text_muted.to_iced());

        let time_section = iced::widget::column![clock, date]
            .spacing(4)
            .align_x(Alignment::Center);

        let avatar_initial = self
            .auth
            .username()
            .trim()
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string());

        let avatar_glyph: Element<'_, Message> = if let Some(ch) = avatar_initial {
            text(ch)
                .font(NOTO_SANS_SEMIBOLD)
                .size(32.0)
                .color(palette.on_primary_container().to_iced())
                .into()
        } else {
            text(Icon::USER)
                .font(LUCIDE_FONT)
                .size(34.0)
                .color(palette.on_primary_container().to_iced())
                .into()
        };

        let avatar = container(avatar_glyph)
            .center_x(Length::Fixed(76.0))
            .center_y(Length::Fixed(76.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(
                    palette.primary_container().to_iced(),
                )),
                border: iced::Border {
                    color: iced::Color::TRANSPARENT,
                    width: 0.0,
                    radius: iced::border::Radius::from(Radius::FULL),
                },
                ..Default::default()
            });

        let mut user_row = row![
            text_input("Kullanıcı Adı", self.auth.username())
                .id(self.user_input_id.clone())
                .size(14.5)
                .padding([11, 14])
                .width(Length::Fill)
                .style(move |_, status| {
                    corten_theme::iced_styles::m3_text_input_style(palette, status)
                })
                .on_input(Message::UsernameChanged)
                .on_submit(Message::SubmitLogin),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        if self.human_users.len() > 1 {
            user_row = user_row.push(
                button(
                    text(Icon::USER)
                        .font(LUCIDE_FONT)
                        .size(14.0)
                        .color(palette.text_muted.to_iced()),
                )
                .padding([11, 12])
                .style(move |_, status| {
                    let bg = match status {
                        button::Status::Hovered => palette.surface_hover.to_iced(),
                        _ => palette.surface.to_iced(),
                    };
                    button::Style {
                        background: Some(iced::Background::Color(bg)),
                        border: iced::Border {
                            radius: iced::border::Radius::from(Radius::MD),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
                .on_press(Message::CycleUser),
            );
        }

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
        .padding([11, 12])
        .style(move |_, status| {
            let bg = match status {
                button::Status::Hovered => palette.surface_hover.to_iced(),
                _ => palette.surface.to_iced(),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: iced::border::Radius::from(Radius::MD),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(Message::TogglePasswordReveal);

        let pass_row = row![
            text_input("Parola", self.auth.secret())
                .id(self.pass_input_id.clone())
                .size(14.5)
                .secure(!self.show_password)
                .padding([11, 14])
                .width(Length::Fill)
                .style(move |_, status| {
                    corten_theme::iced_styles::m3_text_input_style(palette, status)
                })
                .on_input(Message::PasswordChanged)
                .on_submit(Message::SubmitLogin),
            reveal_btn,
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let is_pending = self.auth.is_pending();
        let submit_btn = button(
            container(
                text(if is_pending {
                    "Oturum Açılıyor..."
                } else {
                    "Giriş Yap"
                })
                .size(14.5)
                .color(palette.on_primary().to_iced()),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        )
        .width(Length::Fill)
        .padding([11, 20])
        .style(move |_, status| {
            let mut s = corten_theme::iced_styles::primary_button_style(palette, status);
            s.border.radius = iced::border::Radius::from(Radius::FULL);
            s
        })
        .on_press_maybe((!is_pending).then_some(Message::SubmitLogin));

        let inputs_column = iced::widget::column![user_row, pass_row, submit_btn]
            .spacing(10)
            .width(Length::Fixed(340.0));

        let status_label: Element<'_, Message> = if is_pending {
            text("Kimlik doğrulanıyor...")
                .size(13.0)
                .color(palette.text_muted.to_iced())
                .into()
        } else if let Some(ref err) = self.error_message {
            row![
                text(Icon::SHIELD)
                    .font(LUCIDE_FONT)
                    .size(13.0)
                    .color(palette.danger.to_iced()),
                text(err).size(13.0).color(palette.danger.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        } else if self.caps_lock_on {
            row![
                text(Icon::SHIELD)
                    .font(LUCIDE_FONT)
                    .size(12.0)
                    .color(palette.warning.to_iced()),
                text("Caps Lock Açık")
                    .size(12.0)
                    .color(palette.warning.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        } else {
            text(" ")
                .size(13.0)
                .color(palette.text_muted.to_iced())
                .into()
        };

        let card_content = iced::widget::column![avatar, inputs_column, status_label]
            .spacing(16)
            .align_x(Alignment::Center);

        let card = container(card_content)
            .padding([26, 32])
            .style(move |_| acrylic_glass_style(palette, 24.0));

        let center_column = iced::widget::column![time_section, card]
            .spacing(26)
            .align_x(Alignment::Center);

        let center_box = container(center_column)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        let foreground = container(
            iced::widget::column![top_bar_container, center_box]
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                palette.background.r,
                palette.background.g,
                palette.background.b,
                if self.wallpaper_handle.is_some() {
                    0.55
                } else {
                    1.0
                },
            ))),
            ..Default::default()
        });

        if let Some(ref wp) = self.wallpaper_handle {
            let bg_img = image(wp.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(iced::ContentFit::Cover);
            stack![bg_img, foreground].into()
        } else {
            foreground.into()
        }
    }
}

impl Default for CortenGreeterApp {
    fn default() -> Self {
        let now = Local::now();
        let current_time = now.format("%H:%M").to_string();
        let current_date = Self::format_turkish_date(&now);

        let human_users = SessionManager::discover_human_users();
        let (initial_user, initial_home) = if let Some(first) = human_users.first() {
            (first.username.clone(), Some(first.home_dir.as_str()))
        } else {
            let fallback = users::get_current_username()
                .map(|u| u.to_string_lossy().to_string())
                .filter(|u| u != "greeter")
                .unwrap_or_default();
            (fallback, None)
        };

        let sessions = SessionManager::discover_sessions();
        let active_patina = Self::load_patina_for_home(initial_home);
        let wallpaper_handle = Self::resolve_wallpaper().map(image::Handle::from_path);
        let password_focused = !initial_user.is_empty();

        Self {
            auth: AuthController::new(initial_user),
            human_users,
            selected_user_idx: 0,
            current_time,
            current_date,
            sessions,
            selected_session_idx: 0,
            show_password: false,
            caps_lock_on: Self::detect_caps_lock_led(),
            keyboard_layout: Self::detect_keyboard_layout(),
            wallpaper_handle,
            pending_power_confirm: None,
            error_message: None,
            active_patina,
            user_input_id: text_input::Id::new("greeter-username"),
            pass_input_id: text_input::Id::new("greeter-password"),
            password_focused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_greeter_app_state() {
        let app = CortenGreeterApp::default();
        assert!(!app.current_time.is_empty());
        assert!(!app.current_date.is_empty());
        assert!(!app.sessions.is_empty());
        assert_eq!(app.selected_session_idx, 0);
        assert_eq!(app.error_message, None);
        assert_ne!(app.auth.username(), "greeter");
    }

    #[test]
    fn test_session_cycling() {
        let mut app = CortenGreeterApp::default();
        let total = app.sessions.len();
        if total > 1 {
            let initial = app.selected_session_idx;
            let _ = app.update(Message::CycleSession);
            assert_eq!(app.selected_session_idx, (initial + 1) % total);
        }
    }

    #[test]
    fn test_username_and_password_update() {
        let mut app = CortenGreeterApp::default();
        let _ = app.update(Message::UsernameChanged("alice".into()));
        assert_eq!(app.auth.username(), "alice");

        let _ = app.update(Message::PasswordChanged("secret123".into()));
        assert_eq!(app.auth.secret(), "secret123");
    }

    #[test]
    fn test_submit_login_empty_username_errors() {
        let mut app = CortenGreeterApp::default();
        let _ = app.update(Message::UsernameChanged("   ".into()));
        let _ = app.update(Message::SubmitLogin);
        assert!(app.error_message.is_some());
    }

    #[test]
    fn test_failed_login_clears_secret() {
        let mut app = CortenGreeterApp::default();
        let _ = app.update(Message::PasswordChanged("my_secret".into()));
        assert_eq!(app.auth.secret(), "my_secret");

        let _ = app.update(Message::LoginResult(Err(
            "Kimlik doğrulama başarısız".into()
        )));
        assert_eq!(app.auth.secret(), "");
        assert!(app.error_message.is_some());
    }

    #[test]
    fn test_power_confirmation_required_for_reboot_and_shutdown() {
        let mut app = CortenGreeterApp::default();
        let _ = app.update(Message::RequestPowerAction(PowerAction::Shutdown));
        assert!(app.pending_power_confirm.is_some());
        let _ = app.update(Message::CancelPowerAction);
        assert!(app.pending_power_confirm.is_none());
    }
}
