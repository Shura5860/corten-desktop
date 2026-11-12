use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use zbus::interface;
use zbus::zvariant::{OwnedValue, Value};

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::iced_styles::{
    acrylic_glass_style, input_style, primary_button_style, secondary_button_style,
};
use corten_theme::{
    Icon, Palette, PatinaVariant, Spacing, ThemeMode, Typography, LUCIDE_FONT, LUCIDE_FONT_BYTES,
};
use iced::widget::{button, column, container, horizontal_space, row, text, text_input, vertical_space};
use iced::{Alignment, Color, Element, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

#[derive(Debug, Clone)]
pub struct AuthRequest {
    pub action_id: String,
    pub message: String,
    pub cookie: String,
    pub target_user: String,
    pub target_uid: u32,
}

fn get_current_uid() -> u32 {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("Uid:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(uid_str) = parts.get(1) {
                    if let Ok(uid) = uid_str.parse::<u32>() {
                        return uid;
                    }
                }
            }
        }
    }
    1000
}

#[derive(Clone)]
pub struct PolkitAgentShared {
    pub active_request: Arc<Mutex<Option<AuthRequest>>>,
    pub dbus_connection: Arc<Mutex<Option<zbus::Connection>>>,
}

pub struct PolkitDbusAgent {
    shared: PolkitAgentShared,
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl PolkitDbusAgent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<zbus::zvariant::OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        info!("Polkit BeginAuthentication: action='{}', message='{}'", action_id, message);

        let current_uid = get_current_uid();
        let current_user = std::env::var("USER").unwrap_or_else(|_| "root".into());

        let mut target_uid: u32 = current_uid;
        let mut target_user = current_user.clone();

        for id in &identities {
            if let Ok((id_type, props)) = <(String, HashMap<String, OwnedValue>)>::try_from(id.clone()) {
                if id_type == "unix-user" {
                    if let Some(uid_val) = props.get("uid") {
                        if let Ok(uid) = u32::try_from(uid_val.clone()) {
                            target_uid = uid;
                            if uid == current_uid {
                                target_user = current_user.clone();
                                break;
                            } else if uid == 0 {
                                target_user = "root".into();
                            }
                        }
                    }
                }
            }
        }

