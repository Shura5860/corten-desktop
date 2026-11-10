use std::borrow::Cow;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tracing::{info, warn};
use zbus::connection::Builder;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_theme::{
    Icon, Palette, PatinaVariant, Spacing, ThemeMode, Transition, Typography, LUCIDE_FONT, LUCIDE_FONT_BYTES,
};
use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

mod config;
mod dbus;

use config::{NotifyConfig, Position};
use dbus::{NotificationItem, NotifyEvent, Urgency};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting Corten Notification Daemon...");

    let args: Vec<String> = std::env::args().collect();
    let is_test_mode = args.iter().any(|arg| arg == "--test" || arg == "-t");

    let config_manager = ConfigManager::<NotifyConfig>::load_or_default("notify")?;
    let initial_config = config_manager.get().clone();

    let (event_tx, event_rx) = unbounded_channel();
    let dnd_enabled = Arc::new(AtomicBool::new(initial_config.dnd));

    // If test mode is enabled, immediately inject a sample notification for visual verification
    if is_test_mode {
        let sample_tx = event_tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            let _ = sample_tx.send(NotifyEvent::Add(NotificationItem {
                id: 1001,
                app_name: "Corten System".into(),
                summary: "Corten Notification Service".into(),
                body: "Notification card running smoothly on Wayland with zero desktop blockage."
                    .into(),
                icon: "corten".into(),
                urgency: Urgency::Normal,
                actions: vec![],
                timeout_ms: 10000,
                sync_tag: None,
                progress: None,
            }));
        });
    }

    // Spawn D-Bus org.freedesktop.Notifications service in a background tokio thread
    let dbus_tx = event_tx.clone();
    let dbus_dnd = Arc::clone(&dnd_enabled);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async move {
            let server = dbus::NotificationServer::new(dbus_tx.clone(), dbus_dnd);
            if let Ok(builder) = Builder::session() {
                let res = builder
                    .name("org.freedesktop.Notifications")
                    .map_err(|e| e.to_string())
                    .and_then(|b| b.serve_at("/org/freedesktop/Notifications", server).map_err(|e| e.to_string()));

                match res {
                    Ok(configured_builder) => {
                        match configured_builder.build().await {
                            Ok(conn) => {
                                info!("D-Bus org.freedesktop.Notifications service registered successfully");
                                dbus::set_dbus_connection(conn);
                                std::future::pending::<()>().await;
                            }
                            Err(e) => {
                                warn!("D-Bus org.freedesktop.Notifications already owned ({}), using fallback.", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Could not configure org.freedesktop.Notifications ({}), using fallback.", e);
                    }
                }

                let fallback_server = dbus::NotificationServer::new(dbus_tx, Arc::new(AtomicBool::new(false)));
                if let Ok(fb) = Builder::session() {
                    if let Ok(fb) = fb.serve_at("/org/corten/Notification", fallback_server) {
                        if let Ok(conn) = fb.build().await {
                            dbus::set_dbus_connection(conn);
                        }
                    }
                }
                std::future::pending::<()>().await;
            }
        });
    });

    let config_watcher = if let Ok((w, config_rx)) = config_manager.spawn_watcher() {
        let watcher_tx = event_tx.clone();
        std::thread::spawn(move || {
            while config_rx.recv().is_ok() {
                let _ = watcher_tx.send(NotifyEvent::ReloadConfig);
            }
        });
        Some(w)
    } else {
        None
    };

    let app_watcher = if let Ok(app_mgr) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
        if let Ok((w, app_rx)) = app_mgr.spawn_watcher() {
            let app_tx = event_tx.clone();
            std::thread::spawn(move || {
                while app_rx.recv().is_ok() {
                    let _ = app_tx.send(NotifyEvent::ReloadConfig);
                }
            });
            Some(w)
        } else {
            None
        }
    } else {
        None
    };

    let anchor = match initial_config.position {
        Position::TopRight => Anchor::Top | Anchor::Right,
        Position::TopLeft => Anchor::Top | Anchor::Left,
        Position::BottomRight => Anchor::Bottom | Anchor::Right,
        Position::BottomLeft => Anchor::Bottom | Anchor::Left,
    };

    let flags = NotificationAppFlags {
        config_manager,
        event_rx: Some(event_rx),
        dnd_enabled,
        config_watcher,
        app_watcher,
    };

    // 1x1 initial size complies with wlr-layer-shell protocol without blocking screen clicks
    let layer_size = Some((1, 1));

    CortenNotificationApp::run(Settings {
        id: Some("corten-notify".into()),
        flags,
        fonts: vec![Cow::Borrowed(LUCIDE_FONT_BYTES)],
        layer_settings: LayerShellSettings {
            anchor,
            layer: Layer::Top,
            margin: (
                initial_config.margin as i32,
                initial_config.margin as i32,
                initial_config.margin as i32,
                initial_config.margin as i32,
            ),
            exclusive_zone: 0,
            size: layer_size,
            keyboard_interactivity: KeyboardInteractivity::None,
            binded_output_name: None,
        },
        default_font: iced::Font::default(),
        default_text_size: iced::Pixels(16.0),
        antialiasing: false,
        virtual_keyboard_support: None,
    })?;

    Ok(())
}

