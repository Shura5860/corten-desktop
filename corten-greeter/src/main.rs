mod ipc;
mod power;
mod sessions;

use std::time::Duration;

use chrono::Local;
use corten_auth::AuthController;
use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::{Icon, Palette, PatinaVariant, Radius, ThemeMode, LUCIDE_FONT, LUCIDE_FONT_BYTES};
use iced::widget::{button, container, row, text, text_input};
use iced::{Alignment, Element, Length, Subscription, Task};
use ipc::GreetdClient;
use power::PowerActions;
use sessions::{SessionEntry, SessionManager};
use tracing::info;

pub fn main() -> iced::Result {
    tracing_subscriber::fmt::init();
    info!("Starting corten-greeter (Faz 6: Login Manager / Greeter)...");

    iced::application(
        "Corten Login Screen",
        CortenGreeterApp::update,
        CortenGreeterApp::view,
    )
    .subscription(CortenGreeterApp::subscription)
    .theme(|_| iced::Theme::Dark)
    .font(LUCIDE_FONT_BYTES)
    .window(iced::window::Settings {
        size: iced::Size::new(960.0, 720.0),
        min_size: Some(iced::Size::new(800.0, 600.0)),
        resizable: true,
        decorations: true,
        ..Default::default()
    })
    .antialiasing(true)
    .run()
}

pub struct CortenGreeterApp {
    auth: AuthController,
    current_time: String,
    current_date: String,
    sessions: Vec<SessionEntry>,
    selected_session_idx: usize,
    error_message: Option<String>,
    active_patina: PatinaVariant,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    UsernameChanged(String),
    PasswordChanged(String),
    CycleSession,
    SubmitLogin,
    LoginResult(Result<(), String>),
    Shutdown,
    Reboot,
    Suspend,
}

