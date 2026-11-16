use std::time::Duration;
use tracing::info;

use corten_config::{AppearanceConfig, ConfigManager};
use corten_i18n::Language;
use corten_theme::{
    Icon, Palette, PatinaVariant, Radius, ThemeMode, LUCIDE_FONT, LUCIDE_FONT_BYTES, NOTO_SANS,
    NOTO_SANS_BYTES,
};
use iced::widget::{
    button, container, horizontal_space, mouse_area, progress_bar, row, scrollable, text,
    text_input,
};
use iced::{Alignment, Element, Length, Subscription};
use iced_layershell::actions::LayershellCustomActions;
use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::Application;
use iced_runtime::Task;

mod clock;
mod config;
mod disk;
mod drawer;
mod gpu;
mod music;
mod network;
mod pomodoro;
mod quick_actions;
mod radial_gauge;
mod scratchpad;
mod sysinfo;
mod visualizer;
mod weather;

use clock::ClockData;
use config::{WidgetPosition, WidgetsConfig};
use disk::{DiskMetrics, DiskMonitor};
use drawer::CortenWidgetDrawerApp;
use gpu::{GpuMetrics, GpuMonitor};
use music::{MusicService, MusicTrack};
use network::{NetworkMetrics, NetworkMonitor};
use pomodoro::PomodoroTimer;
use quick_actions::QuickActions;
use radial_gauge::RadialGauge;
use scratchpad::ScratchpadService;
use sysinfo::{SysMetrics, SysMonitor};
use visualizer::WaveformVisualizer;
use weather::{WeatherData, WeatherService};

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();
    let config_manager = ConfigManager::<WidgetsConfig>::load_or_default("widgets")?;

    // If --drawer argument is passed, launch the Slide-in Bottom Widget Drawer
    if args.iter().any(|arg| arg == "--drawer" || arg == "-d") {
        info!("Opening Corten Widget Gallery Drawer (corten-widgets --drawer)...");
        return CortenWidgetDrawerApp::run_drawer(config_manager);
    }

    info!("Starting Corten Desktop Modular Widgets Daemon (corten-widgets)...");

    let initial_config = config_manager.get().clone();

    let anchor = match initial_config.position {
        WidgetPosition::TopRight | WidgetPosition::BottomRight => {
            Anchor::Top | Anchor::Bottom | Anchor::Right
        }
        WidgetPosition::TopLeft | WidgetPosition::BottomLeft => {
            Anchor::Top | Anchor::Bottom | Anchor::Left
        }
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
            size: Some((350, 0)), // Spans full available vertical height cleanly along desktop side
            keyboard_interactivity: KeyboardInteractivity::OnDemand,
            binded_output_name: None,
        },
        default_font: NOTO_SANS,
        default_text_size: iced::Pixels(13.0),
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
    PomodoroToggle,
    PomodoroReset,
    PomodoroSwitchMode,
    QuickActionLaunch(&'static str),
    OpenDrawer,
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
    network_monitor: NetworkMonitor,
    network_metrics: NetworkMetrics,
    gpu_metrics: GpuMetrics,
    disk_monitor: DiskMonitor,
    disk_metrics: DiskMetrics,
    pomodoro: PomodoroTimer,
    clock_data: ClockData,
    weather: WeatherData,
    weather_tick_counter: u32,
    note_content: String,
    editing_note: bool,
    is_visible: bool,
    #[allow(dead_code)]
    language: Language,
}