pub struct NotificationAppFlags {
    pub config_manager: ConfigManager<NotifyConfig>,
    pub event_rx: Option<UnboundedReceiver<NotifyEvent>>,
    pub dnd_enabled: Arc<AtomicBool>,
    pub config_watcher: Option<notify::RecommendedWatcher>,
    pub app_watcher: Option<notify::RecommendedWatcher>,
}

#[derive(Debug, Clone)]
pub struct VisibleNotification {
    pub item: NotificationItem,
    pub remaining_ms: u64,
    pub initial_ms: u64,
    pub entrance: Transition,
}

fn tick_visible_notification(notification: &mut VisibleNotification) -> bool {
    if notification.item.urgency == Urgency::Critical {
        true
    } else if notification.remaining_ms <= 200 {
        false
    } else {
        notification.remaining_ms -= 200;
        true
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    DbusEvent(NotifyEvent),
    Dismiss(u32),
    DismissApp(String),
    DismissHistory(u32),
    ClearHistory,
    ToggleHistoryDrawer,
    ActionInvoked(u32, String),
    Tick,
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

pub struct CortenNotificationApp {
    config_manager: ConfigManager<NotifyConfig>,
    config: NotifyConfig,
    _config_watcher: Option<notify::RecommendedWatcher>,
    _app_watcher: Option<notify::RecommendedWatcher>,
    _dnd_enabled: Arc<AtomicBool>,
    event_rx: Option<Arc<tokio::sync::Mutex<UnboundedReceiver<NotifyEvent>>>>,
    notifications: Vec<VisibleNotification>,
    notification_history: Vec<NotificationItem>,
    show_history_drawer: bool,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
}

fn resolve_notification_icon(icon: &str, app_name: &str, summary: &str) -> char {
    let icon_lower = icon.to_lowercase();
    if !icon_lower.is_empty() {
        if icon_lower.contains("term") || icon_lower.contains("alacritty") || icon_lower.contains("kitty") || icon_lower.contains("foot") || icon_lower.contains("konsole") || icon_lower.contains("bash") {
            return Icon::TERMINAL;
        }
        if icon_lower.contains("music") || icon_lower.contains("spotify") || icon_lower.contains("audacious") || icon_lower.contains("vlc") || icon_lower.contains("mpv") {
            return Icon::MUSIC;
        }
        if icon_lower.contains("wifi") || icon_lower.contains("network") {
            return Icon::WIFI;
        }
        if icon_lower.contains("battery") || icon_lower.contains("power") {
            return Icon::BATTERY;
        }
        if icon_lower.contains("code") || icon_lower.contains("git") || icon_lower.contains("antigravity") || icon_lower.contains("rust") {
            return Icon::CODE;
        }
        if icon_lower.contains("folder") || icon_lower.contains("file") || icon_lower.contains("thunar") || icon_lower.contains("dolphin") || icon_lower.contains("nautilus") {
            return Icon::FOLDER;
        }
        if icon_lower.contains("download") {
            return Icon::DOWNLOAD;
        }
        if icon_lower.contains("web") || icon_lower.contains("browser") || icon_lower.contains("firefox") || icon_lower.contains("chrome") || icon_lower.contains("brave") || icon_lower.contains("zen") {
            return Icon::GLOBE;
        }
        if icon_lower.contains("mail") || icon_lower.contains("thunderbird") || icon_lower.contains("email") {
            return Icon::BELL;
        }
        if icon_lower.contains("setting") || icon_lower.contains("preference") || icon_lower.contains("control") {
            return Icon::SETTINGS;
        }
        if icon_lower.contains("warn") || icon_lower.contains("alert") || icon_lower.contains("shield") || icon_lower.contains("security") {
            return Icon::SHIELD;
        }
        if icon_lower.contains("bell") {
            return Icon::BELL;
        }
    }

    let name_lower = app_name.to_lowercase();
    let summary_lower = summary.to_lowercase();
    if name_lower.contains("term") || name_lower.contains("bash") || name_lower.contains("alacritty") || summary_lower.contains("terminal") {
        Icon::TERMINAL
    } else if name_lower.contains("music") || name_lower.contains("spotify") || summary_lower.contains("track") || summary_lower.contains("music") {
        Icon::MUSIC
    } else if name_lower.contains("wifi") || name_lower.contains("network") || summary_lower.contains("connection") || summary_lower.contains("wifi") {
        Icon::WIFI
    } else if name_lower.contains("battery") || summary_lower.contains("battery") || summary_lower.contains("charge") {
        Icon::BATTERY
    } else if name_lower.contains("code") || name_lower.contains("git") || name_lower.contains("build") || summary_lower.contains("compile") {
        Icon::CODE
    } else if name_lower.contains("download") || summary_lower.contains("download") {
        Icon::DOWNLOAD
    } else if name_lower.contains("file") || summary_lower.contains("file") {
        Icon::FOLDER
    } else if name_lower.contains("web") || name_lower.contains("browser") || name_lower.contains("firefox") || summary_lower.contains("tab") {
        Icon::GLOBE
    } else if name_lower.contains("mail") || summary_lower.contains("mail") || summary_lower.contains("email") {
        Icon::BELL
    } else if name_lower.contains("setting") || summary_lower.contains("setting") || summary_lower.contains("config") {
        Icon::SETTINGS
    } else {
        Icon::APP_WINDOW
    }
}

impl Application for CortenNotificationApp {
    type Message = Message;
    type Flags = NotificationAppFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(mut flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = flags.config_manager.get().clone();
        let event_rx = flags
            .event_rx
            .take()
            .map(|rx| Arc::new(tokio::sync::Mutex::new(rx)));

        let load_font_task = iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::Tick);
        let active_patina = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| PatinaVariant::from_code(&c.get().patina))
            .unwrap_or(PatinaVariant::Rust);

        (
            Self {
                config_manager: flags.config_manager,
                config,
                _config_watcher: flags.config_watcher,
                _app_watcher: flags.app_watcher,
                _dnd_enabled: flags.dnd_enabled,
                event_rx,
                notifications: Vec::new(),
                notification_history: Vec::new(),
                show_history_drawer: false,
                theme_mode: ThemeMode::Dark,
                active_patina,
            },
            load_font_task,
        )
    }

    fn namespace(&self) -> String {
        "corten-notification".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: iced::Color::WHITE,
        }
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let timer = if !self.notifications.is_empty() {
            iced::time::every(Duration::from_millis(200)).map(|_| Message::Tick)
        } else {
            Subscription::none()
        };

        if let Some(rx_arc) = &self.event_rx {
            let rx_clone = Arc::clone(rx_arc);
            let stream = futures::stream::unfold(rx_clone, |rx| async move {
                let mut lock = rx.lock().await;
                let event = lock.recv().await?;
                drop(lock);
                Some((Message::DbusEvent(event), rx))
            });
            let dbus_sub = Subscription::run_with_id("corten-dbus-receiver", stream);
            Subscription::batch(vec![timer, dbus_sub])
        } else {
            timer
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::DbusEvent(event) => match event {
                NotifyEvent::Add(item) => {
                    let timeout = if item.timeout_ms > 0 {
                        item.timeout_ms as u64
                    } else {
                        self.config.default_timeout_ms
                    };
                    // Replace in-place if an existing notification shares the same id or synchronous OSD tag
                    if let Some(existing) = self.notifications.iter_mut().find(|n| {
                        n.item.id == item.id
                            || (item.sync_tag.is_some() && n.item.sync_tag == item.sync_tag)
                    }) {
                        existing.item = item.clone();
                        existing.remaining_ms = timeout;
                        existing.initial_ms = timeout.max(1);
                    } else {
                        self.notifications.push(VisibleNotification {
                            item: item.clone(),
                            remaining_ms: timeout,
                            initial_ms: timeout.max(1),
                            entrance: Transition::new(140),
                        });
                        if self.notifications.len() > self.config.max_visible {
                            self.notifications.remove(0);
                        }
                    }

                    // Keep in recent history ring-buffer (skip transient OSD items)
                    if item.sync_tag.is_none() {
                        self.notification_history.retain(|h| h.id != item.id);
                        self.notification_history.push(item);
                        if self.notification_history.len() > 20 {
                            self.notification_history.remove(0);
                        }
                    }
                }
                NotifyEvent::Close(id) => {
                    let existed = self.notifications.iter().any(|n| n.item.id == id);
                    self.notifications.retain(|n| n.item.id != id);
                    if existed {
                        dbus::emit_notification_closed(id, 3);
                    }
                }
                NotifyEvent::ClearAll => {
                    for n in &self.notifications {
                        dbus::emit_notification_closed(n.item.id, 2);
                    }
                    self.notifications.clear();
                }
                NotifyEvent::ToggleHistory => {
                    self.show_history_drawer = !self.show_history_drawer;
                    info!(
                        "Notification: ToggleHistory drawer: {}",
                        self.show_history_drawer
                    );
                }
                NotifyEvent::ClearHistory => {
                    self.notification_history.clear();
                    info!("Notification: ClearHistory executed");
                }
                NotifyEvent::ReloadConfig => {
                    info!("Notification: ReloadConfig received, reading from ~/.config/corten/notify.toml");
                    if let Ok(new_cfg) = ConfigManager::<NotifyConfig>::load_or_default("notify") {
                        self.config = new_cfg.get().clone();
                        self._dnd_enabled.store(self.config.dnd, std::sync::atomic::Ordering::SeqCst);
                        info!("Notification config reloaded: {:?}", self.config);
                    }
                    if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                        self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                    }
                }
                NotifyEvent::SetDnd(enabled) => {
                    self.config.dnd = enabled;
                    self._dnd_enabled.store(enabled, std::sync::atomic::Ordering::SeqCst);
                    let _ = self.config_manager.update(self.config.clone());
                    info!("Notification: DND set to {}", enabled);
                }
            },
            Message::Dismiss(id) => {
                self.notifications.retain(|n| n.item.id != id);
                dbus::emit_notification_closed(id, 2);
            }
            Message::DismissApp(app) => {
                let dismissed: Vec<u32> = self.notifications.iter()
                    .filter(|n| n.item.app_name == app)
                    .map(|n| n.item.id)
                    .collect();
                self.notifications.retain(|n| n.item.app_name != app);
                for id in dismissed {
                    dbus::emit_notification_closed(id, 2);
                }
            }
            Message::DismissHistory(id) => {
                self.notification_history.retain(|n| n.id != id);
            }
            Message::ClearHistory => {
                self.notification_history.clear();
            }
            Message::ToggleHistoryDrawer => {
                self.show_history_drawer = !self.show_history_drawer;
            }
            Message::ActionInvoked(id, action_key) => {
                info!("Notification {} action invoked: {}", id, action_key);
                self.notifications.retain(|n| n.item.id != id);
                dbus::emit_action_invoked(id, &action_key);
                dbus::emit_notification_closed(id, 2);
            }
            Message::Tick => {
                for notification in &mut self.notifications {
                    notification.entrance.tick(200);
                }
                let mut expired_ids = Vec::new();
                self.notifications.retain_mut(|n| {
                    let keep = tick_visible_notification(n);
                    if !keep {
                        expired_ids.push(n.item.id);
                    }
                    keep
                });
                for id in expired_ids {
                    dbus::emit_notification_closed(id, 1);
                }
            }
            Message::LayerAction(_) => {}
        }

        // Dynamically compute exact layer size to comply with wlr-layer-shell protocol (min 1x1)
        let (width, height) = if self.show_history_drawer {
            // Expanded Notification History Drawer (Right sidebar / drawer)
            (self.config.card_width + 48, 620)
        } else if self.notifications.is_empty() {
            (1, 1)
        } else {
            let mut total_height = 24u32;
            for notif in &self.notifications {
                let mut card_h = 75u32;
                if !notif.item.body.is_empty() {
                    card_h += if notif.item.body.len() > 70 { 48 } else { 32 };
                }
                if !notif.item.actions.is_empty() {
                    card_h += 34;
                }
                if notif.item.progress.is_some() || notif.item.urgency != Urgency::Critical {
                    card_h += 8;
                }
                total_height += card_h + 10;
            }
            (self.config.card_width + 24, total_height.min(900))
        };

        Task::done(Message::LayerAction(LayershellCustomActions::SizeChange((
            width, height,
        ))))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        if self.show_history_drawer {
            let hist_count = self.notification_history.len();
            let count_badge = if hist_count > 0 {
                container(
                    text(format!("{}", hist_count))
                        .size(10.0)
                        .color(palette.primary.to_iced()),
                )
                .padding([1, 6])
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        palette.primary.r, palette.primary.g, palette.primary.b, 0.20,
                    ))),
                    border: iced::Border {
                        color: iced::Color::from_rgba(
                            palette.primary.r, palette.primary.g, palette.primary.b, 0.45,
                        ),
                        width: 1.0,
                        radius: iced::border::Radius::from(corten_theme::Radius::FULL),
                    },
                    ..Default::default()
                })
            } else {
                container(row![])
            };

            let header = row![
                text(Icon::BELL)
                    .font(LUCIDE_FONT)
                    .size(16.0)
                    .color(palette.primary.to_iced()),
                text("Bildirim Merkezi")
                    .size(Typography::TITLE_SM)
                    .color(palette.text_primary.to_iced()),
                count_badge,
                iced::widget::horizontal_space(),
                button(
                    text("Tümünü Temizle")
                        .size(11.0)
                        .color(palette.danger.to_iced())
                )
                .padding([2, 6])
                .style(|_, _| button::Style::default())
                .on_press(Message::ClearHistory),
                button(text("✕").size(12.0).color(palette.text_muted.to_iced()))
                    .padding([2, 6])
                    .style(|_, _| button::Style::default())
                    .on_press(Message::ToggleHistoryDrawer),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill);

            let mut history_list = column![].spacing(Spacing::XS).width(Length::Fill);

            if self.notification_history.is_empty() {
                history_list = history_list.push(
                    container(
                        text("Geçmiş bildirim yok")
                            .size(Typography::BODY_SM)
                            .color(palette.text_muted.to_iced()),
                    )
                    .padding(24)
                    .center_x(Length::Fill),
                );
            } else {
                for item in self.notification_history.iter().rev().take(8) {
                    let id = item.id;
                    let app_hist_count = self
                        .notification_history
                        .iter()
                        .filter(|h| h.app_name == item.app_name)
                        .count();

                    let mut app_header = row![
                        text(&item.app_name)
                            .size(Typography::CAPTION)
                            .color(palette.primary.to_iced()),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center);

                    if app_hist_count > 1 {
                        app_header = app_header.push(
                            container(
                                text(format!("{}", app_hist_count))
                                    .size(9.0)
                                    .color(palette.primary.to_iced()),
                            )
                            .padding([1, 5])
                            .style(move |_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgba(
                                    palette.primary.r, palette.primary.g, palette.primary.b, 0.16,
                                ))),
                                border: iced::Border {
                                    color: iced::Color::from_rgba(
                                        palette.primary.r, palette.primary.g, palette.primary.b, 0.35,
                                    ),
                                    width: 1.0,
                                    radius: iced::border::Radius::from(corten_theme::Radius::FULL),
                                },
                                ..Default::default()
                            }),
                        );
                    }

                    let item_row = container(
                        column![
                            row![
                                app_header,
                                iced::widget::horizontal_space(),
                                button(text("✕").size(10.0).color(palette.text_muted.to_iced()))
                                    .padding([1, 4])
                                    .style(|_, _| button::Style::default())
                                    .on_press(Message::DismissHistory(id)),
                            ]
                            .width(Length::Fill)
                            .align_y(Alignment::Center),
                            text(&item.summary)
                                .size(Typography::BODY_SM)
                                .color(palette.text_primary.to_iced()),
                            text(&item.body)
                                .size(Typography::CAPTION)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .spacing(2),
                    )
                    .padding(8)
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            1.0, 1.0, 1.0, 0.04,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            width: 1.0,
                            radius: iced::border::Radius::from(8.0),
                        },
                        ..Default::default()
                    });

                    history_list = history_list.push(item_row);
                }
            }

            let drawer_card = container(
                column![header, history_list]
                    .spacing(Spacing::MD)
                    .width(Length::Fixed(self.config.card_width as f32 + 20.0)),
            )
            .padding(Spacing::MD)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.13, 0.96,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.15),
                    width: 1.0,
                    radius: iced::border::Radius::from(corten_theme::Radius::LG),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.50),
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });

            return container(drawer_card).padding(Spacing::SM).into();
        }

        if self.notifications.is_empty() {
            return container(column![])
                .width(Length::Shrink)
                .height(Length::Shrink)
                .into();
        }

        let mut cards = column![].spacing(Spacing::SM).align_x(Alignment::End);

        for notif in &self.notifications {
            let item = &notif.item;

            let border_color = match item.urgency {
                Urgency::Critical => palette.danger.to_iced(),
                Urgency::Normal => palette.border.to_iced(),
                Urgency::Low => palette.border.to_iced(),
            };

            let accent_icon = match item.urgency {
                Urgency::Critical => Icon::BELL,
                _ => resolve_notification_icon(&item.icon, &item.app_name, &item.summary),
            };

            // Calculate progress ratio: use explicit OSD value (0..=100) if present, otherwise countdown timer
            let is_osd_bar = item.progress.is_some();
            let progress_ratio = if let Some(pct) = item.progress {
                (pct as f32 / 100.0).clamp(0.0, 1.0)
            } else if item.urgency == Urgency::Critical {
                1.0f32
            } else {
                (notif.remaining_ms as f32 / notif.initial_ms as f32).clamp(0.0, 1.0)
            };

            let bar_height = if is_osd_bar { 6.0 } else { 2.0 };
            let card_inner_width = (self.config.card_width as f32 - 32.0).max(1.0);
            let bar_fill_width = (card_inner_width * progress_ratio).max(1.0);

            let progress_bar = container(
                container(row![])
                    .width(Length::Fixed(bar_fill_width))
                    .height(Length::Fixed(bar_height))
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(
                            if item.urgency == Urgency::Critical {
                                palette.danger.to_iced()
                            } else {
                                palette.primary.to_iced()
                            },
                        )),
                        border: iced::Border {
                            radius: iced::border::Radius::from(corten_theme::Radius::FULL),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
            )
            .width(Length::Fixed(card_inner_width))
            .height(Length::Fixed(bar_height))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(palette.surface_hover.to_iced())),
                border: iced::Border {
                    radius: iced::border::Radius::from(corten_theme::Radius::FULL),
                    ..Default::default()
                },
                ..Default::default()
            });

            let elapsed_sec = (notif.initial_ms.saturating_sub(notif.remaining_ms)) / 1000;
            let time_tag = if elapsed_sec == 0 {
                "now".to_string()
            } else {
                format!("{}s ago", elapsed_sec)
            };

            let app_count = self
                .notifications
                .iter()
                .filter(|n| n.item.app_name == item.app_name)
                .count();

            let app_badge: Element<'_, Self::Message> = if app_count > 1 {
                let app_name_clone = item.app_name.clone();
                button(
                    container(
                        text(format!("+{}", app_count - 1))
                            .size(9.0)
                            .color(palette.primary.to_iced()),
                    )
                    .padding([1, 5])
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(
                            palette.primary.r, palette.primary.g, palette.primary.b, 0.20,
                        ))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(
                                palette.primary.r, palette.primary.g, palette.primary.b, 0.45,
                            ),
                            width: 1.0,
                            radius: iced::border::Radius::from(corten_theme::Radius::FULL),
                        },
                        ..Default::default()
                    }),
                )
                .padding(0)
                .style(|_, _| button::Style::default())
                .on_press(Message::DismissApp(app_name_clone))
                .into()
            } else {
                row![].into()
            };

            let mut card_content = column![
                row![
                    row![
                        text(accent_icon)
                            .font(LUCIDE_FONT)
                            .size(Typography::BODY_SM)
                            .color(if item.urgency == Urgency::Critical {
                                palette.danger.to_iced()
                            } else {
                                palette.primary.to_iced()
                            }),
                        text(&item.app_name)
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                        app_badge,
                        text("•")
                            .size(Typography::CAPTION)
                            .color(palette.border.to_iced()),
                        text(time_tag)
                            .size(Typography::CAPTION)
                            .color(palette.text_muted.to_iced()),
                    ]
                    .spacing(Spacing::XS)
                    .align_y(Alignment::Center),
                    button(text("✕").size(Typography::CAPTION))
                        .style(move |_, _| button::Style {
                            background: None,
                            text_color: palette.text_muted.to_iced(),
                            border: iced::Border::default(),
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(Message::Dismiss(item.id)),
                ]
                .align_y(Alignment::Center)
                .width(Length::Fill),
                text(&item.summary)
                    .size(Typography::BODY)
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(Spacing::XS);

            if !item.body.is_empty() {
                card_content = card_content.push(
                    text(&item.body)
                        .size(Typography::BODY_SM)
                        .color(palette.text_muted.to_iced()),
                );
            }

            // Optional interactive action buttons
            if !item.actions.is_empty() {
                let mut action_buttons = row![].spacing(Spacing::XS).align_y(Alignment::Center);
                for (action_key, action_label) in &item.actions {
                    let key_clone = action_key.clone();
                    let notif_id = item.id;
                    let action_btn = button(
                        text(action_label)
                            .size(Typography::CAPTION)
                            .color(palette.text_primary.to_iced()),
                    )
                    .padding([4, 10])
                    .style(move |_, _| button::Style {
                        background: Some(iced::Background::Color(palette.surface_hover.to_iced())),
                        text_color: palette.text_primary.to_iced(),
                        border: iced::Border {
                            color: palette.border.to_iced(),
                            width: 1.0,
                            radius: iced::border::Radius::from(corten_theme::Radius::SM),
                        },
                        shadow: iced::Shadow::default(),
                    })
                    .on_press(Message::ActionInvoked(notif_id, key_clone));

                    action_buttons = action_buttons.push(action_btn);
                }
                card_content = card_content.push(action_buttons);
            }

            card_content = card_content.push(progress_bar);

            let card = container(card_content)
                .padding(Spacing::MD)
                .width(Length::Fixed(self.config.card_width as f32))
                .style(move |_| container::Style {
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.09,
                        0.10,
                        0.13,
                        0.92 * (0.72 + 0.28 * notif.entrance.eased()),
                    ))),
                    border: iced::Border {
                        color: border_color,
                        width: if item.urgency == Urgency::Critical {
                            2.0
                        } else {
                            1.0
                        },
                        radius: iced::border::Radius::from(corten_theme::Radius::MD),
                    },
                    shadow: iced::Shadow {
                        color: if item.urgency == Urgency::Critical {
                            iced::Color::from_rgba(
                                palette.danger.r,
                                palette.danger.g,
                                palette.danger.b,
                                0.40,
                            )
                        } else {
                            iced::Color::from_rgba(0.0, 0.0, 0.0, 0.40)
                        },
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 16.0,
                    },
                    ..Default::default()
                });

            cards = cards.push(card);
        }

        container(cards).padding(Spacing::SM).into()
    }
}

