use std::time::Duration;
use tracing::info;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_i18n::Language;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, ThemeMode, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS,
    NOTO_SANS_BYTES,
};
use iced::widget::{button, container, horizontal_space, mouse_area, row, text, text_input};
use iced::{Alignment, Element, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

mod config;
mod music;
mod radial_gauge;
mod scratchpad;
mod sysinfo;
mod visualizer;
mod weather;

use config::{WidgetPosition, WidgetsConfig};
use music::{MusicService, MusicTrack};
use radial_gauge::RadialGauge;
use scratchpad::ScratchpadService;
use sysinfo::{SysMetrics, SysMonitor};
use visualizer::WaveformVisualizer;
use weather::{WeatherData, WeatherService};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    info!("Starting Corten Desktop Widgets (corten-widgets)...");

    let config_manager = ConfigManager::<WidgetsConfig>::load_or_default("widgets")?;
    let initial_config = config_manager.get().clone();

    let anchor = match initial_config.position {
        WidgetPosition::TopRight => Anchor::Top | Anchor::Right,
        WidgetPosition::TopLeft => Anchor::Top | Anchor::Left,
        WidgetPosition::BottomRight => Anchor::Bottom | Anchor::Right,
        WidgetPosition::BottomLeft => Anchor::Bottom | Anchor::Left,
    };

    let flags = AppFlags { config_manager };

    CortenWidgetsApp::run(Settings {
        id: Some("corten-widgets".into()),
        flags,
        fonts: vec![
            std::borrow::Cow::Borrowed(LUCIDE_FONT_BYTES),
            std::borrow::Cow::Borrowed(NOTO_SANS_BYTES),
        ],
        layer_settings: LayerShellSettings {
            anchor,
            layer: Layer::Bottom, // Desktop level: placed directly above wallpaper, beneath any open windows
            margin: initial_config.margin,
            exclusive_zone: 0,
            size: Some((340, 420)), // Matches compositor configured max bounds, fits waveform + scratchpad
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            binded_output_name: None,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(14.0),
        antialiasing: true,
        virtual_keyboard_support: None,
    })?;
    Ok(())
}

pub struct AppFlags {
    pub config_manager: ConfigManager<WidgetsConfig>,
}

#[derive(Debug, Clone)]
pub enum Message {
    FastTick,
    SlowTick,
    PlayPause,
    NextTrack,
    PrevTrack,
    TrackUpdated(MusicTrack),
    WeatherUpdated(WeatherData),
    ToggleVisibility,
    NoteChanged(String),
    ToggleNotesEdit,
    SaveNote,
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

pub struct CortenWidgetsApp {
    #[allow(dead_code)]
    config_manager: ConfigManager<WidgetsConfig>,
    config: WidgetsConfig,
    _config_watcher: Option<notify::RecommendedWatcher>,
    config_rx: Option<std::sync::mpsc::Receiver<WidgetsConfig>>,
    theme_mode: ThemeMode,
    active_patina: PatinaVariant,
    current_track: MusicTrack,
    visualizer: WaveformVisualizer,
    sys_monitor: SysMonitor,
    sys_metrics: SysMetrics,
    weather: WeatherData,
    weather_tick_counter: u32,
    note_content: String,
    editing_note: bool,
    is_visible: bool,
    language: Language,
}

impl CortenWidgetsApp {
    fn layer_config_task(&self) -> Task<Message> {
        let anchor = match self.config.position {
            WidgetPosition::TopRight => Anchor::Top | Anchor::Right,
            WidgetPosition::TopLeft => Anchor::Top | Anchor::Left,
            WidgetPosition::BottomRight => Anchor::Bottom | Anchor::Right,
            WidgetPosition::BottomLeft => Anchor::Bottom | Anchor::Left,
        };

        Task::batch(vec![
            Task::done(Message::LayerAction(LayershellCustomActions::AnchorChange(
                anchor,
            ))),
            Task::done(Message::LayerAction(LayershellCustomActions::MarginChange(
                self.config.margin,
            ))),
        ])
    }
}

impl Application for CortenWidgetsApp {
    type Message = Message;
    type Flags = AppFlags;
    type Theme = iced::Theme;
    type Executor = iced::executor::Default;

    fn new(flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = flags.config_manager.get().clone();
        let (watcher, rx) = match flags.config_manager.spawn_watcher() {
            Ok((w, r)) => (Some(w), Some(r)),
            Err(_) => (None, None),
        };

        let track = MusicTrack::default();
        let mut visualizer = WaveformVisualizer::new();
        visualizer.tick(track.is_playing);

        let mut sys_monitor = SysMonitor::new();
        let sys_metrics = sys_monitor.sample();

        let note_content = ScratchpadService::load_note();
        let weather = WeatherService::load_cached_or_default();
        let (active_patina, theme_mode, language) = ConfigManager::<AppearanceConfig>::load_or_default("appearance")
            .map(|c| {
                let p = PatinaVariant::from_code(&c.get().patina);
                let m = match c.get().theme_mode {
                    corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                    corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                };
                let l = Language::from_code(&c.get().language);
                (p, m, l)
            })
            .unwrap_or_else(|_| (PatinaVariant::Rust, ThemeMode::Dark, Language::detect_system()));

        let load_font_task = iced::font::load(LUCIDE_FONT_BYTES).map(|_| Message::SlowTick);
        let init_track_task = Task::perform(
            MusicService::fetch_current_track_async(),
            Message::TrackUpdated,
        );
        let init_weather_task = Task::perform(
            WeatherService::fetch_weather_async(),
            Message::WeatherUpdated,
        );

        let init_task = Task::batch(vec![load_font_task, init_track_task, init_weather_task]);

        (
            Self {
                config_manager: flags.config_manager,
                config,
                _config_watcher: watcher,
                config_rx: rx,
                theme_mode,
                active_patina,
                current_track: track,
                visualizer,
                sys_monitor,
                sys_metrics,
                weather,
                weather_tick_counter: 0,
                note_content,
                editing_note: false,
                is_visible: true,
                language,
            },
            init_task,
        )
    }

    fn namespace(&self) -> String {
        "corten-widgets".to_string()
    }

    fn style(&self, _theme: &Self::Theme) -> iced_layershell::Appearance {
        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        iced_layershell::Appearance {
            background_color: iced::Color::TRANSPARENT,
            text_color: palette.text_primary.to_iced(),
        }
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::FastTick => {
                // High-frequency tick for audio waveform animation
                self.visualizer.tick(self.current_track.is_playing);
            }
            Message::SlowTick => {
                // Event-driven config sync: drain any pending watcher messages
                let mut needs_reconfig = false;
                if let Some(ref rx) = self.config_rx {
                    while let Ok(new_config) = rx.try_recv() {
                        if new_config != self.config {
                            self.config = new_config;
                            needs_reconfig = true;
                        }
                    }
                }

                self.sys_metrics = self.sys_monitor.sample();
                if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                    self.theme_mode = match app_cfg.get().theme_mode {
                        corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                        corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                    };
                    self.language = Language::from_code(&app_cfg.get().language);
                }

                let mut tasks = Vec::new();
                if needs_reconfig {
                    tasks.push(self.layer_config_task());
                }

                // Check music metadata asynchronously (never blocking UI thread)
                tasks.push(Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                ));

                // Check live weather every 15 minutes (900 SlowTicks)
                self.weather_tick_counter = self.weather_tick_counter.wrapping_add(1);
                if self.weather_tick_counter.is_multiple_of(900) {
                    tasks.push(Task::perform(
                        WeatherService::fetch_weather_async(),
                        Message::WeatherUpdated,
                    ));
                }

                return Task::batch(tasks);
            }
            Message::TrackUpdated(track) => {
                self.current_track = track;
            }
            Message::WeatherUpdated(weather) => {
                self.weather = weather;
            }
            Message::ToggleVisibility => {
                self.is_visible = !self.is_visible;
                let target_size = if self.is_visible {
                    (340, 420)
                } else {
                    (1, 1)
                };
                return Task::done(Message::LayerAction(LayershellCustomActions::SizeChange(
                    target_size,
                )));
            }
            Message::PlayPause => {
                MusicService::play_pause();
                return Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                );
            }
            Message::NextTrack => {
                MusicService::next();
                return Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                );
            }
            Message::PrevTrack => {
                MusicService::previous();
                return Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                );
            }
            Message::NoteChanged(val) => {
                self.note_content = val;
            }
            Message::ToggleNotesEdit => {
                self.editing_note = !self.editing_note;
                if !self.editing_note {
                    ScratchpadService::save_note(&self.note_content);
                }
            }
            Message::SaveNote => {
                self.editing_note = false;
                ScratchpadService::save_note(&self.note_content);
            }
            Message::LayerAction(_) => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if !self.is_visible {
            return container(horizontal_space()).width(0).height(0).into();
        }

        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);

        let now = chrono::Local::now();
        let time_str = now.format("%H:%M").to_string();
        let seconds_str = now.format(":%S").to_string();
        let weekday_num = now.format("%u").to_string().parse::<u32>().unwrap_or(1);
        let day_num = now.format("%d").to_string().parse::<u32>().unwrap_or(1);
        let month_num = now.format("%m").to_string().parse::<u32>().unwrap_or(1);
        let date_str = self.language.format_date(weekday_num, day_num, month_num);

        let cpu_gauge = RadialGauge::new(
            self.sys_metrics.cpu_usage as f32,
            "CPU",
            format!("{}%", self.sys_metrics.cpu_usage),
            palette.primary.to_iced(),
        );

        let (_, secondary_patina) = self.active_patina.colors();
        let ram_gauge = RadialGauge::new(
            self.sys_metrics.ram_pct as f32,
            "RAM",
            format!("{}%", self.sys_metrics.ram_pct),
            secondary_patina.to_iced(),
        );

        let weather_icon = if self.weather.condition.contains("Bulut") || self.weather.condition.to_lowercase().contains("cloud") {
            Icon::CLOUD
        } else if self.weather.condition.contains("Gece") || self.weather.condition.to_lowercase().contains("night") {
            Icon::MOON
        } else {
            Icon::SUN
        };

        // Sticky Scratchpad card
        let scratchpad_widget: Element<'_, Self::Message> = if self.config.scratchpad_enabled {
            let edit_btn = button(
                text(if self.editing_note { "✓" } else { "✏" })
                    .size(11.0)
                    .color(palette.primary.to_iced()),
            )
            .padding([2, 6])
            .style(|_, _| button::Style::default())
            .on_press(Message::ToggleNotesEdit);

            let note_body: Element<'_, Self::Message> = if self.editing_note {
                text_input("Notunuzu yazın...", &self.note_content)
                    .on_input(Message::NoteChanged)
                    .on_submit(Message::SaveNote)
                    .size(11.0)
                    .padding(4)
                    .style(move |_, _| text_input::Style {
                        background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.3)),
                        border: iced::Border {
                            color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.5),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::SM),
                        },
                        icon: iced::Color::TRANSPARENT,
                        placeholder: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.4),
                        value: palette.text_primary.to_iced(),
                        selection: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.3),
                    })
                    .into()
            } else {
                let display_text = if self.note_content.trim().is_empty() {
                    "Not eklemek için tıklayın..."
                } else {
                    self.note_content.as_str()
                };
                mouse_area(
                    text(display_text)
                        .size(11.0)
                        .color(if self.note_content.trim().is_empty() {
                            palette.text_muted.to_iced()
                        } else {
                            palette.text_primary.to_iced()
                        }),
                )
                .on_press(Message::ToggleNotesEdit)
                .into()
            };

            container(
                iced::widget::column![
                    row![
                        text(Icon::CLIPBOARD)
                            .font(LUCIDE_FONT)
                            .size(13.0)
                            .color(palette.primary.to_iced()),
                        text("Notlar")
                            .size(11.0)
                            .font(iced::Font {
                                weight: iced::font::Weight::Medium,
                                ..Default::default()
                            })
                            .color(palette.text_primary.to_iced()),
                        horizontal_space(),
                        edit_btn,
                    ]
                    .spacing(6)
                    .align_y(Alignment::Center),
                    note_body,
                ]
                .spacing(4),
            )
            .padding(6)
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            })
            .into()
        } else {
            container(horizontal_space()).height(0.0).into()
        };

        let desklet = container(
            iced::widget::column![
                // Header: Clock & Weather
                row![
                    iced::widget::column![
                        row![
                            text(time_str)
                                .size(32.0)
                                .font(iced::Font {
                                    weight: iced::font::Weight::Bold,
                                    ..Default::default()
                                })
                                .color(palette.text_primary.to_iced()),
                            text(seconds_str)
                                .size(15.0)
                                .font(iced::Font {
                                    weight: iced::font::Weight::Medium,
                                    ..Default::default()
                                })
                                .color(palette.primary.to_iced()),
                        ]
                        .align_y(Alignment::Center),
                        text(date_str)
                            .size(11.0)
                            .color(palette.text_muted.to_iced()),
                    ],
                    horizontal_space(),
                    if self.config.weather_enabled {
                        iced::widget::column![
                            row![
                                text(weather_icon)
                                    .font(LUCIDE_FONT)
                                    .size(16.0)
                                    .color(palette.primary.to_iced()),
                                text(format!("{:.0}°C", self.weather.temperature))
                                    .size(15.0)
                                    .font(iced::Font {
                                        weight: iced::font::Weight::Semibold,
                                        ..Default::default()
                                    })
                                    .color(palette.text_primary.to_iced()),
                            ]
                            .spacing(4)
                            .align_y(Alignment::Center),
                            text(&self.weather.location)
                                .size(11.0)
                                .color(palette.text_muted.to_iced()),
                        ]
                        .align_x(Alignment::End)
                    } else {
                        iced::widget::column![]
                    }
                ]
                .align_y(Alignment::Center),

                // Separator line
                container(horizontal_space())
                    .height(1.0)
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                        ..Default::default()
                    }),

                // System Gauges: CPU & RAM
                if self.config.sysinfo_enabled {
                    row![
                        cpu_gauge.view(),
                        horizontal_space(),
                        ram_gauge.view(),
                    ]
                    .align_y(Alignment::Center)
                    .padding([2, 10])
                } else {
                    row![]
                },

                // Music & Waveform (if playing)
                if self.config.music_enabled && self.current_track.is_playing {
                    container(
                        iced::widget::column![
                            row![
                                text(Icon::MUSIC)
                                    .font(LUCIDE_FONT)
                                    .size(14.0)
                                    .color(palette.primary.to_iced()),
                                text(if self.current_track.artist.is_empty() {
                                    self.current_track.title.clone()
                                } else {
                                    format!("{} - {}", self.current_track.title, self.current_track.artist)
                                })
                                .size(11.0)
                                .color(palette.text_primary.to_iced()),
                                horizontal_space(),
                                button(text("⏸").size(11.0).color(palette.primary.to_iced()))
                                    .padding([2, 5])
                                    .style(|_, _| button::Style::default())
                                    .on_press(Message::PlayPause),
                            ]
                            .spacing(6)
                            .align_y(Alignment::Center),
                            self.visualizer.view(palette.primary.to_iced()),
                        ]
                        .spacing(4),
                    )
                    .padding(6)
                    .style(move |_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25))),
                        border: iced::Border {
                            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                            width: 1.0,
                            radius: iced::border::Radius::from(Radius::SM),
                        },
                        ..Default::default()
                    })
                } else {
                    container(horizontal_space()).height(0.0)
                },

                // Scratchpad Notes
                scratchpad_widget,
            ]
            .spacing(10)
            .width(Length::Fixed(270.0))
        )
        .padding(14)
        .style(move |_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.09, 0.10, 0.12, 0.92,
            ))),
            border: iced::Border {
                color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.35),
                width: 1.0,
                radius: iced::border::Radius::from(16.0),
            },
            shadow: iced::Shadow {
                color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.40),
                offset: iced::Vector::new(0.0, 6.0),
                blur_radius: 16.0,
            },
            ..Default::default()
        });

        container(desklet)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([16, 20])
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // Unix signal stream for SIGUSR1 to toggle widget visibility without killing the process
        let sigusr1_sub = Subscription::run_with_id(
            "sigusr1-stream",
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
                        return Some((Message::ToggleVisibility, sig_opt));
                    }
                }
            }),
        );

        let slow_tick = iced::time::every(Duration::from_millis(1000)).map(|_| Message::SlowTick);

        let mut subs = vec![sigusr1_sub, slow_tick];

        if self.is_visible && self.current_track.is_playing {
            let fast_tick = iced::time::every(Duration::from_millis(33)).map(|_| Message::FastTick);
            subs.push(fast_tick);
        }

        Subscription::batch(subs)
    }
}