impl CortenWidgetsApp {
    fn layer_config_task(&self) -> Task<Message> {
        let anchor = match self.config.position {
            WidgetPosition::TopRight | WidgetPosition::BottomRight => {
                Anchor::Top | Anchor::Bottom | Anchor::Right
            }
            WidgetPosition::TopLeft | WidgetPosition::BottomLeft => {
                Anchor::Top | Anchor::Bottom | Anchor::Left
            }
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

        let mut network_monitor = NetworkMonitor::new();
        let network_metrics = network_monitor.sample();

        let gpu_metrics = GpuMonitor::sample();

        let mut disk_monitor = DiskMonitor::new();
        let disk_metrics = disk_monitor.sample();

        let pomodoro = PomodoroTimer::new();
        let clock_data = ClockData::now();

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
                network_monitor,
                network_metrics,
                gpu_metrics,
                disk_monitor,
                disk_metrics,
                pomodoro,
                clock_data,
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
                Task::none()
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
                self.network_metrics = self.network_monitor.sample();
                self.gpu_metrics = GpuMonitor::sample();
                self.disk_metrics = self.disk_monitor.sample();
                self.clock_data = ClockData::now();
                self.pomodoro.tick();

                if let Ok(app_cfg) = ConfigManager::<AppearanceConfig>::load_or_default("appearance") {
                    self.active_patina = PatinaVariant::from_code(&app_cfg.get().patina);
                    self.theme_mode = match app_cfg.get().theme_mode {
                        corten_config::ThemeModeChoice::Light => ThemeMode::Light,
                        corten_config::ThemeModeChoice::Dark => ThemeMode::Dark,
                    };
                }

                // Poll active MPRIS track status
                let fetch_track = Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                );

                // Periodic weather refresh (~every 5 minutes = 300 slow ticks)
                self.weather_tick_counter = (self.weather_tick_counter + 1) % 300;
                let fetch_weather = if self.weather_tick_counter == 0 {
                    Task::perform(
                        WeatherService::fetch_weather_async(),
                        Message::WeatherUpdated,
                    )
                } else {
                    Task::none()
                };

                let reconfig_task = if needs_reconfig {
                    self.layer_config_task()
                } else {
                    Task::none()
                };

                Task::batch(vec![fetch_track, fetch_weather, reconfig_task])
            }
            Message::PlayPause => {
                MusicService::play_pause();
                Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                )
            }
            Message::NextTrack => {
                MusicService::next();
                Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                )
            }
            Message::PrevTrack => {
                MusicService::previous();
                Task::perform(
                    MusicService::fetch_current_track_async(),
                    Message::TrackUpdated,
                )
            }
            Message::TrackUpdated(track) => {
                self.current_track = track;
                Task::none()
            }
            Message::WeatherUpdated(weather) => {
                self.weather = weather;
                Task::none()
            }
            Message::ToggleVisibility => {
                self.is_visible = !self.is_visible;
                Task::none()
            }
            Message::NoteChanged(val) => {
                self.note_content = val;
                Task::none()
            }
            Message::ToggleNotesEdit => {
                self.editing_note = !self.editing_note;
                if !self.editing_note {
                    ScratchpadService::save_note(&self.note_content);
                }
                Task::none()
            }
            Message::SaveNote => {
                self.editing_note = false;
                ScratchpadService::save_note(&self.note_content);
                Task::none()
            }
            Message::PomodoroToggle => {
                self.pomodoro.toggle_run();
                Task::none()
            }
            Message::PomodoroReset => {
                self.pomodoro.reset();
                Task::none()
            }
            Message::PomodoroSwitchMode => {
                self.pomodoro.switch_mode();
                Task::none()
            }
            Message::QuickActionLaunch(cmd) => {
                QuickActions::launch(cmd);
                Task::none()
            }
            Message::OpenDrawer => {
                let _ = std::process::Command::new("corten-widgets")
                    .arg("--drawer")
                    .spawn();
                Task::none()
            }
            Message::LayerAction(_) => Task::none(),
        }
    }

    fn view(&self) -> Element<'_, Self::Message> {
        if !self.is_visible {
            return container(horizontal_space()).width(0.0).height(0.0).into();
        }

        let palette = Palette::for_mode(self.theme_mode).with_patina(self.active_patina);
        let (_, secondary_patina) = self.active_patina.colors();

        let mut cards = iced::widget::column![].spacing(10).width(Length::Fixed(320.0));

        // 1. Clock & Dual Chrono Widget
        if self.config.clock_enabled {
            cards = cards.push(self.view_clock_card(palette));
        }

        // 2. Sysinfo Gauges (CPU & RAM)
        if self.config.sysinfo_enabled {
            cards = cards.push(self.view_sysinfo_card(palette, secondary_patina));
        }

        // 3. Network Telemetry Widget
        if self.config.network_enabled {
            cards = cards.push(self.view_network_card(palette));
        }

        // 4. GPU & Thermal Telemetry Widget
        if self.config.gpu_enabled {
            cards = cards.push(self.view_gpu_card(palette, secondary_patina));
        }

        // 5. Atmospheric Weather & Forecast Widget
        if self.config.weather_forecast_enabled || self.config.weather_enabled {
            cards = cards.push(self.view_weather_card(palette));
        }

        // 6. Quick Actions & Launchers Widget
        if self.config.quick_actions_enabled {
            cards = cards.push(self.view_quick_actions_card(palette));
        }

        // 7. Disk Health & Storage Widget
        if self.config.disk_enabled {
            cards = cards.push(self.view_disk_card(palette));
        }

        // 8. Focus / Pomodoro Chrono Widget
        if self.config.pomodoro_enabled {
            cards = cards.push(self.view_pomodoro_card(palette));
        }

        // 9. Media & Waveform Widget
        if self.config.music_enabled && self.current_track.is_playing {
            cards = cards.push(self.view_music_card(palette));
        }

        // 10. Scratchpad Sticky Note Widget
        if self.config.scratchpad_enabled {
            cards = cards.push(self.view_scratchpad_card(palette));
        }

        // Drawer shortcut pill at bottom of column
        let drawer_btn = button(
            row![
                text(Icon::APP_WINDOW).font(LUCIDE_FONT).size(13.0).color(palette.primary.to_iced()),
                text("Bileşenleri Özelleştir...")
                    .size(11.0)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(|_, status| {
            let bg = match status {
                button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                _ => iced::Color::from_rgba(0.0, 0.0, 0.0, 0.20),
            };
            button::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                    width: 1.0,
                    radius: iced::border::Radius::from(Radius::SM),
                },
                ..Default::default()
            }
        })
        .on_press(Message::OpenDrawer);

        cards = cards.push(row![horizontal_space(), drawer_btn].align_y(Alignment::Center));

        let scroll_container = scrollable(cards)
            .direction(scrollable::Direction::Vertical(scrollable::Scrollbar::default()))
            .height(Length::Fill);

        container(scroll_container)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([8, 8])
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
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

        let slow_sub = iced::time::every(Duration::from_secs(1)).map(|_| Message::SlowTick);
        let fast_sub = if self.current_track.is_playing {
            iced::time::every(Duration::from_millis(50)).map(|_| Message::FastTick)
        } else {
            Subscription::none()
        };

        Subscription::batch(vec![sigusr1_sub, slow_sub, fast_sub])
    }
}