#[cfg(test)]
mod tests {
    use super::{tick_visible_notification, VisibleNotification};
    use crate::dbus::{NotificationItem, Urgency};

    fn notification(urgency: Urgency, remaining_ms: u64) -> VisibleNotification {
        VisibleNotification {
            item: NotificationItem {
                id: 1,
                app_name: "test".into(),
                summary: "summary".into(),
                body: String::new(),
                icon: String::new(),
                urgency,
                actions: Vec::new(),
                timeout_ms: remaining_ms.min(i32::MAX as u64) as i32,
                sync_tag: None,
                progress: None,
            },
            remaining_ms,
            initial_ms: remaining_ms,
            entrance: super::Transition::new(140),
        }
    }

    #[test]
    fn normal_notification_expires_on_tick() {
        let mut item = notification(Urgency::Normal, 400);
        assert!(tick_visible_notification(&mut item));
        assert_eq!(item.remaining_ms, 200);
        assert!(!tick_visible_notification(&mut item));
    }

    #[test]
    fn critical_notification_does_not_expire() {
        let mut item = notification(Urgency::Critical, 200);
        assert!(tick_visible_notification(&mut item));
        assert_eq!(item.remaining_ms, 200);
    }

    #[test]
    fn test_strip_html_markup() {
        use crate::dbus::strip_html_markup;
        assert_eq!(
            strip_html_markup("<b>Important:</b> File &amp; folder updated"),
            "Important: File & folder updated"
        );
        assert_eq!(
            strip_html_markup("<a href='https://example.com'>Click &lt;here&gt;</a>"),
            "Click <here>"
        );
    }

    #[test]
    fn test_resolve_notification_icon() {
        use super::resolve_notification_icon;
        use corten_theme::Icon;

        assert_eq!(resolve_notification_icon("firefox", "Browser", ""), Icon::GLOBE);
        assert_eq!(resolve_notification_icon("spotify", "Music Player", ""), Icon::MUSIC);
        assert_eq!(resolve_notification_icon("terminal", "bash", ""), Icon::TERMINAL);
        assert_eq!(resolve_notification_icon("", "corten-terminal", ""), Icon::TERMINAL);
        assert_eq!(resolve_notification_icon("", "RandomApp", "Battery low"), Icon::BATTERY);
    }
}