impl CortenGreeterApp {
    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let now = Local::now();
                let new_time = now.format("%H:%M").to_string();
                if new_time != self.current_time {
                    self.current_time = new_time;
                    self.current_date = now.format("%A, %d %B").to_string();
                }
                if let Ok(c) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    let pat = PatinaVariant::from_code(&c.get().patina);
                    if pat != self.active_patina {
                        self.active_patina = pat;
                    }
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
            Message::CycleSession => {
                if !self.sessions.is_empty() {
                    self.selected_session_idx =
                        (self.selected_session_idx + 1) % self.sessions.len();
                }
            }
            Message::SubmitLogin => {
                if self.auth.username().trim().is_empty() {
                    self.error_message = Some("Lütfen geçerli bir kullanıcı adı girin.".into());
                    return Task::none();
                }

                let Some((user, pass)) = self.auth.begin() else {
                    self.error_message = Some("Lütfen parolanızı girin.".into());
                    return Task::none();
                };

                self.error_message = None;

                let session_cmd = self
                    .sessions
                    .get(self.selected_session_idx)
                    .map(|s| s.exec.clone())
                    .unwrap_or_else(|| vec!["corten-compositor".into()]);

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
                            info!("Running in preview/test mode (GREETD_SOCK unset). Simulating login success for user '{}'", user);
                            Ok(())
                        },
                        Message::LoginResult,
                    );
                }

                return Task::perform(
                    async move { GreetdClient::login_and_start_session(&user, &pass, &session_cmd) },
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
                        self.auth.set_secret(String::new());
                    }
                }
            }
            Message::Shutdown => {
                PowerActions::shutdown();
            }
            Message::Reboot => {
                PowerActions::reboot();
            }
            Message::Suspend => {
                PowerActions::suspend();
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(ThemeMode::Dark).with_patina(self.active_patina);

        // ==================== TOP BAR: POWER ACTIONS ====================
        let shutdown_btn = button(
            text(Icon::POWER)
                .font(LUCIDE_FONT)
                .size(20.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([8, 12])
        .style(move |_, status| button::Style {
            background: match status {
                button::Status::Hovered => Some(iced::Background::Color(iced::Color::from_rgb(
                    0.24, 0.12, 0.12,
                ))),
                _ => Some(iced::Background::Color(iced::Color::from_rgb(
                    0.12, 0.13, 0.16,
                ))),
            },
            border: iced::Border {
                color: iced::Color::from_rgb(0.22, 0.24, 0.30),
                width: 1.0,
                radius: iced::border::Radius::from(8.0),
            },
            ..Default::default()
        })
        .on_press(Message::Shutdown);

        let session_name = self
            .sessions
            .get(self.selected_session_idx)
            .map(|s| s.name.as_str())
            .unwrap_or("Masaüstü");

        let session_btn = button(
            row![
                text(session_name)
                    .size(14.0)
                    .color(palette.text_primary.to_iced()),
                text(" ▾").size(12.0).color(palette.text_muted.to_iced())
            ]
            .align_y(Alignment::Center),
        )
        .padding([8, 14])
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.16, 0.18, 0.22,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgb(0.26, 0.30, 0.38),
                width: 1.0,
                radius: iced::border::Radius::from(10.0),
            },
            ..Default::default()
        })
        .on_press(Message::CycleSession);

        let top_bar = row![session_btn, shutdown_btn]
            .spacing(12)
            .align_y(Alignment::Center);

        let top_bar_container = container(top_bar)
            .width(Length::Fill)
            .padding(24)
            .align_x(Alignment::End);

        // ==================== CENTER LOGIN CARD ====================
        let clock = text(&self.current_time)
            .size(76.0)
            .color(palette.text_primary.to_iced());

        let date = text(&self.current_date)
            .size(20.0)
            .color(palette.text_muted.to_iced());

        let time_section = iced::widget::column![clock, date]
            .spacing(4)
            .align_x(Alignment::Center);

        let avatar = container(
            text(Icon::USER)
                .font(LUCIDE_FONT)
                .size(40.0)
                .color(palette.primary.to_iced()),
        )
        .width(Length::Fixed(88.0))
        .height(Length::Fixed(88.0))
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.22, 0.16, 0.14,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgb(0.70, 0.35, 0.22),
                width: 2.0,
                radius: iced::border::Radius::from(Radius::FULL),
            },
            ..Default::default()
        });

        let user_input = text_input("Kullanıcı Adı", self.auth.username())
            .size(16.0)
            .padding([14, 20])
            .on_input(Message::UsernameChanged)
            .on_submit(Message::SubmitLogin);

        let pass_input = text_input("Parola", self.auth.secret())
            .size(16.0)
            .secure(true)
            .padding([14, 20])
            .on_input(Message::PasswordChanged)
            .on_submit(Message::SubmitLogin);

        let submit_btn = button(
            text("Giriş Yap ↵")
                .size(16.0)
                .color(palette.background.to_iced()),
        )
        .width(Length::Fill)
        .padding([12, 20])
        .style(move |_, _| button::Style {
            background: Some(iced::Background::Color(palette.primary.to_iced())),
            border: iced::Border {
                radius: iced::border::Radius::from(12.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(Message::SubmitLogin);

        let inputs_column = iced::widget::column![user_input, pass_input, submit_btn]
            .spacing(12)
            .width(Length::Fixed(360.0));

        let status_label = if self.auth.is_pending() {
            text("Oturum Açılıyor...")
                .size(14.0)
                .color(palette.text_muted.to_iced())
        } else if let Some(ref err) = self.error_message {
            text(err).size(14.0).color(palette.danger.to_iced())
        } else {
            text(" ").size(14.0).color(palette.surface.to_iced())
        };

        let card_content = iced::widget::column![time_section, avatar, inputs_column, status_label]
            .spacing(18)
            .align_x(Alignment::Center);

        let card = container(card_content)
            .padding([28, 36])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    palette.surface.r,
                    palette.surface.g,
                    palette.surface.b,
                    0.88,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(
                        palette.primary.r,
                        palette.primary.g,
                        palette.primary.b,
                        0.40,
                    ),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::LG),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.45),
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });

        let center_box = container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill);

        let full_ui = iced::widget::column![top_bar_container, center_box]
            .width(Length::Fill)
            .height(Length::Fill);

        container(full_ui)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(palette.background.to_iced())),
                ..Default::default()
            })
            .into()
    }
}

impl Default for CortenGreeterApp {
    fn default() -> Self {
        let now = Local::now();
        let current_time = now.format("%H:%M").to_string();
        let current_date = now.format("%A, %d %B").to_string();
        let current_user = users::get_current_username()
            .map(|u| u.to_string_lossy().to_string())
            .unwrap_or_else(|| "user".into());

        let sessions = SessionManager::discover_sessions();
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        Self {
            auth: AuthController::new(current_user),
            current_time,
            current_date,
            sessions,
            selected_session_idx: 0,
            error_message: None,
            active_patina,
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

        let _ = app.update(Message::LoginResult(Err("Kimlik doğrulama başarısız".into())));
        assert_eq!(app.auth.secret(), "");
        assert!(app.error_message.is_some());
    }
}