        let mut guard = self.shared.active_request.lock().await;
        *guard = Some(AuthRequest {
            action_id,
            message,
            cookie,
            target_user,
            target_uid,
        });
        Ok(())
    }

    async fn cancel_authentication(&self, cookie: String) -> zbus::fdo::Result<()> {
        info!("Polkit CancelAuthentication: cookie='{}'", cookie);
        let mut guard = self.shared.active_request.lock().await;
        if guard.as_ref().is_some_and(|r| r.cookie == cookie) {
            *guard = None;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    CheckPending,
    PasswordChanged(String),
    ToggleShowPassword,
    SubmitAuth,
    CancelAuth,
    LayerAction(LayershellCustomActions),
    NoOp,
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

fn secure_wipe(s: &mut String) {
    unsafe {
        let bytes = s.as_bytes_mut();
        for b in bytes {
            std::ptr::write_volatile(b, 0);
        }
    }
    s.clear();
}

fn get_process_start_time() -> u64 {
    if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
        if let Some(idx) = stat.rfind(')') {
            let rest = &stat[idx + 1..];
            let fields: Vec<&str> = rest.split_whitespace().collect();
            // Field 22 (starttime) is at index 19 of fields after comm
            if let Some(st) = fields.get(19) {
                if let Ok(val) = st.parse::<u64>() {
                    return val;
                }
            }
        }
    }
    0
}

pub struct CortenPolkitApp {
    shared: PolkitAgentShared,
    current_request: Option<AuthRequest>,
    password_input: String,
    show_password: bool,
    error_message: Option<String>,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
}

impl Drop for CortenPolkitApp {
    fn drop(&mut self) {
        secure_wipe(&mut self.password_input);
    }
}

impl Application for CortenPolkitApp {
    type Message = Message;
    type Flags = PolkitAgentShared;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(shared: Self::Flags) -> (Self, Task<Self::Message>) {
        let (active_patina, theme_mode) = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let m = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                (p, m)
            })
            .unwrap_or((PatinaVariant::Rust, ThemeMode::Dark));

        (
            Self {
                shared,
                current_request: None,
                password_input: String::new(),
                show_password: false,
                error_message: None,
                theme_mode,
                active_patina,
            },
            iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::CheckPending),
        )
    }

    fn namespace(&self) -> String {
        "corten-polkit".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: iced::Color::WHITE,
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let timer = iced::time::every(std::time::Duration::from_millis(200)).map(|_| Message::CheckPending);
        if self.current_request.is_some() {
            let esc = iced::event::listen_with(|event, _status, _window_id| {
                if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) = event {
                    Some(Message::CancelAuth)
                } else {
                    None
                }
            });
            Subscription::batch(vec![timer, esc])
        } else {
            timer
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::CheckPending => {
                if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                    self.theme_mode = match app_cfg.get().theme_mode {
                        corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                        corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                    };
                }

                if let Ok(guard) = self.shared.active_request.try_lock() {
                    if let Some(req) = guard.as_ref() {
                        if self.current_request.as_ref().map(|r| &r.cookie) != Some(&req.cookie) {
                            self.current_request = Some(req.clone());
                            secure_wipe(&mut self.password_input);
                            self.error_message = None;

                            // Request appeared: activate fullscreen overlay on top of all windows
                            return Task::batch(vec![
                                Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                                    Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
                                ))),
                                Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((0, 0)))),
                                text_input::focus(text_input::Id::new("polkit_password_input")),
                            ]);
                        }
                    } else if self.current_request.is_some() {
                        self.current_request = None;
                        secure_wipe(&mut self.password_input);
                        self.error_message = None;

                        // Request cleared: return to idle 1x1 footprint
                        return Task::batch(vec![
                            Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                                Anchor::empty(),
                            ))),
                            Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((1, 1)))),
                        ]);
                    }
                }
            }
            Message::PasswordChanged(pass) => {
                self.password_input = pass;
                self.error_message = None;
            }
            Message::ToggleShowPassword => {
                self.show_password = !self.show_password;
            }
            Message::SubmitAuth => {
                if let Some(req) = self.current_request.clone() {
                    let user = req.target_user.clone();
                    let pass = self.password_input.clone();
                    let cookie = req.cookie.clone();
                    let uid = req.target_uid;

                    let authenticated = match pam::Client::with_password("polkit-1") {
                        Ok(mut client) => {
                            client.conversation_mut().set_credentials(&user, &pass);
                            client.authenticate().is_ok()
                        }
                        Err(_) => {
                            if let Ok(mut client) = pam::Client::with_password("login") {
                                client.conversation_mut().set_credentials(&user, &pass);
                                client.authenticate().is_ok()
                            } else {
                                false
                            }
                        }
                    };

                    if authenticated {
                        info!("Polkit authentication successful for user '{}' (UID {})", user, uid);
                        if let Ok(mut guard) = self.shared.active_request.try_lock() {
                            *guard = None;
                        }
                        self.current_request = None;
                        secure_wipe(&mut self.password_input);

                        // Notify PolicyKit Authority via AuthenticationAgentResponse
                        let dbus_conn = self.shared.dbus_connection.clone();
                        std::thread::spawn(move || {
                            futures_lite::future::block_on(async move {
                                let guard = dbus_conn.lock().await;
                                if let Some(conn) = guard.as_ref() {
                                    if let Ok(proxy) = zbus::Proxy::new(
                                        conn,
                                        "org.freedesktop.PolicyKit1",
                                        "/org/freedesktop/PolicyKit1/Authority",
                                        "org.freedesktop.PolicyKit1.Authority",
                                    ).await {
                                        let mut id_map: HashMap<&str, Value> = HashMap::new();
                                        id_map.insert("uid", Value::U32(uid));
                                        let identity = ("unix-user", id_map);
                                        let res: zbus::Result<()> = proxy.call(
                                            "AuthenticationAgentResponse",
                                            &(cookie.as_str(), identity),
                                        ).await;
                                        info!("PolicyKit Authority AuthenticationAgentResponse result: {:?}", res);
                                    }
                                }
                            });
                        });

                        return Task::batch(vec![
                            Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                                Anchor::empty(),
                            ))),
                            Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((1, 1)))),
                        ]);
                    } else {
                        warn!("Polkit authentication failed for user '{}'", user);
                        self.error_message = Some("Hatalı parola, lütfen tekrar deneyin".into());
                        secure_wipe(&mut self.password_input);
                    }
                }
            }
            Message::CancelAuth => {
                if let Ok(mut guard) = self.shared.active_request.try_lock() {
                    *guard = None;
                }
                self.current_request = None;
                secure_wipe(&mut self.password_input);
                self.error_message = None;

                return Task::batch(vec![
                    Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                        Anchor::empty(),
                    ))),
                    Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((1, 1)))),
                ]);
            }
            Message::LayerAction(_) | Message::NoOp => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        let Some(req) = &self.current_request else {
            return container(horizontal_space())
                .width(Length::Fixed(1.0))
                .height(Length::Fixed(1.0))
                .into();
        };

        let title = if req.message.is_empty() {
            "Sistem Kimlik Doğrulaması Gerekli"
        } else {
            &req.message
        };

        let card = container(
            column![
                row![
                    text(Icon::LOCK)
                        .font(LUCIDE_FONT)
                        .size(22)
                        .color(palette.primary.to_iced()),
                    column![
                        text(title)
                            .size(Typography::TITLE)
                            .color(palette.text_primary.to_iced()),
                        text(format!("Eylem: {} • Kullanıcı: {}", req.action_id, req.target_user))
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(2),
                ]
                .spacing(Spacing::MD)
                .align_y(Alignment::Center),
                vertical_space().height(Spacing::XS),
                container(
                    row![
                        text_input("Yönetici parolanızı girin...", &self.password_input)
                            .id(text_input::Id::new("polkit_password_input"))
                            .secure(!self.show_password)
                            .padding([Spacing::SM, Spacing::MD])
                            .on_input(Message::PasswordChanged)
                            .on_submit(Message::SubmitAuth)
                            .width(Length::Fill)
                            .style(move |_, _| text_input::Style {
                                background: iced::Background::Color(iced::Color::TRANSPARENT),
                                border: iced::Border::default(),
                                icon: palette.primary.to_iced(),
                                placeholder: palette.text_muted.to_iced(),
                                value: palette.text_primary.to_iced(),
                                selection: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.4),
                            }),
                        button(
                            text(if self.show_password { "👁 Gizle" } else { "👁 Göster" })
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        )
                        .padding([Spacing::XS, Spacing::SM])
                        .style(|_, _| button::Style::default())
                        .on_press(Message::ToggleShowPassword),
                    ]
                    .align_y(Alignment::Center)
                )
                .style(move |_| input_style(palette)),
                if let Some(ref err) = self.error_message {
                    text(err).size(Typography::CAPTION).color(palette.danger.to_iced())
                } else {
                    text("Devam etmek için oturum açma parolanızı onaylayın")
                        .size(Typography::CAPTION)
                        .color(palette.text_muted.to_iced())
                },
                vertical_space().height(Spacing::SM),
                row![
                    button(text("İptal").size(Typography::BODY_SM))
                        .padding([Spacing::SM, Spacing::LG])
                        .style(move |_, status| secondary_button_style(palette, status))
                        .on_press(Message::CancelAuth),
                    horizontal_space(),
                    button(text("Kimliği Doğrula").size(Typography::BODY_SM).color(Color::WHITE))
                        .padding([Spacing::SM, Spacing::LG])
                        .style(move |_, status| primary_button_style(palette, status))
                        .on_press(Message::SubmitAuth),
                ]
                .width(Length::Fill)
                .align_y(Alignment::Center),
            ]
            .spacing(Spacing::SM)
            .width(Length::Fixed(460.0)),
        )
        .padding(Spacing::XL)
        .style(move |_| {
            let mut s = acrylic_glass_style(palette, 16.0);
            s.border.color = iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.50);
            s.border.width = 1.5;
            s
        });

        container(card)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.65))),
                ..Default::default()
            })
            .into()
    }
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting corten-polkit (PolicyKit Authentication Agent with Layer-Shell)...");

    let shared = PolkitAgentShared {
        active_request: Arc::new(Mutex::new(None)),
        dbus_connection: Arc::new(Mutex::new(None)),
    };

    let dbus_shared = shared.clone();
    // Register Polkit D-Bus agent in dedicated background thread with its own Tokio runtime
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                error!("Polkit: Failed to build tokio runtime: {}", e);
                return;
            }
        };

        rt.block_on(async move {
            let agent = PolkitDbusAgent { shared: dbus_shared.clone() };
            if let Ok(connection) = zbus::connection::Builder::system() {
                if let Ok(conn) = connection
                    .serve_at("/org/freedesktop/PolicyKit1/AuthenticationAgent", agent)
                {
                    if let Ok(built_conn) = conn.build().await {
                        {
                            let mut conn_guard = dbus_shared.dbus_connection.lock().await;
                            *conn_guard = Some(built_conn.clone());
                        }
                        let locale = std::env::var("LANG").unwrap_or_else(|_| "tr_TR.UTF-8".into());
                        // Register with Authority using standard PolicyKit-1 Subject: (unix-process, {pid, start-time})
                        if let Ok(proxy) = zbus::Proxy::new(
                            &built_conn,
                            "org.freedesktop.PolicyKit1",
                            "/org/freedesktop/PolicyKit1/Authority",
                            "org.freedesktop.PolicyKit1.Authority",
                        ).await {
                            let mut details: std::collections::HashMap<&str, zbus::zvariant::Value> = std::collections::HashMap::new();
                            let pid = std::process::id();
                            let start_time = get_process_start_time();
                            details.insert("pid", zbus::zvariant::Value::U32(pid));
                            details.insert("start-time", zbus::zvariant::Value::U64(start_time));
                            let subject = ("unix-process", details);
                            let object_path = "/org/freedesktop/PolicyKit1/AuthenticationAgent";
                            let res: zbus::Result<()> = proxy.call(
                                "RegisterAuthenticationAgent",
                                &(subject, locale.as_str(), object_path),
                            ).await;
                            match res {
                                Ok(()) => info!("Successfully registered with PolicyKit-1 Authority!"),
                                Err(e) => warn!("Polkit Authority RegisterAuthenticationAgent: {:?}", e),
                            }
                        }
                        futures_lite::future::pending::<()>().await;
                    }
                }
            }
        });
    });

    CortenPolkitApp::run(Settings {
        id: Some("corten-polkit".into()),
        flags: shared,
        fonts: vec![
            Cow::Borrowed(LUCIDE_FONT_BYTES),
        ],
        layer_settings: LayerShellSettings {
            anchor: Anchor::empty(),
            layer: Layer::Overlay,
            margin: (0, 0, 0, 0),
            exclusive_zone: -1,
            size: Some((1, 1)),
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            binded_output_name: None,
        },
        default_font: iced::Font::default(),
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_request_creation() {
        let req = AuthRequest {
            action_id: "org.corten.test".into(),
            message: "Test Auth".into(),
            cookie: "test_cookie_123".into(),
            target_user: "shura".into(),
            target_uid: 1000,
        };
        assert_eq!(req.action_id, "org.corten.test");
        assert_eq!(req.cookie, "test_cookie_123");
        assert_eq!(req.target_user, "shura");
        assert_eq!(req.target_uid, 1000);
    }
}