// Card View Implementations for all 10 Modular Widgets
impl CortenWidgetsApp {
    fn forged_card<'a>(&self, content: Element<'a, Message>, palette: Palette) -> Element<'a, Message> {
        container(content)
            .padding(12)
            .width(Length::Fixed(320.0))
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.09, 0.10, 0.12, 0.90,
                ))),
                border: iced::Border {
                    color: iced::Color::from_rgba(palette.primary.r, palette.primary.g, palette.primary.b, 0.32),
                    width: 1.0,
                    radius: iced::border::Radius::from(14.0),
                },
                shadow: iced::Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.40),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 14.0,
                },
                ..Default::default()
            })
            .into()
    }

    fn card_header<'a>(
        &self,
        icon_char: char,
        title: &'static str,
        subtitle: String,
        palette: Palette,
    ) -> Element<'a, Message> {
        row![
            text(icon_char)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.primary.to_iced()),
            text(title)
                .size(11.5)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            horizontal_space(),
            text(subtitle)
                .size(10.5)
                .color(palette.text_muted.to_iced()),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
    }

    // 1. Clock Card (Analog Dial + Local & World Clock)
    fn view_clock_card(&self, palette: Palette) -> Element<'_, Message> {
        let time_str = self.clock_data.local_time.format("%H:%M").to_string();
        let sec_str = self.clock_data.local_time.format(":%S").to_string();
        let date_str = self.clock_data.local_time.format("%d %B %A").to_string();
        let utc_str = self.clock_data.utc_time.format("UTC %H:%M").to_string();

        let analog_canvas = self.clock_data.view(palette);

        let digital_info = iced::widget::column![
            row![
                text(time_str)
                    .size(26.0)
                    .font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..Default::default()
                    })
                    .color(palette.text_primary.to_iced()),
                text(sec_str)
                    .size(13.0)
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
            container(
                text(utc_str)
                    .size(9.5)
                    .font(iced::Font {
                        weight: iced::font::Weight::Medium,
                        ..Default::default()
                    })
                    .color(palette.primary.to_iced()),
            )
            .padding([2, 6])
            .style(move |_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.30))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
                    width: 1.0,
                    radius: iced::border::Radius::from(4.0),
                },
                ..Default::default()
            }),
        ]
        .spacing(2);

        let body = row![analog_canvas, horizontal_space(), digital_info]
            .spacing(12)
            .align_y(Alignment::Center);

        self.forged_card(body.into(), palette)
    }

    // 2. Sysinfo Gauges (CPU & RAM)
    fn view_sysinfo_card(&self, palette: Palette, secondary_patina: corten_theme::Color) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::CPU,
            "SİSTEM VİTALLERİ",
            format!("CPU %{} · RAM %{}", self.sys_metrics.cpu_usage, self.sys_metrics.ram_pct),
            palette,
        );

        let cpu_gauge = RadialGauge::new(
            self.sys_metrics.cpu_usage as f32,
            "CPU",
            format!("{}%", self.sys_metrics.cpu_usage),
            palette.primary.to_iced(),
        );

        let ram_gauge = RadialGauge::new(
            self.sys_metrics.ram_pct as f32,
            "RAM",
            format!("{}%", self.sys_metrics.ram_pct),
            secondary_patina.to_iced(),
        );

        let gauges_row = row![
            cpu_gauge.view(),
            horizontal_space(),
            ram_gauge.view(),
        ]
        .align_y(Alignment::Center)
        .padding([4, 16]);

        self.forged_card(
            iced::widget::column![header, gauges_row].spacing(8).into(),
            palette,
        )
    }

    // 3. Network Telemetry Card
    fn view_network_card(&self, palette: Palette) -> Element<'_, Message> {
        let status_text = format!("{} · Bağlı", self.network_metrics.interface);
        let header = self.card_header(Icon::WIFI, "AĞ TELEMETRİSİ", status_text, palette);

        let rx_formatted = NetworkMonitor::format_rate(self.network_metrics.rx_speed_kb);
        let tx_formatted = NetworkMonitor::format_rate(self.network_metrics.tx_speed_kb);

        let rx_box = container(
            iced::widget::column![
                text("↓ İNDİRME").size(9.5).color(palette.text_muted.to_iced()),
                text(rx_formatted)
                    .size(13.0)
                    .font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..Default::default()
                    })
                    .color(palette.primary.to_iced()),
            ]
            .spacing(2),
        )
        .padding([6, 12])
        .width(Length::FillPortion(1))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let tx_box = container(
            iced::widget::column![
                text("↑ YÜKLEME").size(9.5).color(palette.text_muted.to_iced()),
                text(tx_formatted)
                    .size(13.0)
                    .font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..Default::default()
                    })
                    .color(palette.text_primary.to_iced()),
            ]
            .spacing(2),
        )
        .padding([6, 12])
        .width(Length::FillPortion(1))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.25))),
            border: iced::Border {
                color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                width: 1.0,
                radius: iced::border::Radius::from(Radius::SM),
            },
            ..Default::default()
        });

        let content = iced::widget::column![header, row![rx_box, tx_box].spacing(8)].spacing(8);

        self.forged_card(content.into(), palette)
    }

    // 4. GPU & Thermal Telemetry Card
    fn view_gpu_card(&self, palette: Palette, secondary_patina: corten_theme::Color) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::GAUGE,
            "GPU & TERMAL",
            format!("{}°C · Normal", self.gpu_metrics.temp_c),
            palette,
        );

        let gpu_row = row![
            text("GPU Yükü").size(11.0).color(palette.text_muted.to_iced()),
            horizontal_space(),
            text(format!("%{}", self.gpu_metrics.gpu_util))
                .size(11.5)
                .font(iced::Font { weight: iced::font::Weight::Bold, ..Default::default() })
                .color(palette.primary.to_iced()),
        ];

        let gpu_bar = progress_bar(0.0..=100.0, self.gpu_metrics.gpu_util as f32)
            .height(5.0)
            .style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(palette.primary.to_iced()),
                border: iced::Border::default(),
            });

        let temp_row = row![
            text("Sıcaklık").size(11.0).color(palette.text_muted.to_iced()),
            horizontal_space(),
            text(format!("{} °C", self.gpu_metrics.temp_c))
                .size(11.5)
                .font(iced::Font { weight: iced::font::Weight::Bold, ..Default::default() })
                .color(secondary_patina.to_iced()),
        ];

        let temp_bar = progress_bar(20.0..=100.0, self.gpu_metrics.temp_c as f32)
            .height(5.0)
            .style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(secondary_patina.to_iced()),
                border: iced::Border::default(),
            });

        let content = iced::widget::column![
            header,
            iced::widget::column![gpu_row, gpu_bar].spacing(3),
            iced::widget::column![temp_row, temp_bar].spacing(3),
        ]
        .spacing(6);

        self.forged_card(content.into(), palette)
    }

    // 5. Atmospheric Weather Card
    fn view_weather_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = self.card_header(
            Icon::CLOUD,
            "ATMOSFER & HAVA",
            self.weather.location.clone(),
            palette,
        );

        let weather_icon = if self.weather.condition.contains("Bulut") {
            Icon::CLOUD
        } else if self.weather.condition.contains("Gece") {
            Icon::MOON
        } else {
            Icon::SUN
        };

        let main_row = row![
            text(weather_icon)
                .font(LUCIDE_FONT)
                .size(24.0)
                .color(palette.primary.to_iced()),
            iced::widget::column![
                text(format!("{:.0}°C", self.weather.temperature))
                    .size(20.0)
                    .font(iced::Font {
                        weight: iced::font::Weight::Bold,
                        ..Default::default()
                    })
                    .color(palette.text_primary.to_iced()),
                text(&self.weather.condition)
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
            ]
            .spacing(1),
            horizontal_space(),
            iced::widget::column![
                text(format!("Nem: %{}", self.weather.humidity))
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
                text(format!("Rüzgar: {:.0} km/s", self.weather.wind_speed))
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
                text(format!("Basınç: {} hPa", self.weather.pressure_hpa))
                    .size(10.5)
                    .color(palette.text_muted.to_iced()),
            ]
            .align_x(Alignment::End)
            .spacing(1),
        ]
        .spacing(10)
        .align_y(Alignment::Center);

        // 3-Day Forecast projection row
        let mut forecast_row = row![].spacing(6).width(Length::Fill);
        for day in self.weather.forecast.iter().take(3) {
            let day_cell = container(
                iced::widget::column![
                    text(&day.day_name).size(9.0).color(palette.text_muted.to_iced()),
                    text(format!("{}° / {}°", day.max_temp, day.min_temp))
                        .size(10.0)
                        .font(iced::Font { weight: iced::font::Weight::Medium, ..Default::default() })
                        .color(palette.text_primary.to_iced()),
                ]
                .align_x(Alignment::Center)
                .spacing(2),
            )
            .padding([4, 6])
            .width(Length::FillPortion(1))
            .style(|_| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.20))),
                border: iced::Border {
                    color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05),
                    width: 1.0,
                    radius: iced::border::Radius::from(6.0),
                },
                ..Default::default()
            });

            forecast_row = forecast_row.push(day_cell);
        }

        let content = iced::widget::column![header, main_row, forecast_row].spacing(8);
        self.forged_card(content.into(), palette)
    }

    // 6. Quick Actions Card
    fn view_quick_actions_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = self.card_header(Icon::TERMINAL, "HIZLI EYLEMLER", "Kısayollar".into(), palette);

        let mut row1 = row![].spacing(6).width(Length::Fill);
        let mut row2 = row![].spacing(6).width(Length::Fill);

        for (idx, action) in QuickActions::ACTIONS.iter().enumerate() {
            let btn = button(
                row![
                    text(action.icon_char)
                        .font(LUCIDE_FONT)
                        .size(12.0)
                        .color(palette.primary.to_iced()),
                    text(action.label)
                        .size(10.5)
                        .color(palette.text_primary.to_iced()),
                ]
                .spacing(5)
                .align_y(Alignment::Center),
            )
            .padding([5, 8])
            .width(Length::FillPortion(1))
            .style(|_, status| {
                let bg = match status {
                    button::Status::Hovered => iced::Color::from_rgba(1.0, 1.0, 1.0, 0.10),
                    _ => iced::Color::from_rgba(0.0, 0.0, 0.0, 0.22),
                };
                button::Style {
                    background: Some(iced::Background::Color(bg)),
                    border: iced::Border {
                        color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
                        width: 1.0,
                        radius: iced::border::Radius::from(6.0),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::QuickActionLaunch(action.command));

            if idx < 3 {
                row1 = row1.push(btn);
            } else {
                row2 = row2.push(btn);
            }
        }

        let content = iced::widget::column![header, row1, row2].spacing(6);
        self.forged_card(content.into(), palette)
    }

    // 7. Disk Health Card
    fn view_disk_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = self.card_header(Icon::FOLDER, "DEPOLAMA SAĞLIĞI", "NVMe / SSD".into(), palette);

        let root_info = self.disk_metrics.root.as_ref().map(|d| {
            let pct = d.used_pct;
            let bar = progress_bar(0.0..=100.0, pct as f32).height(4.0).style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(palette.primary.to_iced()),
                border: iced::Border::default(),
            });
            iced::widget::column![
                row![
                    text("Kök (/)").size(10.5).color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    text(format!("{:.1} / {:.0} GB (%{})", d.used_gb, d.total_gb, pct))
                        .size(10.5)
                        .color(palette.text_primary.to_iced()),
                ],
                bar,
            ]
            .spacing(2)
        });

        let home_info = self.disk_metrics.home.as_ref().map(|d| {
            let pct = d.used_pct;
            let bar = progress_bar(0.0..=100.0, pct as f32).height(4.0).style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(iced::Color::from_rgba(0.3, 0.8, 0.5, 0.8)),
                border: iced::Border::default(),
            });
            iced::widget::column![
                row![
                    text("Ev (/home)").size(10.5).color(palette.text_muted.to_iced()),
                    horizontal_space(),
                    text(format!("{:.1} / {:.0} GB (%{})", d.used_gb, d.total_gb, pct))
                        .size(10.5)
                        .color(palette.text_primary.to_iced()),
                ],
                bar,
            ]
            .spacing(2)
        });

        let mut content = iced::widget::column![header].spacing(6);
        if let Some(r) = root_info {
            content = content.push(r);
        }
        if let Some(h) = home_info {
            content = content.push(h);
        }

        self.forged_card(content.into(), palette)
    }

    // 8. Pomodoro Chrono Card
    fn view_pomodoro_card(&self, palette: Palette) -> Element<'_, Message> {
        let mode_title = self.pomodoro.mode.title();
        let header = self.card_header(
            Icon::COFFEE,
            "ODAKLANMA KRONOMETRESİ",
            format!("{} Seans", self.pomodoro.completed_sessions),
            palette,
        );

        let time_text = text(self.pomodoro.formatted_time())
            .size(24.0)
            .font(iced::Font { weight: iced::font::Weight::Bold, ..Default::default() })
            .color(palette.text_primary.to_iced());

        let p_bar = progress_bar(0.0..=1.0, self.pomodoro.progress())
            .height(4.0)
            .style(move |_| progress_bar::Style {
                background: iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.35)),
                bar: iced::Background::Color(palette.primary.to_iced()),
                border: iced::Border::default(),
            });

        let play_btn = button(
            text(if self.pomodoro.is_running { "⏸ Durdur" } else { "▶ Başlat" })
                .size(10.5)
                .color(palette.primary.to_iced()),
        )
        .padding([4, 10])
        .style(|_, _| button::Style::default())
        .on_press(Message::PomodoroToggle);

        let reset_btn = button(
            text("↺ Sıfırla")
                .size(10.5)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, _| button::Style::default())
        .on_press(Message::PomodoroReset);

        let mode_btn = button(
            text(mode_title)
                .size(10.0)
                .color(palette.text_muted.to_iced()),
        )
        .padding([4, 8])
        .style(|_, _| button::Style::default())
        .on_press(Message::PomodoroSwitchMode);

        let controls = row![mode_btn, horizontal_space(), reset_btn, play_btn]
            .spacing(6)
            .align_y(Alignment::Center);

        let content = iced::widget::column![
            header,
            row![time_text, horizontal_space(), text(mode_title).size(11.0).color(palette.primary.to_iced())].align_y(Alignment::Center),
            p_bar,
            controls,
        ]
        .spacing(6);

        self.forged_card(content.into(), palette)
    }

    // 9. Media & Waveform Card
    fn view_music_card(&self, palette: Palette) -> Element<'_, Message> {
        let header = row![
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
            button(text("⏮").size(10.0).color(palette.text_muted.to_iced()))
                .padding([2, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::PrevTrack),
            button(text("⏸").size(10.0).color(palette.primary.to_iced()))
                .padding([2, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::PlayPause),
            button(text("⏭").size(10.0).color(palette.text_muted.to_iced()))
                .padding([2, 4])
                .style(|_, _| button::Style::default())
                .on_press(Message::NextTrack),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let visualizer_view = self.visualizer.view(palette.primary.to_iced());
        let content = iced::widget::column![header, visualizer_view].spacing(6);

        self.forged_card(content.into(), palette)
    }

    // 10. Scratchpad Sticky Note Card
    fn view_scratchpad_card(&self, palette: Palette) -> Element<'_, Message> {
        let edit_btn = button(
            text(if self.editing_note { "✓" } else { "✏" })
                .size(11.0)
                .color(palette.primary.to_iced()),
        )
        .padding([2, 6])
        .style(|_, _| button::Style::default())
        .on_press(Message::ToggleNotesEdit);

        let header = row![
            text(Icon::CLIPBOARD)
                .font(LUCIDE_FONT)
                .size(14.0)
                .color(palette.primary.to_iced()),
            text("HIZLI NOTLAR")
                .size(11.5)
                .font(iced::Font {
                    weight: iced::font::Weight::Bold,
                    ..Default::default()
                })
                .color(palette.text_primary.to_iced()),
            horizontal_space(),
            edit_btn,
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let note_body: Element<'_, Message> = if self.editing_note {
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

        let content = iced::widget::column![header, note_body].spacing(6);
        self.forged_card(content.into(), palette)
    }
}
